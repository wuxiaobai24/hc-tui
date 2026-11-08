//! Picker 候选行归一：宿主把九种来源做成结构化行，交给 `picker` 模块。
//!
//! 行只在宿主内部构造；模块拿到的是 `{id, label, section, right, context,
//! cwd, enabled, action: {name, args}}`，不需要解释 `group:` / `binding:` /
//! `custom:` 这类字符串协议。模块不可用（`--no-mods`、未安装、停用）时，
//! 每个入口保留原有的 Rust 选择器。

use super::*;

/// 两种形态：固定宽度的选择器与占满内容区的搜索面板。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerForm {
    Chooser,
    Goto,
}

/// 过滤字段：goto 匹配名称与终端地址，palette 匹配名称与说明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerFilter {
    LabelId,
    LabelContext,
}

/// 一条候选行；`action` 可直接交给 `execute_action`。
#[derive(Debug, Clone)]
pub struct PickerRow {
    pub id: String,
    pub label: String,
    pub section: bool,
    pub right: String,
    pub context: String,
    pub cwd: String,
    pub enabled: bool,
    pub action: Option<Value>,
    /// 内置回退使用的旧值；模块看不到它。
    pub legacy: String,
}

impl PickerRow {
    fn item(id: impl Into<String>, label: impl Into<String>, action: Value) -> PickerRow {
        PickerRow {
            id: id.into(),
            label: label.into(),
            section: false,
            right: String::new(),
            context: String::new(),
            cwd: String::new(),
            enabled: true,
            action: Some(action),
            legacy: String::new(),
        }
    }

    fn with_legacy(mut self, legacy: impl Into<String>) -> Self {
        self.legacy = legacy.into();
        self
    }

    fn section(label: impl Into<String>) -> PickerRow {
        PickerRow {
            id: String::new(),
            label: label.into(),
            section: true,
            right: String::new(),
            context: String::new(),
            cwd: String::new(),
            enabled: true,
            action: None,
            legacy: String::new(),
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "section": self.section,
            "right": self.right,
            "context": self.context,
            "cwd": self.cwd,
            "enabled": self.enabled,
            "action": self.action,
        })
    }
}

/// 一次打开的完整描述；宿主据此算矩形、props 与回退用数据。
#[derive(Debug, Clone)]
pub struct PickerSpec {
    pub form: PickerForm,
    pub title: String,
    pub rows: Vec<PickerRow>,
    pub selected: usize,
    pub machines: bool,
    pub palette: bool,
    pub filter: PickerFilter,
    /// 标题右侧计数单位与提示；文案由宿主本地化后放进 props。
    pub count_label: &'static str,
    pub nav_left: String,
    pub placeholder: String,
    pub hint: String,
    pub empty: String,
}

impl PickerSpec {
    fn chooser(title: String, rows: Vec<PickerRow>, machines: bool) -> PickerSpec {
        PickerSpec {
            form: PickerForm::Chooser,
            title,
            rows,
            selected: 0,
            machines,
            palette: false,
            filter: PickerFilter::LabelId,
            count_label: "terminals",
            nav_left: String::new(),
            placeholder: String::new(),
            hint: String::new(),
            empty: String::new(),
        }
    }

    fn goto(rows: Vec<PickerRow>, selected: usize, palette: bool) -> PickerSpec {
        PickerSpec {
            form: PickerForm::Goto,
            title: String::new(),
            rows,
            selected,
            machines: false,
            palette,
            filter: if palette {
                PickerFilter::LabelContext
            } else {
                PickerFilter::LabelId
            },
            count_label: if palette { "commands" } else { "terminals" },
            nav_left: if palette {
                t("─ Commands ").to_string()
            } else {
                t("─ Go to ").to_string()
            },
            placeholder: if palette {
                t("search commands").to_string()
            } else {
                t("search agents and terminals").to_string()
            },
            hint: t(" / search · ↑↓ rows · enter open · esc close").to_string(),
            empty: t(" No matching agents or terminals").to_string(),
        }
    }

    fn row_height(&self, content: Rect) -> u16 {
        if content.width <= 64 && content.height >= 12 {
            2
        } else {
            1
        }
    }

    /// 与内置选择器完全一致的矩形；`None` 表示内置实现在这个尺寸下不画。
    fn rect(&self, content: Rect, screen: Rect) -> Option<Rect> {
        match self.form {
            PickerForm::Chooser => {
                if content.width < 20 || content.height < 5 {
                    return None;
                }
                let width = content.width.min(64);
                let row_height = self.row_height(content);
                let wanted = (self.rows.len() as u16).saturating_mul(row_height)
                    + 3
                    + 2 * u16::from(self.machines);
                let height = wanted.min(content.height.saturating_sub(2)).max(3);
                Some(Rect::new(
                    content.x + (content.width - width) / 2,
                    content.y + (content.height - height) / 2,
                    width,
                    height,
                ))
            }
            PickerForm::Goto => {
                let area = Rect::new(
                    screen.x + 2,
                    screen.y + 1,
                    screen.width.saturating_sub(4),
                    screen.height.saturating_sub(2),
                );
                let _ = content;
                (area.width >= 24 && area.height >= 8).then_some(area)
            }
        }
    }

    /// 启动参数里的 `props`。
    fn props(&self, rect: Rect, content: Rect) -> Value {
        let rows: Vec<Value> = self.rows.iter().map(PickerRow::to_value).collect();
        let count = self.rows.iter().filter(|row| !row.section).count();
        match self.form {
            PickerForm::Chooser => {
                let row_height = self.row_height(content);
                let inner = rect.height.saturating_sub(2);
                let visible = inner
                    .saturating_sub(2 * u16::from(self.machines))
                    .checked_div(row_height)
                    .unwrap_or(1)
                    .max(1);
                json!({
                    "form": "chooser",
                    "title": self.title,
                    "cols": rect.width,
                    "height": rect.height,
                    "rows": rows,
                    "selected": self.selected,
                    "rowHeight": row_height,
                    "visible": visible,
                    "machines": self.machines,
                    "buttons": [
                        [t(" Open "), t(" Add ")],
                        [t(" Connect "), t(" Rename "), t(" Remove ")],
                    ],
                })
            }
            PickerForm::Goto => json!({
                "form": "goto",
                "cols": rect.width,
                "height": rect.height,
                "rows": rows,
                "selected": self.selected,
                "visible": rect.height.saturating_sub(7).max(1),
                "filter": if self.filter == PickerFilter::LabelContext { "label+context" } else { "label+id" },
                "palette": self.palette,
                "navLeft": self.nav_left,
                "countLabel": self.count_label,
                "count": count,
                "placeholder": self.placeholder,
                "hint": self.hint,
                "empty": self.empty,
            }),
        }
    }
}

impl App {
    /// 打开模块选择器；模块不可用时返回 `false`，由调用方走内置实现。
    pub fn show_picker(&mut self, spec: &PickerSpec) -> bool {
        if self.config.no_mods {
            return false;
        }
        let Some(rect) = spec.rect(self.content, self.screen) else {
            return false;
        };
        let props = spec.props(rect, self.content);
        let size = Some((rect.width, rect.height));
        if self
            .mods
            .surfaces
            .iter()
            .any(|surface| surface.mod_id == "picker")
        {
            self.mods_close("picker");
        }
        match self.mods_open(
            "picker",
            crate::mods::SurfacePlace::Dialog,
            props,
            size,
            None,
            None,
        ) {
            Ok(id) => {
                if let Some(surface) = self.mods.surface_mut(&id) {
                    surface.area = rect;
                }
                true
            }
            Err(_) => false,
        }
    }

    /// 动作菜单与其子菜单；`group` 为 `root` / `panes` / `tabs` /
    /// `terminals` / `global` / `swap` / `preset`。
    pub fn open_actions_picker_group(&mut self, group: &str) -> bool {
        let spec = self.picker_spec_actions(group);
        if self.show_picker(&spec) {
            return true;
        }
        self.open_action_items_legacy(&spec, group);
        true
    }

    /// 工作区选择（`binding:workspace_picker`）。
    pub fn open_workspace_picker(&mut self) -> bool {
        let rows = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(index, workspace)| {
                PickerRow::item(
                    format!("sp{}", workspace.id),
                    self.workspace_label(index),
                    json!({"name": "space.select", "args": {"id": workspace.id}}),
                )
                .with_legacy(format!("workspace:{index}"))
            })
            .collect();
        let spec = PickerSpec::chooser(t("workspace picker").to_string(), rows, false);
        if self.show_picker(&spec) {
            return true;
        }
        self.open_action_items_legacy(&spec, "workspace");
        true
    }

    /// 移动 Pane 的目标 Tab（`binding:move_pane`）。
    pub fn open_move_pane_picker(&mut self) -> bool {
        let pane = self.focused();
        let rows = self
            .workspace()
            .into_iter()
            .flat_map(|workspace| workspace.tabs.iter().enumerate())
            .map(|(index, tab)| {
                PickerRow::item(
                    format!("tb{}", tab.id),
                    tab.display(&self.panes),
                    json!({"name": "picker.move", "args": {"pane": pane, "to": format!("tb{}", tab.id)}}),
                )
                .with_legacy(format!("move:{index}"))
            })
            .chain(std::iter::once(
                PickerRow::item(
                    "new",
                    t("new tab"),
                    json!({"name": "picker.move", "args": {"pane": pane, "to": "new"}}),
                )
                .with_legacy("move:new"),
            ))
            .collect();
        let spec = PickerSpec::chooser(t("move pane").to_string(), rows, false);
        if self.show_picker(&spec) {
            return true;
        }
        self.open_action_items_legacy(&spec, "move");
        true
    }

    /// Resize 菜单；选择一项后由动作重新打开同一份菜单。
    pub fn open_resize_picker(&mut self) -> bool {
        let rows = vec![
            ("Wider", "l"),
            ("Narrower", "h"),
            ("Taller", "j"),
            ("Shorter", "k"),
            ("Done", "done"),
        ]
        .into_iter()
        .map(|(label, key)| {
            PickerRow::item(
                key,
                t(label),
                json!({"name": "picker.resize", "args": {"key": key}}),
            )
            .with_legacy(if key == "done" {
                "done".to_string()
            } else {
                format!("resize:{key}")
            })
        })
        .collect();
        let spec = PickerSpec::chooser(t("RESIZE").to_string(), rows, false);
        if self.show_picker(&spec) {
            return true;
        }
        self.open_action_items_legacy(&spec, "resize");
        true
    }

    /// Swap Pane 的目标。
    pub fn open_swap_picker(&mut self) -> bool {
        let source = self.focused();
        let rows = self
            .tab()
            .into_iter()
            .flat_map(|tab| tab.tree.panes())
            .filter(|id| id.0 != source)
            .map(|id| {
                let label = self
                    .panes
                    .get(&id.0)
                    .map(|pane| pane.label())
                    .unwrap_or_else(|| id.0.to_string());
                PickerRow::item(
                    id.0.to_string(),
                    label,
                    json!({"name": "pane.swap", "args": {"pane": source.to_string(), "with": id.0.to_string()}}),
                )
                .with_legacy(format!("swap:{}", id.0))
            })
            .collect();
        let spec = PickerSpec::chooser(t("Swap pane").to_string(), rows, false);
        if self.show_picker(&spec) {
            return true;
        }
        self.open_action_items_legacy(&spec, "swap");
        true
    }

    /// 终端菜单：固定六项加 `pane` 上下文的插件动作。
    pub fn open_terminal_picker_menu(&mut self, address: &str) -> bool {
        let pinned = self
            .terminals
            .iter()
            .find(|terminal| terminal.address == address)
            .is_some_and(|terminal| terminal.pinned);
        let pane = self
            .panes
            .iter()
            .find(|(_, state)| state.terminal.as_deref() == Some(address))
            .map(|(pane, _)| *pane);
        let target = json!({"pane": pane, "terminal": address});
        let rows = [
            (
                t("Rename"),
                "agent.rename",
                json!({"terminal": address}),
            ),
            (t("Peek"), "terminal.peek", json!({"terminal": address})),
            (
                t("Keep as tab"),
                "terminal.keep",
                json!({"terminal": address}),
            ),
            (
                t("Park (keep running)"),
                "terminal.park",
                json!({"terminal": address}),
            ),
            (
                if pinned { t("Unpin") } else { t("Pin") },
                "terminal.pin",
                json!({"terminal": address, "pinned": !pinned}),
            ),
            (
                t("End program and sub-agents"),
                "terminal.kill",
                json!({"terminal": address}),
            ),
        ]
        .into_iter()
        .map(|(label, name, args)| {
            PickerRow::item(name, label, json!({"name": name, "args": args}))
                .with_legacy(format!("{name} {args}"))
        })
        .chain(
            self.plugins
                .actions()
                .into_iter()
                .filter(|action| action.contexts.iter().any(|context| context == "pane"))
                .map(|action| {
                    PickerRow::item(
                        action.name.clone(),
                        format!("{} · {}", action.title, action.name),
                        json!({"name": "picker.plugin", "args": {"name": action.name, "target": target}}),
                    )
                    .with_legacy(format!(
                        "{} {}",
                        action.name,
                        json!({"_target": target})
                    ))
                }),
        )
        .collect();
        let spec = PickerSpec::chooser(format!("{} · {address}", t("Terminal")), rows, false);
        if self.show_picker(&spec) {
            return true;
        }
        self.open_action_items_legacy(&spec, "terminal");
        true
    }

    /// 模板候选；数据来自最近一次 `templates.list` 缓存。
    pub fn open_templates_picker(&mut self) -> bool {
        let rows = self
            .mod_templates
            .iter()
            .filter_map(|template| {
                let name = template.get("name")?.as_str()?.to_string();
                let description = template
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let label = if description.is_empty() {
                    name.clone()
                } else {
                    format!("{name} — {description}")
                };
                Some(PickerRow::item(
                    &name,
                    label,
                    json!({"name": "picker.template", "args": {"argv": [name]}}),
                ))
            })
            .collect();
        let spec = PickerSpec::chooser(
            "templates（Enter 在新 tab 里运行）".to_string(),
            rows,
            false,
        );
        if self.show_picker(&spec) {
            return true;
        }
        let items = self
            .mod_templates
            .iter()
            .filter_map(|template| {
                let name = template.get("name")?.as_str()?.to_string();
                let description = template
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Some(ChooserItem {
                    label: if description.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} — {description}")
                    },
                    value: name,
                })
            })
            .collect();
        self.open_chooser(Chooser {
            title: "templates（Enter 在新 tab 里运行）".to_string(),
            items,
            selected: 0,
            kind: ChooserKind::Templates,
        });
        true
    }

    /// 机器候选；数据来自最近一次 `machine.list` 结果。
    pub fn open_machines_picker(&mut self) -> bool {
        let rows = self
            .machines
            .iter()
            .map(|machine| {
                PickerRow::item(
                    &machine.name,
                    format!("{} [{}] {}", machine.name, machine.state, machine.target),
                    json!({"name": "picker.machine", "args": {"op": "open", "machine": machine.name}}),
                )
            })
            .collect();
        let spec = PickerSpec::chooser(
            t("machines · Enter/n 新 pane · a 添加 · c 连接 · r 改名 · d 移除").to_string(),
            rows,
            true,
        );
        if self.show_picker(&spec) {
            return true;
        }
        let items = self
            .machines
            .iter()
            .map(|machine| ChooserItem {
                label: format!("{} [{}] {}", machine.name, machine.state, machine.target),
                value: machine.name.clone(),
            })
            .collect();
        self.open_chooser(Chooser {
            title: t("machines · Enter/n 新 pane · a 添加 · c 连接 · r 改名 · d 移除").to_string(),
            items,
            selected: 0,
            kind: ChooserKind::Machines,
        });
        true
    }

    /// 授予确认。
    pub fn open_grants_picker(&mut self, mod_id: &str) -> bool {
        let pending = self.mods.pending_grants_for(mod_id);
        if pending.is_empty() {
            return false;
        }
        let rows: Vec<PickerRow> = pending
            .iter()
            .map(|name| {
                PickerRow::item(
                    name,
                    t1("Approve {}", name),
                    json!({"name": "picker.grant", "args": {"mod": mod_id, "grant": name}}),
                )
            })
            .chain(std::iter::once(PickerRow::item(
                "*",
                t("Approve all"),
                json!({"name": "picker.grant", "args": {"mod": mod_id}}),
            )))
            .collect();
        let spec = PickerSpec::chooser(t1("Grants for {}", mod_id), rows, false);
        if self.show_picker(&spec) {
            return true;
        }
        let items: Vec<ChooserItem> = pending
            .iter()
            .map(|name| ChooserItem {
                label: t1("Approve {}", name),
                value: json!({"mod": mod_id, "name": name}).to_string(),
            })
            .chain(std::iter::once(ChooserItem {
                label: t("Approve all").to_string(),
                value: json!({"mod": mod_id}).to_string(),
            }))
            .collect();
        self.open_chooser(Chooser {
            title: t1("Grants for {}", mod_id),
            selected: 0,
            kind: ChooserKind::Grants,
            items,
        });
        true
    }

    /// Go to / palette 候选；过滤留给模块。
    pub fn open_goto_picker(&mut self, palette: bool) -> bool {
        let rows = self.picker_goto_rows(palette);
        let spec = PickerSpec::goto(rows, if palette { 0 } else { 1 }, palette);
        self.show_picker(&spec)
    }

    pub(super) fn picker_goto_rows(&self, palette: bool) -> Vec<PickerRow> {
        if palette {
            let mut rows: Vec<PickerRow> = actions::COMMANDS
                .iter()
                .map(|(name, description)| PickerRow {
                    id: name.to_string(),
                    label: name.to_string(),
                    section: false,
                    right: String::new(),
                    context: description.to_string(),
                    cwd: String::new(),
                    enabled: true,
                    action: Some(json!({"name": name, "args": {}})),
                    legacy: String::new(),
                })
                .collect();
            for action in self.plugins.actions() {
                if !action.contexts.iter().any(|context| context == "global") {
                    continue;
                }
                rows.push(PickerRow {
                    id: action.name.clone(),
                    label: action.name.clone(),
                    section: false,
                    right: String::new(),
                    context: action.title.clone(),
                    cwd: String::new(),
                    enabled: true,
                    action: Some(json!({"name": "picker.plugin", "args": {"name": action.name}})),
                    legacy: String::new(),
                });
            }
            return rows;
        }
        let mut rows = Vec::new();
        for (index, workspace) in self.workspaces.iter().enumerate() {
            let label = self.workspace_label(index);
            let cwd = self.workspace_cwd(index).unwrap_or_default();
            rows.push(PickerRow {
                id: format!("sp{}", workspace.id),
                label: label.clone(),
                section: true,
                right: self
                    .workspace_cwd(index)
                    .and_then(|cwd| self.branch(&cwd))
                    .unwrap_or_default(),
                context: String::new(),
                cwd,
                enabled: true,
                action: None,
                legacy: String::new(),
            });
            for tab in &workspace.tabs {
                for pane in tab.panes() {
                    let Some(state) = self.panes.get(&pane.0) else {
                        continue;
                    };
                    let name = state.label();
                    let address = state.terminal.clone().unwrap_or_default();
                    let located = self.locate_public(pane.0);
                    let position = located
                        .map(|(workspace, tab)| {
                            format!(
                                "{}/tab{}/{}",
                                self.workspaces[workspace].id,
                                self.workspaces[workspace].tabs[tab].id,
                                pane.0
                            )
                        })
                        .unwrap_or_default();
                    let context = located
                        .map(|(workspace, tab)| {
                            format!(
                                "{} / {} / w{}:p{}",
                                self.workspace_label(workspace),
                                self.workspaces[workspace].tabs[tab].display(&self.panes),
                                self.workspaces[workspace].id,
                                pane.0
                            )
                        })
                        .unwrap_or_default();
                    rows.push(PickerRow {
                        id: address.clone(),
                        label: format!("{} · {}", short(&address), short(&name)),
                        section: false,
                        right: position,
                        context,
                        cwd: state.cwd.clone().unwrap_or_default(),
                        enabled: true,
                        action: Some(
                            json!({"name": "pane.focus", "args": {"pane": pane.0.to_string()}}),
                        ),
                        legacy: String::new(),
                    });
                }
            }
        }
        let unowned: Vec<PickerRow> = self
            .terminals_without_pane()
            .into_iter()
            .map(|terminal| PickerRow {
                id: terminal.address.clone(),
                label: format!("{} · {}", short(&terminal.address), short(terminal.label())),
                section: false,
                right: if terminal.live {
                    "terminal"
                } else {
                    "finished"
                }
                .to_string(),
                context: String::new(),
                cwd: String::new(),
                enabled: true,
                action: Some(json!({"name": "pane.focus", "args": {"terminal": terminal.address}})),
                legacy: String::new(),
            })
            .collect();
        if !unowned.is_empty() {
            rows.push(PickerRow::section(t("other clients")));
            rows.extend(unowned);
        }
        rows
    }

    pub(super) fn picker_spec_actions(&self, group: &str) -> PickerSpec {
        let mut rows: Vec<PickerRow> = Vec::new();
        let mut title = t("Actions").to_string();
        match group {
            "root" => {
                for (label, name) in [
                    (t("panes"), "panes"),
                    (t("tabs and workspaces"), "tabs"),
                    (t("agents and terminals"), "terminals"),
                    (t("global"), "global"),
                ] {
                    rows.push(
                        PickerRow::item(
                            name,
                            label,
                            json!({"name": "picker.menu", "args": {"group": name}}),
                        )
                        .with_legacy(format!("group:{name}")),
                    );
                }
            }
            "panes" => {
                for name in [
                    "split_vertical",
                    "split_horizontal",
                    "zoom",
                    "rename_pane",
                    "move_pane",
                    "resize_mode",
                    "stack_panes",
                    "layout_next",
                    "layout_previous",
                    "float_pane",
                    "float_pin",
                    "float_hide",
                    "cycle_pane_next",
                    "cycle_pane_previous",
                    "last_pane",
                    "close_pane",
                    "kill_pane",
                ] {
                    rows.push(
                        PickerRow::item(
                            name,
                            Self::binding_title(name),
                            json!({"name": "picker.key", "args": {"name": name}}),
                        )
                        .with_legacy(format!("binding:{name}")),
                    );
                }
                rows.push(
                    PickerRow::item(
                        "swap",
                        t("Swap pane"),
                        json!({"name": "picker.menu", "args": {"group": "swap"}}),
                    )
                    .with_legacy("swap:choose"),
                );
            }
            "tabs" => {
                for name in [
                    "new_tab",
                    "rename_tab",
                    "move_tab_previous",
                    "move_tab_next",
                    "next_tab",
                    "previous_tab",
                    "close_tab",
                    "new_workspace",
                    "rename_workspace",
                    "workspace_picker",
                    "next_workspace",
                    "previous_workspace",
                    "close_workspace",
                ] {
                    rows.push(
                        PickerRow::item(
                            name,
                            Self::binding_title(name),
                            json!({"name": "picker.key", "args": {"name": name}}),
                        )
                        .with_legacy(format!("binding:{name}")),
                    );
                }
            }
            "terminals" => {
                rows.push(
                    PickerRow::item(
                        "agent.rename",
                        t("rename agent"),
                        json!({"name": "agent.rename", "args": {}}),
                    )
                    .with_legacy("action:agent.rename"),
                );
                for name in [
                    "copy_mode",
                    "read_pane",
                    "scroll_bottom",
                    "edit_history",
                    "paste_image",
                    "send_text",
                    "send_keys",
                    "wait_output",
                    "put_file",
                    "interrupt",
                    "toggle_keep",
                    "remove_terminal",
                    "open_link",
                    "next_agent",
                    "chat",
                ] {
                    rows.push(
                        PickerRow::item(
                            name,
                            Self::binding_title(name),
                            json!({"name": "picker.key", "args": {"name": name}}),
                        )
                        .with_legacy(format!("binding:{name}")),
                    );
                }
            }
            "global" => {
                for name in [
                    "goto",
                    "machines",
                    "templates",
                    "settings",
                    "help",
                    "toggle_sidebar",
                    "reload_config",
                    "open_notification_target",
                    "detach",
                ] {
                    rows.push(
                        PickerRow::item(
                            name,
                            Self::binding_title(name),
                            json!({"name": "picker.key", "args": {"name": name}}),
                        )
                        .with_legacy(format!("binding:{name}")),
                    );
                }
                rows.push(
                    PickerRow::item(
                        "files.open",
                        t("Files"),
                        json!({"name": "files.open", "args": {}}),
                    )
                    .with_legacy("action:files.open"),
                );
                rows.push(
                    PickerRow::item(
                        "changes.open",
                        t("Changes"),
                        json!({"name": "changes.open", "args": {}}),
                    )
                    .with_legacy("action:changes.open"),
                );
                rows.push(
                    PickerRow::item(
                        "preset",
                        t("Keybindings: default / Zellij"),
                        json!({"name": "picker.menu", "args": {"group": "preset"}}),
                    )
                    .with_legacy("preset"),
                );
                for (index, command) in self.config.commands.iter().enumerate() {
                    rows.push(
                        PickerRow::item(
                            index.to_string(),
                            command.command.clone(),
                            json!({"name": "picker.custom", "args": {"index": index}}),
                        )
                        .with_legacy(format!("custom:{index}")),
                    );
                }
            }
            "swap" => {
                title = t("Swap pane").to_string();
                let source = self.focused();
                for id in self.tab().into_iter().flat_map(|tab| tab.tree.panes()) {
                    if id.0 == source {
                        continue;
                    }
                    let label = self
                        .panes
                        .get(&id.0)
                        .map(|pane| pane.label())
                        .unwrap_or_else(|| id.0.to_string());
                    rows.push(PickerRow::item(
                        id.0.to_string(),
                        label,
                        json!({"name": "pane.swap", "args": {"pane": source.to_string(), "with": id.0.to_string()}}),
                    ));
                }
            }
            "preset" => {
                title = t("Keybindings").to_string();
                for (label, name) in [("Default (Ctrl+B)", "default"), ("Zellij", "zellij")] {
                    rows.push(
                        PickerRow::item(
                            name,
                            label,
                            json!({"name": "picker.preset", "args": {"name": name}}),
                        )
                        .with_legacy(format!("preset:{name}")),
                    );
                }
            }
            _ => {}
        }
        if group != "root" {
            rows.push(
                PickerRow::item(
                    "back",
                    t("Back"),
                    json!({"name": "picker.menu", "args": {"group": "root"}}),
                )
                .with_legacy("back"),
            );
        }
        PickerSpec::chooser(title, rows, false)
    }

    /// 内置回退：同一份行还原成原来的选择器数据（旧值与旧 kind 不变）。
    fn open_action_items_legacy(&mut self, spec: &PickerSpec, kind: &str) {
        let items: Vec<ChooserItem> = spec
            .rows
            .iter()
            .map(|row| ChooserItem {
                label: row.label.clone(),
                value: row.legacy.clone(),
            })
            .collect();
        let chooser_kind = match kind {
            "terminal" => ChooserKind::Commands,
            _ => ChooserKind::Actions,
        };
        self.open_chooser(Chooser {
            title: spec.title.clone(),
            items,
            selected: spec.selected,
            kind: chooser_kind,
        });
    }

    /// `picker.*` 动作的分发；这些名字不出现在命令目录里。
    pub(super) fn execute_picker_action(&mut self, rest: &str, args: Value) -> Result<()> {
        let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_string);
        match rest {
            "menu" => {
                let group = text("group").unwrap_or_else(|| "root".into());
                self.open_actions_picker_group(&group);
            }
            "key" => {
                let name = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                self.activate_pointer_action(&format!("binding:{name}"));
            }
            "custom" => {
                let index = args["index"].as_u64().unwrap_or_default() as usize;
                self.run_custom(index);
            }
            "preset" => {
                let name = text("name").unwrap_or_else(|| "default".into());
                self.activate_pointer_action(&format!("preset:{name}"));
            }
            "resize" => {
                let key = text("key").unwrap_or_default();
                if key == "done" {
                    self.resize_mode = false;
                } else if let Some(key) = key.chars().next() {
                    self.on_resize_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
                    self.open_resize_picker();
                }
            }
            "move" => {
                let pane = args["pane"].as_u64().unwrap_or_default() as u32;
                let to = text("to").unwrap_or_else(|| "new".into());
                self.focus_pane(pane);
                if to == "new" {
                    self.move_pane_to_new_tab();
                } else {
                    let index = self
                        .workspace()
                        .and_then(|workspace| {
                            workspace
                                .tabs
                                .iter()
                                .position(|tab| format!("tb{}", tab.id) == to)
                        })
                        .ok_or_else(|| anyhow::anyhow!(t1("unknown tab {}", to)))?;
                    self.move_pane_to_tab(index);
                }
            }
            "plugin" => {
                let name = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                let mut call_args = json!({});
                if let Some(target) = args.get("target").filter(|target| !target.is_null()) {
                    call_args["_target"] = target.clone();
                }
                if !self.open_plugin_form(&name, call_args.clone()) {
                    self.execute_action(&name, call_args)?;
                }
            }
            "grant" => {
                let mod_id = text("mod").ok_or_else(|| anyhow::anyhow!(t("mod required")))?;
                let mut value = json!({"mod": mod_id});
                if let Some(grant) = text("grant") {
                    value["name"] = json!(grant);
                }
                self.approve_mod_grant(&value.to_string());
            }
            "machine" => {
                let op = text("op").unwrap_or_default();
                let machine = text("machine").unwrap_or_default();
                match op.as_str() {
                    "add" => self.ask(
                        PromptKind::AddMachine,
                        "ssh 目标（user@host 或 ~/.ssh/config 的 Host）",
                    ),
                    "connect" => {
                        self.call_rpc(
                            "machine.connect",
                            "machine.connect",
                            json!({"machine": machine}),
                        );
                        self.note = t1("connecting {}", machine);
                    }
                    "rename" => self.ask(PromptKind::RenameMachine(machine), "机器新名字"),
                    "remove" => {
                        self.call_rpc("machine.rm", "machine.rm", json!({"machine": machine}));
                        self.note = t("removing machine").to_string();
                    }
                    _ => self.open_remote_pane(machine),
                }
            }
            "template" => {
                let argv: Vec<String> = args
                    .get("argv")
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                anyhow::ensure!(!argv.is_empty(), t("argv required"));
                self.spawn_program_in_new_tab(argv, None);
            }
            other => anyhow::bail!(t1("unknown picker action {}", other)),
        }
        Ok(())
    }
}
