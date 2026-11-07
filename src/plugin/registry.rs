//! 插件注册表：配置目录里的开发插件与安装存储的当前代际。
//!
//! 动作名 `<package-id>.<action-id>`；注册表只收清单合法、`host_api` 匹配且
//! 当前平台受支持的包。安装、链接与卸载复用 [`crate::store::Store`] 的代际
//! 目录、`current` 指针与 records，卸载后动作随之消失。

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::pkg::{self, Capabilities, ModuleInfo};
use crate::store::{copy_tree, file_sha256, now_unix, Record, Source, Store};

use super::manifest::Manifest;

/// 安装、更新与链接时的启用策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnablePolicy {
    /// 保留已有记录的状态；没有记录时默认不启用。
    #[default]
    Keep,
    /// 显式启用。
    Enable,
    /// 显式停用。
    Disable,
}

impl EnablePolicy {
    /// 把策略落成一个 `enabled` 值。
    pub fn resolve(self, existing: Option<&Record>) -> bool {
        match self {
            EnablePolicy::Keep => existing.is_some_and(|record| record.enabled),
            EnablePolicy::Enable => true,
            EnablePolicy::Disable => false,
        }
    }
}

/// 一个可用插件。
#[derive(Debug, Clone)]
pub struct Plugin {
    /// 包 ID。
    pub id: String,
    /// 代码根目录（当前代际或配置目录中的源目录）。
    pub root: PathBuf,
    /// 来源。
    pub source: Source,
    /// 代际。
    pub generation: u64,
    /// 是否启用。
    pub enabled: bool,
    /// 统一清单：动作、终端入口与可选模块。
    pub manifest: Manifest,
    /// 已安装来源的记录；开发目录里的包没有记录。
    pub record: Option<Record>,
}

impl Plugin {
    /// 由清单推导的能力。
    pub fn capabilities(&self) -> Capabilities {
        self.manifest.capabilities()
    }
}

/// 一个贡献界面模块的包。
#[derive(Debug, Clone)]
pub struct ModuleEntry {
    /// 包 ID。
    pub id: String,
    /// 代码根目录。
    pub root: PathBuf,
    /// 来源。
    pub source: Source,
    /// 代际。
    pub generation: u64,
    /// 是否启用。
    pub enabled: bool,
    /// 模块摘要。
    pub module: ModuleInfo,
    /// 声明的宿主方法与事件。
    pub grants: Vec<String>,
}

/// `packages list --json` 的一项。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PackageEntry {
    /// 包 ID。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 版本。
    pub version: String,
    /// 来源。
    pub source: String,
    /// 代际。
    pub generation: u64,
    /// 是否启用。
    pub enabled: bool,
    /// 能力。
    pub capabilities: Capabilities,
    /// 模块摘要；没有模块时为 `null`。
    pub module: Option<ModuleInfo>,
    /// 这个包的加载问题。
    pub issues: Vec<String>,
}

/// 加载失败的插件或跳过原因。
#[derive(Debug, Clone)]
pub struct Issue {
    /// 目录。
    pub path: PathBuf,
    /// 可识别的包 ID。
    pub id: Option<String>,
    /// 原因。
    pub message: String,
}

/// 一个可调用的动作。
#[derive(Debug, Clone)]
pub struct ActionInfo {
    /// `<package>.<action>`。
    pub name: String,
    /// 包 ID。
    pub package: String,
    /// 动作 ID。
    pub action: String,
    /// 标题。
    pub title: String,
    /// 描述。
    pub description: String,
    /// 上下文。
    pub contexts: Vec<String>,
    /// 参数 schema。
    pub args: Option<serde_json::Value>,
    /// 代码根目录。
    pub root: PathBuf,
    /// 代码代际。
    pub generation: u64,
    /// 超时。
    pub timeout: std::time::Duration,
    /// 命令参数。
    pub argv: Vec<String>,
}

/// 一个终端入口：`plugins pane open` 与预览使用。
#[derive(Debug, Clone)]
pub struct TerminalInfo {
    /// `<package>.<terminal>`。
    pub name: String,
    /// 包 ID。
    pub package: String,
    /// 终端 ID。
    pub terminal: String,
    /// 标题。
    pub title: String,
    /// 放置方式。
    pub placement: String,
    /// 命令参数。
    pub argv: Vec<String>,
    /// 需要的程序。
    pub requires_programs: Vec<String>,
    /// 代码根目录（`./` 相对程序的基准）。
    pub root: PathBuf,
    /// 代际。
    pub generation: u64,
    /// 来源。
    pub source: Source,
}

/// 一次注册表加载的全部结果。
#[derive(Debug, Clone, Default)]
pub struct Registry {
    /// 可用插件。
    pub plugins: Vec<Plugin>,
    /// 加载失败或跳过的包。
    pub issues: Vec<Issue>,
}

impl Registry {
    /// 扫描安装记录与两个开发目录（插件与模块）。
    ///
    /// 来源：插件与模块两个 store 的已安装代际、`<config>/plugins/`
    /// （开发插件）、`<config>/mods/`（开发模块）。按 id 去重：已安装代际优先。
    pub fn load(config: &Config) -> Registry {
        let disabled = disabled_set(config);
        let mut registry = Registry::default();
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for state in [config.plugins_state_dir(), config.mods_state_dir()] {
            let store = Store::open(state);
            for issue in store.recovery_issues() {
                registry.issues.push(Issue {
                    path: issue.path.clone(),
                    id: issue.id.clone(),
                    message: issue.message.clone(),
                });
            }
            for record in store.list() {
                let Some(root) = store.current_dir(&record.id) else {
                    registry.issues.push(Issue {
                        path: store.root().join("current").join(&record.id),
                        id: Some(record.id.clone()),
                        message: "current generation is missing".to_string(),
                    });
                    continue;
                };
                let loaded = load_plugin(
                    &root,
                    record.source,
                    record.generation,
                    record.enabled,
                    Some(record.clone()),
                    &mut registry.issues,
                );
                match loaded {
                    Ok(plugin) => {
                        if plugin.id != record.id {
                            registry.issues.push(Issue {
                                path: root,
                                id: Some(record.id.clone()),
                                message: format!(
                                    "record says {}, manifest says {}",
                                    record.id, plugin.id
                                ),
                            });
                            continue;
                        }
                        if !seen.insert(plugin.id.clone()) {
                            registry.issues.push(Issue {
                                path: plugin.root.clone(),
                                id: Some(plugin.id.clone()),
                                message: "duplicate package id; the installed generation wins"
                                    .to_string(),
                            });
                            continue;
                        }
                        registry.push(plugin);
                    }
                    Err(issue) => registry.issues.push(issue),
                }
            }
        }
        for dir in [config.plugins_dir(), config.mods_dir()] {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path())
                .collect();
            entries.sort();
            for root in entries {
                let loaded =
                    load_plugin(&root, Source::Linked, 1, true, None, &mut registry.issues);
                match loaded {
                    Ok(plugin) => {
                        if seen.contains(&plugin.id) {
                            registry.issues.push(Issue {
                                path: root,
                                id: Some(plugin.id),
                                message: "duplicate package id; the installed generation wins"
                                    .to_string(),
                            });
                            continue;
                        }
                        seen.insert(plugin.id.clone());
                        let enabled = plugin.enabled && !disabled.contains_key(&plugin.id);
                        registry.push(Plugin { enabled, ..plugin });
                    }
                    Err(issue) => registry.issues.push(issue),
                }
            }
        }
        registry
    }

    fn push(&mut self, plugin: Plugin) {
        if !plugin.manifest.supports_this_platform() {
            self.issues.push(Issue {
                path: plugin.root.clone(),
                id: Some(plugin.id.clone()),
                message: format!(
                    "skipped: platform not supported (manifest: {})",
                    plugin.manifest.platforms().join(", ")
                ),
            });
            return;
        }
        self.plugins.push(plugin);
    }

    /// 启用的动作，按完整名排序。
    pub fn actions(&self) -> Vec<ActionInfo> {
        self.collect_actions(true)
    }

    /// 全部动作，包括已停用的包；闸门判定用。
    pub fn actions_any(&self) -> Vec<ActionInfo> {
        self.collect_actions(false)
    }

    fn collect_actions(&self, enabled_only: bool) -> Vec<ActionInfo> {
        let mut actions = Vec::new();
        for plugin in self
            .plugins
            .iter()
            .filter(|plugin| plugin.enabled || !enabled_only)
        {
            for action in &plugin.manifest.actions {
                actions.push(ActionInfo {
                    name: plugin.manifest.action_name(action),
                    package: plugin.id.clone(),
                    action: action.id().to_string(),
                    title: action.title().to_string(),
                    description: action.description().to_string(),
                    contexts: action.contexts().iter().map(|s| s.to_string()).collect(),
                    args: action.args_schema(),
                    root: plugin.root.clone(),
                    generation: plugin.generation,
                    timeout: action.timeout(),
                    argv: action.argv().iter().map(|s| s.to_string()).collect(),
                });
            }
        }
        actions.sort_by(|a, b| a.name.cmp(&b.name));
        actions
    }

    /// 终端入口。
    pub fn terminals(&self) -> Vec<TerminalInfo> {
        self.collect_terminals(true)
    }

    /// 全部终端入口，包括已停用的包；闸门判定用。
    pub fn terminals_any(&self) -> Vec<TerminalInfo> {
        self.collect_terminals(false)
    }

    fn collect_terminals(&self, enabled_only: bool) -> Vec<TerminalInfo> {
        let mut terminals = Vec::new();
        for plugin in self
            .plugins
            .iter()
            .filter(|plugin| plugin.enabled || !enabled_only)
        {
            for terminal in &plugin.manifest.terminals {
                terminals.push(TerminalInfo {
                    name: format!("{}.{}", plugin.id, terminal.id()),
                    package: plugin.id.clone(),
                    terminal: terminal.id().to_string(),
                    title: terminal.title().to_string(),
                    placement: terminal.placement().to_string(),
                    argv: terminal.argv().iter().map(|s| s.to_string()).collect(),
                    requires_programs: terminal
                        .requires_programs()
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                    root: plugin.root.clone(),
                    generation: plugin.generation,
                    source: plugin.source,
                });
            }
        }
        terminals.sort_by(|a, b| a.name.cmp(&b.name));
        terminals
    }

    /// 已加载但被停用的包中拥有这个名字动作或终端入口的那个包。
    pub fn disabled_owner(&self, name: &str) -> Option<String> {
        self.plugins
            .iter()
            .filter(|plugin| !plugin.enabled)
            .find(|plugin| {
                plugin
                    .manifest
                    .actions
                    .iter()
                    .any(|action| plugin.manifest.action_name(action) == name)
                    || plugin
                        .manifest
                        .terminals
                        .iter()
                        .any(|terminal| format!("{}.{}", plugin.id, terminal.id()) == name)
            })
            .map(|plugin| plugin.id.clone())
    }

    /// 按完整名查找动作。
    pub fn action(&self, name: &str) -> Option<ActionInfo> {
        self.actions().into_iter().find(|entry| entry.name == name)
    }

    /// 该完整名是否被插件动作占用。
    pub fn owns(&self, name: &str) -> bool {
        self.action(name).is_some()
    }

    /// 按完整名查找终端入口。
    pub fn terminal(&self, name: &str) -> Option<TerminalInfo> {
        self.terminals()
            .into_iter()
            .find(|entry| entry.name == name)
    }

    /// 按 id 查找包。
    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|plugin| plugin.id == id)
    }

    /// 启用的界面模块，按 id 排序。
    pub fn modules(&self) -> Vec<ModuleEntry> {
        let mut modules: Vec<ModuleEntry> = self
            .plugins
            .iter()
            .filter(|plugin| plugin.enabled)
            .filter_map(|plugin| {
                let module = plugin.manifest.module_info()?;
                Some(ModuleEntry {
                    id: plugin.id.clone(),
                    root: plugin.root.clone(),
                    source: plugin.source,
                    generation: plugin.generation,
                    enabled: plugin.enabled,
                    module,
                    grants: plugin.manifest.grants(),
                })
            })
            .collect();
        modules.sort_by(|a, b| a.id.cmp(&b.id));
        modules
    }

    /// 已加载包集合的内存指纹；磁盘指纹见 [`revision`]。
    pub fn revision(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for plugin in &self.plugins {
            plugin.id.hash(&mut hasher);
            plugin.root.hash(&mut hasher);
            plugin.generation.hash(&mut hasher);
            plugin.enabled.hash(&mut hasher);
            plugin.manifest.name().hash(&mut hasher);
            plugin.manifest.actions.len().hash(&mut hasher);
            plugin.manifest.terminals.len().hash(&mut hasher);
            plugin.manifest.module.is_some().hash(&mut hasher);
        }
        hasher.finish()
    }

    /// 这个包的加载问题。
    pub fn issues_for(&self, id: &str) -> Vec<String> {
        self.issues
            .iter()
            .filter(|issue| issue.id.as_deref() == Some(id))
            .map(|issue| issue.message.clone())
            .collect()
    }

    /// `packages list` 的数据：两个 store 的记录按 id 合并，能力恒有。
    ///
    /// 旧记录没有 `capabilities` 字段时按清单回填，不进 `issues`。
    pub fn entries(&self, config: &Config) -> Vec<PackageEntry> {
        let mut records: Vec<Record> = Vec::new();
        for state in [config.plugins_state_dir(), config.mods_state_dir()] {
            records.extend(Store::open(state).list());
        }
        records.sort_by(|a, b| a.id.cmp(&b.id));
        records.dedup_by(|a, b| a.id == b.id);
        records
            .into_iter()
            .map(|record| {
                let plugin = self.get(&record.id);
                let capabilities = record
                    .capabilities
                    .or_else(|| plugin.map(Plugin::capabilities))
                    .unwrap_or_default();
                let module = plugin
                    .and_then(|plugin| plugin.manifest.module_info())
                    .or_else(|| record.module.clone());
                let app_field = |key: &str| {
                    record
                        .app
                        .as_ref()
                        .and_then(|app| app.get(key))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                };
                PackageEntry {
                    id: record.id.clone(),
                    name: plugin
                        .map(|plugin| plugin.manifest.name().to_string())
                        .or_else(|| app_field("name"))
                        .unwrap_or_else(|| record.id.clone()),
                    version: plugin
                        .map(|plugin| plugin.manifest.version().to_string())
                        .or_else(|| app_field("version"))
                        .unwrap_or_default(),
                    source: record.source.as_str().to_string(),
                    generation: record.generation,
                    enabled: record.enabled,
                    capabilities,
                    module,
                    issues: self.issues_for(&record.id),
                }
            })
            .collect()
    }
}

/// 统一包注册表；[`Registry`] 是它的旧名字。
pub type PackageRegistry = Registry;

fn load_plugin(
    root: &Path,
    source: Source,
    generation: u64,
    enabled: bool,
    record: Option<Record>,
    issues: &mut Vec<Issue>,
) -> Result<Plugin, Issue> {
    let loaded = pkg::load_dir(root).map_err(|error| Issue {
        path: error.path.clone(),
        id: None,
        message: error.to_string(),
    })?;
    let id = loaded.manifest.id().to_string();
    for notice in loaded.notices {
        issues.push(Issue {
            path: root.to_path_buf(),
            id: Some(id.clone()),
            message: notice,
        });
    }
    Ok(Plugin {
        id,
        root: root.to_path_buf(),
        source,
        generation,
        enabled,
        manifest: loaded.manifest,
        record,
    })
}

fn disabled_path(config: &Config) -> PathBuf {
    config.plugins_state_dir().join("disabled.json")
}

fn disabled_set(config: &Config) -> std::collections::BTreeMap<String, ()> {
    let keys: Vec<String> = std::fs::read_to_string(disabled_path(config))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    keys.into_iter().map(|id| (id, ())).collect()
}

/// 停用一个开发插件：写入 `disabled.json`；安装来源同时更新记录。
pub fn set_enabled(config: &Config, id: &str, enabled: bool) -> Result<(), String> {
    let store = Store::open(config.plugins_state_dir());
    if let Some(mut record) = store.record(id) {
        record.enabled = enabled;
        return store
            .save_record(&record)
            .map_err(|error| error.to_string());
    }
    let mut disabled: Vec<String> = disabled_set(config).keys().cloned().collect();
    disabled.retain(|item| item != id);
    if !enabled {
        disabled.push(id.to_string());
        disabled.sort();
    }
    let path = disabled_path(config);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(&disabled).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

/// 安装一个包目录到新代际；先校验清单与模块 bundle。
///
/// 发布顺序在包锁内完成：分配代际 → 复制到暂存目录 → rename 成正式代际 →
/// 原子写记录 → 切换 `current`。任何一步失败都不会改动旧记录与旧指针。
pub fn install(config: &Config, dir: &Path, policy: EnablePolicy) -> Result<Record, String> {
    let loaded = pkg::install_ready(dir)?;
    let manifest = loaded.manifest;
    if !manifest.supports_this_platform() {
        return Err(format!(
            "{} does not support this platform ({})",
            manifest.id(),
            manifest.platforms().join(", ")
        ));
    }
    let store = Store::open(config.plugins_state_dir());
    let id = manifest.id().to_string();
    let lock = store.lock_package(&id).map_err(|error| error.to_string())?;
    let enabled = policy.resolve(store.record(&id).as_ref());
    let (generation, staged) = store
        .alloc_generation(&id)
        .map_err(|error| error.to_string())?;
    if let Err(error) = copy_tree(dir, &staged) {
        store.discard_staged(&staged);
        return Err(error.to_string());
    }
    let target = match store.publish_staged(&id, generation, &staged) {
        Ok(target) => target,
        Err(error) => {
            store.discard_staged(&staged);
            return Err(error.to_string());
        }
    };
    let record = Record {
        id: id.clone(),
        source: Source::Local,
        generation,
        installed_at: now_unix(),
        enabled,
        manifest_hash: file_sha256(&target.join("hctui.toml"))
            .or_else(|| file_sha256(&target.join("plugin.toml"))),
        origin: Some(dir.display().to_string()),
        sha256: None,
        key_id: None,
        app: Some(serde_json::json!({
            "name": manifest.name(),
            "version": manifest.version(),
        })),
        min_runtime: None,
        capabilities: Some(manifest.capabilities()),
        module: manifest.module_info(),
    };
    if let Err(error) = store.save_record(&record) {
        let _ = std::fs::remove_dir_all(&target);
        return Err(error.to_string());
    }
    store
        .set_current(&id, &target)
        .map_err(|error| error.to_string())?;
    drop(lock);
    Ok(record)
}

/// 链接一个开发包目录；不拷贝。
pub fn link(config: &Config, dir: &Path, policy: EnablePolicy) -> Result<Record, String> {
    let loaded = pkg::install_ready(dir)?;
    let manifest = loaded.manifest;
    let store = Store::open(config.plugins_state_dir());
    let id = manifest.id().to_string();
    let lock = store.lock_package(&id).map_err(|error| error.to_string())?;
    let target = std::path::absolute(dir).map_err(|error| error.to_string())?;
    let enabled = policy.resolve(store.record(&id).as_ref());
    let record = Record {
        id: id.clone(),
        source: Source::Linked,
        generation: 1,
        installed_at: now_unix(),
        enabled,
        manifest_hash: file_sha256(&target.join("hctui.toml"))
            .or_else(|| file_sha256(&target.join("plugin.toml"))),
        origin: Some(target.display().to_string()),
        sha256: None,
        key_id: None,
        app: Some(serde_json::json!({
            "name": manifest.name(),
            "version": manifest.version(),
        })),
        min_runtime: None,
        capabilities: Some(manifest.capabilities()),
        module: manifest.module_info(),
    };
    store
        .save_record(&record)
        .map_err(|error| error.to_string())?;
    store
        .set_current(&id, &target)
        .map_err(|error| error.to_string())?;
    drop(lock);
    Ok(record)
}

/// 这个动作或终端入口属于一个已安装但被停用的包时返回包 ID。
pub fn disabled_package(config: &Config, name: &str) -> Option<String> {
    Registry::load(config).disabled_owner(name)
}

/// 注册表在磁盘上的指纹：两个 store 的记录、启用状态与开发清单的内容哈希。
/// 运行中的 TUI 用它判断是否需要重新加载，不重启也能感知启用变化。
pub fn revision(config: &Config) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for state in [config.plugins_state_dir(), config.mods_state_dir()] {
        hash_files(&mut hasher, &state.join("records"));
    }
    if let Ok(text) = std::fs::read_to_string(config.plugins_state_dir().join("disabled.json")) {
        text.hash(&mut hasher);
    }
    for dev in [config.plugins_dir(), config.mods_dir()] {
        if let Ok(entries) = std::fs::read_dir(&dev) {
            let mut manifests: Vec<PathBuf> = Vec::new();
            for entry in entries.filter_map(Result::ok) {
                for name in [pkg::MANIFEST_FILE, pkg::PLUGIN_FILE] {
                    manifests.push(entry.path().join(name));
                }
            }
            manifests.sort();
            for path in manifests {
                path.hash(&mut hasher);
                if let Ok(text) = std::fs::read_to_string(&path) {
                    text.hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

fn hash_files(hasher: &mut impl std::hash::Hasher, dir: &Path) {
    use std::hash::Hash as _;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();
    paths.sort();
    for path in paths {
        path.hash(hasher);
        if let Ok(text) = std::fs::read_to_string(&path) {
            text.hash(hasher);
        }
    }
}

/// 卸载插件：删除记录与 `current` 指针；代际目录只在没有任何 pin 持有者
/// （动作调用与终端地址）时回收。开发目录里的源码与用户日志不动。
pub fn uninstall(config: &Config, id: &str) -> Result<(), String> {
    let store = Store::open(config.plugins_state_dir());
    store.detach(id).map_err(|error| error.to_string())?;
    crate::plugin::lease::gc(
        config,
        id,
        &crate::plugin::lease::RetentionEvidence::default(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("hctui-plugin-registry-{tag}-{unique}"))
    }

    fn config_in(root: &Path) -> Config {
        Config {
            config_path: root.join("config").join("config.toml"),
            state_path: root.join("state").join("state.json"),
            ..Config::default()
        }
    }

    fn write_plugin(root: &Path, id: &str) -> PathBuf {
        write_plugin_at(&root.join("config").join("plugins").join(id), id)
    }

    fn write_plugin_at(dir: &Path, id: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "manifest_version = 1\nid = \"{id}\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\nargv = [\"true\"]\n\n[[terminals]]\nid = \"ui\"\ntitle = \"UI\"\nargv = [\"true\"]\nplacement = \"tab\"\n"
            ),
        )
        .unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn development_plugins_register_actions_and_unregister_with_the_directory() {
        let root = unique_dir("dev");
        let config = config_in(&root);
        write_plugin(&root, "dev.example.probe");
        let registry = Registry::load(&config);
        assert_eq!(registry.actions().len(), 1);
        assert_eq!(registry.actions()[0].name, "dev.example.probe.run");

        std::fs::remove_dir_all(
            root.join("config")
                .join("plugins")
                .join("dev.example.probe"),
        )
        .unwrap();
        let registry = Registry::load(&config);
        assert!(registry.actions().is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn installed_plugins_use_the_store_generation_and_disappear_after_uninstall() {
        let root = unique_dir("store");
        let config = config_in(&root);
        let source = write_plugin(&root, "dev.example.probe");
        install(&config, &source, EnablePolicy::Enable).unwrap();
        let registry = Registry::load(&config);
        assert_eq!(registry.actions().len(), 1);
        assert_eq!(registry.plugins[0].source, Source::Local);
        assert!(registry.plugins[0]
            .root
            .starts_with(config.plugins_state_dir()));

        uninstall(&config, "dev.example.probe").unwrap();
        let registry = Registry::load(&config);
        // 源码目录仍在配置目录：卸载后只剩开发来源。
        assert_eq!(registry.actions().len(), 1);
        assert_eq!(registry.plugins[0].source, Source::Linked);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn broken_manifests_are_reported_with_the_line() {
        let root = unique_dir("broken");
        let config = config_in(&root);
        let dir = root.join("config").join("plugins").join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "manifest_version = 1\nid = \"dev.example.broken\"\nname = \"Broken\"\nversion = \"0.1.0\"\nhost_api = 1\nunknown = 1\n",
        )
        .unwrap();
        let registry = Registry::load(&config);
        assert!(registry.actions().is_empty());
        assert_eq!(registry.issues.len(), 1);
        assert!(
            registry.issues[0].message.contains(":6:"),
            "{:?}",
            registry.issues
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn disabled_plugins_leave_the_action_catalog() {
        let root = unique_dir("disabled");
        let config = config_in(&root);
        write_plugin(&root, "dev.example.probe");
        set_enabled(&config, "dev.example.probe", false).unwrap();
        assert!(Registry::load(&config).actions().is_empty());
        set_enabled(&config, "dev.example.probe", true).unwrap();
        assert_eq!(Registry::load(&config).actions().len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn install_and_link_start_disabled_until_enabled() {
        let root = unique_dir("disabled-by-default");
        let config = config_in(&root);
        let source = write_plugin_at(&root.join("source").join("install"), "dev.example.install");
        let record = install(&config, &source, EnablePolicy::Keep).unwrap();
        assert!(!record.enabled, "fresh install must start disabled");
        let store = crate::store::Store::open(config.plugins_state_dir());
        assert!(!store.record("dev.example.install").unwrap().enabled);
        assert!(Registry::load(&config).actions().is_empty());
        assert!(Registry::load(&config).terminals().is_empty());

        set_enabled(&config, "dev.example.install", true).unwrap();
        assert_eq!(Registry::load(&config).actions().len(), 1);

        let linked = write_plugin_at(&root.join("source").join("link"), "dev.example.link");
        let record = link(&config, &linked, EnablePolicy::Keep).unwrap();
        assert!(!record.enabled, "fresh link must start disabled");
        let actions = Registry::load(&config).actions();
        assert!(actions
            .iter()
            .all(|action| action.package != "dev.example.link"));
        set_enabled(&config, "dev.example.link", true).unwrap();
        assert!(Registry::load(&config)
            .actions()
            .iter()
            .any(|action| action.package == "dev.example.link"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn update_preserves_the_disabled_state() {
        let root = unique_dir("update-enabled");
        let config = config_in(&root);
        let source = write_plugin_at(&root.join("source").join("update"), "dev.example.update");

        // 新装默认不启用；更新保留当前状态，不会把已禁用的包重新打开。
        assert!(
            !install(&config, &source, EnablePolicy::Keep)
                .unwrap()
                .enabled
        );
        assert!(
            !install(&config, &source, EnablePolicy::Keep)
                .unwrap()
                .enabled
        );

        set_enabled(&config, "dev.example.update", true).unwrap();
        assert!(
            install(&config, &source, EnablePolicy::Keep)
                .unwrap()
                .enabled
        );

        set_enabled(&config, "dev.example.update", false).unwrap();
        let record = install(&config, &source, EnablePolicy::Keep).unwrap();
        assert!(!record.enabled);
        assert_eq!(record.generation, 4);

        // 显式策略优先于记录状态。
        assert!(
            install(&config, &source, EnablePolicy::Enable)
                .unwrap()
                .enabled
        );
        assert!(
            !install(&config, &source, EnablePolicy::Disable)
                .unwrap()
                .enabled
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn two_installers_racing_leave_one_generation() {
        let root = unique_dir("race");
        let config = config_in(&root);
        let source = write_plugin_at(&root.join("source").join("race"), "dev.example.race");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let config = config.clone();
            let source = source.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                install(&config, &source, EnablePolicy::Keep).map(|record| record.generation)
            }));
        }
        let results: Vec<Result<u64, String>> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(results.iter().any(Result::is_ok), "{results:?}");

        let store = crate::store::Store::open(config.plugins_state_dir());
        assert_eq!(store.list().len(), 1, "one record only");
        let record = store.record("dev.example.race").expect("record published");
        let current = store
            .current_dir("dev.example.race")
            .expect("current resolves");
        // 唯一被发布的代际：记录与指针指向同一个完整目录。
        assert_eq!(
            current,
            store.generation_dir("dev.example.race", record.generation)
        );
        assert!(current.join("plugin.toml").is_file());

        // 两个安装者串行：成功的调用各自拿到不同代际，没有共享同一目录。
        let mut generations: Vec<u64> = results
            .iter()
            .filter_map(|result| result.as_ref().ok().copied())
            .collect();
        assert!(!generations.is_empty());
        let total = generations.len();
        generations.sort_unstable();
        generations.dedup();
        assert_eq!(generations.len(), total, "installers shared one generation");
        for generation in &generations {
            assert!(store
                .generation_dir("dev.example.race", *generation)
                .join("plugin.toml")
                .is_file());
        }
        // 发布态只有一个：记录指向的那一代就是 current，其他代际没有指针。
        assert_eq!(record.generation, *generations.last().unwrap());
        assert!(!store
            .root()
            .join("store")
            .join("dev.example.race")
            .join(format!(".tmp-{}", record.generation))
            .exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_old_record_without_capabilities_loads_and_backfills() {
        let root = unique_dir("old-record");
        let config = config_in(&root);
        let source = write_plugin_at(&root.join("source").join("old"), "dev.example.old");
        let record = install(&config, &source, EnablePolicy::Enable).unwrap();

        // 退回 M5 形态：记录里没有 capabilities / module 字段。
        let store = crate::store::Store::open(config.plugins_state_dir());
        let path = store
            .root()
            .join("records")
            .join(format!("{}.json", record.id));
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("capabilities");
        object.remove("module");
        std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

        let reopened = crate::store::Store::open(config.plugins_state_dir());
        assert!(
            reopened.record(&record.id).unwrap().capabilities.is_none(),
            "an old record must read without the new field"
        );

        let registry = Registry::load(&config);
        assert!(registry.issues.is_empty(), "{:?}", registry.issues);
        let entries = registry.entries(&config);
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(
            entries[0].capabilities,
            Capabilities {
                actions: true,
                terminals: true,
                module: false,
            }
        );
        assert!(entries[0].issues.is_empty(), "{:?}", entries[0].issues);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn packages_list_json_carries_capabilities_and_module() {
        let root = unique_dir("entries");
        let config = config_in(&root);
        let dir = root.join("source").join("board");
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(
            dir.join(crate::pkg::MANIFEST_FILE),
            "manifest_version = 2\nid = \"dev.hctui.pkg.board\"\nname = \"Board\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"summary\"\ntitle = \"Summary\"\nargv = [\"true\"]\n\n[[terminals]]\nid = \"board\"\ntitle = \"Board\"\nargv = [\"true\"]\nplacement = \"right\"\n\n[module]\ntitle = \"Board\"\nbundle = \"dist/bundle.js\"\nplace = \"right\"\nsize = 32\nfocusable = true\n\n[grants]\nhost = [\"state\"]\nevents = [\"state.changed\"]\n",
        )
        .unwrap();
        std::fs::write(dir.join("dist").join("bundle.js"), "// bundle").unwrap();
        install(&config, &dir, EnablePolicy::Enable).unwrap();

        let registry = Registry::load(&config);
        let entries = registry.entries(&config);
        let json = serde_json::to_value(&entries).unwrap();
        let item = &json[0];
        for key in [
            "id",
            "name",
            "version",
            "source",
            "generation",
            "enabled",
            "capabilities",
            "module",
            "issues",
        ] {
            assert!(item.get(key).is_some(), "missing {key}: {item}");
        }
        assert_eq!(item["id"], "dev.hctui.pkg.board");
        assert_eq!(item["capabilities"]["actions"], true);
        assert_eq!(item["capabilities"]["terminals"], true);
        assert_eq!(item["capabilities"]["module"], true);
        assert_eq!(item["module"]["bundle"], "dist/bundle.js");
        assert_eq!(item["module"]["place"], "right");
        assert_eq!(item["module"]["size"], 32.0);
        assert_eq!(item["module"]["focusable"], true);
        assert_eq!(Registry::load(&config).modules().len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    /// 临时把目录设成只读，离开作用域恢复原权限。
    #[cfg(unix)]
    struct ReadOnlyDir {
        path: PathBuf,
        mode: u32,
    }

    #[cfg(unix)]
    impl Drop for ReadOnlyDir {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt as _;
            let _ =
                std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.mode));
        }
    }

    #[cfg(unix)]
    fn read_only(dir: &Path) -> ReadOnlyDir {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir).unwrap().permissions().mode();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        // 注入必须真的拒写，否则这轮用例测到的不是写失败。
        assert!(
            std::fs::write(dir.join(".write-probe"), b"x").is_err(),
            "0o555 on {} still accepts writes",
            dir.display()
        );
        ReadOnlyDir {
            path: dir.to_path_buf(),
            mode,
        }
    }

    /// `records/<id>.json` 写入失败：记录与指针都停在旧代际，新代际不残留。
    #[cfg(unix)]
    #[test]
    fn a_failed_record_write_keeps_the_published_generation() {
        let root = unique_dir("record-fail");
        let config = config_in(&root);
        let id = "dev.example.recordfail";
        let source = write_plugin_at(&root.join("source").join("record-fail"), id);
        let first = install(&config, &source, EnablePolicy::Enable).unwrap();
        let store = crate::store::Store::open(config.plugins_state_dir());
        let record_path = store.root().join("records").join(format!("{id}.json"));
        let before = std::fs::read(&record_path).unwrap();
        let old_dir = store.current_dir(id).expect("old generation resolves");

        let guard = read_only(&store.root().join("records"));
        let error = install(&config, &source, EnablePolicy::Keep).unwrap_err();
        drop(guard);
        assert!(error.to_lowercase().contains("denied"), "{error}");

        assert_eq!(
            std::fs::read(&record_path).unwrap(),
            before,
            "record changed"
        );
        assert_eq!(store.current_dir(id).as_deref(), Some(old_dir.as_path()));
        assert!(old_dir.join("plugin.toml").is_file());
        assert_eq!(store.record(id).unwrap().generation, first.generation);
        assert!(
            !store.generation_dir(id, first.generation + 1).exists(),
            "the new generation survived a failed record write"
        );

        let reopened = crate::store::Store::open(config.plugins_state_dir());
        assert_eq!(reopened.list().len(), 1, "one record only");
        assert_eq!(reopened.record(id).unwrap().generation, first.generation);
        assert_eq!(reopened.current_dir(id).as_deref(), Some(old_dir.as_path()));
        let package_dir = reopened.root().join("store").join(id);
        let staging: Vec<String> = std::fs::read_dir(&package_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".tmp-"))
            .collect();
        assert!(staging.is_empty(), "staging left behind: {staging:?}");
        std::fs::remove_dir_all(root).ok();
    }

    /// `current/<id>` 切换失败：记录已发布，下一次 `Store::open` 按记录修好指针。
    #[cfg(unix)]
    #[test]
    fn a_failed_pointer_switch_is_repaired_from_the_record() {
        let root = unique_dir("current-fail");
        let config = config_in(&root);
        let id = "dev.example.currentfail";
        let source = write_plugin_at(&root.join("source").join("current-fail"), id);
        let first = install(&config, &source, EnablePolicy::Enable).unwrap();
        let store = crate::store::Store::open(config.plugins_state_dir());
        let old_dir = store.current_dir(id).expect("old generation resolves");

        let guard = read_only(&store.root().join("current"));
        let error = install(&config, &source, EnablePolicy::Keep).unwrap_err();
        drop(guard);
        assert!(error.to_lowercase().contains("denied"), "{error}");

        // 记录先行：新记录已落盘，指针还停在旧代际。
        let record = store.record(id).expect("record published");
        assert_eq!(record.generation, first.generation + 1);
        assert_eq!(store.current_dir(id).as_deref(), Some(old_dir.as_path()));

        let reopened = crate::store::Store::open(config.plugins_state_dir());
        assert!(
            reopened.recovery_issues().is_empty(),
            "{:?}",
            reopened.recovery_issues()
        );
        assert_eq!(
            reopened.current_dir(id),
            Some(reopened.generation_dir(id, first.generation + 1))
        );
        let registry = Registry::load(&config);
        assert!(!registry.actions().is_empty(), "{:?}", registry.issues);
        std::fs::remove_dir_all(root).ok();
    }
}
