//! Shared named actions for key bindings and pointer menus.
use super::*;

impl App {
    pub(super) fn binding_title(name: &str) -> &'static str {
        t(match name {
            "prefix" => "prefix mode",
            "help" => "keybinds",
            "settings" => "settings",
            "detach" => "detach",
            "reload_config" => "reload config",
            "open_notification_target" => "open notification target",
            "goto" => "search agents and terminals",
            "new_tab" => "new tab",
            "rename_tab" => "rename tab",
            "next_tab" => "next tab",
            "previous_tab" => "previous tab",
            "move_tab_previous" => "move tab toward front",
            "move_tab_next" => "move tab toward back",
            "close_tab" => "close tab",
            "new_workspace" => "new workspace",
            "rename_workspace" => "rename workspace",
            "close_workspace" => "close workspace",
            "workspace_picker" => "workspace picker",
            "next_workspace" => "next workspace",
            "previous_workspace" => "previous workspace",
            "split_vertical" => "split vertical",
            "split_horizontal" => "split horizontal",
            "focus_pane_left" => "focus pane left",
            "focus_pane_down" => "focus pane down",
            "focus_pane_up" => "focus pane up",
            "focus_pane_right" => "focus pane right",
            "cycle_pane_next" => "cycle pane next",
            "cycle_pane_previous" => "cycle pane previous",
            "last_pane" => "last pane",
            "zoom" => "zoom",
            "stack_panes" => "stack panes",
            "layout_next" => "next layout preset",
            "layout_previous" => "previous layout preset",
            "float_pane" => "float pane",
            "float_pin" => "pin floating pane",
            "float_hide" => "hide floating panes",
            "close_pane" => "close pane",
            "rename_pane" => "rename pane",
            "kill_pane" => "kill terminal",
            "move_pane" => "move pane",
            "resize_mode" => "resize mode",
            "toggle_sidebar" => "toggle sidebar",
            "scroll_bottom" => "scroll to bottom",
            "copy_mode" => "copy mode",
            "next_agent" => "next agent in displayed order",
            "chat" => "chat view of the agent session",
            "send_text" => "send text to pane",
            "interrupt" => "interrupt turn",
            "toggle_keep" => "toggle keep",
            "remove_terminal" => "remove finished terminal",
            "open_link" => "open link",
            "edit_history" => "edit scrollback",
            "read_pane" => "read pane",
            "send_keys" => "send named keys",
            "wait_output" => "wait for output",
            "put_file" => "send file",
            "templates" => "templates",
            "machines" => "machines",
            "paste_image" => "paste image",
            _ => "",
        })
    }
    pub(super) fn run_named_binding(&mut self, name: &str) {
        match name {
            "help" => {
                self.help = Some(HelpOverlay {
                    query: String::new(),
                    selected: 0,
                    scroll: 0,
                });
                self.dirty = true;
            }
            "settings" => {
                self.open_settings();
                self.dirty = true;
            }
            "goto" => {
                self.palette = false;
                if !self.open_goto_picker(false) {
                    self.goto = Some(GotoOverlay {
                        query: String::new(),
                        selected: 1,
                    });
                }
                self.dirty = true;
            }
            "open_notification_target" => {
                self.open_notification_target();
            }
            "detach" => {
                self.quit = true;
            }
            "reload_config" => {
                self.reload_config();
            }
            "new_tab" => {
                if self.config.prompt_new_tab {
                    let suggested = format!("{}", self.tabs_in_workspace() + 1);
                    self.ask_with(PromptKind::NewTab, t("tab name"), suggested);
                } else {
                    self.new_tab(None);
                }
            }
            "rename_tab" => {
                if let Some(tab) = self.tab() {
                    let (workspace, id, label) = (
                        self.workspace().expect("tab has a workspace").id,
                        tab.id,
                        tab.label.clone(),
                    );
                    self.ask_with(
                        PromptKind::RenameTab { workspace, tab: id },
                        t("rename tab"),
                        label,
                    );
                }
            }
            "next_tab" => {
                self.cycle_tab(1);
            }
            "previous_tab" => {
                self.cycle_tab(-1);
            }
            "move_tab_previous" => {
                self.move_tab(-1);
            }
            "move_tab_next" => {
                self.move_tab(1);
            }
            "close_tab" => {
                if self.config.confirm_close {
                    self.ask(
                        PromptKind::CloseTab(self.active_ws),
                        t("close this tab and every pane in it? (y/N)"),
                    );
                } else {
                    let workspace = self.active_ws;
                    self.close_tab_in(workspace);
                }
            }
            "new_workspace" => {
                if self.config.prompt_new_workspace {
                    self.ask(PromptKind::NewWorkspace, t("workspace name"));
                } else {
                    self.new_workspace(None);
                }
            }
            "rename_workspace" => {
                let (index, label) = self
                    .workspace()
                    .map(|workspace| (self.active_ws, workspace.label.clone()))
                    .unwrap_or((self.active_ws, String::new()));
                self.ask_with(
                    PromptKind::RenameWorkspace(index),
                    t("rename workspace"),
                    label,
                );
            }
            "close_workspace" => {
                self.ask(
                    PromptKind::CloseWorkspace(self.active_ws),
                    t("close workspace and all its tabs? (y/N)"),
                );
            }
            "workspace_picker" => {
                self.workspace_mode = true;
                self.note = String::new();
                self.dirty = true;
            }
            "next_workspace" => {
                self.select_workspace((self.active_ws + 1) % self.workspaces.len().max(1));
            }
            "previous_workspace" => {
                let index = self
                    .active_ws
                    .checked_sub(1)
                    .unwrap_or(self.workspaces.len().saturating_sub(1));
                self.select_workspace(index);
            }
            "split_vertical" => {
                self.split_focused(Axis::Horizontal);
            }
            "split_horizontal" => {
                self.split_focused(Axis::Vertical);
            }
            "focus_pane_left" => {
                self.move_focus(Direction::Left);
            }
            "focus_pane_down" => {
                self.move_focus(Direction::Down);
            }
            "focus_pane_up" => {
                self.move_focus(Direction::Up);
            }
            "focus_pane_right" => {
                self.move_focus(Direction::Right);
            }
            "cycle_pane_next" => {
                self.cycle_pane(1);
            }
            "cycle_pane_previous" => {
                self.cycle_pane(-1);
            }
            "last_pane" => {
                self.toggle_last_pane();
            }
            "zoom" => {
                self.zoom = !self.zoom;
                self.note = String::new();
                self.dirty = true;
            }
            "layout_next" => {
                self.cycle_layout(1);
            }
            "layout_previous" => {
                self.cycle_layout(-1);
            }
            "stack_panes" => {
                self.stack_tab_panes();
            }
            "float_pane" => {
                self.toggle_float();
            }
            "float_pin" => {
                self.toggle_float_pin();
            }
            "float_hide" => {
                self.toggle_floating_hidden();
            }
            "close_pane" => {
                self.close_focused_pane();
            }
            "rename_pane" => {
                if let Err(error) = self.execute_action("pane.rename", json!({})) {
                    self.note = error.to_string();
                }
            }
            "kill_pane" => {
                let terminal = self
                    .focused_pane()
                    .and_then(|pane| pane.terminal.clone())
                    .unwrap_or_default();
                self.ask(
                    PromptKind::KillPane(self.focused()),
                    &format!("结束 {terminal} 里的程序？(y/N)"),
                );
            }
            "move_pane" => {
                self.move_mode = true;
                self.note = t1(
                    "move: 1-{} 移到目标 tab，t 移成新 tab，Esc 取消",
                    self.tabs_in_workspace(),
                );
                self.dirty = true;
            }
            "resize_mode" => {
                self.resize_mode = true;
                self.note = String::new();
                self.dirty = true;
            }
            "toggle_sidebar" => {
                self.sidebar = !self.sidebar;
                self.dirty = true;
            }
            "scroll_bottom" => {
                if let Some(pane) = self.panes.get_mut(&self.focused()) {
                    pane.view.scroll_bottom();
                }
                self.note = t("bottom").to_string();
                self.dirty = true;
            }
            "copy_mode" => {
                self.enter_copy_mode();
            }
            "next_agent" => {
                self.cycle_agent(1);
            }
            "chat" => {
                self.toggle_chat();
            }
            "send_text" => {
                let pane = self.input_pane();
                let terminal = self
                    .panes
                    .get(&pane)
                    .and_then(|state| state.terminal.clone())
                    .unwrap_or_else(|| t1("pane {}", pane));
                self.ask(PromptKind::SendText(pane), &format!("发给 {terminal}"));
            }
            "interrupt" => {
                let terminal = self
                    .panes
                    .get(&self.input_pane())
                    .and_then(|state| state.terminal.clone());
                if let Some(terminal) = terminal {
                    self.call_rpc(
                        "interrupt",
                        "terminal.interrupt",
                        json!({"terminal": terminal}),
                    );
                    self.note = t1("interrupting {}", terminal);
                }
            }
            "toggle_keep" => {
                self.toggle_keep();
            }
            "remove_terminal" => {
                self.remove_finished_terminal();
            }
            "open_link" => {
                self.open_link_at_cursor();
            }
            "edit_history" => {
                self.edit_history_popup();
            }
            "read_pane" => {
                self.open_reader();
            }
            "send_keys" => {
                let pane = self.input_pane();
                self.ask(PromptKind::SendKeys(pane), "按下按键：enter, ctrl+c, esc");
            }
            "wait_output" => {
                let pane = self.input_pane();
                self.ask(PromptKind::WaitOutput(pane), "等待输出包含");
            }
            "put_file" => {
                let pane = self.input_pane();
                self.ask(PromptKind::PutFile(pane), "发送文件路径");
            }
            "templates" => {
                self.call_rpc("templates", "templates.list", json!({}));
            }
            "machines" => {
                self.call_rpc("machines", "machine.list", json!({}));
            }
            "paste_image" => {
                self.paste_image();
            }
            _ => {}
        }
    }
}
