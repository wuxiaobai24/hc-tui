//! 通用代际存储：`store/<id>/<generation>`、`current/<id>` 指针与
//! `records/<id>.json`。
//!
//! 每个模块的代码按代际存放在 `store/<id>/<generation>/`，`current/<id>`
//! 指向当前代际；记录文件保存来源、代际、启用状态与安装时间。运行中的
//! 表面继续使用已加载的代码，旧代际在最后一个引用关闭后回收。

use std::io;
use std::path::{Path, PathBuf};

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
}

/// 代际存储的根：`<state>/mods/`。
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// 打开或创建存储根；目录按需创建。
    pub fn open(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    /// 存储根。
    pub fn root(&self) -> &Path {
        &self.root
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

    /// 写一条记录。
    pub fn save_record(&self, record: &Record) -> io::Result<()> {
        std::fs::create_dir_all(self.records_dir())?;
        let text = serde_json::to_string_pretty(record).map_err(io::Error::other)?;
        std::fs::write(self.record_path(&record.id), text)
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

    /// 把 `current/<id>` 指向某个代际目录。
    pub fn set_current(&self, id: &str, dir: &Path) -> io::Result<()> {
        let link = self.current_link(id);
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::symlink_metadata(&link).is_ok() {
            std::fs::remove_file(&link)?;
        }
        let target = std::path::absolute(dir)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link)?;
        #[cfg(not(unix))]
        std::fs::write(&link, target.to_string_lossy().as_bytes())?;
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
            if !existing.is_empty() && !existing.ends_with('\n') {
                text.push_str(&existing);
            }
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

/// 当前 Unix 时间秒数；平台时间不可用时返回 0。
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
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
