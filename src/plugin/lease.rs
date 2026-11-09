//! 插件代码代际的显式租约：pin 文件 + 显式 GC。
//!
//! 代际目录一旦生成就不可变。是否回收只看两类事实：存储记录仍指向它，或者
//! 仍有 pin 持有者。持有者分两种：一次动作调用（排队中也持有）与带
//! core/session 身份的终端地址。Pane 关闭、客户端 PID 消失都不释放终端 pin；
//! 只有 core 侧报告该终端结束的证据才允许 `gc` 释放它。拿不到证据时代际
//! 保守保留，由 [`gc`] 显式回收不再有任何持有者的代际。
//!
//! 布局：`<state>/plugins/pins/<package>/<generation>/<owner>.json`。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::Digest;

use crate::config::Config;
use crate::store::Store;

/// 一次调用的 pin 持有者：排队中与运行中都算。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InvocationOwner {
    /// 请求 ID。
    pub request: String,
}

/// 一个终端地址；`session` 与 `machine` 构成 core 侧身份。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TerminalAddress {
    /// core 会话。
    pub session: String,
    /// 机器名；本机为 `local`。
    pub machine: String,
    /// 终端地址；尚未 attach 时为 `pending/<request>`。
    pub address: String,
}

impl TerminalAddress {
    /// 从 core 返回的地址（`<machine>/<id>` 或裸 id）构造。
    pub fn from_core(session: &str, address: &str) -> TerminalAddress {
        let (machine, rest) = address.split_once('/').unwrap_or(("local", address));
        TerminalAddress {
            session: session.to_string(),
            machine: machine.to_string(),
            address: rest.to_string(),
        }
    }

    /// 证据与 pin 文件使用的稳定键。
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.session, self.machine, self.address)
    }

    /// attach 之前登记的占位地址。
    pub fn is_pending(&self) -> bool {
        self.address.starts_with("pending/")
    }
}

/// pin 的持有者。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceOwner {
    /// 一次动作调用。
    Invocation(InvocationOwner),
    /// 一个终端。
    Terminal(TerminalAddress),
}

impl ResourceOwner {
    fn key(&self) -> String {
        match self {
            ResourceOwner::Invocation(owner) => format!("invocation:{}", owner.request),
            ResourceOwner::Terminal(owner) => format!("terminal:{}", owner.key()),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            ResourceOwner::Invocation(owner) => json!({
                "kind": "invocation",
                "request": owner.request,
            }),
            ResourceOwner::Terminal(owner) => json!({
                "kind": "terminal",
                "session": owner.session,
                "machine": owner.machine,
                "address": owner.address,
            }),
        }
    }

    fn from_json(value: &Value) -> Option<ResourceOwner> {
        match value.get("kind").and_then(Value::as_str)? {
            "invocation" => Some(ResourceOwner::Invocation(InvocationOwner {
                request: value.get("request").and_then(Value::as_str)?.to_string(),
            })),
            "terminal" => Some(ResourceOwner::Terminal(TerminalAddress {
                session: value.get("session").and_then(Value::as_str)?.to_string(),
                machine: value.get("machine").and_then(Value::as_str)?.to_string(),
                address: value.get("address").and_then(Value::as_str)?.to_string(),
            })),
            _ => None,
        }
    }
}

/// 一份 pin；删除文件即释放。可克隆，重复释放是幂等的。
#[derive(Debug, Clone)]
pub struct GenerationPin {
    path: PathBuf,
    /// 包 ID。
    pub package: String,
    /// 代际。
    pub generation: u64,
    /// 代际根目录。
    pub root: PathBuf,
    /// 持有者。
    pub owner: ResourceOwner,
}

impl GenerationPin {
    /// 释放这份 pin；文件已不存在时也算成功。
    pub fn release(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// core 侧给出的回收证据。
#[derive(Debug, Clone, Default)]
pub struct RetentionEvidence {
    /// 已确认结束的终端（[`TerminalAddress::key`]）。
    pub ended_terminals: BTreeSet<String>,
}

impl RetentionEvidence {
    /// 记一个已结束的终端地址。
    pub fn ended(&mut self, address: &TerminalAddress) {
        self.ended_terminals.insert(address.key());
    }

    /// 是否为空的证据；空的证据不会释放任何终端 pin。
    pub fn is_empty(&self) -> bool {
        self.ended_terminals.is_empty()
    }
}

/// 一次回收的结果。
#[derive(Debug, Clone, Default)]
pub struct GcReport {
    /// 被证据释放的 pin 持有者。
    pub released: Vec<ResourceOwner>,
    /// 删除的代际。
    pub generations: Vec<u64>,
}

impl GcReport {
    /// 是否有任何变化。
    pub fn is_empty(&self) -> bool {
        self.released.is_empty() && self.generations.is_empty()
    }
}

fn pins_root(config: &Config) -> PathBuf {
    config.plugins_state_dir().join("pins")
}

fn owner_stem(owner: &ResourceOwner) -> String {
    let readable: String = owner
        .key()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let digest = format!("{:x}", sha2::Sha256::digest(owner.key().as_bytes()));
    format!("{}-{}", readable, &digest[..8])
}

fn pin_path(config: &Config, package: &str, generation: u64, owner: &ResourceOwner) -> PathBuf {
    pins_root(config)
        .join(package)
        .join(generation.to_string())
        .join(format!("{}.json", owner_stem(owner)))
}

/// 登记一份符合条件的 pin。
pub fn pin(
    config: &Config,
    package: &str,
    generation: u64,
    root: &Path,
    owner: ResourceOwner,
) -> std::io::Result<GenerationPin> {
    let path = pin_path(config, package, generation, &owner);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = json!({
        "package": package,
        "generation": generation,
        "root": root.display().to_string(),
        "owner": owner.to_json(),
    });
    std::fs::write(&path, serde_json::to_string_pretty(&body)?)?;
    Ok(GenerationPin {
        path,
        package: package.to_string(),
        generation,
        root: root.to_path_buf(),
        owner,
    })
}

/// 把一个持有者的 pin 换成另一个（终端拿到 core 地址后升级身份）。
pub fn repin(
    config: &Config,
    package: &str,
    generation: u64,
    root: &Path,
    from: &ResourceOwner,
    to: ResourceOwner,
) -> std::io::Result<GenerationPin> {
    let previous = pin_path(config, package, generation, from);
    let _ = std::fs::remove_file(&previous);
    pin(config, package, generation, root, to)
}

fn load_file(path: &Path) -> Option<GenerationPin> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    Some(GenerationPin {
        path: path.to_path_buf(),
        package: value.get("package").and_then(Value::as_str)?.to_string(),
        generation: value.get("generation").and_then(Value::as_u64)?,
        root: PathBuf::from(
            value
                .get("root")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        owner: ResourceOwner::from_json(value.get("owner")?)?,
    })
}

/// 某个包的全部 pin。
pub fn pins(config: &Config, package: &str) -> Vec<GenerationPin> {
    let mut out = Vec::new();
    let root = pins_root(config).join(package);
    let Ok(generations) = std::fs::read_dir(&root) else {
        return out;
    };
    for generation in generations.filter_map(Result::ok) {
        let Ok(entries) = std::fs::read_dir(generation.path()) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            if let Some(pin) = load_file(&entry.path()) {
                out.push(pin);
            }
        }
    }
    out.sort_by(|a, b| {
        a.package
            .cmp(&b.package)
            .then(a.generation.cmp(&b.generation))
            .then(a.owner.cmp(&b.owner))
    });
    out
}

/// 存储里出现过的全部包 ID：记录、`current` 指针、代际目录与 pin 的并集。
///
/// 只出现在 `store/<id>/` 里的包就是崩溃或手工删除记录后的孤儿代际，
/// 显式 GC 必须能看见它们。
pub fn packages(config: &Config) -> Vec<String> {
    let store = Store::open(config.plugins_state_dir());
    let mut ids: BTreeSet<String> = store.list().into_iter().map(|record| record.id).collect();
    for dir in [
        store.root().join("store"),
        store.root().join("current"),
        pins_root(config),
    ] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            ids.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    ids.into_iter().collect()
}

/// 仍被 pin 引用的代际。
pub fn live_generations(config: &Config, package: &str) -> Vec<u64> {
    let mut generations: Vec<u64> = pins(config, package)
        .into_iter()
        .map(|pin| pin.generation)
        .collect();
    generations.sort_unstable();
    generations.dedup();
    generations
}

/// 按证据回收：先释放已确认结束的终端 pin，再删除没有持有者也没有记录指向的代际。
///
/// 空证据不会释放任何终端 pin；已经没有任何持有者、也没有记录指向的代际仍然
/// 会被删除，因为它们不可能再被使用。
pub fn gc(config: &Config, package: &str, evidence: &RetentionEvidence) -> GcReport {
    let store = Store::open(config.plugins_state_dir());
    let mut report = GcReport::default();
    let mut live: BTreeSet<u64> = BTreeSet::new();
    for pin in pins(config, package) {
        if let ResourceOwner::Terminal(address) = &pin.owner {
            if evidence.ended_terminals.contains(&address.key()) {
                pin.release();
                report.released.push(pin.owner.clone());
                continue;
            }
        }
        live.insert(pin.generation);
    }
    let current = store.record(package).map(|record| record.generation);
    let dir = store.root().join("store").join(package);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return report;
    };
    let mut generations: Vec<u64> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u64>().ok())
        .collect();
    generations.sort_unstable();
    for generation in generations {
        if Some(generation) == current || live.contains(&generation) {
            continue;
        }
        if std::fs::remove_dir_all(store.generation_dir(package, generation)).is_ok() {
            report.generations.push(generation);
        }
    }
    if report.generations.contains(&1)
        && std::fs::read_dir(&dir).is_ok_and(|mut e| e.next().is_none())
    {
        // 空的代际目录留着没有意义；记录与 pin 都不需要它。
        let _ = std::fs::remove_dir_all(&dir);
    }
    report
}

/// 对全部包做一次显式 GC。
pub fn gc_all(config: &Config, evidence: &RetentionEvidence) -> Vec<(String, GcReport)> {
    packages(config)
        .into_iter()
        .map(|package| {
            let report = gc(config, &package, evidence);
            (package, report)
        })
        .filter(|(_, report)| !report.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Record, Source};

    fn unique_dir(tag: &str) -> PathBuf {
        crate::testutil::unique_dir("hctui-plugin-lease", tag)
    }

    fn config_in(root: &Path) -> Config {
        Config {
            config_path: root.join("config").join("config.toml"),
            state_path: root.join("state").join("state.json"),
            ..Config::default()
        }
    }

    fn record(generation: u64) -> Record {
        Record {
            id: "dev.example.git".into(),
            source: Source::Local,
            generation,
            installed_at: 0,
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
        }
    }

    fn install_generation(config: &Config) -> (u64, PathBuf) {
        let store = Store::open(config.plugins_state_dir());
        let (generation, dir) = store.prepare_generation("dev.example.git").unwrap();
        std::fs::write(dir.join("bin"), "one").unwrap();
        store.set_current("dev.example.git", &dir).unwrap();
        store.save_record(&record(generation)).unwrap();
        (generation, dir)
    }

    fn terminal(address: &str) -> TerminalAddress {
        TerminalAddress::from_core("s1", address)
    }

    #[test]
    fn a_pane_close_is_not_evidence_and_keeps_the_generation() {
        let root = unique_dir("pane");
        let config = config_in(&root);
        let (generation, dir) = install_generation(&config);
        let pin = pin(
            &config,
            "dev.example.git",
            generation,
            &dir,
            ResourceOwner::Terminal(terminal("local/t7")),
        )
        .unwrap();
        // 只断视图：pin 仍在。
        drop(pin);
        Store::open(config.plugins_state_dir())
            .detach("dev.example.git")
            .unwrap();
        let report = gc(&config, "dev.example.git", &RetentionEvidence::default());
        assert!(report.is_empty(), "{report:?}");
        assert!(dir.join("bin").exists());

        // core 说这条终端结束了：这才允许释放并回收。
        let mut evidence = RetentionEvidence::default();
        evidence.ended(&terminal("local/t7"));
        let report = gc(&config, "dev.example.git", &evidence);
        assert_eq!(report.released.len(), 1, "{report:?}");
        assert_eq!(report.generations, vec![generation]);
        assert!(!dir.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn queued_invocations_hold_their_generation() {
        let root = unique_dir("invocation");
        let config = config_in(&root);
        let (generation, dir) = install_generation(&config);
        let pin = pin(
            &config,
            "dev.example.git",
            generation,
            &dir,
            ResourceOwner::Invocation(InvocationOwner {
                request: "p1".into(),
            }),
        )
        .unwrap();
        assert_eq!(
            live_generations(&config, "dev.example.git"),
            vec![generation]
        );
        Store::open(config.plugins_state_dir())
            .detach("dev.example.git")
            .unwrap();
        // 端点证据不会释放动作 pin。
        let mut evidence = RetentionEvidence::default();
        evidence.ended(&terminal("local/t7"));
        assert!(gc(&config, "dev.example.git", &evidence).is_empty());
        assert!(dir.exists());

        pin.release();
        let report = gc(&config, "dev.example.git", &RetentionEvidence::default());
        assert_eq!(report.generations, vec![generation]);
        assert!(!dir.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn two_generations_are_kept_side_by_side() {
        let root = unique_dir("generations");
        let config = config_in(&root);
        let store = Store::open(config.plugins_state_dir());
        let (first, dir) = store.prepare_generation("dev.example.git").unwrap();
        store.set_current("dev.example.git", &dir).unwrap();
        pin(
            &config,
            "dev.example.git",
            first,
            &dir,
            ResourceOwner::Terminal(terminal("local/t1")),
        )
        .unwrap();
        let (second, dir2) = store.prepare_generation("dev.example.git").unwrap();
        assert_eq!(second, first + 1);
        pin(
            &config,
            "dev.example.git",
            second,
            &dir2,
            ResourceOwner::Terminal(terminal("local/t2")),
        )
        .unwrap();
        assert_eq!(
            live_generations(&config, "dev.example.git"),
            vec![first, second]
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn repin_moves_the_terminal_to_its_core_address() {
        let root = unique_dir("repin");
        let config = config_in(&root);
        let (generation, dir) = install_generation(&config);
        let pending = ResourceOwner::Terminal(TerminalAddress {
            session: "s1".into(),
            machine: "local".into(),
            address: "pending/tp1".into(),
        });
        pin(
            &config,
            "dev.example.git",
            generation,
            &dir,
            pending.clone(),
        )
        .unwrap();
        repin(
            &config,
            "dev.example.git",
            generation,
            &dir,
            &pending,
            ResourceOwner::Terminal(terminal("local/t9")),
        )
        .unwrap();
        let owners: Vec<ResourceOwner> = pins(&config, "dev.example.git")
            .into_iter()
            .map(|pin| pin.owner)
            .collect();
        assert_eq!(owners, vec![ResourceOwner::Terminal(terminal("local/t9"))]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gc_all_reclaims_generations_without_record_or_pin() {
        let root = unique_dir("orphan");
        let config = config_in(&root);
        let store = Store::open(config.plugins_state_dir());
        let (generation, dir) = store.prepare_generation("dev.example.git").unwrap();
        std::fs::write(dir.join("bin"), "one").unwrap();
        // 孤儿：没有记录、没有指针、没有 pin。
        assert!(packages(&config).contains(&"dev.example.git".to_string()));
        let reports = gc_all(&config, &RetentionEvidence::default());
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert_eq!(reports[0].1.generations, vec![generation]);
        assert!(!dir.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn stale_pins_of_other_processes_survive_until_evidence_arrives() {
        let root = unique_dir("stale");
        let config = config_in(&root);
        let (generation, dir) = install_generation(&config);
        pin(
            &config,
            "dev.example.git",
            generation,
            &dir,
            ResourceOwner::Terminal(terminal("local/t4")),
        )
        .unwrap();
        Store::open(config.plugins_state_dir())
            .detach("dev.example.git")
            .unwrap();
        assert!(gc(&config, "dev.example.git", &RetentionEvidence::default()).is_empty());
        assert!(dir.exists());
        std::fs::remove_dir_all(root).ok();
    }
}
