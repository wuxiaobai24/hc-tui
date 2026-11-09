//! 统一包模型：一个 `hctui.toml`（v2）同时声明动作、终端入口与界面模块。
//!
//! 清单只有一个解析器：`[module]` 与 `[grants]` 由 [`crate::plugin::manifest`]
//! 的严格 TOML 反序列化负责，未知键、未知表、非法 id、重复 id 与越界授予都在
//! 清单期报 `文件:行:列: 原因`。旧清单仍按同一模型读出：
//!
//! - `plugin.toml`（`manifest_version = 1`）：动作与终端入口，`module = None`。
//! - 旧 `hctui.toml`（只有 `[mod]` / `[grants]`，没有 `manifest_version`）：
//!   只有界面模块，动作与终端入口为空。
//!
//! 能力由清单推导（[`Manifest::capabilities`]），不在清单里手写声明。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml::Spanned;

use crate::config::Config;
use crate::plugin::manifest::{self, fail, HostApiRange, Manifest, ManifestError, HOST_API};
use crate::store::{Record, Source, Store};

pub mod source;

/// 包根目录里唯一清单的文件名。
pub const MANIFEST_FILE: &str = "hctui.toml";
/// v1 插件清单的文件名。
pub const PLUGIN_FILE: &str = "plugin.toml";
/// 模块 bundle 的默认相对路径。
pub const DEFAULT_BUNDLE: &str = "dist/bundle.js";
/// 模块停靠位置。
pub const PLACES: &[&str] = &["left", "right", "top", "bottom", "toast", "none"];
/// 模块浮层的形态。
pub const OVERLAY_KINDS: &[&str] = &["dialog", "popup", "window"];

/// 包提供的能力；由清单推导，记录里只是缓存。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// 是否有动作。
    pub actions: bool,
    /// 是否有终端入口。
    pub terminals: bool,
    /// 是否贡献界面模块。
    pub module: bool,
}

impl Capabilities {
    /// `packages list` 的能力徽章。
    pub fn badges(&self) -> String {
        let mut out = String::new();
        for (present, badge) in [
            (self.actions, 'A'),
            (self.terminals, 'T'),
            (self.module, 'M'),
        ] {
            if present {
                out.push(badge);
            }
        }
        if out.is_empty() {
            "-".to_string()
        } else {
            out
        }
    }
}

/// `[module]` / 旧 `[mod]` 的展示形态；记录与 `packages list --json` 使用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleInfo {
    /// 标题。
    pub title: String,
    /// 包内相对的 bundle 路径。
    pub bundle: String,
    /// 停靠位置。
    pub place: String,
    /// 停靠厚度；`None` 用宿主默认。
    #[serde(default)]
    pub size: Option<f64>,
    /// 是否允许把键盘焦点移入。
    pub focusable: bool,
    /// 接管的内置区域名；`None` 表示不接管。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

/// `[module]`：有这一段才算贡献界面。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Module {
    /// 标题；缺省用包名。
    #[serde(default)]
    pub title: Option<Spanned<String>>,
    /// 包内相对的 bundle 路径；缺省 `dist/bundle.js`。
    #[serde(default)]
    pub bundle: Option<Spanned<String>>,
    /// `left` / `right` / `top` / `bottom` / `none`。
    #[serde(default)]
    pub place: Option<Spanned<String>>,
    /// 接管哪个内置区域（`spaces` / `tabs` / `status`）；省略表示不接管。
    #[serde(default)]
    pub region: Option<Spanned<String>>,
    /// 停靠厚度；小于等于 1 视为占比。
    #[serde(default)]
    pub size: Option<Spanned<f64>>,
    /// 是否允许把键盘焦点移入。
    #[serde(default)]
    pub focusable: Option<Spanned<bool>>,
    /// 要求的最低 hc-tui 版本。
    #[serde(default)]
    pub min_hc_tui_version: Option<Spanned<String>>,
    /// 浮层形态；`place = "none"` 或显式打开时使用。
    #[serde(default)]
    pub overlay: Option<Overlay>,
}

impl Module {
    /// 标题；没有标题时用包名。
    pub fn title(&self) -> Option<&str> {
        self.title.as_ref().map(|value| value.get_ref().as_str())
    }

    /// 包内相对的 bundle 路径。
    pub fn bundle(&self) -> &str {
        self.bundle
            .as_ref()
            .map(|value| value.get_ref().as_str())
            .unwrap_or(DEFAULT_BUNDLE)
    }

    /// 停靠位置。
    pub fn place(&self) -> &str {
        self.place
            .as_ref()
            .map(|value| value.get_ref().as_str())
            .unwrap_or("right")
    }

    /// 停靠厚度。
    pub fn size(&self) -> Option<f64> {
        self.size.as_ref().map(|value| *value.get_ref())
    }

    /// 接管的内置区域名。
    pub fn region(&self) -> Option<&str> {
        self.region.as_ref().map(|value| value.get_ref().as_str())
    }

    /// 是否允许把键盘焦点移入。
    pub fn focusable(&self) -> bool {
        self.focusable
            .as_ref()
            .map(|value| *value.get_ref())
            .unwrap_or(true)
    }

    /// 要求的最低 hc-tui 版本。
    pub fn min_hc_tui_version(&self) -> Option<&str> {
        self.min_hc_tui_version
            .as_ref()
            .map(|value| value.get_ref().as_str())
    }

    /// 落成记录与 JSON 里的形状。
    pub fn info(&self, fallback_title: &str) -> ModuleInfo {
        ModuleInfo {
            title: self
                .title()
                .map(str::to_string)
                .unwrap_or_else(|| fallback_title.to_string()),
            bundle: self.bundle().to_string(),
            place: self.place().to_string(),
            size: self.size(),
            focusable: self.focusable(),
            region: self.region().map(str::to_string),
        }
    }
}

/// `[overlay]`：模块浮层的默认尺寸。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overlay {
    /// `dialog` / `popup` / `window`。
    #[serde(default)]
    pub kind: Option<Spanned<String>>,
    /// 宽度。
    #[serde(default)]
    pub width: Option<Spanned<f64>>,
    /// 高度。
    #[serde(default)]
    pub height: Option<Spanned<f64>>,
}

/// `[grants]`：模块可用的宿主方法与事件。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grants {
    /// 宿主方法名；必须在 [`crate::mods::host::METHODS`] 内。
    #[serde(default)]
    pub host: Vec<Spanned<String>>,
    /// 事件名；必须在 [`crate::mods::EVENTS`] 内。
    #[serde(default)]
    pub events: Vec<Spanned<String>>,
}

impl Grants {
    /// `host` 与 `events` 依次展开成一个名单。
    pub fn names(&self) -> Vec<String> {
        self.host
            .iter()
            .chain(self.events.iter())
            .map(|value| value.get_ref().clone())
            .collect()
    }
}

/// 清单的来源形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// `plugin.toml`，`manifest_version = 1`。
    V1Plugin,
    /// `hctui.toml`，`manifest_version = 2`。
    V2,
    /// 旧 `hctui.toml`：只有 `[mod]` / `[grants]`。
    LegacyMod,
}

impl Origin {
    /// 人读的来源名。
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::V1Plugin => "v1",
            Origin::V2 => "v2",
            Origin::LegacyMod => "legacy-mod",
        }
    }
}

/// 一份读出并校验过的清单。
#[derive(Debug, Clone)]
pub struct Loaded {
    /// 统一模型。
    pub manifest: Manifest,
    /// 来源形态。
    pub origin: Origin,
    /// 清单路径。
    pub path: PathBuf,
    /// 不影响加载的提示（例如同目录的 `plugin.toml` 被忽略）。
    pub notices: Vec<String>,
}

impl Loaded {
    /// 能力由清单推导。
    pub fn capabilities(&self) -> Capabilities {
        self.manifest.capabilities()
    }
}

/// 一个包按记录解析出的运行期形态。
///
/// `Record.module` 与 `Record.capabilities.module` 决定是否加载、bundle 相对
/// 哪一代际目录解析；`Record.grants` 是运行期授予的唯一来源。旧记录缺字段时
/// 只在同一代际目录里读一次清单回填，不要求重新安装。
#[derive(Debug, Clone)]
pub struct Resolved {
    /// 包 ID。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 代码根目录（记录指向的代际，或开发目录本身）。
    pub root: PathBuf,
    /// 来源。
    pub source: Source,
    /// 代际。
    pub generation: u64,
    /// 是否启用。
    pub enabled: bool,
    /// 记录里的能力；旧记录按清单回填。
    pub capabilities: Capabilities,
    /// 声明了模块时的模块摘要；旧记录按清单回填。
    pub module: Option<ModuleInfo>,
    /// 记录里的 `[grants]`；旧记录按清单回填。
    pub grants: Vec<String>,
}

impl Resolved {
    /// 模块 bundle 的绝对路径；没有模块时为 `None`。
    pub fn bundle_path(&self) -> Option<PathBuf> {
        self.module
            .as_ref()
            .map(|module| self.root.join(&module.bundle))
    }
}

/// 按包记录解析一个包；两个 store 的记录优先，其次是两个开发目录。
pub fn resolve(config: &Config, id: &str) -> Result<Option<Resolved>, String> {
    for state in [config.plugins_state_dir(), config.mods_state_dir()] {
        let store = Store::open(state);
        let Some(record) = store.record(id) else {
            continue;
        };
        let root = record_root(&store, &record)?;
        let loaded = load_dir(&root).ok();
        return Ok(Some(from_record(&root, record, loaded.as_ref())));
    }
    for dir in [config.plugins_dir(), config.mods_dir()] {
        let root = dir.join(id);
        if !root.is_dir() {
            continue;
        }
        let loaded = match load_dir(&root) {
            Ok(loaded) if loaded.manifest.id() == id => Some(loaded),
            Ok(_) => continue,
            // 只有 bundle、没有清单的开发目录按模块处理：与 `store::module_root`
            // 的旧行为一致，授予为空，等价于清单没有 `[grants]`。
            Err(_) if root.join(DEFAULT_BUNDLE).is_file() => None,
            Err(error) => return Err(error.to_string()),
        };
        let resolved = match loaded {
            Some(loaded) => Resolved {
                id: id.to_string(),
                name: loaded.manifest.name().to_string(),
                capabilities: loaded.manifest.capabilities(),
                module: loaded.manifest.module_info(),
                grants: loaded.manifest.grants(),
                root,
                source: Source::Linked,
                generation: 1,
                enabled: true,
            },
            None => Resolved {
                id: id.to_string(),
                name: id.to_string(),
                capabilities: Capabilities {
                    module: true,
                    ..Capabilities::default()
                },
                module: Some(ModuleInfo {
                    title: id.to_string(),
                    bundle: DEFAULT_BUNDLE.to_string(),
                    place: "right".to_string(),
                    size: None,
                    focusable: true,
                    region: None,
                }),
                grants: Vec::new(),
                root,
                source: Source::Linked,
                generation: 1,
                enabled: true,
            },
        };
        return Ok(Some(Resolved {
            enabled: resolved.enabled && crate::plugin::registry::dev_enabled(config, id),
            ..resolved
        }));
    }
    Ok(None)
}

/// 记录指向的代码根：链接来源用来源目录，其余用记录里的代际目录。
fn record_root(store: &Store, record: &Record) -> Result<PathBuf, String> {
    let root = match record.source {
        Source::Linked => record.origin.as_deref().map(PathBuf::from),
        Source::Local | Source::Packaged => {
            Some(store.generation_dir(&record.id, record.generation))
        }
    }
    .ok_or_else(|| format!("{}: linked record has no origin", record.id))?;
    if !root.is_dir() {
        return Err(format!("{}: {} is missing", record.id, root.display()));
    }
    Ok(root)
}

/// 记录缺字段时，从同一代际目录的清单回填能力、模块与授予。
fn from_record(root: &Path, record: Record, loaded: Option<&Loaded>) -> Resolved {
    let capabilities = record
        .capabilities
        .or_else(|| loaded.map(Loaded::capabilities))
        .unwrap_or_default();
    let module = record
        .module
        .clone()
        .or_else(|| loaded.and_then(|loaded| loaded.manifest.module_info()));
    let grants = record
        .grants
        .clone()
        .or_else(|| loaded.map(|loaded| loaded.manifest.grants()))
        .unwrap_or_default();
    let name = loaded
        .map(|loaded| loaded.manifest.name().to_string())
        .or_else(|| {
            record
                .app
                .as_ref()
                .and_then(|app| app.get("name"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| record.id.clone());
    Resolved {
        id: record.id.clone(),
        name,
        root: root.to_path_buf(),
        source: record.source,
        generation: record.generation,
        enabled: record.enabled,
        capabilities,
        module,
        grants,
    }
}

/// 单一启用闸门：已安装包改记录，开发目录写 `disabled.json`。
pub fn set_enabled(config: &Config, id: &str, enabled: bool) -> Result<(), String> {
    for state in [config.plugins_state_dir(), config.mods_state_dir()] {
        let store = Store::open(state);
        if let Some(mut record) = store.record(id) {
            record.enabled = enabled;
            return store
                .save_record(&record)
                .map_err(|error| error.to_string());
        }
    }
    crate::plugin::registry::set_enabled(config, id, enabled)
}

/// 包代码根：先按记录解析，其次开发目录；这是“模块装在哪”的唯一判断。
pub fn module_root(config: &Config, id: &str) -> Option<PathBuf> {
    resolve(config, id)
        .ok()
        .flatten()
        .map(|resolved| resolved.root)
}

/// 旧 `hctui.toml` 的整份文件。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyFile {
    #[serde(rename = "mod")]
    module: Option<LegacyMod>,
    #[serde(default)]
    overlay: Option<Overlay>,
    #[serde(default)]
    grants: Option<Grants>,
}

/// 旧 `[mod]` 段。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyMod {
    id: Spanned<String>,
    #[serde(default)]
    title: Option<Spanned<String>>,
    #[serde(default)]
    place: Option<Spanned<String>>,
    #[serde(default)]
    region: Option<Spanned<String>>,
    #[serde(default)]
    size: Option<Spanned<f64>>,
    #[serde(default)]
    focusable: Option<Spanned<bool>>,
    #[serde(default)]
    bundle: Option<Spanned<String>>,
    #[serde(default)]
    host_api: Option<Spanned<HostApiRange>>,
    #[serde(default)]
    min_hc_tui_version: Option<Spanned<String>>,
}

impl LegacyFile {
    /// 映射成统一模型：动作与终端入口为空，模块从 `[mod]` / `[grants]` 取。
    fn into_manifest(self, path: &Path, source: &str) -> Result<Manifest, ManifestError> {
        let missing = fail(path, source, 0..0, "[mod] is required");
        let legacy = self.module.ok_or(missing)?;
        let span = legacy.id.span();
        let id = legacy.id.clone();
        let title = legacy
            .title
            .clone()
            .unwrap_or_else(|| Spanned::new(span.clone(), id.get_ref().clone()));
        let manifest = Manifest {
            manifest_version: Spanned::new(span.clone(), 2),
            id,
            name: title.clone(),
            version: Spanned::new(span.clone(), "0.0.0".to_string()),
            host_api: legacy
                .host_api
                .clone()
                .unwrap_or_else(|| Spanned::new(span.clone(), HostApiRange::exact(HOST_API))),
            platforms: None,
            description: None,
            actions: Vec::new(),
            terminals: Vec::new(),
            services: Vec::new(),
            module: Some(Module {
                title: Some(title),
                bundle: legacy.bundle,
                place: legacy.place,
                region: legacy.region,
                size: legacy.size,
                focusable: legacy.focusable,
                min_hc_tui_version: legacy.min_hc_tui_version,
                overlay: self.overlay,
            }),
            grants: self.grants,
        };
        manifest.validate(path, source)?;
        Ok(manifest)
    }
}

/// 读一份清单文件；`hctui.toml` 可以是 v2 或旧形态，`plugin.toml` 是 v1。
pub fn load_file(path: &Path) -> Result<Loaded, ManifestError> {
    let source = std::fs::read_to_string(path).map_err(|error| ManifestError {
        path: path.to_path_buf(),
        line: 1,
        column: 1,
        message: error.to_string(),
    })?;
    parse_file(path, &source)
}

/// 从文本解析并校验一份清单；按 `manifest_version` 区分 v2 与旧形态。
pub fn parse_file(path: &Path, source: &str) -> Result<Loaded, ManifestError> {
    let table: toml::Table =
        toml::from_str(source).map_err(|error| toml_error(path, source, &error))?;
    if table.contains_key("manifest_version") {
        let manifest = manifest::parse(path, source)?;
        let origin = if *manifest.manifest_version.get_ref() >= 2 {
            Origin::V2
        } else {
            Origin::V1Plugin
        };
        return Ok(Loaded {
            manifest,
            origin,
            path: path.to_path_buf(),
            notices: Vec::new(),
        });
    }
    let legacy: LegacyFile =
        toml::from_str(source).map_err(|error| toml_error(path, source, &error))?;
    let manifest = legacy.into_manifest(path, source)?;
    Ok(Loaded {
        manifest,
        origin: Origin::LegacyMod,
        path: path.to_path_buf(),
        notices: Vec::new(),
    })
}

fn toml_error(path: &Path, source: &str, error: &toml::de::Error) -> ManifestError {
    fail(
        path,
        source,
        error.span().unwrap_or(0..0),
        error
            .message()
            .lines()
            .next()
            .unwrap_or("invalid manifest")
            .to_string(),
    )
}

/// 目录里是否已经落了清单文件；只判存在，不解析内容。
/// 代际目录「完整落盘」的判据（见 [`crate::store::Store::publish_staged`]）。
pub fn has_manifest(dir: &Path) -> bool {
    dir.join(MANIFEST_FILE).is_file() || dir.join(PLUGIN_FILE).is_file()
}

/// 读包根目录的清单：`hctui.toml` 优先于同目录的 `plugin.toml`。
pub fn load_dir(dir: &Path) -> Result<Loaded, ManifestError> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let plugin_path = dir.join(PLUGIN_FILE);
    match (manifest_path.is_file(), plugin_path.is_file()) {
        (true, has_plugin) => {
            let mut loaded = load_file(&manifest_path)?;
            if has_plugin {
                loaded.notices.push(format!(
                    "using {}, ignoring {}",
                    manifest_path.display(),
                    plugin_path.display()
                ));
            }
            Ok(loaded)
        }
        (false, true) => load_file(&plugin_path),
        (false, false) => Err(ManifestError {
            path: manifest_path,
            line: 1,
            column: 1,
            message: format!("no {MANIFEST_FILE} or {PLUGIN_FILE}; a package manifest is required"),
        }),
    }
}

/// 清单读不过去时的三段式错误块；`target` 是用户传入的 token，原样回显。
pub fn manifest_error(target: &str, error: &ManifestError) -> String {
    crate::diag::detail(
        &format!(
            "{target}: 清单非法 {}:{}:{}: {}",
            error.path.display(),
            error.line,
            error.column,
            error.message
        ),
        "这个包目录的清单不符合 hctui.toml 的语法或字段约束，hc-tui 拒绝加载",
        &format!("sed -n '{}p' {}", error.line, error.path.display()),
    )
}

/// 安装前检查：清单合法，且声明了模块时 bundle 存在、非空。
pub fn install_ready(dir: &Path) -> Result<Loaded, String> {
    let target = dir.display().to_string();
    let loaded = load_dir(dir).map_err(|error| manifest_error(&target, &error))?;
    if let Some(module) = &loaded.manifest.module {
        let bundle = dir.join(module.bundle());
        match std::fs::metadata(&bundle) {
            Ok(meta) if meta.is_file() && meta.len() > 0 => {}
            Ok(meta) if meta.is_file() => {
                return Err(format!("{}: module bundle is empty", bundle.display()));
            }
            Ok(_) => {
                return Err(format!("{}: module bundle is not a file", bundle.display()));
            }
            Err(error) => {
                return Err(format!("{}: module bundle: {error}", bundle.display()));
            }
        }
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2: &str = r#"
manifest_version = 2
id = "dev.hctui.pkg.git-board"
name = "Git board"
version = "0.1.0"
host_api = 1
platforms = ["linux", "macos"]
description = "动作 + 终端 + 界面，一个包"

[[actions]]
id = "summary"
title = "Git 摘要"
contexts = ["global", "space", "pane"]
argv = ["sh", "./bin/summary.sh"]

[actions.args]
type = "object"
properties = { short = { type = "boolean", default = true } }

[[terminals]]
id = "git-ui"
title = "Git UI"
argv = ["lazygit"]
placement = "right"

[module]
title = "Git board"
bundle = "dist/bundle.js"
place = "right"
size = 32
focusable = true

[grants]
host = ["state", "run", "notify"]
events = ["state.changed", "terminal.status"]
"#;

    fn temp_dir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("hctui-pkg-{tag}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_v2_manifest_declares_all_three_capabilities() {
        let path = PathBuf::from("hctui.toml");
        let loaded = parse_file(&path, V2).unwrap();
        assert_eq!(loaded.origin, Origin::V2);
        assert_eq!(
            loaded.capabilities(),
            Capabilities {
                actions: true,
                terminals: true,
                module: true,
            }
        );
        let module = loaded.manifest.module_info().unwrap();
        assert_eq!(module.title, "Git board");
        assert_eq!(module.bundle, "dist/bundle.js");
        assert_eq!(module.place, "right");
        assert_eq!(module.size, Some(32.0));
        assert!(module.focusable);
        assert_eq!(
            loaded.manifest.grants(),
            vec![
                "state".to_string(),
                "run".to_string(),
                "notify".to_string(),
                "state.changed".to_string(),
                "terminal.status".to_string()
            ]
        );
    }

    #[test]
    fn a_v1_plugin_manifest_still_loads_as_a_package() {
        let text = "manifest_version = 1\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\nargv = [\"true\"]\n\n[[terminals]]\nid = \"ui\"\ntitle = \"UI\"\nargv = [\"true\"]\nplacement = \"tab\"\n";
        let loaded = parse_file(Path::new("plugin.toml"), text).unwrap();
        assert_eq!(loaded.origin, Origin::V1Plugin);
        assert_eq!(
            loaded.capabilities(),
            Capabilities {
                actions: true,
                terminals: true,
                module: false,
            }
        );
        assert!(loaded.manifest.module_info().is_none());
    }

    #[test]
    fn a_legacy_hctui_manifest_still_loads_as_a_module_only_package() {
        let text = "[mod]\nid = \"agents\"\ntitle = \"Agents\"\nplace = \"right\"\nsize = 32\nfocusable = true\nhost_api = 1\n\n[overlay]\nkind = \"dialog\"\nwidth = 80\nheight = 18\n\n[grants]\nhost = [\"state\", \"run\"]\nevents = [\"state.changed\"]\n";
        let loaded = parse_file(Path::new("hctui.toml"), text).unwrap();
        assert_eq!(loaded.origin, Origin::LegacyMod);
        assert_eq!(loaded.manifest.id(), "agents");
        assert_eq!(loaded.manifest.name(), "Agents");
        assert_eq!(
            loaded.capabilities(),
            Capabilities {
                actions: false,
                terminals: false,
                module: true,
            }
        );
        assert!(loaded.manifest.module_info().is_some());
        assert_eq!(
            loaded.manifest.grants(),
            vec![
                "state".to_string(),
                "run".to_string(),
                "state.changed".to_string()
            ]
        );
    }

    #[test]
    fn a_v2_manifest_wins_over_a_sibling_plugin_toml() {
        let dir = temp_dir("sibling");
        std::fs::write(dir.join(MANIFEST_FILE), V2).unwrap();
        std::fs::write(
            dir.join(PLUGIN_FILE),
            "manifest_version = 1\nid = \"dev.example.legacy\"\nname = \"Legacy\"\nversion = \"0.1.0\"\nhost_api = 1\n",
        )
        .unwrap();
        std::fs::write(dir.join("dist/bundle.js").as_path(), "// bundle").ok();
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("dist").join("bundle.js"), "// bundle").unwrap();
        let loaded = load_dir(&dir).unwrap();
        assert_eq!(loaded.origin, Origin::V2);
        assert_eq!(loaded.manifest.id(), "dev.hctui.pkg.git-board");
        assert_eq!(loaded.notices.len(), 1, "{:?}", loaded.notices);
        assert!(
            loaded.notices[0].contains("ignoring"),
            "{:?}",
            loaded.notices
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_unknown_key_in_a_v2_manifest_is_rejected_with_file_line_column() {
        let text = "manifest_version = 2\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\nunknown = true\n";
        let error = parse_file(Path::new("/tmp/hctui.toml"), text).unwrap_err();
        assert_eq!(error.line, 6);
        assert!(error.message.contains("unknown field"), "{}", error.message);
        assert!(
            error.to_string().starts_with("/tmp/hctui.toml:6:"),
            "{}",
            error
        );
    }

    #[test]
    fn module_grants_outside_the_host_whitelist_are_rejected() {
        let text = "manifest_version = 2\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[module]\nbundle = \"dist/bundle.js\"\n\n[grants]\nhost = [\"files\"]\n";
        let error = parse_file(Path::new("hctui.toml"), text).unwrap_err();
        assert!(
            error.message.contains("not a host method"),
            "{}",
            error.message
        );
        assert_eq!(error.line, 11, "{error}");

        let text = "manifest_version = 2\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[module]\nbundle = \"dist/bundle.js\"\n\n[grants]\nevents = [\"core.exploded\"]\n";
        let error = parse_file(Path::new("hctui.toml"), text).unwrap_err();
        assert!(error.message.contains("known event"), "{}", error.message);
    }

    #[test]
    fn a_missing_module_bundle_is_rejected_at_install_time() {
        let dir = temp_dir("missing-bundle");
        std::fs::write(
            dir.join(MANIFEST_FILE),
            "manifest_version = 2\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[module]\nbundle = \"dist/bundle.js\"\n",
        )
        .unwrap();
        // 只读清单不需要 bundle。
        assert!(load_dir(&dir).is_ok());
        let error = install_ready(&dir).unwrap_err();
        assert!(error.contains("bundle"), "{error}");

        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("dist").join("bundle.js"), "").unwrap();
        let error = install_ready(&dir).unwrap_err();
        assert!(error.contains("empty"), "{error}");

        std::fs::write(dir.join("dist").join("bundle.js"), "// bundle").unwrap();
        assert!(install_ready(&dir).is_ok());
        std::fs::remove_dir_all(dir).ok();
    }
}
