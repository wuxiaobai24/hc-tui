//! 插件注册表：配置目录里的开发插件与安装存储的当前代际。
//!
//! 动作名 `<package-id>.<action-id>`；注册表只收清单合法、`host_api` 匹配且
//! 当前平台受支持的包。安装、链接与卸载复用 [`crate::store::Store`] 的代际
//! 目录、`current` 指针与 records，卸载后动作随之消失。

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::store::{copy_tree, file_sha256, now_unix, Record, Source, Store};

use super::manifest::{self, Manifest};

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
    /// 清单。
    pub manifest: Manifest,
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
    /// 扫描配置目录与安装记录。
    pub fn load(config: &Config) -> Registry {
        let store = Store::open(config.plugins_state_dir());
        let disabled = disabled_set(config);
        let mut registry = Registry::default();
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for record in store.list() {
            let Some(root) = store.current_dir(&record.id) else {
                registry.issues.push(Issue {
                    path: store.root().join("current").join(&record.id),
                    id: Some(record.id.clone()),
                    message: "current generation is missing".to_string(),
                });
                continue;
            };
            match load_plugin(&root, record.source, record.generation, record.enabled) {
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
                    seen.insert(plugin.id.clone());
                    registry.push(plugin);
                }
                Err(issue) => registry.issues.push(issue),
            }
        }
        let dir = config.plugins_dir();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut entries: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path())
                .collect();
            entries.sort();
            for root in entries {
                match load_plugin(&root, Source::Linked, 1, true) {
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
        let mut actions = Vec::new();
        for plugin in self.plugins.iter().filter(|plugin| plugin.enabled) {
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
        let mut terminals = Vec::new();
        for plugin in self.plugins.iter().filter(|plugin| plugin.enabled) {
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
}

fn load_plugin(
    root: &Path,
    source: Source,
    generation: u64,
    enabled: bool,
) -> Result<Plugin, Issue> {
    let path = root.join("plugin.toml");
    let manifest = manifest::load(&path).map_err(|error| Issue {
        path: path.clone(),
        id: None,
        message: error.to_string(),
    })?;
    let mut plugin = Plugin {
        id: manifest.id().to_string(),
        root: root.to_path_buf(),
        source,
        generation,
        enabled,
        manifest,
    };
    plugin.enabled = enabled;
    Ok(plugin)
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

/// 安装一个插件目录到新代际；先校验清单。
pub fn install(config: &Config, dir: &Path) -> Result<Record, String> {
    let manifest = manifest::load(&dir.join("plugin.toml")).map_err(|e| e.to_string())?;
    if !manifest.supports_this_platform() {
        return Err(format!(
            "{} does not support this platform ({})",
            manifest.id(),
            manifest.platforms().join(", ")
        ));
    }
    let store = Store::open(config.plugins_state_dir());
    let (generation, target) = store
        .prepare_generation(manifest.id())
        .map_err(|error| error.to_string())?;
    copy_tree(dir, &target).map_err(|error| error.to_string())?;
    store
        .set_current(manifest.id(), &target)
        .map_err(|error| error.to_string())?;
    let record = Record {
        id: manifest.id().to_string(),
        source: Source::Local,
        generation,
        installed_at: now_unix(),
        enabled: true,
        manifest_hash: file_sha256(&target.join("plugin.toml")),
        origin: Some(dir.display().to_string()),
        sha256: None,
        key_id: None,
        app: Some(serde_json::json!({
            "name": manifest.name(),
            "version": manifest.version(),
        })),
        min_runtime: None,
    };
    store
        .save_record(&record)
        .map_err(|error| error.to_string())?;
    Ok(record)
}

/// 链接一个开发插件目录；不拷贝。
pub fn link(config: &Config, dir: &Path) -> Result<Record, String> {
    let manifest = manifest::load(&dir.join("plugin.toml")).map_err(|e| e.to_string())?;
    let store = Store::open(config.plugins_state_dir());
    let target = std::path::absolute(dir).map_err(|error| error.to_string())?;
    store
        .set_current(manifest.id(), &target)
        .map_err(|error| error.to_string())?;
    let record = Record {
        id: manifest.id().to_string(),
        source: Source::Linked,
        generation: 1,
        installed_at: now_unix(),
        enabled: true,
        manifest_hash: file_sha256(&target.join("plugin.toml")),
        origin: Some(target.display().to_string()),
        sha256: None,
        key_id: None,
        app: Some(serde_json::json!({
            "name": manifest.name(),
            "version": manifest.version(),
        })),
        min_runtime: None,
    };
    store
        .save_record(&record)
        .map_err(|error| error.to_string())?;
    Ok(record)
}

/// 卸载插件：删除记录与 `current` 指针；仍有终端在用的代际保留到关闭后回收。
/// 开发目录里的源码与用户日志不动。
pub fn uninstall(config: &Config, id: &str) -> Result<(), String> {
    let store = Store::open(config.plugins_state_dir());
    let live = crate::plugin::lease::live_generations(config, id);
    store.detach(id).map_err(|error| error.to_string())?;
    if live.is_empty() {
        store.remove_store(id).map_err(|error| error.to_string())?;
    }
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
        let dir = root.join("config").join("plugins").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "manifest_version = 1\nid = \"{id}\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\nargv = [\"true\"]\n"
            ),
        )
        .unwrap();
        dir
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
        install(&config, &source).unwrap();
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
}
