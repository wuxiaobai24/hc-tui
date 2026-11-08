//! 参数表单：按动作的 args schema 生成控件，提交前复用宿主同一套递归校验。
//!
//! 数据形状：[`ActionForm`] 按 schema 展开成一维行表，叶子行持有文本缓冲；
//! 提交时按 schema 还原 JSON，再交给 [`crate::plugin::manifest::validate_args`]
//! 填默认值并校验（含嵌套对象与数组项的路径）。
//! 模块边界：表单只负责取值与显示，派发仍走 [`App::execute_action`]；
//! 无窗口路径与 `hc-tui do` 不经过这里。
//! 不变量：不合法值不提交、不派发；`_target` 在打开表单时冻结，提交时原样带回。

use serde_json::{Map, Number, Value};

use super::*;
use crate::plugin::manifest;

/// 一行表单控件。
#[derive(Debug, Clone)]
pub struct FormRow {
    /// 渲染缩进层级。
    pub depth: usize,
    /// 显示名：对象键名、数组项的 `[i]`，或数组的 `＋ 添加一项`。
    pub label: String,
    /// 从根对象到该值的键路径；数组项沿用数组属性的路径。
    pub keys: Vec<String>,
    /// 校验错误里出现的路径（`remote.name`、`hosts[0]`）。
    pub display: String,
    pub kind: RowKind,
    /// 文本缓冲（`Text` 与数组项）。
    pub text: String,
    /// 光标位置（字节偏移，永远落在字符边界上）。
    pub cursor: usize,
    /// `Bool` 的当前值。
    pub checked: bool,
}

/// 控件类型；只有 schema 子集里的对应项。
#[derive(Debug, Clone)]
pub enum RowKind {
    /// 嵌套对象或数组的分组标题，不可聚焦。
    Group,
    /// `string` / `integer` / `number` 的单行输入。
    Text { required: bool },
    /// `string` + `enum` 的单选菜单。
    Enum {
        options: Vec<Value>,
        selected: Option<usize>,
    },
    /// `boolean` 开关。
    Bool,
    /// 数组的一项；`array` 是所属数组的下标。
    ArrayItem { array: usize },
    /// 数组的「添加一项」行；回车追加一项。
    AddItem { array: usize },
}

impl RowKind {
    pub fn focusable(&self) -> bool {
        !matches!(self, Self::Group)
    }
}

/// 一个数组属性。
#[derive(Debug, Clone)]
struct ArrayForm {
    /// 数组属性的键路径。
    keys: Vec<String>,
    /// `items` 子 schema（标量）。
    items: Value,
}

/// 一次参数填写。
#[derive(Debug, Clone)]
pub struct ActionForm {
    /// `<package>.<action>`。
    pub action: String,
    /// 动作标题。
    pub title: String,
    /// 冻结的目标参数 `_target`。
    target: Value,
    schema: Value,
    rows: Vec<FormRow>,
    arrays: Vec<ArrayForm>,
    /// 当前聚焦的行。
    pub selected: usize,
    /// 字段区的滚动起点（行）。
    pub scroll: usize,
    /// 内联校验错误。
    pub error: Option<String>,
    /// 错误所在的行；渲染时贴在该行下面。
    pub error_row: Option<usize>,
    /// 打开中的枚举菜单：行下标 + 选中项。
    pub menu: Option<(usize, usize)>,
}

impl ActionForm {
    /// 按 schema 展开行表；`target` 是冻结的 `_target`。
    pub fn build(action: &str, title: &str, schema: &Value, target: Value) -> Self {
        let mut form = Self {
            action: action.to_string(),
            title: title.to_string(),
            target,
            schema: schema.clone(),
            rows: Vec::new(),
            arrays: Vec::new(),
            selected: 0,
            scroll: 0,
            error: None,
            error_row: None,
            menu: None,
        };
        let mut keys = Vec::new();
        form.push_object(schema, &mut keys, 0);
        form.selected = form.focus_first();
        form
    }

    /// 是否有可填写的字段；没有时不弹表单。
    pub fn has_fields(&self) -> bool {
        self.rows.iter().any(|row| row.kind.focusable())
    }

    pub fn rows(&self) -> &[FormRow] {
        &self.rows
    }

    /// 焦点行，仅当它是可输入文本的控件。
    pub fn focused_row_mut(&mut self) -> Option<&mut FormRow> {
        self.rows
            .get_mut(self.selected)
            .filter(|row| matches!(row.kind, RowKind::Text { .. } | RowKind::ArrayItem { .. }))
    }

    /// 按 schema 还原参数对象；空的可选文本不出现。
    pub fn values(&self) -> Value {
        let mut keys = Vec::new();
        self.object_value(&self.schema, &mut keys)
    }

    /// 冻结的目标参数。
    pub fn target(&self) -> Value {
        self.target.clone()
    }

    fn push_object(&mut self, schema: &Value, keys: &mut Vec<String>, depth: usize) {
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return;
        };
        let required = schema.get("required").and_then(Value::as_array);
        for (key, child) in properties {
            let needed = required.is_some_and(|list| {
                list.iter()
                    .any(|entry| entry.as_str() == Some(key.as_str()))
            });
            keys.push(key.clone());
            match child.get("type").and_then(Value::as_str) {
                Some("object") => {
                    self.rows.push(group_row(key, keys, depth));
                    self.push_object(child, keys, depth + 1);
                }
                Some("array") => {
                    self.rows.push(group_row(key, keys, depth));
                    let id = self.arrays.len();
                    let items = child.get("items").cloned().unwrap_or(Value::Null);
                    self.arrays.push(ArrayForm {
                        keys: keys.clone(),
                        items: items.clone(),
                    });
                    if let Some(defaults) = child.get("default").and_then(Value::as_array) {
                        for (index, value) in defaults.iter().enumerate() {
                            self.rows.push(item_row(
                                &items,
                                keys,
                                depth + 1,
                                index,
                                Some(value),
                                id,
                            ));
                        }
                    }
                    self.rows.push(FormRow {
                        depth: depth + 1,
                        label: t("＋ add item").to_string(),
                        keys: keys.clone(),
                        display: format!("{}[]", display_path(keys)),
                        kind: RowKind::AddItem { array: id },
                        text: String::new(),
                        cursor: 0,
                        checked: false,
                    });
                }
                _ => {
                    self.rows.push(leaf_row(
                        child,
                        key,
                        keys,
                        depth,
                        needed,
                        child.get("default"),
                        None,
                    ));
                }
            }
            keys.pop();
        }
    }

    /// 聚焦行的下标；没有可聚焦行时保持 0。
    fn focus_first(&self) -> usize {
        self.rows
            .iter()
            .position(|row| row.kind.focusable())
            .unwrap_or(0)
    }

    fn next_focus(&self, from: usize, forward: bool) -> usize {
        let len = self.rows.len();
        if len == 0 {
            return 0;
        }
        let mut index = from;
        for _ in 0..len {
            index = if forward {
                (index + 1) % len
            } else if index == 0 {
                len - 1
            } else {
                index - 1
            };
            if self.rows[index].kind.focusable() {
                return index;
            }
        }
        from
    }

    /// 把第一个能匹配错误路径的行作为焦点。
    fn focus_error(&mut self, message: &str) {
        let token = message.split_whitespace().next().unwrap_or_default();
        let path = token.strip_prefix("args.").unwrap_or(token);
        let mut best: Option<(usize, usize)> = None;
        for (index, row) in self.rows.iter().enumerate() {
            if !row.kind.focusable() || row.display.is_empty() {
                continue;
            }
            let inside = path == row.display
                || path.starts_with(&format!("{}.", row.display))
                || path.starts_with(&format!("{}[", row.display));
            if inside && best.is_none_or(|(_, len)| row.display.len() > len) {
                best = Some((index, row.display.len()));
            }
        }
        if let Some((index, _)) = best {
            self.selected = index;
        }
        self.error_row = best.map(|(index, _)| index);
    }

    fn leaf_row(&self, keys: &[String]) -> Option<&FormRow> {
        self.rows.iter().find(|row| {
            row.keys == keys
                && matches!(
                    row.kind,
                    RowKind::Text { .. } | RowKind::Enum { .. } | RowKind::Bool
                )
        })
    }

    fn array_id(&self, keys: &[String]) -> Option<usize> {
        self.arrays.iter().position(|array| array.keys == keys)
    }

    fn object_value(&self, schema: &Value, keys: &mut Vec<String>) -> Value {
        let mut map = Map::new();
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return Value::Object(map);
        };
        for (key, child) in properties {
            keys.push(key.clone());
            match child.get("type").and_then(Value::as_str) {
                Some("object") => {
                    map.insert(key.clone(), self.object_value(child, keys));
                }
                Some("array") => {
                    map.insert(key.clone(), self.array_value(child, keys));
                }
                _ => {
                    if let Some(row) = self.leaf_row(keys) {
                        if let Some(value) = leaf_value(child, row) {
                            map.insert(key.clone(), value);
                        }
                    }
                }
            }
            keys.pop();
        }
        Value::Object(map)
    }

    fn array_value(&self, schema: &Value, keys: &[String]) -> Value {
        let Some(id) = self.array_id(keys) else {
            return Value::Array(Vec::new());
        };
        let items = schema
            .get("items")
            .cloned()
            .unwrap_or_else(|| self.arrays[id].items.clone());
        let values = self
            .rows
            .iter()
            .filter(|row| matches!(row.kind, RowKind::ArrayItem { array } if array == id))
            .map(|row| scalar_value(&items, &row.text))
            .collect();
        Value::Array(values)
    }

    /// 数组追加一项，焦点落到新行。
    fn add_item(&mut self, id: usize) {
        let Some(at) = self
            .rows
            .iter()
            .position(|row| matches!(row.kind, RowKind::AddItem { array } if array == id))
        else {
            return;
        };
        let array = &self.arrays[id];
        let items = array.items.clone();
        let keys = array.keys.clone();
        let depth = self.rows[at].depth;
        self.rows
            .insert(at, item_row(&items, &keys, depth, 0, None, id));
        self.selected = at;
        self.reindex(id);
    }

    /// 删除焦点的数组项。
    fn remove_item(&mut self, index: usize) {
        let Some(&RowKind::ArrayItem { array }) = self.rows.get(index).map(|row| &row.kind) else {
            return;
        };
        self.rows.remove(index);
        self.reindex(array);
        self.selected = index.min(self.rows.len().saturating_sub(1));
        if self
            .rows
            .get(self.selected)
            .is_none_or(|row| !row.kind.focusable())
        {
            self.selected = self.next_focus(self.selected, true);
        }
    }

    /// 数组项显示名按当前顺序重排。
    fn reindex(&mut self, id: usize) {
        let mut index = 0;
        for row in self.rows.iter_mut() {
            if matches!(row.kind, RowKind::ArrayItem { array } if array == id) {
                row.label = format!("[{index}]");
                row.display = format!("{}{}", display_path(&row.keys), row.label);
                index += 1;
            }
        }
    }

    fn menu_key(&mut self, key: &KeyEvent) {
        let Some((row, mut selected)) = self.menu else {
            return;
        };
        let options = match self.rows.get(row).map(|row| &row.kind) {
            Some(RowKind::Enum { options, .. }) => options.clone(),
            _ => {
                self.menu = None;
                return;
            }
        };
        match key.code {
            KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab => self.menu = None,
            KeyCode::Up => {
                selected = selected.saturating_sub(1);
                self.menu = Some((row, selected));
            }
            KeyCode::Down => {
                selected = (selected + 1).min(options.len().saturating_sub(1));
                self.menu = Some((row, selected));
            }
            KeyCode::Enter => {
                if let Some(RowKind::Enum {
                    selected: chosen, ..
                }) = self.rows.get_mut(row).map(|row| &mut row.kind)
                {
                    *chosen = Some(selected);
                }
                self.menu = None;
            }
            _ => {}
        }
    }

    /// 编辑焦点行的文本；返回是否消费了这次按键。
    fn edit(&mut self, key: &KeyEvent) -> bool {
        let Some(row) = self.rows.get_mut(self.selected) else {
            return false;
        };
        match &row.kind {
            RowKind::Bool => match key.code {
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                    row.checked = !row.checked;
                    true
                }
                _ => false,
            },
            RowKind::Text { .. } | RowKind::ArrayItem { .. } => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Backspace => {
                        if let Some(previous) = row.text[..row.cursor].char_indices().last() {
                            row.text.replace_range(previous.0..row.cursor, "");
                            row.cursor = previous.0;
                        }
                        true
                    }
                    KeyCode::Delete => {
                        if let Some(next) = row.text[row.cursor..].chars().next() {
                            row.text
                                .replace_range(row.cursor..row.cursor + next.len_utf8(), "");
                        }
                        true
                    }
                    KeyCode::Left => {
                        if let Some(previous) = row.text[..row.cursor].char_indices().last() {
                            row.cursor = previous.0;
                        }
                        true
                    }
                    KeyCode::Right => {
                        if let Some(next) = row.text[row.cursor..].chars().next() {
                            row.cursor += next.len_utf8();
                        }
                        true
                    }
                    KeyCode::Home => {
                        row.cursor = 0;
                        true
                    }
                    KeyCode::End => {
                        row.cursor = row.text.len();
                        true
                    }
                    KeyCode::Char(character) if control && matches!(character, 'u' | 'U') => {
                        row.text.clear();
                        row.cursor = 0;
                        true
                    }
                    KeyCode::Char(character) if !control => {
                        row.text.insert(row.cursor, character);
                        row.cursor += character.len_utf8();
                        true
                    }
                    _ => false,
                }
            }
            RowKind::Enum { .. } | RowKind::Group | RowKind::AddItem { .. } => false,
        }
    }
}

fn group_row(label: &str, keys: &[String], depth: usize) -> FormRow {
    FormRow {
        depth,
        label: label.to_string(),
        keys: keys.to_vec(),
        display: display_path(keys),
        kind: RowKind::Group,
        text: String::new(),
        cursor: 0,
        checked: false,
    }
}

fn item_row(
    schema: &Value,
    keys: &[String],
    depth: usize,
    index: usize,
    initial: Option<&Value>,
    array: usize,
) -> FormRow {
    let mut row = leaf_row(schema, "", keys, depth, false, initial, Some(array));
    row.label = format!("[{index}]");
    row.display = format!("{}{}", display_path(keys), row.label);
    row
}

fn leaf_row(
    schema: &Value,
    label: &str,
    keys: &[String],
    depth: usize,
    required: bool,
    initial: Option<&Value>,
    array: Option<usize>,
) -> FormRow {
    let kind = match array {
        Some(array) => RowKind::ArrayItem { array },
        None => match schema.get("enum").and_then(Value::as_array) {
            Some(options) => RowKind::Enum {
                options: options.clone(),
                selected: initial.and_then(|value| options.iter().position(|item| item == value)),
            },
            None if schema.get("type").and_then(Value::as_str) == Some("boolean") => RowKind::Bool,
            None => RowKind::Text { required },
        },
    };
    let text = match &kind {
        RowKind::Text { .. } | RowKind::ArrayItem { .. } => {
            initial.map(text_value).unwrap_or_default()
        }
        _ => String::new(),
    };
    FormRow {
        depth,
        label: label.to_string(),
        keys: keys.to_vec(),
        display: display_path(keys),
        checked: initial.and_then(Value::as_bool).unwrap_or(false),
        cursor: text.len(),
        kind,
        text,
    }
}

/// 默认值或数组项的初始文本。
fn text_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn display_path(keys: &[String]) -> String {
    keys.join(".")
}

/// 一行字段还原成 JSON 值；空的可选文本表示「没填」，整键省略。
fn leaf_value(schema: &Value, row: &FormRow) -> Option<Value> {
    match &row.kind {
        RowKind::Bool => Some(Value::Bool(row.checked)),
        RowKind::Enum {
            options, selected, ..
        } => selected.and_then(|index| options.get(index).cloned()),
        RowKind::Text { required } => {
            if row.text.is_empty() && !*required {
                None
            } else {
                Some(scalar_value(schema, &row.text))
            }
        }
        RowKind::ArrayItem { .. } => Some(scalar_value(schema, &row.text)),
        RowKind::Group | RowKind::AddItem { .. } => None,
    }
}

/// 文本按 schema 的标量类型转换；转不动时保留字符串，交给 validate_args 报类型错误。
fn scalar_value(schema: &Value, text: &str) -> Value {
    match schema.get("type").and_then(Value::as_str) {
        Some("integer") => text
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(text.to_string())),
        Some("number") => text
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(text.to_string())),
        Some("boolean") => text
            .parse::<bool>()
            .map(Value::Bool)
            .unwrap_or_else(|_| Value::String(text.to_string())),
        _ => Value::String(text.to_string()),
    }
}

impl App {
    /// 菜单、命令面板选中插件动作时按 schema 打开表单。
    /// 没有可填写字段时返回 `false`，由调用方走原来的分发器。
    pub(super) fn open_plugin_form(&mut self, name: &str, args: Value) -> bool {
        if crate::plugin::registry::disabled_package(&self.config, name).is_some() {
            return false;
        }
        let Some(action) = self.plugins.action(name) else {
            return false;
        };
        let Some(schema) = action.args.as_ref() else {
            return false;
        };
        let form = ActionForm::build(
            &action.name,
            &action.title,
            schema,
            args.get("_target").cloned().unwrap_or(Value::Null),
        );
        if !form.has_fields() {
            return false;
        }
        self.form = Some(form);
        self.note = t1("fill in {} arguments", &action.title);
        self.dirty = true;
        true
    }

    pub(super) fn on_form_key(&mut self, key: KeyEvent) {
        let Some(mut form) = self.form.take() else {
            return;
        };
        if key.kind == KeyEventKind::Release {
            self.form = Some(form);
            return;
        }
        if form.menu.is_some() {
            form.menu_key(&key);
            self.form = Some(form);
            self.dirty = true;
            return;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.note = t("cancelled").to_string();
                self.dirty = true;
                return;
            }
            KeyCode::Enter => {
                let kind = form.rows.get(form.selected).map(|row| row.kind.clone());
                match kind {
                    Some(RowKind::AddItem { array }) => form.add_item(array),
                    Some(RowKind::Enum { selected, .. }) => {
                        form.menu = Some((form.selected, selected.unwrap_or(0)));
                    }
                    _ => {
                        self.form = Some(form);
                        self.submit_form();
                        return;
                    }
                }
            }
            KeyCode::Tab | KeyCode::Down => {
                form.selected = form.next_focus(form.selected, true);
            }
            KeyCode::BackTab | KeyCode::Up => {
                form.selected = form.next_focus(form.selected, false);
            }
            KeyCode::Char(character) if control && matches!(character, 'a' | 'A') => {
                let array = match form.rows.get(form.selected).map(|row| &row.kind) {
                    Some(RowKind::ArrayItem { array } | RowKind::AddItem { array }) => Some(*array),
                    _ => None,
                };
                if let Some(array) = array {
                    form.add_item(array);
                }
            }
            KeyCode::Char(character) if control && matches!(character, 'd' | 'D') => {
                if matches!(
                    form.rows.get(form.selected).map(|row| &row.kind),
                    Some(RowKind::ArrayItem { .. })
                ) {
                    form.remove_item(form.selected);
                }
            }
            _ => {
                form.edit(&key);
            }
        }
        self.form = Some(form);
        self.dirty = true;
    }

    /// 校验通过才派发；错误留在表单里并把焦点落到出错的行。
    pub(super) fn submit_form(&mut self) {
        let Some(mut form) = self.form.take() else {
            return;
        };
        let Some(action) = self.plugins.action(&form.action) else {
            self.note = t1("unknown plugin action {}", &form.action);
            self.dirty = true;
            return;
        };
        let values = form.values();
        let args = match action.args.as_ref() {
            Some(schema) => match manifest::validate_args(schema, &values) {
                Ok(checked) => checked,
                Err(message) => {
                    form.focus_error(&message);
                    form.error = Some(message);
                    self.form = Some(form);
                    self.dirty = true;
                    return;
                }
            },
            None => values,
        };
        let mut args = args;
        let target = form.target();
        if !target.is_null() && target != json!({}) {
            if let Some(map) = args.as_object_mut() {
                map.insert("_target".to_string(), target);
            }
        }
        if let Err(error) = self.execute_action(&form.action, args) {
            self.note = error.to_string();
        }
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["env", "count"],
            "properties": {
                "env": {"type": "string", "enum": ["dev", "prod"]},
                "count": {"type": "integer"},
                "dry": {"type": "boolean", "default": true},
                "remote": {
                    "type": "object",
                    "properties": {"name": {"type": "string", "default": "box"}}
                },
                "hosts": {"type": "array", "items": {"type": "string"}}
            }
        })
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn rows_follow_the_schema_and_submit_validated_values() {
        let mut form = ActionForm::build("dev.example.deploy", "Deploy", &schema(), json!({}));
        let labels: Vec<&str> = form.rows().iter().map(|row| row.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "count",
                "dry",
                "env",
                "hosts",
                "＋ add item",
                "remote",
                "name"
            ]
        );
        // 默认值落进控件：boolean 打开、嵌套对象预填。
        let dry = form.rows().iter().find(|row| row.label == "dry").unwrap();
        assert!(dry.checked);
        let name = form.rows().iter().find(|row| row.label == "name").unwrap();
        assert_eq!(name.text, "box");

        // 选中 env，打开枚举菜单并选 dev；count 填字母先让校验拦下来。
        form.selected = form
            .rows()
            .iter()
            .position(|row| row.label == "env")
            .unwrap();
        form.menu = Some((form.selected, 0));
        form.menu_key(&key(KeyCode::Down));
        form.menu_key(&key(KeyCode::Up));
        form.menu_key(&key(KeyCode::Enter));
        assert!(form.menu.is_none());

        form.selected = form
            .rows()
            .iter()
            .position(|row| row.label == "count")
            .unwrap();
        form.edit(&key(KeyCode::Char('x')));
        let values = form.values();
        assert_eq!(values["env"], json!("dev"));
        assert_eq!(values["dry"], json!(true));
        assert_eq!(values["remote"], json!({"name": "box"}));
        assert_eq!(values["hosts"], json!([]));
        let error = manifest::validate_args(&schema(), &values).unwrap_err();
        assert!(error.starts_with("args.count"), "{error}");
        form.focus_error(&error);
        assert_eq!(form.error_row, Some(form.selected));

        // 改成数字后校验通过，数组项一并还原。
        form.rows[form.selected].text = "3".into();
        form.selected = form
            .rows()
            .iter()
            .position(|row| matches!(row.kind, RowKind::AddItem { .. }))
            .unwrap();
        form.add_item(0);
        form.add_item(0);
        assert_eq!(
            form.rows()
                .iter()
                .filter(|r| r.label.starts_with('['))
                .count(),
            2
        );
        {
            let items: Vec<usize> = form
                .rows()
                .iter()
                .enumerate()
                .filter(|(_, row)| matches!(row.kind, RowKind::ArrayItem { .. }))
                .map(|(index, _)| index)
                .collect();
            form.rows[items[0]].text = "a".into();
            form.rows[items[1]].text = "b".into();
            form.selected = items[1];
        }
        form.remove_item(form.selected);
        let checked = manifest::validate_args(&schema(), &form.values()).unwrap();
        assert_eq!(checked["count"], json!(3));
        assert_eq!(checked["dry"], json!(true));
        assert_eq!(checked["hosts"], json!(["a"]));
        assert_eq!(checked["remote"]["name"], json!("box"));
        assert_eq!(checked["env"], json!("dev"));
    }

    #[test]
    fn unpicked_required_enum_is_omitted_and_errors_land_on_its_row() {
        let schema = json!({
            "type": "object",
            "required": ["env"],
            "properties": {
                "env": {"type": "string", "enum": ["dev", "prod"]},
                "note": {"type": "string"}
            }
        });
        let form = ActionForm::build("dev.example.run", "Run", &schema, json!({}));
        assert_eq!(form.values(), json!({}));
        let error = manifest::validate_args(&schema, &form.values()).unwrap_err();
        assert_eq!(error, "args.env is required");
        let mut form = form;
        form.focus_error(&error);
        assert_eq!(form.error_row, Some(0));
        assert_eq!(form.rows[form.selected].label, "env");
    }

    const MANIFEST: &str = r#"
manifest_version = 1
id = "dev.example.form"
name = "Form probe"
version = "0.1.0"
host_api = 1

[[actions]]
id = "deploy"
title = "Deploy"
contexts = ["global", "pane"]
argv = ["sh", "-c", "cat"]
timeout_ms = 5000

[actions.args]
type = "object"
additionalProperties = false
required = ["env", "count"]
properties = { env = { type = "string", enum = ["dev", "prod"] }, count = { type = "integer" }, dry = { type = "boolean", default = true } }
"#;

    fn app() -> App {
        let (_, events) = std::sync::mpsc::channel();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let root =
            std::env::temp_dir().join(format!("hctui-form-app-{}-{unique}", std::process::id()));
        App::with_network(
            Config {
                restore: false,
                prompt_new_tab: false,
                config_path: root.join("config").join("config.toml"),
                state_path: root.join("state").join("state.json"),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap()
    }

    fn install(app: &mut App) -> String {
        let dir = app.config.plugins_dir().join("dev.example.form");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), MANIFEST).unwrap();
        app.plugins = crate::plugin::registry::Registry::load(&app.config);
        "dev.example.form.deploy".to_string()
    }

    fn focus(form: &mut ActionForm, label: &str) {
        form.selected = form
            .rows()
            .iter()
            .position(|row| row.label == label)
            .unwrap();
    }

    fn wait_for_status(app: &App, request: &str) -> crate::plugin::runner::View {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let view = app.plugin_runner.status(request).expect("request exists");
            if view.status.finished() {
                return view;
            }
            assert!(std::time::Instant::now() < deadline, "{request} stuck");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn the_form_collects_enum_bool_and_number_then_dispatches_once() {
        let mut app = app();
        let name = install(&mut app);
        assert!(app.open_plugin_form(&name, json!({"_target":{"pane":1}})));
        let form = app.form.as_mut().unwrap();
        focus(form, "env");
        app.on_key(key(KeyCode::Enter));
        assert!(app.form.as_ref().unwrap().menu.is_some(), "enum menu opens");
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(
            app.form.as_ref().unwrap().rows()[app.form.as_ref().unwrap().selected].label,
            "dry"
        );
        app.on_key(key(KeyCode::Char(' ')));
        focus(app.form.as_mut().unwrap(), "count");
        app.on_key(key(KeyCode::Char('3')));
        app.on_key(key(KeyCode::Enter));
        assert!(app.form.is_none(), "{:?}", app.form);
        let request = app.plugin_request.clone().expect("dispatched");
        let view = wait_for_status(&app, &request);
        assert_eq!(view.status.as_str(), "done", "{view:?}");
        let context: Value = serde_json::from_str(&view.stdout).unwrap();
        assert_eq!(context["args"], json!({"env":"prod","count":3,"dry":false}));
        assert_eq!(context["context"]["pane"], json!(1));
    }

    #[test]
    fn invalid_values_stay_inline_and_never_dispatch() {
        let mut app = app();
        let name = install(&mut app);
        assert!(app.open_plugin_form(&name, json!({})));
        focus(app.form.as_mut().unwrap(), "env");
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Enter));
        focus(app.form.as_mut().unwrap(), "count");
        app.on_key(key(KeyCode::Char('9')));
        app.on_key(key(KeyCode::Char('x')));
        app.on_key(key(KeyCode::Enter));
        let form = app.form.as_ref().expect("form stays open");
        assert_eq!(form.error.as_deref(), Some("args.count must be integer"));
        assert_eq!(form.rows()[form.error_row.unwrap()].label, "count");
        assert!(app.plugin_request.is_none(), "nothing was dispatched");
        // 改回合法值后同一次回车就派发。
        app.on_key(key(KeyCode::Backspace));
        app.on_key(key(KeyCode::Char('9')));
        app.on_key(key(KeyCode::Enter));
        assert!(app.form.is_none());
        assert!(app.plugin_request.is_some());
    }

    #[test]
    fn control_requests_report_confirmation_pending_while_the_form_is_open() {
        let mut app = app();
        let name = install(&mut app);
        assert!(app.open_plugin_form(&name, json!({})));
        let reply = app.control_reply(&json!({"method":"do","command":name,"args":{}}));
        assert_eq!(reply, json!({"accepted":true,"confirmation_pending":true}));
        assert!(app.plugin_request.is_none(), "control did not dispatch");
        // 取消：表单关闭、界面记 cancelled、控制通道随后照常派发。
        app.on_key(key(KeyCode::Esc));
        assert!(app.form.is_none());
        assert_eq!(app.note, "cancelled");
        let reply = app
            .control_reply(&json!({"method":"do","command":name,"args":{"env":"dev","count":1}}));
        assert_eq!(reply["accepted"], json!(true));
        assert_eq!(reply["confirmation_pending"], json!(false));
        assert!(reply["request"].as_str().is_some(), "{reply}");
    }

    #[test]
    fn explicit_arguments_bypass_the_form() {
        let mut app = app();
        let name = install(&mut app);
        app.execute_action(&name, json!({"env":"dev","count":1}))
            .unwrap();
        assert!(app.form.is_none(), "do/winless path keeps its semantics");
        assert!(app.plugin_request.is_some());
    }
}
