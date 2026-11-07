//! 插件清单：标准 TOML 解析、严格校验与行号错误。
//!
//! 清单格式见 `docs/plugins.md` §2。未知键、未知表、非法 id、空 `argv`、
//! 未知上下文、未知 placement 与不合规的参数 schema 都拒绝加载；错误带
//! 文件、行、列，便于直接定位到清单行。

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

/// 当前清单格式版本。
pub const MANIFEST_VERSION: u32 = 1;
/// 当前宿主接口版本。
pub const HOST_API: u32 = 1;

/// 支持的平台名。
pub const PLATFORMS: &[&str] = &["linux", "macos"];
/// 动作可用的上下文。
pub const CONTEXTS: &[&str] = &["global", "space", "pane"];
/// 终端入口的放置方式。
pub const PLACEMENTS: &[&str] = &["tab", "right", "down", "float", "popup", "zoomed"];

/// 一条清单错误，带文件、行、列。
#[derive(Debug, Clone)]
pub struct ManifestError {
    /// 清单路径。
    pub path: PathBuf,
    /// 1 起算的行。
    pub line: usize,
    /// 1 起算的列。
    pub column: usize,
    /// 人读的原因。
    pub message: String,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.path.display(),
            self.line,
            self.column,
            self.message
        )
    }
}

impl std::error::Error for ManifestError {}

fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let before = &source[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit_once('\n')
        .map(|(_, tail)| tail.chars().count() + 1)
        .unwrap_or_else(|| before.chars().count() + 1);
    (line, column)
}

fn fail(
    path: &Path,
    source: &str,
    span: std::ops::Range<usize>,
    message: impl Into<String>,
) -> ManifestError {
    let (line, column) = line_col(source, span.start);
    ManifestError {
        path: path.to_path_buf(),
        line,
        column,
        message: message.into(),
    }
}

/// 插件清单。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// 清单格式版本。
    pub manifest_version: toml::Spanned<u32>,
    /// 包 ID。
    pub id: toml::Spanned<String>,
    /// 显示名。
    pub name: toml::Spanned<String>,
    /// 包版本。
    pub version: toml::Spanned<String>,
    /// 宿主接口版本。
    pub host_api: toml::Spanned<u32>,
    /// 支持的平台；空表示全部。
    #[serde(default)]
    pub platforms: Option<toml::Spanned<Vec<toml::Spanned<String>>>>,
    /// 描述。
    #[serde(default)]
    pub description: Option<toml::Spanned<String>>,
    /// 动作。
    #[serde(default)]
    pub actions: Vec<Action>,
    /// 终端入口；M6 只校验与展示，执行在 M7。
    #[serde(default)]
    pub terminals: Vec<Terminal>,
}

/// `[[actions]]`。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    /// 动作 ID，限 `[a-z0-9_-]{1,32}`。
    pub id: toml::Spanned<String>,
    /// 菜单与目录里的标题。
    pub title: toml::Spanned<String>,
    /// 描述。
    #[serde(default)]
    pub description: Option<toml::Spanned<String>>,
    /// 出现的上下文。
    #[serde(default)]
    pub contexts: Option<toml::Spanned<Vec<toml::Spanned<String>>>>,
    /// 命令参数；`./` 开头相对插件目录。
    pub argv: toml::Spanned<Vec<toml::Spanned<String>>>,
    /// 超时毫秒；默认 30 秒，上限 600000。
    #[serde(default)]
    pub timeout_ms: Option<toml::Spanned<u64>>,
    /// 参数 schema（JSON Schema 子集）。
    #[serde(default)]
    pub args: Option<toml::Spanned<toml::Value>>,
}

impl Action {
    /// 动作 ID。
    pub fn id(&self) -> &str {
        self.id.get_ref()
    }

    /// 标题。
    pub fn title(&self) -> &str {
        self.title.get_ref()
    }

    /// 描述。
    pub fn description(&self) -> &str {
        self.description
            .as_ref()
            .map(|value| value.get_ref().as_str())
            .unwrap_or("")
    }

    /// 上下文；缺省为 `global`。
    pub fn contexts(&self) -> Vec<&str> {
        match &self.contexts {
            Some(values) => values
                .get_ref()
                .iter()
                .map(|v| v.get_ref().as_str())
                .collect(),
            None => vec!["global"],
        }
    }

    /// 命令参数。
    pub fn argv(&self) -> Vec<&str> {
        self.argv
            .get_ref()
            .iter()
            .map(|value| value.get_ref().as_str())
            .collect()
    }

    /// 超时。
    pub fn timeout(&self) -> std::time::Duration {
        let millis = self
            .timeout_ms
            .as_ref()
            .map(|v| *v.get_ref())
            .unwrap_or(30_000);
        std::time::Duration::from_millis(millis.min(600_000))
    }

    /// 参数 schema 的 JSON 形态。
    pub fn args_schema(&self) -> Option<Value> {
        self.args
            .as_ref()
            .and_then(|value| serde_json::to_value(value.get_ref()).ok())
    }
}

/// `[[terminals]]`。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    /// 终端入口 ID。
    pub id: toml::Spanned<String>,
    /// 标题。
    pub title: toml::Spanned<String>,
    /// 命令参数。
    pub argv: toml::Spanned<Vec<toml::Spanned<String>>>,
    /// 放置方式。
    pub placement: toml::Spanned<String>,
    /// 启动前检查的程序。
    #[serde(default)]
    pub requires_programs: Option<toml::Spanned<Vec<toml::Spanned<String>>>>,
}

impl Terminal {
    /// 终端入口 ID。
    pub fn id(&self) -> &str {
        self.id.get_ref()
    }

    /// 标题。
    pub fn title(&self) -> &str {
        self.title.get_ref()
    }

    /// 放置方式。
    pub fn placement(&self) -> &str {
        self.placement.get_ref()
    }

    /// 命令参数。
    pub fn argv(&self) -> Vec<&str> {
        self.argv
            .get_ref()
            .iter()
            .map(|value| value.get_ref().as_str())
            .collect()
    }

    /// 需要预先存在的程序。
    pub fn requires_programs(&self) -> Vec<&str> {
        self.requires_programs
            .as_ref()
            .map(|values| {
                values
                    .get_ref()
                    .iter()
                    .map(|v| v.get_ref().as_str())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Manifest {
    /// 包 ID。
    pub fn id(&self) -> &str {
        self.id.get_ref()
    }

    /// 显示名。
    pub fn name(&self) -> &str {
        self.name.get_ref()
    }

    /// 包版本。
    pub fn version(&self) -> &str {
        self.version.get_ref()
    }

    /// 支持的平台；空表示全部。
    pub fn platforms(&self) -> Vec<&str> {
        self.platforms
            .as_ref()
            .map(|values| {
                values
                    .get_ref()
                    .iter()
                    .map(|v| v.get_ref().as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 是否支持当前平台。
    pub fn supports_this_platform(&self) -> bool {
        let platforms = self.platforms();
        if platforms.is_empty() {
            return true;
        }
        let current = if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        platforms.contains(&current)
    }

    /// 动作的完整名：`<package>.<action>`。
    pub fn action_name(&self, action: &Action) -> String {
        format!("{}.{}", self.id(), action.id())
    }
}

/// 读取并校验清单；错误带行号。
pub fn load(path: &Path) -> Result<Manifest, ManifestError> {
    let source = std::fs::read_to_string(path).map_err(|error| ManifestError {
        path: path.to_path_buf(),
        line: 1,
        column: 1,
        message: error.to_string(),
    })?;
    parse(path, &source)
}

/// 从文本解析并校验清单。
pub fn parse(path: &Path, source: &str) -> Result<Manifest, ManifestError> {
    let manifest: Manifest = toml::from_str(source).map_err(|error| {
        let span = error.span().unwrap_or(0..0);
        let message = error
            .message()
            .lines()
            .next()
            .unwrap_or("invalid manifest")
            .to_string();
        fail(path, source, span, message)
    })?;
    manifest.validate(path, source)?;
    Ok(manifest)
}

impl Manifest {
    fn validate(&self, path: &Path, source: &str) -> Result<(), ManifestError> {
        let check = |span: std::ops::Range<usize>, condition: bool, message: &str| {
            if condition {
                Ok(())
            } else {
                Err(fail(path, source, span, message))
            }
        };
        check(
            self.manifest_version.span(),
            *self.manifest_version.get_ref() == MANIFEST_VERSION,
            &format!("manifest_version must be {MANIFEST_VERSION}"),
        )?;
        check(
            self.host_api.span(),
            *self.host_api.get_ref() == HOST_API,
            &format!(
                "host_api {} is not supported (host api {HOST_API})",
                self.host_api.get_ref()
            ),
        )?;
        check(
            self.id.span(),
            valid_package_id(self.id()),
            "id must be dot-separated segments of [a-z0-9_-]{1,32}",
        )?;
        check(
            self.version.span(),
            valid_version(self.version()),
            "version must be MAJOR.MINOR.PATCH",
        )?;
        check(
            self.name.span(),
            !self.name().trim().is_empty(),
            "name must not be empty",
        )?;
        if let Some(platforms) = &self.platforms {
            for platform in platforms.get_ref() {
                check(
                    platform.span(),
                    PLATFORMS.contains(&platform.get_ref().as_str()),
                    "platform must be linux or macos",
                )?;
            }
        }
        let mut action_ids = std::collections::BTreeSet::new();
        for action in &self.actions {
            check(
                action.id.span(),
                valid_element_id(action.id()),
                "action id must match [a-z0-9_-]{1,32}",
            )?;
            check(
                action.id.span(),
                action_ids.insert(action.id().to_string()),
                &format!("duplicate action id `{}`", action.id()),
            )?;
            check(
                action.title.span(),
                !action.title().trim().is_empty(),
                "action title must not be empty",
            )?;
            check(
                action.argv.span(),
                !action.argv().is_empty(),
                "action argv must not be empty",
            )?;
            check(
                action.argv.span(),
                action
                    .argv()
                    .first()
                    .is_some_and(|program| !program.is_empty()),
                "action argv[0] must not be empty",
            )?;
            if let Some(contexts) = &action.contexts {
                for context in contexts.get_ref() {
                    check(
                        context.span(),
                        CONTEXTS.contains(&context.get_ref().as_str()),
                        "context must be global, space or pane",
                    )?;
                }
            }
            if let Some(timeout) = &action.timeout_ms {
                check(
                    timeout.span(),
                    *timeout.get_ref() <= 600_000,
                    "timeout_ms must not exceed 600000",
                )?;
            }
            if let Some(args) = &action.args {
                validate_schema(path, source, args.span(), args.get_ref())?;
            }
        }
        let mut terminal_ids = std::collections::BTreeSet::new();
        for terminal in &self.terminals {
            check(
                terminal.id.span(),
                valid_element_id(terminal.id()),
                "terminal id must match [a-z0-9_-]{1,32}",
            )?;
            check(
                terminal.id.span(),
                terminal_ids.insert(terminal.id().to_string()),
                &format!("duplicate terminal id `{}`", terminal.id()),
            )?;
            check(
                terminal.title.span(),
                !terminal.title().trim().is_empty(),
                "terminal title must not be empty",
            )?;
            check(
                terminal.argv.span(),
                !terminal.argv().is_empty(),
                "terminal argv must not be empty",
            )?;
            check(
                terminal.placement.span(),
                PLACEMENTS.contains(&terminal.placement()),
                "placement must be tab, right, down, float, popup or zoomed",
            )?;
            if let Some(programs) = &terminal.requires_programs {
                for program in programs.get_ref() {
                    check(
                        program.span(),
                        valid_program_name(program.get_ref()),
                        "requires_programs entries must be plain program names",
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// `requires_programs` 只接受 PATH 里的程序名：探测脚本会把它当词使用。
fn valid_program_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
}

fn valid_package_id(id: &str) -> bool {
    !id.is_empty() && id.split('.').all(valid_element_id) && id.split('.').count() <= 8
}

fn valid_element_id(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    id.len() <= 32
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn valid_version(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// 参数 schema 支持的嵌套深度上限。
pub const SCHEMA_MAX_DEPTH: usize = 8;
/// 单层 `properties` 数量上限。
pub const SCHEMA_MAX_PROPERTIES: usize = 64;

/// 参数 schema 的 JSON Schema 子集校验。
fn validate_schema(
    path: &Path,
    source: &str,
    span: std::ops::Range<usize>,
    value: &toml::Value,
) -> Result<(), ManifestError> {
    let json = serde_json::to_value(value)
        .map_err(|error| fail(path, source, span.clone(), error.to_string()))?;
    validate_schema_value(path, source, &span, &json, 1)
}

fn validate_schema_value(
    path: &Path,
    source: &str,
    span: &std::ops::Range<usize>,
    schema: &Value,
    depth: usize,
) -> Result<(), ManifestError> {
    let error_at = |message: String| fail(path, source, span.clone(), message);
    if depth > SCHEMA_MAX_DEPTH {
        return Err(error_at(format!(
            "args schema nesting exceeds {SCHEMA_MAX_DEPTH} levels"
        )));
    }
    let Some(object) = schema.as_object() else {
        return Err(error_at("args schema must be an object".to_string()));
    };
    const KEYS: &[&str] = &[
        "type",
        "properties",
        "required",
        "enum",
        "default",
        "additionalProperties",
        "description",
        "items",
    ];
    for key in object.keys() {
        if !KEYS.contains(&key.as_str()) {
            return Err(fail(
                path,
                source,
                span.clone(),
                format!("args schema key `{key}` is not supported"),
            ));
        }
    }
    if let Some(kind) = object.get("type") {
        let Some(kind) = kind.as_str() else {
            return Err(fail(
                path,
                source,
                span.clone(),
                "args schema type must be a string",
            ));
        };
        if !matches!(
            kind,
            "object" | "string" | "boolean" | "number" | "integer" | "array" | "null"
        ) {
            return Err(fail(
                path,
                source,
                span.clone(),
                format!("args schema type `{kind}` is not supported"),
            ));
        }
    }
    if let Some(properties) = object.get("properties") {
        let Some(properties) = properties.as_object() else {
            return Err(error_at(
                "args schema properties must be a table".to_string(),
            ));
        };
        if properties.len() > SCHEMA_MAX_PROPERTIES {
            return Err(error_at(format!(
                "args schema has more than {SCHEMA_MAX_PROPERTIES} properties at one level"
            )));
        }
        for property in properties.values() {
            validate_schema_value(path, source, span, property, depth + 1)?;
        }
    }
    if let Some(required) = object.get("required") {
        let Some(required) = required.as_array() else {
            return Err(fail(
                path,
                source,
                span.clone(),
                "args schema required must be an array",
            ));
        };
        if required.iter().any(|value| !value.is_string()) {
            return Err(fail(
                path,
                source,
                span.clone(),
                "args schema required entries must be strings",
            ));
        }
    }
    if let Some(additional) = object.get("additionalProperties") {
        if !additional.is_boolean() {
            return Err(fail(
                path,
                source,
                span.clone(),
                "args schema additionalProperties must be a boolean",
            ));
        }
    }
    if let Some(list) = object.get("enum") {
        if !list.is_array() {
            return Err(fail(
                path,
                source,
                span.clone(),
                "args schema enum must be an array",
            ));
        }
    }
    if let Some(items) = object.get("items") {
        let Some(items_object) = items.as_object() else {
            return Err(error_at("args schema items must be an object".to_string()));
        };
        match items_object.get("type").and_then(Value::as_str) {
            Some("object" | "string" | "number" | "integer" | "boolean" | "null") => {}
            Some("array") => {
                return Err(error_at(
                    "args schema items must be an object or scalar schema".to_string(),
                ))
            }
            Some(_) => {
                return Err(error_at(
                    "args schema items type is not supported".to_string(),
                ))
            }
            None => {
                return Err(error_at(
                    "args schema items must declare a type".to_string(),
                ))
            }
        }
        validate_schema_value(path, source, span, items, depth + 1)?;
    }
    if let Some(default) = object.get("default") {
        validate_into(schema, default, "default")
            .map_err(|message| error_at(format!("args schema default is invalid: {message}")))?;
    }
    Ok(())
}

/// 按 schema 校验调用参数：逐层填默认值、拒绝未知键、类型不符与缺失必填。
/// 错误信息带完整路径（`args.remote.name`）。
pub fn validate_args(schema: &Value, args: &Value) -> Result<Value, String> {
    if !args.is_object() {
        return Err("arguments must be a JSON object".to_string());
    }
    validate_into(schema, args, "args")
}

fn validate_into(schema: &Value, value: &Value, path: &str) -> Result<Value, String> {
    check_value_at(schema, value, path)?;
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => validate_object_into(schema, value, path),
        Some("array") => validate_array_into(schema, value, path),
        _ => Ok(value.clone()),
    }
}

fn validate_object_into(schema: &Value, value: &Value, path: &str) -> Result<Value, String> {
    let Some(map) = value.as_object() else {
        return Err(format!("{path} must be object"));
    };
    let properties = schema.get("properties").and_then(Value::as_object);
    let additional = schema
        .get("additionalProperties")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let required = schema.get("required").and_then(Value::as_array);
    let mut out = serde_json::Map::new();
    for (key, item) in map {
        let child = format!("{path}.{key}");
        match properties.and_then(|properties| properties.get(key)) {
            Some(property) => {
                out.insert(key.clone(), validate_into(property, item, &child)?);
            }
            None => {
                if !additional {
                    return Err(format!("{child} is not a known argument"));
                }
                out.insert(key.clone(), item.clone());
            }
        }
    }
    if let Some(properties) = properties {
        for (key, property) in properties {
            if out.contains_key(key) {
                continue;
            }
            let child = format!("{path}.{key}");
            if let Some(default) = property.get("default") {
                out.insert(key.clone(), validate_into(property, default, &child)?);
                continue;
            }
            if required.is_some_and(|required| {
                required
                    .iter()
                    .any(|item| item.as_str() == Some(key.as_str()))
            }) {
                return Err(format!("{child} is required"));
            }
        }
    }
    Ok(Value::Object(out))
}

fn validate_array_into(schema: &Value, value: &Value, path: &str) -> Result<Value, String> {
    let Some(items) = value.as_array() else {
        return Err(format!("{path} must be array"));
    };
    let item_schema = schema.get("items");
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        match item_schema {
            Some(item_schema) => out.push(validate_into(
                item_schema,
                item,
                &format!("{path}[{index}]"),
            )?),
            None => out.push(item.clone()),
        }
    }
    Ok(Value::Array(out))
}

fn check_value_at(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(kind) = schema.get("type").and_then(Value::as_str) {
        let ok = match kind {
            "object" => value.is_object(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "array" => value.is_array(),
            "null" => value.is_null(),
            _ => false,
        };
        if !ok {
            return Err(format!("{path} must be {kind}"));
        }
    }
    if let Some(list) = schema.get("enum").and_then(Value::as_array) {
        if !list.contains(value) {
            return Err(format!("{path} is not one of the allowed values"));
        }
    }
    Ok(())
}

/// 完整动作名：`<package>.<action>`。
pub fn action_name(package: &str, action: &str) -> String {
    format!("{package}.{action}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_err(text: &str) -> ManifestError {
        parse(Path::new("plugin.toml"), text).unwrap_err()
    }

    const VALID: &str = r#"
manifest_version = 1
id = "dev.example.git-tools"
name = "Git tools"
version = "0.1.0"
host_api = 1

[[actions]]
id = "summary"
title = "Git summary"
contexts = ["global", "pane"]
argv = ["sh", "-c", "echo hi"]
timeout_ms = 10000

[actions.args]
type = "object"
properties = { short = { type = "boolean", default = true } }
additionalProperties = false
"#;

    #[test]
    fn valid_manifest_parses_actions() {
        let manifest = parse(Path::new("plugin.toml"), VALID).unwrap();
        assert_eq!(manifest.id(), "dev.example.git-tools");
        assert_eq!(manifest.actions.len(), 1);
        let action = &manifest.actions[0];
        assert_eq!(
            manifest.action_name(action),
            "dev.example.git-tools.summary"
        );
        assert_eq!(action.contexts(), vec!["global", "pane"]);
        assert_eq!(action.timeout(), std::time::Duration::from_millis(10000));
        let schema = action.args_schema().unwrap();
        let args = validate_args(&schema, &serde_json::json!({})).unwrap();
        assert_eq!(args["short"], serde_json::json!(true));
    }

    #[test]
    fn unknown_keys_and_invalid_fields_report_a_line() {
        let error = parse_err(
            "manifest_version = 1\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\nunknown = true\n",
        );
        assert_eq!(error.line, 6);
        assert!(error.message.contains("unknown field"), "{}", error.message);

        let error = parse_err(
            "manifest_version = 1\nid = \"Bad.Id\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n",
        );
        assert_eq!(error.line, 2);
        assert!(error.message.contains("id must be"), "{}", error.message);

        let error = parse_err(
            "manifest_version = 1\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 9\n",
        );
        assert_eq!(error.line, 5);
        assert!(error.message.contains("host_api"), "{}", error.message);

        let error = parse_err(
            "manifest_version = 1\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"empty\"\ntitle = \"Empty\"\nargv = []\n",
        );
        assert!(error.line >= 10, "line: {}", error.line);
        assert!(error.message.contains("argv"), "{}", error.message);
    }

    #[test]
    fn unknown_context_and_schema_keys_are_rejected() {
        let error = parse_err(
            "manifest_version = 1\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\ncontexts = [\"remote\"]\nargv = [\"true\"]\n",
        );
        assert!(error.message.contains("context"), "{}", error.message);

        let error = parse_err(
            "manifest_version = 1\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\nargv = [\"true\"]\n\n[actions.args]\ntype = \"object\"\npatternProperties = {}\n",
        );
        assert!(
            error.message.contains("patternProperties"),
            "{}",
            error.message
        );
    }

    #[test]
    fn validate_args_rejects_unknown_and_wrong_types() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "count": { "type": "integer" } },
            "required": ["count"],
            "additionalProperties": false
        });
        assert!(validate_args(&schema, &serde_json::json!({})).is_err());
        assert!(validate_args(&schema, &serde_json::json!({"count": "x"})).is_err());
        assert!(validate_args(&schema, &serde_json::json!({"count": 1, "extra": true})).is_err());
        assert!(validate_args(&schema, &serde_json::json!({"count": 1})).is_ok());
        // integer 只收整数，number 整数与浮点都收。
        assert!(validate_args(&schema, &serde_json::json!({"count": 1.5})).is_err());
        let number = serde_json::json!({
            "type": "object",
            "properties": { "ratio": { "type": "number" } },
        });
        assert!(validate_args(&number, &serde_json::json!({"ratio": 1})).is_ok());
        assert!(validate_args(&number, &serde_json::json!({"ratio": 1.5})).is_ok());
    }

    const NESTED: &str = r#"
manifest_version = 1
id = "dev.example.nested"
name = "Nested"
version = "0.1.0"
host_api = 1

[[actions]]
id = "run"
title = "Run"
argv = ["true"]

[actions.args]
type = "object"
required = ["remote"]

[actions.args.properties.remote]
type = "object"
required = ["name"]
additionalProperties = false

[actions.args.properties.remote.properties.name]
type = "string"
"#;

    #[test]
    fn nested_object_schema_rejects_missing_required_and_unknown_keys() {
        let manifest = parse(Path::new("plugin.toml"), NESTED).unwrap();
        let schema = manifest.actions[0].args_schema().unwrap();

        assert_eq!(
            validate_args(&schema, &serde_json::json!({})).unwrap_err(),
            "args.remote is required"
        );
        assert_eq!(
            validate_args(&schema, &serde_json::json!({"remote": {}})).unwrap_err(),
            "args.remote.name is required"
        );
        assert_eq!(
            validate_args(&schema, &serde_json::json!({"remote": {"name": 1}})).unwrap_err(),
            "args.remote.name must be string"
        );
        assert_eq!(
            validate_args(
                &schema,
                &serde_json::json!({"remote": {"name": "box", "extra": true}})
            )
            .unwrap_err(),
            "args.remote.extra is not a known argument"
        );
        let filled =
            validate_args(&schema, &serde_json::json!({"remote": {"name": "box"}})).unwrap();
        assert_eq!(filled["remote"]["name"], serde_json::json!("box"));
    }

    #[test]
    fn array_items_are_validated_element_wise() {
        let text = r#"
manifest_version = 1
id = "dev.example.list"
name = "List"
version = "0.1.0"
host_api = 1

[[actions]]
id = "run"
title = "Run"
argv = ["true"]

[actions.args]
type = "object"
required = ["hosts"]

[actions.args.properties.hosts]
type = "array"

[actions.args.properties.hosts.items]
type = "object"
required = ["name"]
additionalProperties = false

[actions.args.properties.hosts.items.properties.name]
type = "string"
"#;
        let manifest = parse(Path::new("plugin.toml"), text).unwrap();
        let schema = manifest.actions[0].args_schema().unwrap();

        assert_eq!(
            validate_args(&schema, &serde_json::json!({"hosts": [{"name": "a"}, {}]})).unwrap_err(),
            "args.hosts[1].name is required"
        );
        assert_eq!(
            validate_args(&schema, &serde_json::json!({"hosts": [{"name": 2}]})).unwrap_err(),
            "args.hosts[0].name must be string"
        );
        assert_eq!(
            validate_args(
                &schema,
                &serde_json::json!({"hosts": [{"name": "a", "port": 22}]})
            )
            .unwrap_err(),
            "args.hosts[0].port is not a known argument"
        );
        assert!(validate_args(&schema, &serde_json::json!({"hosts": [{"name": "a"}]})).is_ok());
    }

    #[test]
    fn an_invalid_default_is_rejected_at_load_time() {
        let error = parse_err(
            "manifest_version = 1\nid = \"dev.example.default\"\nname = \"Default\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\nargv = [\"true\"]\n\n[actions.args]\ntype = \"object\"\nproperties = { count = { type = \"integer\", default = \"many\" } }\n",
        );
        assert!(error.to_string().starts_with("plugin.toml:"), "{}", error);
        assert!(
            error.message.contains("default") && error.message.contains("integer"),
            "{}",
            error.message
        );
        assert!(error.line >= 12, "line: {}", error.line);
    }

    #[test]
    fn oversized_and_too_deep_schemas_are_rejected_at_load_time() {
        let header = "manifest_version = 1\nid = \"dev.example.wide\"\nname = \"Wide\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\nargv = [\"true\"]\n\n[actions.args]\ntype = \"object\"\n";
        let mut wide = String::from(header);
        for index in 0..65 {
            wide.push_str(&format!(
                "\n[actions.args.properties.p{index}]\ntype = \"string\"\n"
            ));
        }
        let error = parse_err(&wide);
        assert!(
            error.message.contains("more than 64 properties"),
            "{}",
            error.message
        );

        let mut deep = String::from(header);
        let mut path = String::from("actions.args");
        for level in 0..9 {
            path.push_str(&format!(".properties.l{level}"));
            deep.push_str(&format!("\n[{path}]\ntype = \"object\"\n"));
        }
        let error = parse_err(&deep);
        assert!(
            error.message.contains("nesting exceeds"),
            "{}",
            error.message
        );

        let error = parse_err(
            "manifest_version = 1\nid = \"dev.example.items\"\nname = \"Items\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\nargv = [\"true\"]\n\n[actions.args]\ntype = \"object\"\n\n[actions.args.properties.grid]\ntype = \"array\"\nitems = { type = \"array\" }\n",
        );
        assert!(
            error.message.contains("object or scalar"),
            "{}",
            error.message
        );
    }
}
