//! dialogs 模块：宿主侧冻结对话框，模块负责绘制与编辑。
//!
//! 数据形状：[`Dialog`] 在打开时冻结一份 [`Prompt`]（单行输入）或
//! [`FormContext`]（动作参数 schema 与目标）；props 按行表展开后交给模块，
//! 模块提交时只回值，校验与派发仍走 [`App::commit_prompt`] 与
//! `manifest::validate_args` 那一条路径。
//! 模块边界：模块独占按键与光标（宿主不设 `prompt`/`form`），提交经
//! `run dialogs.submit` / `run dialogs.cancel` 回到这里；派发仍走
//! [`App::execute_action`]。
//! 不变量：不合法值不派发（错误留在对话框里）；`_target` 打开时冻结、提交时
//! 原样带回；模块不可用时调用方回落到内置实现。

use serde_json::{json, Value};

use super::form::RowKind;
use super::*;
use crate::i18n::t;
use crate::plugin::manifest;
use crate::plugin::registry::ActionInfo;

/// 内置 Prompt 浮层的几何：宽 56、高 7（`src/ui/mod.rs:1651`）。
const PROMPT_WIDTH: u16 = 56;
const PROMPT_HEIGHT: u16 = 7;
/// 内置参数表单的宽度（`src/ui/mod.rs:1412`）。
const FORM_WIDTH: u16 = 64;

/// 模块接管的对话框。
#[derive(Debug, Clone)]
pub struct Dialog {
    /// 表面 ID。
    pub surface: String,
    /// 打开参数；粘贴时复制一份追加 `paste` 再下发。
    pub props: Value,
    /// 单行输入；与 `form` 互斥。
    pub prompt: Option<Prompt>,
    /// 参数表单。
    pub form: Option<FormContext>,
}

/// 参数表单的冻结上下文。
#[derive(Debug, Clone)]
pub struct FormContext {
    /// 插件动作名 `<package>.<action>`。
    pub action: String,
    /// 动作标题。
    pub title: String,
    /// 动作参数 schema。
    pub schema: Value,
    /// 冻结的 `_target`。
    pub target: Value,
}

impl App {
    /// `--no-mods`、停用或未安装时不走模块。
    pub fn dialogs_module_available(&self) -> bool {
        !self.config.no_mods
            && self.mods.is_enabled("dialogs")
            && matches!(
                crate::pkg::resolve(&self.config, "dialogs"),
                Ok(Some(package)) if package.enabled && package.module.is_some()
            )
    }

    /// PromptKind → props 里的 `kind`；返回 `None` 的语义留在内置实现。
    fn dialogs_prompt_kind(kind: &PromptKind) -> Option<&'static str> {
        Some(match kind {
            PromptKind::NewTab => "new_tab",
            PromptKind::NewWorkspace => "new_workspace",
            PromptKind::RenameTab { .. } => "rename_tab",
            PromptKind::RenameWorkspace(_) => "rename_workspace",
            PromptKind::RenamePane(_) => "rename_pane",
            PromptKind::RenameAgent(_) => "rename_agent",
            PromptKind::SendText(_) => "send_text",
            PromptKind::SendKeys(_) => "send_keys",
            PromptKind::WaitOutput(_) => "wait_output",
            PromptKind::PutFile(_) => "put_file",
            PromptKind::AddMachine => "add_machine",
            PromptKind::RenameMachine(_) => "rename_machine",
            PromptKind::Command => "command",
            PromptKind::CloseTab(_)
            | PromptKind::CloseWorkspace(_)
            | PromptKind::KillPane(_)
            | PromptKind::OpenLink(_) => return None,
        })
    }

    /// 打开单行输入；模块不可用或语义未迁移时返回 `false`。
    pub fn open_dialogs_prompt(&mut self, kind: &PromptKind, label: &str, initial: &str) -> bool {
        self.open_dialogs_prompt_at(kind, label, initial, initial.len())
    }

    /// 带光标位置的单行输入（重命名带初值）。
    pub fn open_dialogs_prompt_at(
        &mut self,
        kind: &PromptKind,
        label: &str,
        initial: &str,
        cursor: usize,
    ) -> bool {
        if !self.dialogs_module_available() {
            return false;
        }
        let Some(kind_name) = Self::dialogs_prompt_kind(kind) else {
            return false;
        };
        let prompt = Prompt {
            label: label.to_string(),
            buffer: initial.to_string(),
            cursor: cursor.min(initial.len()),
            history_pos: None,
            kind: kind.clone(),
        };
        let props = self.dialog_prompt_props(&prompt, kind_name);
        match self.open_dialogs_surface(props.clone(), PROMPT_WIDTH, PROMPT_HEIGHT) {
            Ok(surface) => {
                self.dialogs = Some(Dialog {
                    surface,
                    props,
                    prompt: Some(prompt),
                    form: None,
                });
                self.prefix = false;
                self.dirty = true;
                true
            }
            Err(_) => false,
        }
    }

    /// 打开参数表单；模块不可用或没有可填字段时返回 `false`。
    pub fn open_dialogs_form(&mut self, action: &ActionInfo, target: Value) -> bool {
        if !self.dialogs_module_available() {
            return false;
        }
        let Some(schema) = action.args.as_ref() else {
            return false;
        };
        let form = ActionForm::build(&action.name, &action.title, schema, target.clone());
        if !form.has_fields() {
            return false;
        }
        let fields = dialog_fields(schema, &form);
        let height = (form.rows().len() as u16 + 5).min(self.content.height.max(5));
        let props = json!({
            "mode": "form",
            "kind": "action_form",
            "title": action.title,
            "description": action.name,
            "action": action.name,
            "_target": target,
            "value": "",
            "cursor": 0,
            "fields": fields,
            "history": [],
            "buttons": {
                "primary": t(" Submit "),
                "secondary": t(" Cancel "),
                "close": " [x] ",
            },
            "footer": t("↵ submit    ⇥ next    esc cancel    ^a add    ^d remove"),
        });
        match self.open_dialogs_surface(props.clone(), FORM_WIDTH, height) {
            Ok(surface) => {
                self.dialogs = Some(Dialog {
                    surface,
                    props,
                    prompt: None,
                    form: Some(FormContext {
                        action: action.name.clone(),
                        title: action.title.clone(),
                        schema: schema.clone(),
                        target,
                    }),
                });
                self.prefix = false;
                self.dirty = true;
                true
            }
            Err(_) => false,
        }
    }

    /// 单行输入的 props。
    fn dialog_prompt_props(&self, prompt: &Prompt, kind: &str) -> Value {
        json!({
            "mode": "prompt",
            "kind": kind,
            "title": prompt.kind.title(),
            "description": prompt.label,
            "value": prompt.buffer,
            "cursor": utf16_offset(&prompt.buffer, prompt.cursor),
            "fields": [{
                "label": "",
                "kind": "text",
                "type": "string",
                "required": false,
                "display": "value",
                "depth": 0,
                "keys": ["value"],
                "value": prompt.buffer,
                "cursor": utf16_offset(&prompt.buffer, prompt.cursor),
            }],
            "history": self.prompt_history,
            "buttons": {
                "primary": if prompt.kind.needs_confirm() { t(" Confirm ") } else { t(" Save ") },
                "secondary": t(" Cancel "),
                "close": " [x] ",
            },
        })
    }

    /// 开一条模块表面；矩形正好等于内置对话框。
    ///
    /// 内置框宽是 `min(常量, 内容区宽 - 2)`，`dialog_rect` 的收窄规则不
    /// 一样（内容区窄时少 4 列），所以开完表面后直接把矩形改成算好的值。
    /// 形态仍用 `dialog`：覆盖层的清屏开关按形态判定，`popup` 不清屏会留下
    /// 上一帧的格子。
    fn open_dialogs_surface(
        &mut self,
        props: Value,
        width: u16,
        height: u16,
    ) -> Result<String, String> {
        if let Some(existing) = self.dialogs.as_ref().map(|dialog| dialog.surface.clone()) {
            self.mods.close(&existing);
            if self.focused_mod.as_deref() == Some(existing.as_str()) {
                self.focus_mod_surface(None);
            }
        }
        let content = self.content;
        let box_width = width.min(content.width.saturating_sub(2));
        let box_height = height.min(content.height);
        if content.width < 20 || content.height < 8 {
            return Err(t("content area too small for a dialog").to_string());
        }
        let left = content.x + (content.width - box_width) / 2;
        let top = content.y + (content.height - box_height) / 2;
        let rect = Rect::new(left, top, box_width, box_height);
        let id = self.mods_open(
            "dialogs",
            crate::mods::SurfacePlace::Dialog,
            props.clone(),
            None,
            None,
            None,
        )?;
        if let Some(surface) = self.mods.surface_mut(&id) {
            surface.area = rect;
            surface.clear = crate::mods::ClearArea::Surface;
            surface.esc_to_host = false;
            surface.props_revision += 1;
            surface
                .host
                .send("props.changed", json!({ "props": props }));
        }
        self.dirty = true;
        Ok(id)
    }

    /// 模块提交：值回到冻结上下文，校验与派发复用内置路径。
    pub fn submit_dialog(&mut self, data: &Value) -> Result<()> {
        let Some(dialog) = self.dialogs.take() else {
            anyhow::bail!(t("no dialog is open"));
        };
        if let Some(mut prompt) = dialog.prompt {
            let value = data
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            prompt.buffer = value;
            prompt.cursor = prompt.buffer.len();
            // 与内置 Enter 分支同一套历史规则。
            if !prompt.kind.needs_confirm()
                && !prompt.buffer.trim().is_empty()
                && self.prompt_history.last() != Some(&prompt.buffer)
            {
                self.prompt_history.push(prompt.buffer.clone());
                if self.prompt_history.len() > 50 {
                    self.prompt_history.remove(0);
                }
            }
            self.commit_prompt(prompt);
            self.settle_mod_confirm("done");
            return Ok(());
        }
        let Some(form) = dialog.form else {
            return Ok(());
        };
        let values = data.get("values").cloned().unwrap_or_else(|| json!({}));
        let checked = match manifest::validate_args(&form.schema, &values) {
            Ok(checked) => checked,
            Err(message) => {
                self.dialogs = Some(Dialog {
                    surface: dialog.surface,
                    props: dialog.props,
                    prompt: None,
                    form: Some(form),
                });
                self.note = message.clone();
                self.dirty = true;
                return Err(anyhow::anyhow!(message));
            }
        };
        let mut args = checked;
        // `_target` 由模块带回；缺失时退回打开时冻结的目标。
        let target = data
            .get("_target")
            .cloned()
            .filter(|value| !value.is_null())
            .unwrap_or_else(|| form.target.clone());
        if !target.is_null() && target != json!({}) {
            if let Some(map) = args.as_object_mut() {
                map.insert("_target".to_string(), target);
            }
        }
        if let Err(error) = self.execute_action(&form.action, args) {
            self.dialogs = Some(Dialog {
                surface: dialog.surface,
                props: dialog.props,
                prompt: None,
                form: Some(form),
            });
            self.dirty = true;
            return Err(error);
        }
        self.settle_mod_confirm("done");
        Ok(())
    }

    /// 模块取消：丢掉冻结上下文并如实回报。
    pub fn cancel_dialog(&mut self) {
        if self.dialogs.take().is_none() {
            return;
        }
        self.note = t("cancelled").to_string();
        self.settle_mod_confirm("cancelled");
        self.dirty = true;
    }

    /// 数组增删改变行数：表面跟着内置框重新居中。
    pub fn resize_dialog(&mut self, data: &Value) {
        let Some(surface_id) = self.dialogs.as_ref().map(|dialog| dialog.surface.clone()) else {
            return;
        };
        if self
            .dialogs
            .as_ref()
            .is_none_or(|dialog| dialog.form.is_none())
        {
            return;
        }
        let rows = data.get("rows").and_then(Value::as_u64).unwrap_or(0) as u16;
        let content = self.content;
        let box_width = FORM_WIDTH.min(content.width.saturating_sub(2));
        let box_height = (rows + 5).min(content.height);
        if content.width < 20 || content.height < 8 || box_height < 5 {
            return;
        }
        let left = content.x + (content.width - box_width) / 2;
        let top = content.y + (content.height - box_height) / 2;
        if let Some(surface) = self.mods.surface_mut(&surface_id) {
            surface.area = Rect::new(left, top, box_width, box_height);
        }
        self.dirty = true;
    }

    /// 宿主粘贴：模块拿不到 nsh 的 Paste 输入，经 `props.changed` 下发。
    pub fn paste_dialog(&mut self, text: &str) -> bool {
        let Some(dialog) = self.dialogs.as_ref() else {
            return false;
        };
        let surface_id = dialog.surface.clone();
        let mut props = dialog.props.clone();
        props["paste"] = json!(text);
        if let Some(surface) = self.mods.surface_mut(&surface_id) {
            if surface.state != crate::mods::SurfaceState::Running {
                return true;
            }
            surface.props_revision += 1;
            surface
                .host
                .send("props.changed", json!({ "props": props }));
        }
        self.dirty = true;
        true
    }

    /// 模块表面被关闭（`closeOverlay` 或用户关窗）后丢掉冻结上下文。
    pub fn dialog_surface_closed(&mut self, surface: &str) {
        if self
            .dialogs
            .as_ref()
            .is_some_and(|dialog| dialog.surface == surface)
        {
            self.dialogs = None;
            self.settle_mod_confirm("cancelled");
        }
    }
}

/// schema 里按行键路径取子 schema；数组项沿用数组属性的路径。
fn schema_at<'a>(schema: &'a Value, keys: &[String]) -> &'a Value {
    let mut node = schema;
    for key in keys {
        let Some(child) = node.get("properties").and_then(|map| map.get(key)) else {
            return &Value::Null;
        };
        node = child;
    }
    node
}

/// 字节偏移 → JS 字符串下标（UTF-16 码元数）。
fn utf16_offset(text: &str, bytes: usize) -> usize {
    text[..bytes.min(text.len())]
        .chars()
        .map(char::len_utf16)
        .sum()
}

fn option_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 把 [`ActionForm`] 的行表展开成 props 里的 `fields`。
fn dialog_fields(schema: &Value, form: &ActionForm) -> Value {
    // 数组分组行不带 id，先从 item/add 行反查键路径 → 数组下标。
    let mut array_ids: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for row in form.rows() {
        if let RowKind::ArrayItem { array } | RowKind::AddItem { array } = &row.kind {
            array_ids.insert(row.keys.join("."), *array);
        }
    }
    let rows: Vec<Value> = form
        .rows()
        .iter()
        .map(|row| {
            let leaf = schema_at(schema, &row.keys);
            let leaf_type = leaf.get("type").and_then(Value::as_str).unwrap_or("string");
            let mut field = json!({
                "label": row.label,
                "display": row.display,
                "depth": row.depth,
                "keys": row.keys,
                "required": matches!(row.kind, RowKind::Text { required: true }),
            });
            match &row.kind {
                RowKind::Group => {
                    if leaf_type == "array" {
                        let items = leaf.get("items").cloned().unwrap_or(Value::Null);
                        field["kind"] = json!("array");
                        field["type"] = json!("array");
                        field["array"] =
                            json!(array_ids.get(&row.keys.join(".")).copied().unwrap_or(0));
                        field["itemType"] = items
                            .get("type")
                            .cloned()
                            .unwrap_or_else(|| json!("string"));
                        field["itemOptions"] =
                            items.get("enum").cloned().unwrap_or_else(|| json!([]));
                    } else {
                        field["kind"] = json!("group");
                        field["type"] = json!("object");
                    }
                }
                RowKind::Text { .. } => {
                    field["kind"] = json!("text");
                    field["type"] = json!(leaf_type);
                    field["value"] = json!(row.text);
                    field["cursor"] = json!(utf16_offset(&row.text, row.cursor));
                }
                RowKind::Enum {
                    options, selected, ..
                } => {
                    field["kind"] = json!("enum");
                    field["type"] = json!("string");
                    field["options"] = Value::Array(
                        options
                            .iter()
                            .map(|value| json!({"value": value, "label": option_text(value)}))
                            .collect(),
                    );
                    field["selected"] = json!(selected);
                }
                RowKind::Bool => {
                    field["kind"] = json!("bool");
                    field["type"] = json!("boolean");
                    field["checked"] = json!(row.checked);
                }
                RowKind::ArrayItem { array } => {
                    field["kind"] = json!("item");
                    field["array"] = json!(array);
                    field["type"] = json!(leaf_type);
                    field["value"] = json!(row.text);
                    field["cursor"] = json!(utf16_offset(&row.text, row.cursor));
                }
                RowKind::AddItem { array } => {
                    field["kind"] = json!("add");
                    field["array"] = json!(array);
                    field["type"] = json!("string");
                }
            }
            field
        })
        .collect();
    Value::Array(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let (_, events) = std::sync::mpsc::channel();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let root =
            std::env::temp_dir().join(format!("hctui-dialogs-app-{}-{unique}", std::process::id()));
        App::with_network(
            crate::config::Config {
                restore: false,
                prompt_new_tab: false,
                config_path: root.join("config").join("config.toml"),
                state_path: root.join("state").join("state.json"),
                ..crate::config::Config::default()
            },
            None,
            events,
        )
        .unwrap()
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["name"],
            "properties": {
                "name": {"type": "string"},
                "env": {"type": "string", "enum": ["dev", "prod"], "default": "dev"},
                "dry": {"type": "boolean", "default": true},
                "hosts": {"type": "array", "items": {"type": "string"}}
            }
        })
    }

    #[test]
    fn dialog_fields_mirror_the_form_rows() {
        let form = ActionForm::build("dev.example.run", "Run", &schema(), Value::Null);
        let fields = dialog_fields(&schema(), &form);
        let kinds: Vec<&str> = fields
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["bool", "enum", "array", "add", "text"]);
        let env = &fields[1];
        assert_eq!(env["selected"], 0);
        assert_eq!(env["options"][0]["value"], "dev");
        assert_eq!(fields[4]["display"], "name");
        assert_eq!(fields[4]["required"], true);
    }

    #[test]
    fn submitting_a_form_without_target_validates_first() {
        let mut app = app();
        let action = ActionInfo {
            name: "dev.example.run".into(),
            package: "dev.example".into(),
            action: "run".into(),
            title: "Run".into(),
            description: String::new(),
            contexts: vec!["global".into()],
            args: Some(schema()),
            root: std::path::PathBuf::from("/tmp"),
            generation: 1,
            timeout: std::time::Duration::from_secs(5),
            argv: vec!["true".into()],
        };
        assert!(!app.open_dialogs_form(&action, Value::Null));
        app.dialogs = Some(Dialog {
            surface: "s1".into(),
            props: json!({}),
            prompt: None,
            form: Some(FormContext {
                action: "dev.example.run".into(),
                title: "Run".into(),
                schema: schema(),
                target: Value::Null,
            }),
        });
        let error = app.submit_dialog(&json!({"values": {}})).unwrap_err();
        assert!(
            format!("{error}").contains("args.name is required"),
            "{error}"
        );
        assert!(
            app.dialogs.is_some(),
            "an invalid value keeps the dialog open"
        );
    }
}
