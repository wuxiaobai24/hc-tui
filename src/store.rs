//! 通用代际存储：`store/<id>/<generation>`、`current/<id>` 指针与
//! `records/<id>.json`。
//!
//! 每个模块的代码按代际存放在 `store/<id>/<generation>/`，`current/<id>`
//! 指向当前代际；记录文件保存来源、代际、启用状态与安装时间。运行中的
//! 表面继续使用已加载的代码，旧代际在最后一个引用关闭后回收。
//!
//! 发布不变量：`current` 只指向已经完整落盘的代际——内容先写进包锁内独占的
//! 暂存目录、逐文件 fsync，再 rename 成正式代际，最后才写记录、切指针。
//!
//! 记录是唯一发布真相，`current` 只是可重建索引：记录先原子落盘，指针再
//! 原子切换；[`Store::open`] 在每次打开时做一次幂等恢复，清理临时目录、按
//! 记录补回缺失或与记录不一致的指针，把「目录在但清单不在」的代际报成问题
//! 并摘掉指向它的指针，把「有指针没记录」的包报成问题而不是悄悄删除。

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 模块来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// 本地目录符号链接或直接路径，修改后重载即生效。
    Linked,
    /// 从目录拷贝安装。
    Local,
    /// 已验证的签名包。
    Packaged,
}

impl Source {
    /// 序列化名与 `mods list --json` 一致。
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Linked => "linked",
            Source::Local => "local",
            Source::Packaged => "packaged",
        }
    }
}

/// `records/<id>.json`：来源、清单哈希、启用状态、代际与安装时间。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    /// 模块 ID，与配置目录名一致。
    pub id: String,
    /// 来源。
    pub source: Source,
    /// 当前代际；linked 来源固定为 1。
    pub generation: u64,
    /// 安装时间（Unix 秒）。
    pub installed_at: u64,
    /// 是否启用。
    pub enabled: bool,
    /// 清单哈希（`hctui.toml` 的 SHA-256）；linked 来源每次重载时重算。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    /// 来源路径：linked 的目录、local 的安装来源。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// 签名包整体的 SHA-256。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// 签名包发布密钥 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// 签名包的应用元数据：`{id, name, version, build}`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<serde_json::Value>,
    /// 签名包要求的最低 nsh 运行时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_runtime: Option<String>,
    /// 这个包提供的能力；由清单推导后缓存。
    ///
    /// 旧记录没有这个字段：读出后按清单回填（见
    /// [`crate::plugin::registry::PackageRegistry::entries`]），不报问题。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<crate::pkg::Capabilities>,
    /// 声明了 `[module]` 时的模块摘要。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<crate::pkg::ModuleInfo>,
    /// 清单一 `[grants]` 的宿主方法与事件；运行期只读这一份。
    ///
    /// 旧记录没有这个字段：读出后按本代际目录里的清单回填（见
    /// [`crate::pkg::resolve`]），不报问题。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grants: Option<Vec<String>>,
}

/// 一次恢复中发现的不一致；调用方把它报给用户，不静默修正数据。
#[derive(Debug, Clone)]
pub struct RecoveryIssue {
    /// 可识别的包 ID。
    pub id: Option<String>,
    /// 相关路径。
    pub path: PathBuf,
    /// 人读的原因。
    pub message: String,
}

/// 包级互斥的持有者；离开作用域即释放锁。
#[derive(Debug)]
pub struct PackageLock {
    path: PathBuf,
    /// unix 上持着 `flock` 排他锁的文件描述符；进程消失时内核释放它。
    #[cfg(unix)]
    _held: std::fs::File,
}

impl Drop for PackageLock {
    fn drop(&mut self) {
        // 先删锁文件，再随字段释放 `flock`：等在同一个 inode 上的进程拿到锁后
        // 会发现路径换了（见 [`same_file`]），回头重来，不会两个进程各持一把。
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 等待另一个安装者释放包锁的期限。
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// 等待包锁时的轮询间隔。
const LOCK_POLL: Duration = Duration::from_millis(10);
/// 不靠进程存活判断的平台，锁文件超过这个年龄视为残留。
const LOCK_STALE_AGE: Duration = Duration::from_secs(120);

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 同目录临时文件名的唯一后缀。
fn temp_id() -> u64 {
    TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// 代际存储的根：`<state>/mods/`。
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    recovery: Vec<RecoveryIssue>,
}

impl Store {
    /// 打开或创建存储根；目录按需创建，并做一次幂等恢复。
    pub fn open(root: impl Into<PathBuf>) -> Store {
        let mut store = Store {
            root: root.into(),
            recovery: Vec::new(),
        };
        store.recover();
        store
    }

    /// 存储根。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 本次打开时发现的不一致。
    pub fn recovery_issues(&self) -> &[RecoveryIssue] {
        &self.recovery
    }

    /// 幂等恢复：清理临时残渣、按记录补回缺失的 `current`、上报无记录的指针。
    /// 整个过程不动记录，也不删除有指针没记录的包目录。
    fn recover(&mut self) {
        self.clean_temporary_staging();
        self.clean_temporary_records();
        self.rebuild_missing_current();
        self.report_current_without_record();
    }

    /// 删除安装中途留下的 `store/<id>/.tmp-*`。
    ///
    /// 正在安装的包持有 `.lock`，此时跳过，避免删掉别人正在复制的暂存目录。
    fn clean_temporary_staging(&mut self) {
        let Ok(packages) = std::fs::read_dir(self.root.join("store")) else {
            return;
        };
        for package in packages.filter_map(Result::ok) {
            if !package.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let id = package.file_name().to_string_lossy().to_string();
            let Ok(entries) = std::fs::read_dir(package.path()) else {
                continue;
            };
            let staging: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(".tmp-"))
                .map(|entry| entry.path())
                .collect();
            if staging.is_empty() {
                continue;
            }
            let Some(_lock) = self.try_lock_package(&id) else {
                continue;
            };
            for path in staging {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }

    /// 删除写记录中途留下的临时文件。
    ///
    /// 记录写入没有包锁保护：名字里的持有者进程还活着就跳过
    /// （`write_atomic` 的临时文件名形如 `.tmp-<pid>-<seq>-<记录名>`），
    /// 只有持有者已消失或文件老到不可能还在写时才当残留。
    fn clean_temporary_records(&mut self) {
        let Ok(entries) = std::fs::read_dir(self.records_dir()) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".tmp-") && temp_entry_is_stale(&name, &entry.path()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// 指针缺失、指向不存在的目标或与记录不一致时，按记录重建 `current`。
    ///
    /// 只把指针指向完整代际（目录里已有清单）。目录在而清单不在，说明那一代
    /// 没落完：上报问题、摘掉指向它的指针，不删记录也不删目录。
    fn rebuild_missing_current(&mut self) {
        for record in self.list() {
            let target = match record.source {
                Source::Linked => record.origin.as_deref().map(PathBuf::from),
                _ => Some(self.generation_dir(&record.id, record.generation)),
            };
            let Some(target) = target.filter(|path| path.is_dir()) else {
                self.recovery.push(RecoveryIssue {
                    id: Some(record.id.clone()),
                    path: self.current_link(&record.id),
                    message: format!(
                        "record points at a missing generation {}; the package is not loaded",
                        record.generation
                    ),
                });
                continue;
            };
            // 记录是发布真相：指针停在旧代际（切换中途失败）也算需要重建。
            let target = std::path::absolute(&target).unwrap_or(target);
            if !crate::pkg::has_manifest(&target) {
                self.recovery.push(RecoveryIssue {
                    id: Some(record.id.clone()),
                    path: target.clone(),
                    message: format!(
                        "generation {} is incomplete (no {} or {}); the package is not loaded",
                        record.generation,
                        crate::pkg::MANIFEST_FILE,
                        crate::pkg::PLUGIN_FILE
                    ),
                });
                self.drop_incomplete_current(&record.id, &target);
                continue;
            }
            if self.current_dir(&record.id).as_deref() == Some(target.as_path()) {
                continue;
            }
            // 与正在切换代际的安装者抢锁时让路，下次打开再修。
            let Some(_lock) = self.try_lock_package(&record.id) else {
                continue;
            };
            if let Err(error) = self.set_current(&record.id, &target) {
                self.recovery.push(RecoveryIssue {
                    id: Some(record.id.clone()),
                    path: self.current_link(&record.id),
                    message: format!("cannot rebuild the current pointer: {error}"),
                });
            }
        }
    }

    /// 摘掉指向不完整代际的指针；与正在切换代际的安装者抢锁时让路。
    fn drop_incomplete_current(&self, id: &str, target: &Path) {
        let Some(_lock) = self.try_lock_package(id) else {
            return;
        };
        if self.current_dir(id).as_deref() == Some(target) {
            let _ = std::fs::remove_file(self.current_link(id));
        }
    }

    /// 有 `current` 指针却没有记录：只报告，不删除。
    fn report_current_without_record(&mut self) {
        let Ok(entries) = std::fs::read_dir(self.root.join("current")) else {
            return;
        };
        let mut broken: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().to_string(),
                    entry.path(),
                )
            })
            .collect();
        broken.sort();
        for (id, path) in broken {
            if self.record(&id).is_some() {
                continue;
            }
            self.recovery.push(RecoveryIssue {
                id: Some(id),
                path,
                message: "current pointer has no record; the package is not loaded".to_string(),
            });
        }
    }

    /// 信任根与撤销状态的目录。
    pub fn trust_dir(&self) -> PathBuf {
        self.root.join("trust")
    }

    /// 日志目录：`logs/<id>/`。
    pub fn log_dir(&self, id: &str) -> PathBuf {
        self.root.join("logs").join(id)
    }

    fn records_dir(&self) -> PathBuf {
        self.root.join("records")
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.records_dir().join(format!("{id}.json"))
    }

    fn current_link(&self, id: &str) -> PathBuf {
        self.root.join("current").join(id)
    }

    /// 某模块的代际目录 `store/<id>/<generation>/`。
    pub fn generation_dir(&self, id: &str, generation: u64) -> PathBuf {
        self.root
            .join("store")
            .join(id)
            .join(generation.to_string())
    }

    /// 全部记录，按 ID 排序。
    pub fn list(&self) -> Vec<Record> {
        let Ok(entries) = std::fs::read_dir(self.records_dir()) else {
            return Vec::new();
        };
        let mut records: Vec<Record> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .filter_map(|text| serde_json::from_str::<Record>(&text).ok())
            .collect();
        records.sort_by(|a, b| a.id.cmp(&b.id));
        records
    }

    /// 读一条记录。
    pub fn record(&self, id: &str) -> Option<Record> {
        let text = std::fs::read_to_string(self.record_path(id)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// 原子写一条记录：同目录临时文件 → fsync → rename → 父目录 fsync。
    /// 失败时旧记录保持不变。
    pub fn save_record(&self, record: &Record) -> io::Result<()> {
        std::fs::create_dir_all(self.records_dir())?;
        let text = serde_json::to_string_pretty(record).map_err(io::Error::other)?;
        write_atomic(&self.record_path(&record.id), text.as_bytes())
    }

    /// 等待一个包的锁：unix 上靠 `flock` 排队，其他平台靠 PID 文件。
    ///
    /// 判不了持有者（内容为空、不可解析、PID 已消失）一律视为可接管，接管不等待
    /// 锁文件自己变旧：macOS 没有 `/proc`，靠 `kill(pid, 0)` 探活会把 `EPERM` 当成
    /// 活着、把被复用的 PID 当成活着，SIGKILL 之后的下一个安装者就会卡到超时。
    pub fn lock_package(&self, id: &str) -> io::Result<PackageLock> {
        let dir = self.root.join("store").join(id);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(".lock");
        let deadline = std::time::Instant::now() + LOCK_TIMEOUT;
        loop {
            match try_create_lock(&path) {
                Ok(lock) => return Ok(lock),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    #[cfg(not(unix))]
                    if stale_lock(&path) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("another install is in progress for {id}"),
                        ));
                    }
                    std::thread::sleep(LOCK_POLL);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// 不等待地尝试包锁；被占用时返回 `None`（恢复路径用）。
    pub fn try_lock_package(&self, id: &str) -> Option<PackageLock> {
        let dir = self.root.join("store").join(id);
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(".lock");
        try_create_lock(&path).ok()
    }

    /// 在包锁内分配下一个代际，返回独占的暂存目录
    /// `store/<id>/.tmp-<pid>-<seq>-<gen>`。名字带持有者 PID 与会话内序号，
    /// 两个安装者永不共用一个暂存目录。调用方复制完成后用
    /// [`Store::publish_staged`] 换入正式目录。
    pub fn alloc_generation(&self, id: &str) -> io::Result<(u64, PathBuf)> {
        let generation = self.next_generation(id);
        let staged = self.root.join("store").join(id).join(format!(
            ".tmp-{}-{}-{generation}",
            std::process::id(),
            temp_id()
        ));
        std::fs::create_dir_all(&staged)?;
        Ok((generation, staged))
    }

    /// 把暂存目录整体落盘后 rename 成 `store/<id>/<generation>`。
    /// rename 之前内容已经 fsync：从外部看这一代要么不存在、要么内容完整，
    /// `current` 才能指向它。
    pub fn publish_staged(&self, id: &str, generation: u64, staged: &Path) -> io::Result<PathBuf> {
        let dir = self.generation_dir(id, generation);
        if dir.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("generation {generation} already exists"),
            ));
        }
        sync_tree(staged);
        std::fs::rename(staged, &dir)?;
        if let Some(parent) = dir.parent() {
            sync_directory(parent);
        }
        Ok(dir)
    }

    /// 丢弃暂存目录；失败不影响错误返回。
    pub fn discard_staged(&self, staged: &Path) {
        let _ = std::fs::remove_dir_all(staged);
    }

    /// 下一个代际号：记录与磁盘上已有代际的较大者加一。
    ///
    /// 卸载会保留正在被终端使用的旧代际；重装必须跳过它们，不能覆盖运行中的代码。
    pub fn next_generation(&self, id: &str) -> u64 {
        let recorded = self.record(id).map(|record| record.generation).unwrap_or(0);
        let on_disk = std::fs::read_dir(self.root.join("store").join(id))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u64>().ok())
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        recorded.max(on_disk) + 1
    }

    /// 建好下一个代际目录并返回 `(代际, 目录)`。
    pub fn prepare_generation(&self, id: &str) -> io::Result<(u64, PathBuf)> {
        let generation = self.next_generation(id);
        let dir = self.generation_dir(id, generation);
        std::fs::create_dir_all(&dir)?;
        Ok((generation, dir))
    }

    /// 原子切换 `current/<id>`：先在同目录建好临时指针，再 rename 覆盖。
    /// 任何时刻从外部看指针要么指向旧代际，要么指向新代际。
    pub fn set_current(&self, id: &str, dir: &Path) -> io::Result<()> {
        let link = self.current_link(id);
        let Some(parent) = link.parent() else {
            return Err(io::Error::other("current pointer has no parent"));
        };
        std::fs::create_dir_all(parent)?;
        let target = std::path::absolute(dir)?;
        let staged = parent.join(format!(".tmp-{}-{}-{id}", std::process::id(), temp_id()));
        let _ = std::fs::remove_file(&staged);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &staged)?;
        #[cfg(not(unix))]
        std::fs::write(&staged, target.to_string_lossy().as_bytes())?;
        if let Err(error) = std::fs::rename(&staged, &link) {
            let _ = std::fs::remove_file(&staged);
            return Err(error);
        }
        sync_directory(parent);
        Ok(())
    }

    /// 当前代际目录；指向不存在的目标时返回 `None`。
    pub fn current_dir(&self, id: &str) -> Option<PathBuf> {
        let link = self.current_link(id);
        let target = match std::fs::read_link(&link) {
            Ok(target) if target.is_absolute() => target,
            Ok(target) => link.parent()?.join(target),
            Err(_) => PathBuf::from(std::fs::read_to_string(&link).ok()?.trim()),
        };
        target.is_dir().then_some(target)
    }

    /// 回收旧代际：保留记录里的当前代际与 `live` 中仍被表面使用的代际。
    /// 返回删除的代际号。
    pub fn prune(&self, id: &str, live: &[u64]) -> Vec<u64> {
        let current = self.record(id).map(|record| record.generation);
        let Ok(entries) = std::fs::read_dir(self.root.join("store").join(id)) else {
            return Vec::new();
        };
        let mut removed = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let Ok(generation) = entry.file_name().to_string_lossy().parse::<u64>() else {
                continue;
            };
            if Some(generation) == current || live.contains(&generation) {
                continue;
            }
            if std::fs::remove_dir_all(entry.path()).is_ok() {
                removed.push(generation);
            }
        }
        removed
    }

    /// 删除一个模块的记录、指针、代际与日志。
    pub fn remove(&self, id: &str) -> io::Result<()> {
        self.detach(id)?;
        self.remove_store(id)?;
        let logs = self.log_dir(id);
        if logs.exists() {
            std::fs::remove_dir_all(logs)?;
        }
        Ok(())
    }

    /// 只删除记录与 `current` 指针，代际目录留给引用释放后再回收。
    pub fn detach(&self, id: &str) -> io::Result<()> {
        let record = self.record_path(id);
        if record.exists() {
            std::fs::remove_file(record)?;
        }
        let link = self.current_link(id);
        if std::fs::symlink_metadata(&link).is_ok() {
            std::fs::remove_file(link)?;
        }
        Ok(())
    }

    /// 删除 `store/<id>` 下的全部代际目录，保留记录、指针与日志。
    pub fn remove_store(&self, id: &str) -> io::Result<()> {
        let store = self.root.join("store").join(id);
        if store.exists() {
            std::fs::remove_dir_all(store)?;
        }
        Ok(())
    }

    /// 追加引擎日志，超过 1 MiB 时轮转到 `.1`（保留两份）。
    pub fn append_log(&self, id: &str, line: &str) -> io::Result<()> {
        let dir = self.log_dir(id);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("engine.log");
        if path.metadata().map(|meta| meta.len()).unwrap_or(0) > 1024 * 1024 {
            let rotated = dir.join("engine.log.1");
            let _ = std::fs::remove_file(&rotated);
            std::fs::rename(&path, rotated)?;
        }
        let mut text = String::new();
        if let Ok(existing) = std::fs::read_to_string(&path) {
            text.push_str(&existing);
        }
        text.push_str(line);
        text.push('\n');
        std::fs::write(&path, text)
    }

    /// 日志最后 `lines` 行。
    pub fn log_tail(&self, id: &str, lines: usize) -> io::Result<String> {
        let path = self.log_dir(id).join("engine.log");
        let text = std::fs::read_to_string(path)?;
        let all: Vec<&str> = text.lines().collect();
        let start = all.len().saturating_sub(lines);
        Ok(all[start..].join("\n"))
    }
}

/// 同目录临时文件 → fsync → rename → 父目录 fsync。失败时目标文件不变。
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let staged = dir.join(format!(".tmp-{}-{}-{name}", std::process::id(), temp_id()));
    let result = (|| {
        let mut file = std::fs::File::create(&staged)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&staged, path)?;
        sync_directory(dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

/// 目录 fsync；平台不支持时不影响已完成的 rename。
fn sync_directory(dir: &Path) {
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
}

/// 递归落盘：每个文件与目录都 fsync，保证 rename 之前内容已经在盘上。
fn sync_tree(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            sync_tree(&path);
        } else if let Ok(file) = std::fs::File::open(&path) {
            let _ = file.sync_all();
        }
    }
    sync_directory(dir);
}

/// 临时条目名里的持有者 PID：`<prefix><pid>-<seq>-...`；解析失败返回 `None`。
fn temp_owner(name: &str) -> Option<u32> {
    name.strip_prefix(".tmp-")?.split('-').next()?.parse().ok()
}

/// 临时条目是否残留：持有者已消失，或文件老到不可能还在写。
fn temp_entry_is_stale(name: &str, path: &Path) -> bool {
    let age = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| std::time::SystemTime::now().duration_since(time).ok());
    if age.is_some_and(|age| age > LOCK_STALE_AGE) {
        return true;
    }
    match temp_owner(name) {
        Some(pid) => !process_alive(pid),
        None => false,
    }
}

/// `EPERM` 说明进程存在但不属于自己，按活着算；只有 `ESRCH` 才是死了。
fn process_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    #[cfg(unix)]
    {
        if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
            return true;
        }
        let error = io::Error::last_os_error();
        !matches!(error.raw_os_error(), Some(libc::ESRCH))
    }
    #[cfg(not(unix))]
    {
        // 没有廉价探活手段时按活着算，残渣交给年龄上限处理。
        true
    }
}

/// 排他取得锁文件，成功时返回持有者。
///
/// unix 上的互斥由内核 `flock` 提供：锁跟着打开的文件描述符走，持有者被
/// SIGKILL 时自动释放，与 PID 是否被复用、平台探活语义无关；锁文件内容只是
/// 持有者 PID，供排查用。锁文件被别的进程删掉重建时描述符指向的 inode 已经
/// 不同，拿到锁后核对一次，不一致就当作被占用重来。
#[cfg(unix)]
fn try_create_lock(path: &Path) -> io::Result<PackageLock> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let acquired = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if acquired != 0 {
        let error = io::Error::last_os_error();
        // EWOULDBLOCK 与 EAGAIN 在 Linux 与 macOS 上同值，用比较避免重复模式。
        let busy = matches!(error.raw_os_error(), Some(code)
            if code == libc::EWOULDBLOCK || code == libc::EAGAIN);
        return Err(if busy {
            io::Error::new(io::ErrorKind::AlreadyExists, "the package lock is held")
        } else {
            error
        });
    }
    if !same_file(&file, path) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the package lock file was replaced",
        ));
    }
    let mut file = file;
    file.set_len(0)?;
    writeln!(file, "{}", std::process::id())?;
    let _ = file.sync_all();
    #[cfg(test)]
    if let Some(delay) = lock_create_delay() {
        std::thread::sleep(delay);
    }
    Ok(PackageLock {
        path: path.to_path_buf(),
        _held: file,
    })
}

/// 打开的描述符与路径上现在那个文件是不是同一个 inode。
#[cfg(unix)]
fn same_file(file: &std::fs::File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), std::fs::symlink_metadata(path)) {
        (Ok(held), Ok(at_path)) => held.dev() == at_path.dev() && held.ino() == at_path.ino(),
        _ => false,
    }
}

/// 非 unix：先把持有者 PID 写进临时文件，再用硬链接原子占位。
/// 锁文件一出现就带着完整内容，不存在「已建、还没写 PID」的可疑窗口。
#[cfg(not(unix))]
fn try_create_lock(path: &Path) -> io::Result<PackageLock> {
    let staged = path.with_file_name(format!(".tmp-{}-{}-lock", std::process::id(), temp_id()));
    std::fs::write(&staged, format!("{}\n", std::process::id()))?;
    let linked = std::fs::hard_link(&staged, path);
    let _ = std::fs::remove_file(&staged);
    linked?;
    Ok(PackageLock {
        path: path.to_path_buf(),
    })
}

/// 锁文件年龄；读不到元数据时返回 `None`。
#[cfg(not(unix))]
fn lock_age(path: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()?;
    std::time::SystemTime::now().duration_since(modified).ok()
}

/// 锁文件是否残留。内容为空、不可解析（备份/重启后只留下半个文件）或持有者
/// 已消失一律视为可接管；锁文件已经被删掉时也直接接管。
#[cfg(not(unix))]
fn stale_lock(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return true;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return true;
    };
    // 同一进程的另一线程不算残留，必须等它释放。
    if pid == std::process::id() {
        return false;
    }
    lock_age(path).is_some_and(|age| age > LOCK_STALE_AGE)
}

/// 当前 Unix 时间秒数；平台时间不可用时返回 0。
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// 当前 Unix 时间毫秒数；平台时间不可用时返回 0。
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 递归复制目录，跳过构建缓存与版本库。
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), ".git" | ".nsh" | "node_modules" | "target") {
            continue;
        }
        let source = entry.path();
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            std::fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

/// 放大并发用例用的注入延迟：每个安装者拿到包锁后停这么久。
#[cfg(test)]
static LOCK_CREATE_DELAY: std::sync::Mutex<Option<Duration>> = std::sync::Mutex::new(None);

/// 打开或关闭放大用例的锁内延迟。
#[cfg(test)]
pub fn set_lock_create_delay(delay: Option<Duration>) {
    if let Ok(mut held) = LOCK_CREATE_DELAY.lock() {
        *held = delay;
    }
}

#[cfg(test)]
fn lock_create_delay() -> Option<Duration> {
    LOCK_CREATE_DELAY.lock().map(|held| *held).unwrap_or(None)
}

/// 文件的 SHA-256 十六进制；读不到时返回 `None`。
pub fn file_sha256(path: &Path) -> Option<String> {
    use sha2::Digest;
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", sha2::Sha256::digest(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("hctui-store-{tag}-{unique}"))
    }

    #[test]
    fn generations_advance_and_prune_keeps_live_ones() {
        let root = unique_dir("prune");
        let store = Store::open(&root);
        let mut record = Record {
            id: "probe".into(),
            source: Source::Local,
            generation: 1,
            installed_at: now_unix(),
            enabled: true,
            manifest_hash: None,
            origin: None,
            sha256: None,
            key_id: None,
            app: None,
            min_runtime: None,
            capabilities: None,
            module: None,
            grants: None,
        };
        let (generation, dir) = store.prepare_generation("probe").unwrap();
        assert_eq!(generation, 1);
        std::fs::write(dir.join("bundle.js"), "one").unwrap();
        store.set_current("probe", &dir).unwrap();
        store.save_record(&record).unwrap();
        assert_eq!(store.current_dir("probe").as_deref(), Some(dir.as_path()));

        let (generation, dir2) = store.prepare_generation("probe").unwrap();
        assert_eq!(generation, 2);
        record.generation = generation;
        std::fs::write(dir2.join("bundle.js"), "two").unwrap();
        store.set_current("probe", &dir2).unwrap();
        store.save_record(&record).unwrap();
        assert!(store.generation_dir("probe", 1).exists());

        // 旧代际仍在被表面使用时保留。
        assert!(store.prune("probe", &[1]).is_empty());
        assert_eq!(store.prune("probe", &[]), vec![1]);
        assert!(!store.generation_dir("probe", 1).exists());
        assert!(store.generation_dir("probe", 2).exists());
        assert_eq!(store.list().len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    fn record(id: &str, generation: u64) -> Record {
        Record {
            id: id.to_string(),
            source: Source::Local,
            generation,
            installed_at: now_unix(),
            enabled: false,
            manifest_hash: None,
            origin: None,
            sha256: None,
            key_id: None,
            app: None,
            min_runtime: None,
            capabilities: None,
            module: None,
            grants: None,
        }
    }

    #[test]
    fn a_crash_between_copy_and_record_keeps_the_old_generation() {
        let root = unique_dir("crash");
        let store = Store::open(&root);
        let (first, dir) = store.prepare_generation("probe").unwrap();
        std::fs::write(dir.join("bundle.js"), "one").unwrap();
        std::fs::write(dir.join(crate::pkg::PLUGIN_FILE), "manifest_version = 1\n").unwrap();
        store.set_current("probe", &dir).unwrap();
        store.save_record(&record("probe", first)).unwrap();

        // 下一代已分配并复制完，然后在写记录前崩溃（锁随进程消失）。
        let lock = store.lock_package("probe").unwrap();
        let (second, staged) = store.alloc_generation("probe").unwrap();
        assert_eq!(second, 2);
        std::fs::write(staged.join("bundle.js"), "two").unwrap();
        drop(lock);

        let reopened = Store::open(&root);
        assert_eq!(reopened.record("probe").unwrap().generation, first);
        assert_eq!(
            reopened.current_dir("probe").as_deref(),
            Some(dir.as_path())
        );
        assert!(!staged.exists(), "staging directory survived recovery");
        assert!(!reopened.generation_dir("probe", second).exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn store_rebuilds_a_missing_current_from_the_record() {
        let root = unique_dir("rebuild");
        let store = Store::open(&root);
        let (generation, dir) = store.prepare_generation("probe").unwrap();
        std::fs::write(dir.join("bundle.js"), "one").unwrap();
        std::fs::write(dir.join(crate::pkg::PLUGIN_FILE), "manifest_version = 1\n").unwrap();
        store.set_current("probe", &dir).unwrap();
        store.save_record(&record("probe", generation)).unwrap();
        std::fs::remove_file(root.join("current").join("probe")).unwrap();

        let reopened = Store::open(&root);
        assert_eq!(
            reopened.current_dir("probe").as_deref(),
            Some(dir.as_path())
        );
        assert!(
            reopened.recovery_issues().is_empty(),
            "{:?}",
            reopened.recovery_issues()
        );

        // 幂等：再开一次仍指同一代际。
        let again = Store::open(&root);
        assert_eq!(again.current_dir("probe").as_deref(), Some(dir.as_path()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn store_reports_a_current_without_record_instead_of_deleting_it() {
        let root = unique_dir("orphan");
        let store = Store::open(&root);
        let (_, dir) = store.prepare_generation("probe").unwrap();
        store.set_current("probe", &dir).unwrap();

        let reopened = Store::open(&root);
        let issues = reopened.recovery_issues();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].id.as_deref(), Some("probe"));
        assert!(
            issues[0].message.contains("no record"),
            "{}",
            issues[0].message
        );
        assert_eq!(
            reopened.current_dir("probe").as_deref(),
            Some(dir.as_path()),
            "pointer was deleted instead of reported"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn old_records_without_newer_fields_still_load() {
        let root = unique_dir("legacy");
        let store = Store::open(&root);
        std::fs::create_dir_all(root.join("records")).unwrap();
        std::fs::write(
            root.join("records").join("probe.json"),
            r#"{"id":"probe","source":"local","generation":3,"installedAt":1,"enabled":true}"#,
        )
        .unwrap();
        let record = store.record("probe").expect("legacy record loads");
        assert_eq!(record.generation, 3);
        assert!(record.enabled);
        assert_eq!(record.manifest_hash, None);
        std::fs::remove_dir_all(root).ok();
    }

    /// 锁文件内容为「空」「垃圾字符串」「不存在」三种情形都要能接管：认不出
    /// 持有者就不是「别人正在装」。unix 上互斥由 `flock` 提供，内容只是诊断；
    /// 非 unix 上按内容加年龄判残留，空与垃圾直接接管。
    #[test]
    fn a_lock_with_an_unknown_holder_is_taken_over() {
        for content in [Some(""), Some("not-a-pid\n"), None] {
            let root = unique_dir("unknown-holder");
            let store = Store::open(&root);
            let dir = root.join("store").join("probe");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(".lock");
            match content {
                Some(text) => std::fs::write(&path, text).unwrap(),
                None => assert!(!path.exists()),
            }
            let lock = store
                .lock_package("probe")
                .unwrap_or_else(|error| panic!("lock content {content:?}: {error}"));
            drop(lock);
            assert!(
                !path.exists(),
                "lock content {content:?} left the lock file behind"
            );
            std::fs::remove_dir_all(root).ok();
        }
    }

    /// 目录在、清单不在：恢复不把指针指向它，已经指进去的摘掉；只报问题，
    /// 记录与目录都不删。清单落地后下一次打开自动把指针修回来。
    #[test]
    fn a_generation_without_its_manifest_is_not_published() {
        let root = unique_dir("incomplete");
        let store = Store::open(&root);
        let (generation, dir) = store.prepare_generation("probe").unwrap();
        store.set_current("probe", &dir).unwrap();
        store.save_record(&record("probe", generation)).unwrap();

        let reopened = Store::open(&root);
        let issues = reopened.recovery_issues();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].message.contains("incomplete"),
            "{}",
            issues[0].message
        );
        assert_eq!(
            reopened.current_dir("probe"),
            None,
            "pointer still points at a half-written generation"
        );
        assert!(!root.join("current").join("probe").exists());
        // 不删数据：记录还在，代际目录还在，用户重装即可覆盖。
        assert_eq!(reopened.record("probe").unwrap().generation, generation);
        assert!(dir.is_dir());

        std::fs::write(dir.join(crate::pkg::PLUGIN_FILE), "manifest_version = 1\n").unwrap();
        let repaired = Store::open(&root);
        assert!(
            repaired.recovery_issues().is_empty(),
            "{:?}",
            repaired.recovery_issues()
        );
        assert_eq!(
            repaired.current_dir("probe").as_deref(),
            Some(dir.as_path())
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// 记录临时文件跟持有者走：正在写的不能删，持有者已消失才当残留。
    #[test]
    fn record_temp_files_follow_their_writer() {
        let root = unique_dir("record-temp");
        let _store = Store::open(&root);
        let records = root.join("records");
        std::fs::create_dir_all(&records).unwrap();
        let live = records.join(format!(".tmp-{}-0-probe.json", std::process::id()));
        let dead = records.join(".tmp-4294967295-0-probe.json");
        std::fs::write(&live, "half").unwrap();
        std::fs::write(&dead, "half").unwrap();
        let reopened = Store::open(&root);
        assert!(live.exists(), "a live writer's temp file was removed");
        assert!(!dead.exists(), "a dead writer's temp file survived");
        assert!(
            reopened.recovery_issues().is_empty(),
            "{:?}",
            reopened.recovery_issues()
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_dead_holders_lock_is_taken_over() {
        let root = unique_dir("lock");
        let store = Store::open(&root);
        let dir = root.join("store").join("probe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".lock"), "4294967295").unwrap();
        let lock = store.lock_package("probe").expect("stale lock taken over");
        drop(lock);
        assert!(!dir.join(".lock").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn retained_generations_are_not_reused_after_uninstall() {
        let root = unique_dir("retained");
        let store = Store::open(&root);
        let (first, dir) = store.prepare_generation("probe").unwrap();
        assert_eq!(first, 1);
        std::fs::write(dir.join("bundle.js"), "one").unwrap();
        store.detach("probe").unwrap();
        // 记录已删、代际目录还在：下一代必须跳过它，避免覆盖运行中的代码。
        assert_eq!(store.next_generation("probe"), 2);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn linked_current_points_at_the_source_directory() {
        let root = unique_dir("linked");
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let store = Store::open(root.join("store-root"));
        store.set_current("probe", &source).unwrap();
        assert_eq!(store.current_dir("probe"), Some(source.clone()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn logs_rotate_once_past_a_megabyte() {
        let root = unique_dir("logs");
        let store = Store::open(&root);
        store.append_log("probe", "first").unwrap();
        assert_eq!(store.log_tail("probe", 10).unwrap(), "first");
        let filler = "x".repeat(1024 * 1024 + 1);
        store.append_log("probe", &filler).unwrap();
        store.append_log("probe", "after").unwrap();
        assert_eq!(store.log_tail("probe", 10).unwrap(), "after");
        assert!(store.log_dir("probe").join("engine.log.1").exists());
        std::fs::remove_dir_all(root).ok();
    }
}
