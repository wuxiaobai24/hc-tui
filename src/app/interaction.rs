//! Named actions shared by keyboard bindings and pointer menus.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyboardMode {
    #[default]
    Normal,
    Locked,
    Pane,
    Tab,
    Resize,
    Move,
    Session,
}

#[derive(Debug, Clone, Copy)]
pub enum PointerTarget {
    Close,
    Confirm,
    Chooser(usize),
    Goto(usize),
    SettingsSection(usize),
    Setting(usize),
    Key(KeyCode),
}

#[derive(Clone, Copy)]
pub(super) enum PointerDrag {
    Pane { workspace: u32, tab: u32, pane: u32 },
    Tab { workspace: u32, tab: u32 },
    Header { x: u16, y: u16 },
    Switcher { y: u16, moved: bool },
}

impl App {
    pub fn open_actions_menu(&mut self) {
        self.keyboard_mode = KeyboardMode::Normal;
        self.open_action_items(
            t("Actions"),
            vec![
                (t("panes").into(), "group:panes".into()),
                (t("tabs and workspaces").into(), "group:tabs".into()),
                (t("agents and terminals").into(), "group:terminals".into()),
                (t("global").into(), "group:global".into()),
            ],
        );
    }

    fn open_action_items(&mut self, title: &str, items: Vec<(String, String)>) {
        self.pointer_drag = None;
        self.open_chooser(Chooser {
            title: title.into(),
            selected: 0,
            kind: ChooserKind::Actions,
            items: items
                .into_iter()
                .map(|(label, value)| ChooserItem { label, value })
                .collect(),
        });
    }

    pub(super) fn activate_pointer_action(&mut self, value: &str) {
        if let Some(group) = value.strip_prefix("group:") {
            let names: &[&str] = match group {
                "panes" => &[
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
                ],
                "tabs" => &[
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
                ],
                "terminals" => &[
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
                ],
                _ => &[
                    "goto",
                    "machines",
                    "templates",
                    "settings",
                    "help",
                    "toggle_sidebar",
                    "reload_config",
                    "open_notification_target",
                    "detach",
                ],
            };
            let mut items: Vec<_> = names
                .iter()
                .map(|name| (Self::binding_title(name).into(), format!("binding:{name}")))
                .collect();
            if group == "terminals" {
                items.insert(0, (t("rename agent").into(), "action:agent.rename".into()));
            }
            if group == "panes" {
                items.push((t("Swap pane").into(), "swap:choose".into()));
            }
            if group == "global" {
                items.push((t("Files").into(), "action:files.open".into()));
                items.push((t("Changes").into(), "action:changes.open".into()));
                items.push((t("Keybindings: default / Zellij").into(), "preset".into()));
                for (index, command) in self.config.commands.iter().enumerate() {
                    items.push((command.command.clone(), format!("custom:{index}")));
                }
            }
            items.push((t("Back").into(), "back".into()));
            self.open_action_items(t("Actions"), items);
        } else if let Some(target) = value.strip_prefix("swap:") {
            if target == "choose" {
                let source = self.focused();
                let items = self
                    .tab()
                    .into_iter()
                    .flat_map(|tab| tab.tree.panes())
                    .filter(|id| id.0 != source)
                    .map(|id| {
                        (
                            self.panes
                                .get(&id.0)
                                .map(|pane| pane.label())
                                .unwrap_or_else(|| id.0.to_string()),
                            format!("swap:{}", id.0),
                        )
                    })
                    .collect();
                self.open_action_items(t("Swap pane"), items);
            } else if let Ok(target) = target.parse() {
                if let Err(error) = self.swap_panes(self.focused(), target) {
                    self.note = error.to_string();
                }
            }
        } else if let Some(name) = value.strip_prefix("action:") {
            if let Err(error) = self.execute_action(name, json!({})) {
                self.note = error.to_string();
            }
        } else if let Some(name) = value.strip_prefix("binding:") {
            match name {
                "workspace_picker" => {
                    let items = self
                        .workspaces
                        .iter()
                        .enumerate()
                        .map(|(i, _)| (self.workspace_label(i), format!("workspace:{i}")))
                        .collect();
                    self.open_action_items(t("workspace picker"), items);
                }
                "move_pane" => {
                    let mut items: Vec<_> = self
                        .workspace()
                        .into_iter()
                        .flat_map(|w| w.tabs.iter().enumerate())
                        .map(|(i, tab)| (tab.display(&self.panes), format!("move:{i}")))
                        .collect();
                    items.push((t("new tab").into(), "move:new".into()));
                    self.open_action_items(t("move pane"), items);
                }
                "resize_mode" => self.open_action_items(
                    t("RESIZE"),
                    vec![
                        (t("Wider").into(), "resize:l".into()),
                        (t("Narrower").into(), "resize:h".into()),
                        (t("Taller").into(), "resize:j".into()),
                        (t("Shorter").into(), "resize:k".into()),
                        (t("Done").into(), "done".into()),
                    ],
                ),
                _ => self.run_named_binding(name),
            }
        } else if let Some(index) = value
            .strip_prefix("workspace:")
            .and_then(|v| v.parse().ok())
        {
            self.select_workspace(index);
        } else if let Some(target) = value.strip_prefix("move:") {
            if target == "new" {
                self.move_pane_to_new_tab();
            } else if let Ok(index) = target.parse() {
                self.move_pane_to_tab(index);
            }
        } else if let Some(key) = value.strip_prefix("resize:").and_then(|v| v.chars().next()) {
            self.on_resize_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
            self.activate_pointer_action("binding:resize_mode");
        } else if let Some(index) = value.strip_prefix("custom:").and_then(|v| v.parse().ok()) {
            self.run_custom(index);
        } else {
            match value {
                "back" => self.open_actions_menu(),
                "preset" => self.open_action_items(
                    t("Keybindings"),
                    vec![
                        ("Default (Ctrl+B)".into(), "preset:default".into()),
                        ("Zellij".into(), "preset:zellij".into()),
                    ],
                ),
                "preset:default" | "preset:zellij" => {
                    self.config.zellij_keys = value == "preset:zellij";
                    self.keyboard_mode = KeyboardMode::Normal;
                    let preset = if self.config.zellij_keys {
                        "zellij"
                    } else {
                        "default"
                    };
                    self.note = match crate::config::write_setting(
                        &self.config.config_path,
                        "keys.preset",
                        &format!("\"{preset}\""),
                    ) {
                        Ok(()) => format!("Keybindings: {preset}"),
                        Err(error) => t1("applied for this session; {}", error),
                    };
                }
                _ => {}
            }
        }
        self.dirty = true;
    }
}

impl App {
    pub(super) fn on_pointer_mouse(&mut self, mouse: MouseEvent) -> bool {
        let modal = self.prompt.is_some()
            || self.chooser.is_some()
            || self.goto.is_some()
            || self.settings.is_some()
            || self.help.is_some()
            || self.reader.is_some();
        if modal {
            self.pointer_drag = None;
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                let target = self
                    .pointer_targets
                    .iter()
                    .rev()
                    .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                    .map(|(target, _)| *target);
                if let Some(target) = target {
                    self.pointer_targets.clear();
                    self.activate_pointer_target(target);
                }
            } else if matches!(
                mouse.kind,
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            ) {
                let key = if mouse.kind == MouseEventKind::ScrollUp {
                    KeyCode::Up
                } else {
                    KeyCode::Down
                };
                if self.prompt.is_none() {
                    self.on_key(KeyEvent::new(key, KeyModifiers::NONE));
                }
            }
            return true;
        }
        if let Some(copy) = self.copy.as_ref() {
            let pane_id = copy.pane;
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                let target = self
                    .pointer_targets
                    .iter()
                    .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                    .map(|(target, _)| *target);
                if let Some(target) = target {
                    self.activate_pointer_target(target);
                    return true;
                }
            }
            if matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
            ) {
                if let Some(slot) = self.slots.iter().find(|slot| {
                    slot.pane == pane_id && contains(slot.inner, mouse.column, mouse.row)
                }) {
                    let position = (mouse.column - slot.inner.x, mouse.row - slot.inner.y);
                    let copy = self.copy.as_mut().expect("copy mode exists");
                    copy.cursor = position;
                    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                        copy.anchor = Some(position);
                    }
                    self.dirty = true;
                }
                return true;
            }
        }
        if self.chat.is_some() || self.popup.is_some() {
            self.pointer_drag = None;
            return false;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.pointer_drag = None;
                if let Some((pane, _)) = self
                    .pane_menu_hits
                    .iter()
                    .rev()
                    .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                    .copied()
                {
                    self.set_focus(pane);
                    self.activate_pointer_action("group:panes");
                    return true;
                }
                if let Some((pane, _)) = self
                    .pane_drag_hits
                    .iter()
                    .rev()
                    .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                    .copied()
                {
                    if let (Some(workspace), Some(tab)) = (self.workspace(), self.tab()) {
                        self.pointer_drag = Some(PointerDrag::Pane {
                            workspace: workspace.id,
                            tab: tab.id,
                            pane,
                        });
                    }
                    self.set_focus(pane);
                    self.note = t("Drag to another pane to swap, or to a tab to move").into();
                    self.dirty = true;
                    return true;
                }
                if contains(self.menu_hit, mouse.column, mouse.row) {
                    self.open_actions_menu();
                    return true;
                }
                if self.mobile
                    && mouse.row == self.content.y.saturating_sub(1)
                    && !contains(self.mobile_switch, mouse.column, mouse.row)
                    && !contains(self.mobile_close, mouse.column, mouse.row)
                {
                    self.pointer_drag = Some(PointerDrag::Header {
                        x: mouse.column,
                        y: mouse.row,
                    });
                    return true;
                }
                if self.mobile && self.switcher && mouse.row >= self.content.y {
                    self.pointer_drag = Some(PointerDrag::Switcher {
                        y: mouse.row,
                        moved: false,
                    });
                    return true;
                }
                if let Some((index, _)) = self
                    .tab_hits
                    .iter()
                    .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                {
                    if let Some(workspace) = self.workspace() {
                        if let Some(tab) = workspace.tabs.get(*index) {
                            self.pointer_drag = Some(PointerDrag::Tab {
                                workspace: workspace.id,
                                tab: tab.id,
                            });
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(PointerDrag::Switcher { y, moved }) = self.pointer_drag {
                    let delta = i32::from(y) - i32::from(mouse.row);
                    self.mobile_scroll = (self.mobile_scroll as i64 + i64::from(delta))
                        .clamp(0, self.mobile_max_scroll as i64)
                        as usize;
                    self.pointer_drag = Some(PointerDrag::Switcher {
                        y: mouse.row,
                        moved: moved || delta != 0,
                    });
                    self.dirty = true;
                    return true;
                }
                if self.pointer_drag.is_some() {
                    return true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(drag) = self.pointer_drag.take() {
                    match drag {
                        PointerDrag::Pane {
                            workspace,
                            tab,
                            pane,
                        } => {
                            let valid = self.workspace().is_some_and(|w| w.id == workspace)
                                && self.tab().is_some_and(|t| {
                                    t.id == tab && t.panes().contains(&PaneId(pane))
                                });
                            if valid {
                                self.set_focus(pane);
                                if let Some((index, _)) = self
                                    .tab_hits
                                    .iter()
                                    .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                                    .copied()
                                {
                                    self.move_pane_to_tab(index);
                                } else if let Some(slot) = self
                                    .slots
                                    .iter()
                                    .rev()
                                    .find(|s| {
                                        !s.overlay && contains(s.outer, mouse.column, mouse.row)
                                    })
                                    .copied()
                                {
                                    if slot.pane != pane {
                                        if let Err(error) = self.swap_panes(pane, slot.pane) {
                                            self.note = error.to_string();
                                        }
                                    }
                                }
                            }
                        }
                        PointerDrag::Tab { workspace, tab } => {
                            let target = self
                                .tab_hits
                                .iter()
                                .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                                .map(|(index, _)| *index);
                            if let Some(target) = target {
                                if let Some(current) =
                                    self.workspace_mut().filter(|w| w.id == workspace)
                                {
                                    if let Some(from) =
                                        current.tabs.iter().position(|t| t.id == tab)
                                    {
                                        if target < current.tabs.len() && from != target {
                                            let active = current.tab().map(|t| t.id);
                                            let moved = current.tabs.remove(from);
                                            current.tabs.insert(target, moved);
                                            current.active = active
                                                .and_then(|id| {
                                                    current.tabs.iter().position(|t| t.id == id)
                                                })
                                                .unwrap_or(0);
                                            self.mark_layout();
                                        }
                                    }
                                }
                            }
                        }
                        PointerDrag::Header { x, y } => {
                            let dx = i32::from(mouse.column) - i32::from(x);
                            if dx.abs() >= 6 && mouse.row.abs_diff(y) <= 1 {
                                if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                                    self.cycle_pane(if dx < 0 { 1 } else { -1 });
                                } else {
                                    self.cycle_tab(if dx < 0 { 1 } else { -1 });
                                }
                            }
                        }
                        PointerDrag::Switcher { moved, .. } => {
                            if moved {
                                return true;
                            }
                            // A tap activates on release; dragging cannot accidentally select a row.
                            let target = self
                                .mobile_targets
                                .iter()
                                .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                                .map(|(target, _)| *target);
                            if let Some(target) = target {
                                self.run_mobile_target(target);
                            }
                        }
                    }
                    self.dirty = true;
                    return true;
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some((index, _)) = self
                    .tab_hits
                    .iter()
                    .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                    .copied()
                {
                    self.select_tab(index);
                    self.activate_pointer_action("group:tabs");
                    return true;
                }
                if let Some((index, _)) = self
                    .workspace_hits
                    .iter()
                    .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                    .copied()
                {
                    self.select_workspace(index);
                    self.activate_pointer_action("group:tabs");
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn activate_pointer_target(&mut self, target: PointerTarget) {
        match target {
            PointerTarget::Close => self.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            PointerTarget::Confirm => {
                if let Some(prompt) = self.prompt.as_mut().filter(|p| p.kind.needs_confirm()) {
                    prompt.buffer = "y".into();
                    prompt.cursor = 1;
                }
                self.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            }
            PointerTarget::Chooser(index) => {
                if let Some(chooser) = &mut self.chooser {
                    chooser.selected = index;
                }
                if !self
                    .chooser
                    .as_ref()
                    .is_some_and(|c| c.kind == ChooserKind::Machines)
                {
                    self.activate_chooser();
                }
            }
            PointerTarget::Goto(index) => {
                if let Some(goto) = &mut self.goto {
                    goto.selected = index;
                }
                self.on_goto_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            }
            PointerTarget::SettingsSection(section) => {
                let selected = self.settings_selected(section);
                if let Some(settings) = &mut self.settings {
                    settings.section = section;
                    settings.selected = selected;
                }
            }
            PointerTarget::Setting(selected) => {
                if let Some(settings) = &mut self.settings {
                    settings.selected = selected;
                }
                self.on_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            }
            PointerTarget::Key(key) => self.on_key(KeyEvent::new(key, KeyModifiers::NONE)),
        }
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_core::{backend::TestBackend, terminal::Terminal};

    fn app() -> App {
        let (_, events) = std::sync::mpsc::channel();
        App::with_network(
            Config {
                restore: false,
                prompt_new_tab: false,
                config_path: std::env::temp_dir()
                    .join(crate::shared::identity("hctui-pointer-test-"))
                    .join("config.toml"),
                state_path: "/nonexistent/hctui-pointer-state".into(),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap()
    }

    fn draw(app: &mut App, width: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
        terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    }

    fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn click(app: &mut App, target: impl Fn(PointerTarget) -> bool) {
        let rect = app
            .pointer_targets
            .iter()
            .find(|(t, _)| target(*t))
            .expect("rendered click target")
            .1;
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            rect.x,
            rect.y,
        ));
    }

    #[test]
    fn click_menu_rename_save_and_close_confirmation_without_shortcuts() {
        let mut app = app();
        app.config.keys.rename_tab = crate::config::Binding::Off;
        draw(&mut app, 110);
        let menu = app.menu_hit;
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x,
            menu.y,
        ));
        assert!(app.chooser.is_some());
        draw(&mut app, 110);
        click(&mut app, |t| matches!(t, PointerTarget::Chooser(1)));
        let index = app
            .chooser
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|i| i.value == "binding:rename_tab")
            .unwrap();
        draw(&mut app, 110);
        click(
            &mut app,
            |t| matches!(t, PointerTarget::Chooser(i) if i == index),
        );
        assert!(matches!(
            app.prompt.as_ref().unwrap().kind,
            PromptKind::RenameTab { .. }
        ));
        app.on_paste("renamed".into());
        draw(&mut app, 110);
        click(&mut app, |t| matches!(t, PointerTarget::Confirm));
        assert!(app.tab().unwrap().label.contains("renamed"));
        app.activate_pointer_action("binding:close_tab");
        draw(&mut app, 110);
        click(&mut app, |t| matches!(t, PointerTarget::Close));
        assert_eq!(app.workspace().unwrap().tabs.len(), 1);
        app.activate_pointer_action("binding:close_tab");
        draw(&mut app, 110);
        click(&mut app, |t| matches!(t, PointerTarget::Confirm));
        assert!(app.workspaces.is_empty());
    }

    #[test]
    fn settings_and_goto_are_clickable_and_modals_capture_background_clicks() {
        let mut app = app();
        app.new_tab(Some("second".into()));
        app.run_named_binding("settings");
        draw(&mut app, 110);
        let active = app.workspace().unwrap().active;
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        assert_eq!(app.workspace().unwrap().active, active);
        click(&mut app, |t| matches!(t, PointerTarget::SettingsSection(1)));
        draw(&mut app, 110);
        click(&mut app, |t| matches!(t, PointerTarget::Setting(1)));
        assert_eq!(app.config.indicators, crate::config::Indicators::Symbols);
        draw(&mut app, 110);
        click(&mut app, |t| matches!(t, PointerTarget::Close));
        app.run_named_binding("goto");
        draw(&mut app, 110);
        click(&mut app, |t| matches!(t, PointerTarget::Goto(1)));
        assert!(app.goto.is_none());
        assert_eq!(app.workspace().unwrap().active, 0);
        let _ = std::fs::remove_dir_all(app.config.config_path.parent().unwrap());
    }

    #[test]
    fn tab_drag_preserves_identity_and_marks_layout_for_sync() {
        let mut app = app();
        app.new_tab(Some("two".into()));
        let id = app.tab().unwrap().id;
        draw(&mut app, 110);
        let from = app.tab_hits[1].1;
        let to = app.tab_hits[0].1;
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            from.x,
            from.y,
        ));
        app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), to.x, to.y));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), to.x, to.y));
        assert_eq!(app.workspace().unwrap().tabs[0].id, id);
        assert_eq!(app.tab().unwrap().id, id);
        assert!(app.layout_dirty);
    }

    #[test]
    fn mobile_header_swipe_switches_tabs_and_list_drag_does_not_activate_a_row() {
        let mut app = app();
        app.new_tab(Some("two".into()));
        draw(&mut app, 40);
        assert!(app.mobile);
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 20, 0));
        app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 0));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 0));
        assert_eq!(app.workspace().unwrap().active, 0);
        app.open_switcher();
        draw(&mut app, 40);
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 10, 20));
        app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 10, 10));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 10, 10));
        assert!(app.switcher);
    }

    #[test]
    fn zellij_preset_modes_lock_and_escape_do_not_run_terminal_input_as_commands() {
        let mut app = app();
        app.config.zellij_keys = true;
        let key = |c, modifiers| KeyEvent::new(KeyCode::Char(c), modifiers);
        app.on_key(key('p', KeyModifiers::CONTROL));
        assert_eq!(app.keyboard_mode, KeyboardMode::Pane);
        app.on_key(key('r', KeyModifiers::NONE));
        assert_eq!(app.keyboard_mode, KeyboardMode::Normal);
        assert_eq!(app.tab().unwrap().panes().len(), 2);
        app.on_key(key('g', KeyModifiers::CONTROL));
        app.on_key(key('p', KeyModifiers::CONTROL));
        app.on_key(key('r', KeyModifiers::NONE));
        assert_eq!(app.keyboard_mode, KeyboardMode::Locked);
        assert_eq!(app.tab().unwrap().panes().len(), 2);
        app.on_key(key('g', KeyModifiers::CONTROL));
        app.on_key(key('t', KeyModifiers::CONTROL));
        app.on_key(key('n', KeyModifiers::NONE));
        assert_eq!(app.workspace().unwrap().tabs.len(), 2);
        app.on_key(key('n', KeyModifiers::CONTROL));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.keyboard_mode, KeyboardMode::Normal);
        app.config.zellij_keys = false;
        app.on_key(key('p', KeyModifiers::CONTROL));
        assert_eq!(app.keyboard_mode, KeyboardMode::Normal);
    }
    #[test]
    fn pane_grip_swaps_views_and_moves_to_another_tab_without_replacing_terminals() {
        let mut app = app();
        app.split_focused(Axis::Horizontal);
        let source = app.focused();
        draw(&mut app, 110);
        let target = app.slots.iter().find(|s| s.pane != source).unwrap().pane;
        let grip = app
            .pane_drag_hits
            .iter()
            .find(|(id, _)| *id == source)
            .unwrap()
            .1;
        let drop = app.slots.iter().find(|s| s.pane == target).unwrap().inner;
        let before = app.tab().unwrap().tree.panes();
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            grip.x,
            grip.y,
        ));
        app.on_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            drop.x,
            drop.y,
        ));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), drop.x, drop.y));
        assert_ne!(app.tab().unwrap().tree.panes(), before);
        assert_eq!(app.focused(), source);
        app.new_tab(Some("target".into()));
        let destination = app.tab().unwrap().id;
        app.select_tab(0);
        app.set_focus(source);
        draw(&mut app, 110);
        let grip = app
            .pane_drag_hits
            .iter()
            .find(|(id, _)| *id == source)
            .unwrap()
            .1;
        let drop = app.tab_hits[1].1;
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            grip.x,
            grip.y,
        ));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), drop.x, drop.y));
        assert_eq!(app.tab().unwrap().id, destination);
        assert!(app.tab().unwrap().panes().contains(&PaneId(source)));
        assert!(app.panes.contains_key(&target));
    }

    #[test]
    fn copy_action_bar_remains_visible_and_selection_can_be_made_with_a_pointer() {
        let mut app = app();
        draw(&mut app, 110);
        app.enter_copy_mode();
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let last: String = (0..110).map(|x| buffer[(x, 29)].symbol()).collect();
        assert!(last.contains("COPY"), "{last}");
        let slot = app.slots[0];
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            slot.inner.x + 1,
            slot.inner.y + 1,
        ));
        app.on_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            slot.inner.x + 4,
            slot.inner.y + 2,
        ));
        let copy = app.copy.as_ref().unwrap();
        assert_eq!(copy.anchor, Some((1, 1)));
        assert_eq!(copy.cursor, (4, 2));
        click(&mut app, |t| {
            matches!(t, PointerTarget::Key(KeyCode::Char('/')))
        });
        app.on_paste("needle".into());
        assert_eq!(app.copy.as_ref().unwrap().search.as_deref(), Some("needle"));
        draw(&mut app, 110);
        click(&mut app, |t| {
            matches!(t, PointerTarget::Key(KeyCode::Enter))
        });
        assert!(!app.copy.as_ref().unwrap().typing);
        draw(&mut app, 110);
        click(&mut app, |t| matches!(t, PointerTarget::Key(KeyCode::Esc)));
        assert!(app.copy.is_none());
    }
    #[test]
    fn all_settings_sections_are_clickable_on_narrow_screens() {
        let mut app = app();
        app.run_named_binding("settings");
        draw(&mut app, 40);
        for section in 0..App::SETTINGS_SECTIONS.len() {
            click(
                &mut app,
                |t| matches!(t, PointerTarget::SettingsSection(i) if i == section),
            );
            assert_eq!(app.settings.as_ref().unwrap().section, section);
            draw(&mut app, 40);
        }
    }
    #[test]
    fn external_agent_context_menu_rename_can_be_saved_with_a_click() {
        let mut app = app();
        app.terminals.push(
            TerminalSummary::parse(
                &json!({"address":"peer/t9", "status":"idle", "labels":{"hctui":"before"}}),
            )
            .unwrap(),
        );
        draw(&mut app, 110);
        let hit = app
            .terminal_hits
            .iter()
            .find(|(address, _)| address == "peer/t9")
            .unwrap()
            .1;
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            hit.x + 1,
            hit.y,
        ));
        draw(&mut app, 110);
        click(&mut app, |t| matches!(t, PointerTarget::Chooser(0)));
        assert_eq!(app.prompt.as_ref().unwrap().buffer, "before");
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        app.on_paste("renamed agent".into());
        draw(&mut app, 110);
        click(&mut app, |t| matches!(t, PointerTarget::Confirm));
        assert_eq!(app.terminals[0].name.as_deref(), Some("renamed agent"));
        assert_eq!(app.inflight["label"].1["terminal"], "peer/t9");
        assert!(app.pane_by_terminal("peer/t9").is_none());
    }
}
