//! Shared command dispatch for the palette, key commands and integrations.
use super::*;
use crate::i18n::{t, t1};
use crate::mods::chrome::{ChromeSize, DockPlace};

pub const COMMANDS: &[(&str, &str)] = &[
    ("space.new", "Create a space; name, folder, machine, argv"),
    ("space.close", "Close a space and end its terminals"),
    ("space.rename", "Rename a space"),
    ("space.move", "Move space to index"),
    ("space.select", "Select by id or index"),
    ("space.next", "Next space"),
    ("space.prev", "Previous space"),
    ("tab.new", "New tab; name, folder, argv"),
    ("tab.close", "Close tab and end its terminals"),
    ("tab.rename", "Rename tab"),
    ("tab.move", "Move tab to index"),
    ("tab.select", "Select by id or index"),
    ("tab.next", "Next tab"),
    ("tab.prev", "Previous tab"),
    ("tab.settle", "Put tab away, keep running"),
    ("tab.unsettle", "Bring tab back"),
    ("pane.split", "Split right or down"),
    ("pane.rename", "Rename a pane"),
    ("pane.close", "Close pane and end its terminal"),
    ("pane.zoom", "Toggle zoom"),
    ("pane.focus", "Focus pane or terminal"),
    ("pane.go", "Focus a directional neighbor"),
    (
        "pane.swap",
        "Swap panes in the same tab; pane and with, or direction",
    ),
    (
        "pane.move",
        "Move a pane to another tab; pane, to, before, after",
    ),
    ("terminal.open", "Run argv in tab, right or down"),
    ("terminal.keep", "Keep terminal; create view if needed"),
    (
        "terminal.park",
        "Remove view, disable keep, keep program running",
    ),
    ("terminal.pin", "Pin or unpin agent"),
    ("terminal.kill", "End terminal and its descendants"),
    ("terminal.peek", "Read terminal without changing layout"),
    ("terminal.show", "Temporarily show another terminal"),
    ("terminal.copy", "Copy current visible text"),
    ("terminal.paste", "Paste text"),
    ("terminal.paste_image", "Paste clipboard image"),
    ("terminal.scroll", "Scroll history; lines, pages, to"),
    ("terminal.search", "Search terminal history"),
    ("terminal.right_click", "Right click to program or menu"),
    ("terminal.send", "Send text to a terminal; terminal, text"),
    (
        "terminal.keys",
        "Send named keys to a terminal; terminal, keys",
    ),
    (
        "terminal.wait",
        "Wait for output in a terminal; terminal, output",
    ),
    (
        "terminal.put",
        "Upload a local file to a terminal; terminal, path",
    ),
    ("agent.rename", "Rename an agent terminal"),
    ("agent.focus", "Focus agent, including sub-agents"),
    ("agent.next", "Next agent in displayed order"),
    ("agent.prev", "Previous agent in displayed order"),
    ("machine.list", "List machines"),
    ("machine.connect", "Connect a machine"),
    ("machine.add", "Add a machine"),
    ("machine.remove", "Remove a machine"),
    ("machine.rename", "Rename a machine; machine, name"),
    ("mod.open", "Open a built-in or configured script panel"),
    ("files.open", "Browse files on this space's machine"),
    (
        "changes.open",
        "Git status and diff on this space's machine",
    ),
    ("file.open", "Read a file; path"),
    (
        "layout.clone",
        "Save current pane layout as a personal profile",
    ),
    ("layout.set", "Apply a pane layout tree"),
    ("layout.select", "Select a child of a pane stack"),
    ("layout.reset", "Reset to balanced pane layout"),
    ("mod.clone", "Clone a script panel"),
    (
        "mods.dock",
        "Dock a mod region; name, place, size, index, props",
    ),
    ("mods.undock", "Undock a mod region; name"),
    ("mods.chrome", "Replace the chrome tree; tree"),
    (
        "mods.open",
        "Open a mod surface; name, props, place, size, at, key",
    ),
    ("mods.close", "Close a mod surface; name or surface"),
    ("mods.reload", "Reload a mod surface; name"),
    (
        "mods.grant",
        "Grant or revoke host methods; name, host, events, revoke",
    ),
    ("mods.enable", "Enable a mod; name"),
    ("mods.disable", "Disable a mod; name"),
    ("layout.reload", "Reload shared layout after a conflict"),
    ("layout.switch", "Cycle pane layout preset"),
    ("layout.show", "Show sidebar"),
    ("layout.hide", "Hide sidebar"),
    ("layout.toggle", "Toggle sidebar"),
    ("layout.resize", "Resize sidebar; px"),
    ("app.palette", "Search commands"),
    ("app.settings", "Settings"),
    ("app.help", "Key bindings"),
    ("app.theme", "Choose theme; name"),
    ("app.indicators", "Status indicators; value dots or symbols"),
    (
        "app.notifications",
        "System notification delivery; value off, toast or system",
    ),
    ("app.sound", "Notification sound; value on or off"),
    ("app.quit", "Detach client, keep programs running"),
    ("overlay.close", "Close top overlay"),
];

impl App {
    pub fn open_palette(&mut self) {
        self.palette = true;
        if self.open_goto_picker(true) {
            self.dirty = true;
            return;
        }
        self.goto = Some(GotoOverlay {
            query: String::new(),
            selected: 0,
        });
        self.dirty = true;
    }

    /// 打开设置：装了 `settings` 模块就开模块浮层，否则用内置浮层。
    pub fn open_settings(&mut self) {
        if self.settings_module_available() {
            let data = json!({
                "width": self.content.width,
                "height": self.content.height,
                "onEsc": "module",
            });
            match self.open_mod_overlay("settings", crate::mods::SurfacePlace::Dialog, data) {
                Ok(_) => {
                    self.dirty = true;
                    return;
                }
                Err(error) => self.note = error,
            }
        }
        self.settings = Some(SettingsOverlay {
            section: 0,
            selected: self.settings_selected(0),
        });
        self.dirty = true;
    }

    /// `--no-mods`、停用或未安装时不走模块。
    fn settings_module_available(&self) -> bool {
        !self.config.no_mods
            && self.mods.is_enabled("settings")
            && matches!(
                crate::pkg::resolve(&self.config, "settings"),
                Ok(Some(package)) if package.enabled && package.module.is_some()
            )
    }

    /// 写配置并保持调用方的控制流：调用方决定内存是否跟着变。
    fn write_setting(&self, key: &str, value: &str) -> Result<()> {
        crate::config::write_setting(&self.config.config_path, key, value)
            .map_err(|error| anyhow::anyhow!(error))
    }

    pub fn execute_action(&mut self, name: &str, args: Value) -> Result<()> {
        anyhow::ensure!(args.is_object(), t("command arguments must be an object"));
        // picker 内部动作不出现在命令目录里，先于插件与模块命令解析。
        if let Some(rest) = name.strip_prefix("picker.") {
            return self.execute_picker_action(rest, args);
        }
        // 插件动作与内置动作共用分发器；内置名不可被覆盖。
        if !COMMANDS.iter().any(|(builtin, _)| *builtin == name) {
            if self.plugins.owns(name) {
                self.invoke_plugin(name, args)?;
                return Ok(());
            }
            // 键位与菜单走同一道闸门：停用后的包里动作报 disabled。
            if let Some(package) = crate::plugin::registry::disabled_package(&self.config, name) {
                anyhow::bail!(t1("plugin {} is disabled", &package));
            }
            // 模块命令：`<mod>.<command>` 经 `command` 事件投递给该模块的表面。
            if let Some((mod_id, command)) = name.split_once('.') {
                if self
                    .mods
                    .surfaces
                    .iter()
                    .any(|surface| surface.mod_id == mod_id)
                {
                    anyhow::ensure!(
                        self.mods.granted(mod_id, "command"),
                        t1("method {} not granted", "command")
                    );
                    self.send_mod_event(mod_id, "command", &json!({"name": command, "args": args}));
                    return Ok(());
                }
            }
        }
        let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_string);
        let direction = || match text("direction").as_deref().unwrap_or("right") {
            "left" => Direction::Left,
            "up" => Direction::Up,
            "down" => Direction::Down,
            _ => Direction::Right,
        };
        let argv = || {
            args.get("argv")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        // Explicit targets are resolved before mutating the model.
        if name.starts_with("space.") && name != "space.new" {
            if let Some(id) = text("id") {
                let index = self
                    .workspaces
                    .iter()
                    .position(|w| w.origin.as_deref() == Some(&id) || w.id.to_string() == id)
                    .ok_or_else(|| anyhow::anyhow!(t1("unknown space {}", id)))?;
                self.select_workspace(index);
            }
        }
        if name.starts_with("tab.") && name != "tab.new" {
            if let Some(id) = text("id") {
                let target = self
                    .workspaces
                    .iter()
                    .enumerate()
                    .find_map(|(wi, w)| {
                        w.tabs
                            .iter()
                            .position(|t| {
                                t.origin.as_deref() == Some(&id) || t.id.to_string() == id
                            })
                            .map(|ti| (wi, ti))
                    })
                    .ok_or_else(|| anyhow::anyhow!(t1("unknown tab {}", id)))?;
                self.select_workspace(target.0);
                self.select_tab(target.1);
            }
        }
        if name.starts_with("pane.") && !matches!(name, "pane.swap" | "pane.move") {
            if let Some(id) = text("pane") {
                let target = self
                    .panes
                    .iter()
                    .find(|(n, p)| p.origin.as_deref() == Some(&id) || n.to_string() == id)
                    .map(|(id, _)| *id)
                    .ok_or_else(|| anyhow::anyhow!(t1("unknown pane {}", id)))?;
                self.focus_pane(target);
            }
        }
        let target_pane = if let Some(address) = text("terminal") {
            self.pane_by_terminal(&address)
        } else {
            Some(self.input_pane())
        };
        let terminal =
            || text("terminal").or_else(|| self.focused_pane().and_then(|p| p.terminal.clone()));
        match name {
            "space.new" => {
                self.new_workspace(text("name"));
                let mut stale_origins = Vec::new();
                {
                    let ws = self.workspaces.get_mut(self.active_ws).unwrap();
                    if args.get("machine").is_some() {
                        let next = text("machine").filter(|m| !m.is_empty());
                        if next != ws.machine {
                            ws.folder = None;
                            // 刚建的 Space 还没发布；换机器后不能沿用上一台机器的身份。
                            ws.origin = None;
                            for tab in &mut ws.tabs {
                                tab.origin = None;
                                stale_origins.extend(tab.panes().into_iter().map(|pane| pane.0));
                            }
                        }
                        ws.machine = next;
                    }
                }
                for pane in stale_origins {
                    if let Some(state) = self.panes.get_mut(&pane) {
                        state.origin = None;
                    }
                }
                let ws = self.workspaces.get_mut(self.active_ws).unwrap();
                if args.get("folder").is_some() {
                    ws.folder = text("folder");
                }
                let on = ws.machine.clone();
                let cwd = ws.folder.clone();
                let pane = self.focused();
                if let Some(p) = self.panes.get_mut(&pane) {
                    let mut launch = p.launch.take().unwrap_or_default();
                    launch.on = on;
                    launch.cwd = cwd;
                    if !argv().is_empty() {
                        launch.argv = argv();
                    }
                    p.launch = Some(launch);
                }
                self.mark_layout();
            }
            "space.close" => {
                if args["confirmed"] == true || !self.config.confirm_close {
                    self.close_workspace_in(self.active_ws);
                } else {
                    self.ask(
                        PromptKind::CloseWorkspace(self.active_ws),
                        t("End every terminal in this space? (y/N)"),
                    );
                }
            }
            "space.rename" => {
                if let Some(label) = text("name") {
                    if let Some(w) = self.workspaces.get_mut(self.active_ws) {
                        w.label = label;
                    }
                    self.mark_layout();
                } else {
                    self.ask(PromptKind::RenameWorkspace(self.active_ws), t("space name"));
                }
            }
            "space.select" => {
                if let Some(i) = args["index"].as_u64() {
                    anyhow::ensure!(
                        i > 0 && i as usize <= self.workspaces.len(),
                        t("space index out of range")
                    );
                    self.select_workspace(i as usize - 1);
                }
            }
            "space.next" | "space.prev" => {
                if !self.workspaces.is_empty() {
                    let step = if name.ends_with("next") { 1 } else { -1 };
                    self.select_workspace(
                        (self.active_ws as isize + step).rem_euclid(self.workspaces.len() as isize)
                            as usize,
                    );
                }
            }
            "space.move" => {
                let index = args["index"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!(t("index required")))?
                    as usize;
                anyhow::ensure!(
                    index > 0 && index <= self.workspaces.len(),
                    t("space index out of range")
                );
                let ws = self.workspaces.remove(self.active_ws);
                self.workspaces.insert(index - 1, ws);
                self.active_ws = index - 1;
                self.mark_layout();
            }
            "tab.new" | "terminal.open" => {
                if let Some(id) = text("space") {
                    let wi = self
                        .workspaces
                        .iter()
                        .position(|w| w.origin.as_deref() == Some(&id) || w.id.to_string() == id)
                        .ok_or_else(|| anyhow::anyhow!(t1("unknown space {}", id)))?;
                    self.select_workspace(wi);
                }
                let place = text("place").unwrap_or_else(|| "tab".into());
                if place == "right" || place == "down" {
                    self.split_focused(if place == "down" {
                        Axis::Vertical
                    } else {
                        Axis::Horizontal
                    });
                } else {
                    self.new_tab(text("name"));
                }
                if !argv().is_empty() || text("folder").is_some() {
                    let mut launch = self.new_pane_launch().unwrap_or_default();
                    if !argv().is_empty() {
                        launch.argv = argv();
                    }
                    if text("folder").is_some() {
                        launch.cwd = text("folder");
                    }
                    let id = self.focused();
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.launch = Some(launch);
                    }
                }
            }
            "tab.close" => {
                if args["confirmed"] == true || !self.config.confirm_close {
                    self.close_tab_in(self.active_ws);
                } else {
                    self.ask(
                        PromptKind::CloseTab(self.active_ws),
                        t("End every terminal in this tab? (y/N)"),
                    );
                }
            }
            "tab.rename" => {
                let ws = self.active_ws;
                let id = self.tab().ok_or_else(|| anyhow::anyhow!(t("no tab")))?.id;
                if let Some(label) = text("name") {
                    self.workspaces[ws].tab_mut().unwrap().label = label;
                    self.mark_layout();
                } else {
                    let initial = self.tab().unwrap().label.clone();
                    self.ask_with(
                        PromptKind::RenameTab {
                            workspace: self.workspaces[ws].id,
                            tab: id,
                        },
                        t("tab name"),
                        initial,
                    );
                }
            }
            "tab.select" => {
                if let Some(i) = args["index"].as_u64() {
                    anyhow::ensure!(
                        i > 0 && i as usize <= self.tabs_in_workspace(),
                        t("tab index out of range")
                    );
                    self.select_tab(i as usize - 1);
                }
            }
            "tab.next" => self.cycle_tab(1),
            "tab.prev" => self.cycle_tab(-1),
            "tab.move" => {
                let to = args["index"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!(t("index required")))?
                    as usize;
                let w = self
                    .workspaces
                    .get_mut(self.active_ws)
                    .ok_or_else(|| anyhow::anyhow!(t("no space")))?;
                anyhow::ensure!(to > 0 && to <= w.tabs.len(), t("tab index out of range"));
                let t = w.tabs.remove(w.active);
                w.tabs.insert(to - 1, t);
                w.active = to - 1;
                self.mark_layout();
            }
            "tab.settle" | "tab.unsettle" => {
                let settle = name == "tab.settle";
                let index = self.workspace().map(|w| w.active).unwrap_or(0);
                if let Some(t) = self
                    .workspaces
                    .get_mut(self.active_ws)
                    .and_then(|w| w.tab_mut())
                {
                    t.settled = settle;
                }
                if settle {
                    let next = self
                        .workspace()
                        .and_then(|w| w.tabs.iter().position(|t| !t.settled));
                    if let Some(next) = next {
                        self.select_tab(next);
                    }
                } else {
                    self.select_tab(index);
                }
                self.mark_layout();
            }
            "pane.split" => self.split_focused(if text("direction").as_deref() == Some("down") {
                Axis::Vertical
            } else {
                Axis::Horizontal
            }),
            "pane.rename" => {
                let pane =
                    target_pane.ok_or_else(|| anyhow::anyhow!(t("terminal has no open view")))?;
                if let Some(name) = text("name") {
                    self.rename_pane(pane, &name)?;
                } else {
                    let initial = self
                        .panes
                        .get(&pane)
                        .ok_or_else(|| anyhow::anyhow!(t("no pane")))?
                        .name
                        .clone()
                        .unwrap_or_default();
                    self.ask_with(
                        PromptKind::RenamePane(pane),
                        t("pane name (empty clears the alias)"),
                        initial,
                    );
                }
            }
            "agent.rename" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                let initial = self.terminal_alias(&address)?;
                if let Some(name) = text("name") {
                    self.rename_terminal(&address, &name)?;
                } else {
                    self.ask_with(
                        PromptKind::RenameAgent(address),
                        t("agent name (empty clears the alias)"),
                        initial,
                    );
                }
            }
            "pane.close" => self.close_focused_pane(),
            "pane.zoom" => {
                self.zoom = !self.zoom;
                self.dirty = true;
            }
            "pane.focus" | "agent.focus" => {
                if let Some(address) = text("terminal") {
                    self.open_terminal(&address);
                }
            }
            "pane.go" => self.move_focus(direction()),
            "pane.swap" => match (text("pane"), text("with")) {
                (Some(first), Some(second)) => {
                    let first = self.resolve_pane(&first)?;
                    let second = self.resolve_pane(&second)?;
                    self.swap_panes(first, second)?;
                }
                (None, None) => self.swap_direction(direction())?,
                _ => anyhow::bail!(t("pane.swap takes both pane and with")),
            },
            "pane.move" => {
                let pane = match text("pane") {
                    Some(address) => self.resolve_pane(&address)?,
                    None => self.focused(),
                };
                self.move_pane(
                    pane,
                    text("to").as_deref(),
                    text("before").as_deref(),
                    text("after").as_deref(),
                )?;
            }
            "agent.next" => self.cycle_agent(1),
            "agent.prev" => self.cycle_agent(-1),
            "terminal.park" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                self.park_terminal(&address);
            }
            "terminal.kill" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                self.kill_terminal_tree(&address);
            }
            "terminal.keep" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                self.call_rpc(
                    "keep",
                    "terminal.keep",
                    json!({"terminal":address,"keep":true}),
                );
                self.open_terminal(&address);
            }
            "terminal.send" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                let text = text("text").ok_or_else(|| anyhow::anyhow!(t("text required")))?;
                self.call_rpc(
                    "send",
                    "terminal.send",
                    json!({"terminal": address, "text": text}),
                );
            }
            "terminal.keys" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                let keys: Vec<String> = match args.get("keys") {
                    Some(Value::Array(list)) => list
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                    Some(Value::String(line)) => line
                        .split(|character: char| character == ',' || character.is_whitespace())
                        .filter(|part| !part.is_empty())
                        .map(str::to_string)
                        .collect(),
                    _ => Vec::new(),
                };
                anyhow::ensure!(!keys.is_empty(), t("keys required"));
                self.call_rpc(
                    "keys",
                    "terminal.keys",
                    json!({"terminal": address, "keys": keys}),
                );
            }
            "terminal.wait" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                let output = text("output").ok_or_else(|| anyhow::anyhow!(t("output required")))?;
                self.call_rpc(
                    "wait",
                    "terminal.wait",
                    json!({"terminal": address, "output": output}),
                );
            }
            "terminal.put" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                let path = text("path").ok_or_else(|| anyhow::anyhow!(t("path required")))?;
                let bytes = std::fs::read(&path)
                    .map_err(|error| anyhow::anyhow!(t1("read failed: {}", error)))?;
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".to_string());
                self.call_rpc(
                    "put",
                    "terminal.put",
                    json!({
                        "terminal": address,
                        "name": name,
                        "data": crate::clipboard::base64(&bytes),
                    }),
                );
            }
            "machine.rename" => {
                let machine =
                    text("machine").ok_or_else(|| anyhow::anyhow!(t("machine required")))?;
                let name = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                self.call_rpc(
                    "machine.rename",
                    "machine.rename",
                    json!({"machine": machine, "name": name}),
                );
            }
            "terminal.pin" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                let pinned = args["pinned"].as_bool().unwrap_or(true);
                self.update_terminal(Some(&address), |t| t.pinned = pinned);
                self.call_rpc(
                    "pin",
                    "terminal.label",
                    json!({"terminal":address,"labels":{"herdr.pinned":if pinned{"1"}else{""}}}),
                );
            }
            "terminal.peek" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                self.call_rpc(
                    "read",
                    "terminal.read",
                    json!({"terminal":address,"lines":500}),
                );
            }
            "terminal.show" => {
                if let Some(address) = text("terminal") {
                    self.show_terminal(&address)?;
                } else {
                    let id = self.focused();
                    if let Some(Some(address)) = self.shown_original.remove(&id) {
                        self.show_terminal(&address)?;
                        self.shown_original.remove(&id);
                    }
                }
            }
            "terminal.copy" => {
                let p = target_pane
                    .and_then(|id| self.panes.get(&id))
                    .ok_or_else(|| anyhow::anyhow!(t("terminal has no open view")))?;
                self.note = clipboard::copy(&p.view.viewport_lines().join("\n"));
            }
            "terminal.paste" => {
                let value = text("text").ok_or_else(|| anyhow::anyhow!(t("text required")))?;
                let id =
                    target_pane.ok_or_else(|| anyhow::anyhow!(t("terminal has no open view")))?;
                let pane = self
                    .panes
                    .get_mut(&id)
                    .ok_or_else(|| anyhow::anyhow!(t("no pane")))?;
                anyhow::ensure!(pane.is_live(), t("terminal is not attached"));
                pane.view.scroll_bottom();
                let bytes = pane.view.encode_paste(&value);
                self.send(Command::Input { pane: id, bytes });
            }
            "terminal.paste_image" => self.paste_image(),
            "terminal.right_click" => {
                let address = terminal().ok_or_else(|| anyhow::anyhow!(t("terminal required")))?;
                match text("to").as_deref() {
                    Some("program") => {
                        self.program_right_click.insert(address);
                    }
                    Some("menu") => {
                        self.program_right_click.remove(&address);
                    }
                    _ => anyhow::bail!(t("to must be program or menu")),
                }
            }
            "terminal.search" => {
                let id =
                    target_pane.ok_or_else(|| anyhow::anyhow!(t("terminal has no open view")))?;
                self.focus_pane(id);
                self.enter_copy_mode();
                if let Some(copy) = self.copy.as_mut() {
                    copy.typing = true;
                    copy.search = Some(text("query").unwrap_or_default());
                }
            }
            "terminal.scroll" => {
                let id =
                    target_pane.ok_or_else(|| anyhow::anyhow!(t("terminal has no open view")))?;
                let height = self.content.height as f64;
                if let Some(p) = self.panes.get_mut(&id) {
                    match text("to").as_deref() {
                        Some("bottom") => p.view.scroll_bottom(),
                        Some("top") => p.view.scroll(isize::MIN / 2),
                        _ => {
                            let lines = args["lines"].as_i64().unwrap_or_else(|| {
                                (args["pages"].as_f64().unwrap_or(1.0) * height) as i64
                            });
                            p.view.scroll(lines as isize);
                        }
                    }
                }
                self.dirty = true;
            }
            "machine.list" => self.call_rpc("machines", "machine.list", json!({})),
            "machine.connect" | "machine.add" | "machine.remove" => {
                let method = if name == "machine.remove" {
                    "machine.rm"
                } else {
                    name
                };
                self.call_rpc(method, method, args.clone());
            }
            "mod.open" => {
                let name = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                self.new_tab(Some(name.clone()));
                let id = self.focused();
                if let Some(p) = self.panes.get_mut(&id) {
                    p.module = Some(
                        json!({"id":p.origin,"type":"block","content":{"mod":name,"props":args.get("props").cloned().unwrap_or(json!({}))}}),
                    );
                }
                self.mark_layout();
            }
            "files.open" | "changes.open" | "file.open" => {
                self.open_workspace_panel(name, &args)?
            }
            "layout.reload" => {
                let machine = text("machine").unwrap_or_else(|| {
                    let m = self.workspace_machine(self.active_ws);
                    if m == "local" {
                        String::new()
                    } else {
                        m
                    }
                });
                if let Some(replica) = self.shared_docs.get_mut(&machine) {
                    replica.blocked = false;
                    replica.pending = None;
                    replica.version = u64::MAX;
                }
                self.request_shared(&machine);
                // 布局文档合并失败时同样走这里：接受共享版本，丢弃本地意图。
                self.layout.blocked = false;
                self.layout.pending = None;
                self.layout_dirty = false;
                self.layout_since = None;
                self.request_layout("doc.get");
            }
            "layout.switch" => {
                if let Some(name) = text("name") {
                    self.load_layout(&name)?;
                } else {
                    self.cycle_layout(1);
                }
            }
            "layout.clone" => {
                let target = text("as").unwrap_or_else(|| {
                    format!("{}-copy", text("name").unwrap_or_else(|| "layout".into()))
                });
                self.clone_layout(&target)?;
            }
            "layout.set" => {
                let value = args
                    .get("layout")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!(t("layout required")))?;
                self.apply_layout(value)?;
            }
            "layout.select" => {
                let child = text("child");
                let index = args["index"].as_u64().map(|n| n.saturating_sub(1) as usize);
                let target = child.as_ref().and_then(|id| {
                    self.panes
                        .iter()
                        .find(|(n, p)| p.origin.as_ref() == Some(id) || n.to_string() == *id)
                        .map(|(n, _)| *n)
                });
                let tab = self
                    .workspaces
                    .get_mut(self.active_ws)
                    .and_then(|w| w.tab_mut())
                    .ok_or_else(|| anyhow::anyhow!(t("no tab")))?;
                match &mut tab.tree {
                    SplitNode::Stack { panes, active } => {
                        let index = target
                            .and_then(|id| panes.iter().position(|p| p.0 == id))
                            .or(index)
                            .ok_or_else(|| anyhow::anyhow!(t("child or index required")))?;
                        anyhow::ensure!(index < panes.len(), t("child index out of range"));
                        *active = index;
                        tab.focus = panes[index].0;
                    }
                    _ => anyhow::bail!(t("shown tab is not a stack")),
                };
                self.mark_layout();
            }
            "layout.reset" => self.load_layout("balanced")?,
            "mod.clone" => {
                let name = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                let target = text("as").unwrap_or_else(|| format!("{name}-copy"));
                self.clone_panel(&name, &target)?;
            }

            "layout.show" => {
                self.sidebar = true;
            }
            "layout.hide" => {
                self.sidebar = false;
            }
            "layout.toggle" => {
                self.sidebar = !self.sidebar;
            }
            "layout.resize" => {
                let px = args["px"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!(t("px required")))?;
                self.sidebar_width = (px.min(u16::MAX as u64) as u16)
                    .clamp(self.config.sidebar_min_width, self.config.sidebar_max_width);
            }
            "app.palette" => self.open_palette(),
            "app.settings" => self.open_settings(),
            "app.help" => {
                self.help = Some(HelpOverlay {
                    query: String::new(),
                    selected: 0,
                    scroll: 0,
                })
            }
            "app.theme" => {
                let name = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                let theme = crate::theme::builtin(&name)
                    .ok_or_else(|| anyhow::anyhow!(t("unknown theme")))?;
                let quoted = serde_json::to_string(&name)?;
                self.write_setting("theme.name", &quoted)?;
                self.config.theme = theme;
                self.config.theme.name = name;
                self.note = format!("theme.name = {quoted}");
                self.notify_mods_theme();
                self.bump_mod_revision_domains(&["theme", "settings"]);
            }
            "app.indicators" => {
                let value = text("value").ok_or_else(|| anyhow::anyhow!(t("value required")))?;
                let indicators = match value.as_str() {
                    "dots" => crate::config::Indicators::Dots,
                    "symbols" => crate::config::Indicators::Symbols,
                    _ => anyhow::bail!(t("unknown indicators")),
                };
                let quoted = format!("\"{value}\"");
                self.write_setting("ui.status_indicators", &quoted)?;
                self.config.indicators = indicators;
                self.note = format!("ui.status_indicators = {quoted}");
                self.bump_mod_revision_domains(&["settings"]);
            }
            "app.notifications" => {
                let value = text("value").ok_or_else(|| anyhow::anyhow!(t("value required")))?;
                let delivery = match value.as_str() {
                    "off" => crate::config::Delivery::Off,
                    "toast" => crate::config::Delivery::Toast,
                    "system" => crate::config::Delivery::System,
                    _ => anyhow::bail!(t("unknown delivery")),
                };
                let quoted = format!("\"{value}\"");
                self.write_setting("ui.toast.delivery", &quoted)?;
                self.config.notifications = delivery;
                self.note = format!("ui.toast.delivery = {quoted}");
                self.bump_mod_revision_domains(&["settings"]);
            }
            "app.sound" => {
                let on = match args.get("value") {
                    Some(Value::Bool(on)) => *on,
                    Some(Value::String(value)) => match value.as_str() {
                        "on" => true,
                        "off" => false,
                        _ => anyhow::bail!(t("unknown sound")),
                    },
                    _ => anyhow::bail!(t("value required")),
                };
                self.write_setting("ui.sound.enabled", &format!("{on}"))?;
                self.config.sound = on;
                self.note = format!("ui.sound.enabled = {on}");
                self.bump_mod_revision_domains(&["settings"]);
            }
            "app.quit" => self.quit = true,
            "mods.dock" | "mods.undock" | "mods.chrome" | "mods.open" | "mods.close"
            | "mods.reload" | "mods.grant" | "mods.enable" | "mods.disable"
                if self.config.no_mods =>
            {
                anyhow::bail!(t("mods are disabled (--no-mods)"))
            }
            "mods.dock" => {
                let name = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                let place = match text("place").as_deref() {
                    Some("right") => DockPlace::Right,
                    Some("top") => DockPlace::Top,
                    Some("bottom") => DockPlace::Bottom,
                    _ => DockPlace::Left,
                };
                let size = args
                    .get("size")
                    .and_then(|value| serde_json::from_value::<ChromeSize>(value.clone()).ok());
                let index = args
                    .get("index")
                    .and_then(|value| value.as_u64())
                    .map(|v| v as usize);
                let props = args.get("props").cloned().unwrap_or_else(|| json!({}));
                self.chrome.dock(&name, place, size, index, props);
                self.dirty = true;
                self.save_local_state();
            }
            "mods.undock" => {
                let name = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                if self.chrome.undock(&name) {
                    self.dirty = true;
                    self.save_local_state();
                }
            }
            "mods.chrome" => {
                let tree = args
                    .get("tree")
                    .ok_or_else(|| anyhow::anyhow!(t("tree required")))?;
                let packaged = if tree.get("root").is_some() {
                    tree.clone()
                } else {
                    json!({"schema_version": 1, "root": tree})
                };
                let chrome = crate::mods::chrome::Chrome::from_state(Some(&packaged))
                    .ok_or_else(|| anyhow::anyhow!(t("invalid chrome tree")))?;
                self.chrome = chrome;
                self.dirty = true;
                self.save_local_state();
            }
            "mods.open" => {
                let name = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                let place = match text("place").as_deref() {
                    Some(value) => crate::mods::SurfacePlace::parse(value)
                        .ok_or_else(|| anyhow::anyhow!(t("unknown place")))?,
                    None => crate::mods::SurfacePlace::Region,
                };
                let size = match args.get("size") {
                    Some(Value::Array(list)) if list.len() == 2 => Some((
                        list[0].as_u64().unwrap_or(0).min(u16::MAX as u64) as u16,
                        list[1].as_u64().unwrap_or(0).min(u16::MAX as u64) as u16,
                    )),
                    Some(value) => {
                        let cols = value.get("cols").and_then(Value::as_u64);
                        let rows = value.get("rows").and_then(Value::as_u64);
                        match (cols, rows) {
                            (Some(cols), Some(rows)) => Some((cols as u16, rows as u16)),
                            _ => None,
                        }
                    }
                    None => None,
                };
                let at = args.get("at").and_then(Value::as_array).and_then(|list| {
                    Some((
                        list.first()?.as_u64().min(Some(u16::MAX as u64))? as u16,
                        list.get(1)?.as_u64().min(Some(u16::MAX as u64))? as u16,
                    ))
                });
                let props = args.get("props").cloned().unwrap_or_else(|| json!({}));
                let key = text("key");
                let surface = self
                    .mods_open(&name, place, props, size, at, key)
                    .map_err(anyhow::Error::msg)?;
                self.apply_surface_options(&surface, &args);
            }
            "mods.close" => {
                let target = text("surface")
                    .or_else(|| text("name"))
                    .ok_or_else(|| anyhow::anyhow!(t("name or surface required")))?;
                if !self.mods_close(&target) {
                    anyhow::bail!(t1("no such mod surface: {}", target));
                }
            }
            "mods.reload" => {
                let target = text("name");
                self.mods_reload_target(target.as_deref())
                    .map_err(anyhow::Error::msg)?;
            }
            "mods.grant" => {
                let name = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                let list = |key: &str| {
                    args.get(key)
                        .and_then(Value::as_array)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                };
                let names: Vec<String> = list("host").into_iter().chain(list("events")).collect();
                if names.is_empty() {
                    anyhow::bail!(t("host or events required"));
                }
                let revoke = args.get("revoke").and_then(Value::as_bool).unwrap_or(false);
                if revoke {
                    self.mods.revoke_grants(&name, &names);
                } else {
                    self.mods.approve_grants(&name, &names);
                }
                let grants = self.mods.grants_for(&name);
                self.send_mod_event(&name, "grant.changed", &json!({ "grants": grants }));
                self.persist_mod_grants(&name);
                self.seen_dirty = true;
                self.bump_mod_revision_domains(&[]);
            }
            "mods.enable" | "mods.disable" => {
                let enable = name == "mods.enable";
                let mod_id = text("name").ok_or_else(|| anyhow::anyhow!(t("name required")))?;
                self.mods.enabled.insert(mod_id.clone(), enable);
                crate::pkg::set_enabled(&self.config, &mod_id, enable)
                    .map_err(anyhow::Error::msg)?;
                self.refresh_plugins_if_changed();
                self.bump_mod_revision();
            }
            "overlay.close" => {
                self.prompt = None;
                self.settle_mod_confirm("cancelled");
                self.goto = None;
                self.reader = None;
                self.chooser = None;
                self.help = None;
                self.settings = None;
                self.chat = None;
                self.copy = None;
                if self.popup.is_some() {
                    self.close_popup();
                }
            }
            _ => anyhow::bail!(t1("Unknown command: {}", name)),
        }
        if matches!(name, "space.new" | "tab.new" | "terminal.open") {
            if let Some(module) = text("mod") {
                let id = self.focused();
                if let Some(p) = self.panes.get_mut(&id) {
                    p.module = Some(
                        json!({"id":p.origin,"type":"block","content":{"mod":module,"props":args.get("props").cloned().unwrap_or(json!({}))}}),
                    );
                }
                self.mark_layout();
            }
            if let Some(task) = text("task") {
                self.initial_tasks.insert(self.focused(), task);
            }
        }
        self.dirty = true;
        Ok(())
    }

    /// 动作参数里的 Pane 地址：`origin` 或进程内编号。
    fn resolve_pane(&self, address: &str) -> Result<u32> {
        self.panes
            .iter()
            .find(|(id, pane)| pane.origin.as_deref() == Some(address) || id.to_string() == address)
            .map(|(id, _)| *id)
            .ok_or_else(|| anyhow::anyhow!(t1("unknown pane {}", address)))
    }

    /// 动作参数里的 Tab 地址：`origin` 或编号，跨工作区查找。
    fn tab_position(&self, address: &str) -> Result<(usize, usize)> {
        self.workspaces
            .iter()
            .enumerate()
            .find_map(|(index, workspace)| {
                workspace
                    .tabs
                    .iter()
                    .position(|tab| {
                        tab.origin.as_deref() == Some(address) || tab.id.to_string() == address
                    })
                    .map(|position| (index, position))
            })
            .ok_or_else(|| anyhow::anyhow!(t1("unknown tab {}", address)))
    }

    fn tab_position_by_id(&self, id: u32) -> Option<(usize, usize)> {
        self.workspaces
            .iter()
            .enumerate()
            .find_map(|(index, workspace)| {
                workspace
                    .tabs
                    .iter()
                    .position(|tab| tab.id == id)
                    .map(|position| (index, position))
            })
    }

    pub(super) fn swap_direction(&mut self, direction: Direction) -> Result<()> {
        let id = self.focused();
        let leaves: Vec<_> = self
            .slots
            .iter()
            .filter(|s| !s.overlay)
            .map(|s| (PaneId(s.pane), s.outer))
            .collect();
        let target = neighbor(&leaves, PaneId(id), direction)
            .ok_or_else(|| anyhow::anyhow!(t("no neighbor")))?
            .0;
        self.swap_panes(id, target)
    }

    /// 同一 Tab 里交换两个 Pane 的位置。`self.panes` 里的终端、标题与模块不搬家，
    /// 只换它们在树和浮动层里占的位置；焦点是 Pane 编号，所以跟着 Pane 走。
    /// 跨 Tab 不交换，需要跨 Tab 时用 `pane.move`。
    pub(super) fn swap_panes(&mut self, id: u32, target: u32) -> Result<()> {
        anyhow::ensure!(id != target, t("cannot swap a pane with itself"));
        let here = self
            .locate(id)
            .ok_or_else(|| anyhow::anyhow!(t1("unknown pane {}", id)))?;
        let there = self
            .locate(target)
            .ok_or_else(|| anyhow::anyhow!(t1("unknown pane {}", target)))?;
        anyhow::ensure!(here == there, t("swap needs both panes in the same tab"));
        fn swap(node: &mut SplitNode, a: u32, b: u32) {
            match node {
                SplitNode::Leaf(id) => {
                    if id.0 == a {
                        id.0 = b;
                    } else if id.0 == b {
                        id.0 = a;
                    }
                }
                SplitNode::Split { first, second, .. } => {
                    swap(first, a, b);
                    swap(second, a, b);
                }
                SplitNode::Stack { panes, .. } => {
                    for id in panes {
                        if id.0 == a {
                            id.0 = b;
                        } else if id.0 == b {
                            id.0 = a;
                        }
                    }
                }
            }
        }
        let tab = &mut self.workspaces[here.0].tabs[here.1];
        swap(&mut tab.tree, id, target);
        for entry in &mut tab.floating {
            if entry.pane == id {
                entry.pane = target;
            } else if entry.pane == target {
                entry.pane = id;
            }
        }
        self.mark_layout();
        Ok(())
    }

    /// 把 Pane 移到目标 Tab 的指定位置。终端、标题与模块跟着 Pane 走；源 Tab 因此
    /// 没有 Pane 时按现有规则收回（先提浮动 Pane，再删 Tab，最后删空工作区），
    /// 已经搬走的终端不结束。
    pub(super) fn move_pane(
        &mut self,
        pane: u32,
        to_tab: Option<&str>,
        before: Option<&str>,
        after: Option<&str>,
    ) -> Result<()> {
        anyhow::ensure!(
            before.is_none() || after.is_none(),
            t("use before or after, not both")
        );
        let (source_ws, source_tab) = self
            .locate(pane)
            .ok_or_else(|| anyhow::anyhow!(t1("unknown pane {}", pane)))?;
        let (target_ws, target_tab) = match to_tab {
            Some(address) => self.tab_position(address)?,
            None => (
                self.active_ws,
                self.workspace()
                    .map(|workspace| workspace.active)
                    .ok_or_else(|| anyhow::anyhow!(t("no tab")))?,
            ),
        };
        let anchor = match (before, after) {
            (Some(address), _) => Some((self.resolve_pane(address)?, true)),
            (None, Some(address)) => Some((self.resolve_pane(address)?, false)),
            (None, None) => None,
        };
        if let Some((anchor, _)) = anchor {
            anyhow::ensure!(anchor != pane, t("cannot move a pane next to itself"));
            anyhow::ensure!(
                self.workspaces[target_ws].tabs[target_tab]
                    .tree
                    .panes()
                    .contains(&PaneId(anchor)),
                t("anchor pane is not in the target tab")
            );
        }
        let same_tab = source_ws == target_ws && source_tab == target_tab;
        if same_tab
            && anchor.is_none()
            && self.workspaces[target_ws].tabs[target_tab]
                .tree
                .panes()
                .last()
                == Some(&PaneId(pane))
        {
            // 已经在目标位置，只要把焦点给回来。
            self.focus_pane(pane);
            return Ok(());
        }
        let target_id = self.workspaces[target_ws].tabs[target_tab].id;
        let source_active = self.workspaces[source_ws].tab().map(|tab| tab.id);
        // 把 Pane 从源 Tab 的结构里摘出来；视图与终端留在 `self.panes` 里。
        let removed_tab = {
            let tab = &mut self.workspaces[source_ws].tabs[source_tab];
            if let Some(index) = tab.floating.iter().position(|entry| entry.pane == pane) {
                tab.floating.remove(index);
                false
            } else {
                let tree = std::mem::replace(&mut tab.tree, SplitNode::Leaf(PaneId(pane)));
                match tree.remove_pane(PaneId(pane)) {
                    Some(tree) => {
                        tab.tree = tree;
                        false
                    }
                    // 唯一的分屏 Pane：先让浮动 Pane 接住分屏，没有就让 Tab 空掉。
                    None => !tab.promote_floating(),
                }
            }
        };
        if removed_tab {
            self.workspaces[source_ws].tabs.remove(source_tab);
            if let Some(index) = self.workspaces[source_ws]
                .tabs
                .iter()
                .position(|tab| Some(tab.id) == source_active)
            {
                self.workspaces[source_ws].active = index;
            }
            if self.workspaces[source_ws].tabs.is_empty() {
                self.workspaces.remove(source_ws);
                if source_ws < self.active_ws {
                    self.active_ws -= 1;
                }
                self.active_ws = self.active_ws.min(self.workspaces.len().saturating_sub(1));
            }
        } else {
            let tab = &mut self.workspaces[source_ws].tabs[source_tab];
            if tab.focus == pane {
                tab.focus = tab.tree.panes().first().map(|id| id.0).unwrap_or(0);
            }
        }
        // 源 Tab 可能已经消失，按 id 重新定位目标。
        let (target_ws, target_tab) = self
            .tab_position_by_id(target_id)
            .ok_or_else(|| anyhow::anyhow!(t("no tab")))?;
        let tab = &mut self.workspaces[target_ws].tabs[target_tab];
        match anchor {
            Some((anchor, before)) => anyhow::ensure!(
                tab.tree.insert_beside(PaneId(anchor), PaneId(pane), before),
                t("anchor pane is not in the target tab")
            ),
            None => match tab.tree.panes().last().map(|id| id.0) {
                Some(last) => {
                    tab.tree
                        .split_pane(PaneId(last), Axis::Horizontal, PaneId(pane), 0.5);
                }
                None => tab.tree = SplitNode::leaf(pane),
            },
        }
        self.focus_pane(pane);
        self.note = t1("pane moved to tab {}", target_tab + 1);
        self.mark_layout();
        Ok(())
    }

    pub(super) fn cycle_agent(&mut self, delta: isize) {
        let mut rows: Vec<_> = self
            .navigation_rows()
            .into_iter()
            .filter(|r| r.badge != Badge::None)
            .collect();
        // 模块通过 `setAgentOrder` 指定的顺序优先；未列出的保持相对顺序。
        if let Some(order) = &self.mod_agent_order {
            let position = |row: &NavigationRow| {
                row.terminal
                    .as_ref()
                    .and_then(|address| order.iter().position(|item| item == address))
                    .unwrap_or(usize::MAX)
            };
            rows.sort_by_key(position);
        }
        if rows.is_empty() {
            return;
        }
        let address = self.focused_pane().and_then(|p| p.terminal.as_deref());
        let current = rows.iter().position(|r| r.terminal.as_deref() == address);
        let next = current
            .map(|i| (i as isize + delta).rem_euclid(rows.len() as isize) as usize)
            .unwrap_or(if delta > 0 { 0 } else { rows.len() - 1 });
        if let Some(address) = &rows[next].terminal {
            self.open_terminal(address);
        }
    }

    pub(super) fn kill_terminal_tree(&mut self, address: &str) {
        let mut queue = vec![address.to_string()];
        let mut visited = std::collections::HashSet::new();
        let mut order = Vec::new();
        while let Some(address) = queue.pop() {
            if !visited.insert(address.clone()) {
                continue;
            }
            queue.extend(
                self.terminals
                    .iter()
                    .filter(|t| t.parent.as_deref() == Some(&address))
                    .map(|t| t.address.clone()),
            );
            order.push(address);
        }
        for address in order.into_iter().rev() {
            self.call_rpc(
                &format!("kill:{address}"),
                "terminal.kill",
                json!({"terminal":address}),
            );
        }
    }
    fn park_terminal(&mut self, address: &str) {
        self.call_rpc(
            "keep",
            "terminal.keep",
            json!({"terminal":address,"keep":false}),
        );
        if let Some(pane) = self.pane_by_terminal(address) {
            self.detach_view_from_layout(pane);
        }
        self.mark_layout();
    }
    fn show_terminal(&mut self, address: &str) -> Result<()> {
        let id = self.focused();
        let p = self
            .panes
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!(t("no pane")))?;
        if p.terminal.as_deref() == Some(address) {
            return Ok(());
        }
        let old = p.terminal.clone();
        self.shown_original.entry(id).or_insert(old);
        self.send(Command::Close { pane: id });
        let origin = self.panes[&id].origin.clone();
        self.spawn_pane(id);
        let p = self.panes.get_mut(&id).unwrap();
        p.origin = origin;
        p.terminal = Some(address.to_string());
        self.sync_terminals();
        self.dirty = true;
        Ok(())
    }

    fn open_workspace_panel(&mut self, name: &str, args: &Value) -> Result<()> {
        let cwd = self.workspace_cwd(self.active_ws);
        let machine = self.workspace_machine(self.active_ws);
        let on = (machine != "local").then_some(machine);
        if name == "file.open" {
            let path = args["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!(t("path required")))?;
            self.call_rpc(
                "panel.file",
                "file.read",
                json!({"path":path,"cwd":cwd,"on":on}),
            );
        } else {
            let argv = if name == "changes.open" {
                vec![
                    "git",
                    "-c",
                    "color.ui=false",
                    "diff",
                    "--stat",
                    "--patch",
                    "HEAD",
                ]
            } else {
                vec![
                    "git",
                    "ls-files",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                ]
            };
            self.call_rpc(
                "panel.exec",
                "machine.exec",
                json!({"argv":argv,"cwd":cwd,"on":on}),
            );
        }
        Ok(())
    }
}

impl App {
    pub(super) fn detach_view_from_layout(&mut self, pane: u32) {
        let focused = self.focused();
        let Some((wi, ti)) = self.locate(pane) else {
            self.remove_pane_view(pane);
            return;
        };
        let active_tab = self.workspaces[wi].tab().map(|t| t.id);
        self.remove_pane_view(pane);
        let tab = &mut self.workspaces[wi].tabs[ti];
        if let Some(index) = tab.floating.iter().position(|f| f.pane == pane) {
            tab.floating.remove(index);
        } else if let Some(tree) = tab.tree.clone().remove_pane(PaneId(pane)) {
            tab.tree = tree;
        } else if !tab.promote_floating() {
            self.workspaces[wi].tabs.remove(ti);
        }
        if let Some(tab) = self.workspaces[wi].tabs.get_mut(ti) {
            if tab.focus == pane {
                tab.focus = tab.panes().first().map(|p| p.0).unwrap_or(0);
            }
        }
        if let Some(index) = self.workspaces[wi]
            .tabs
            .iter()
            .position(|t| Some(t.id) == active_tab)
        {
            self.workspaces[wi].active = index;
        }
        if self.workspaces[wi].tabs.is_empty() {
            self.workspaces.remove(wi);
            if wi < self.active_ws {
                self.active_ws -= 1;
            }
        }
        self.active_ws = self.active_ws.min(self.workspaces.len().saturating_sub(1));
        for w in &mut self.workspaces {
            w.active = w.active.min(w.tabs.len().saturating_sub(1));
        }
        self.zoom = false;
        self.layout_drag = None;
        let next = self.focused();
        if next != focused && self.panes.contains_key(&next) {
            self.send(Command::Focus {
                pane: next,
                focused: true,
            });
        }
        self.mark_layout();
    }
}

impl App {
    pub(super) fn open_terminal_menu(&mut self, address: &str) {
        self.open_terminal_picker_menu(address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> App {
        let (_, events) = std::sync::mpsc::channel();
        App::with_network(
            Config {
                restore: false,
                confirm_close: false,
                prompt_new_tab: false,
                state_path: "/nonexistent/hctui-state".into(),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap()
    }
    #[test]
    fn direct_bindings_execute_actions_without_a_prefix() {
        let mut app = app();
        app.config.keys.new_tab = crate::config::Binding::parse("ctrl+t").unwrap();
        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        assert_eq!(app.workspace().unwrap().tabs.len(), 2);
    }
    #[test]
    fn closing_last_view_kills_descendants_but_parking_does_not() {
        let mut app = app();
        let pane = app.focused();
        app.panes.get_mut(&pane).unwrap().terminal = Some("t1".into());
        app.terminals = vec![
            TerminalSummary::parse(&json!({"address":"t1"})).unwrap(),
            TerminalSummary::parse(&json!({"address":"t2","parent":"t1"})).unwrap(),
        ];
        app.execute_action("terminal.park", json!({"terminal":"t1"}))
            .unwrap();
        assert!(app.workspaces.is_empty());
        assert!(!app.inflight.values().any(|(m, _)| m == "terminal.kill"));
        assert_eq!(app.inflight["keep"].1["keep"], false);
        app.new_workspace(None);
        let pane = app.focused();
        app.panes.get_mut(&pane).unwrap().terminal = Some("t1".into());
        app.execute_action("pane.close", json!({})).unwrap();
        assert!(app.workspaces.is_empty());
        assert_eq!(app.inflight["kill:t1"].0, "terminal.kill");
        assert_eq!(app.inflight["kill:t2"].0, "terminal.kill");
    }
    #[test]
    fn pane_swap_changes_positions_without_changing_identity() {
        let mut app = app();
        app.reflow(Rect::new(0, 0, 100, 30));
        let first = app.focused();
        app.split_focused(Axis::Horizontal);
        let second = app.focused();
        app.reflow(Rect::new(0, 0, 100, 30));
        app.execute_action("pane.swap", json!({"direction":"left"}))
            .unwrap();
        assert_eq!(
            app.tab().unwrap().tree.panes(),
            vec![PaneId(second), PaneId(first)]
        );
        assert_eq!(app.focused(), second);
    }
    #[test]
    fn swapping_two_panes_by_id_keeps_their_terminals_and_follows_the_focus() {
        let mut app = app();
        let first = app.focused();
        app.panes.get_mut(&first).unwrap().terminal = Some("t1".into());
        app.split_focused(Axis::Horizontal);
        let second = app.focused();
        app.panes.get_mut(&second).unwrap().terminal = Some("t2".into());
        app.execute_action(
            "pane.swap",
            json!({"pane": first.to_string(), "with": second.to_string()}),
        )
        .unwrap();
        assert_eq!(
            app.tab().unwrap().tree.panes(),
            vec![PaneId(second), PaneId(first)]
        );
        assert_eq!(app.focused(), second);
        assert_eq!(app.panes[&first].terminal.as_deref(), Some("t1"));
        assert_eq!(app.panes[&second].terminal.as_deref(), Some("t2"));
    }
    #[test]
    fn a_cross_tab_swap_is_refused_and_leaves_both_tabs_alone() {
        let mut app = app();
        let first = app.focused();
        app.new_tab(None);
        let second = app.focused();
        let before = app.tab().unwrap().tree.panes();
        let error = app
            .execute_action(
                "pane.swap",
                json!({"pane": first.to_string(), "with": second.to_string()}),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("same tab"), "{error}");
        assert_eq!(app.tab().unwrap().tree.panes(), before);
        assert_eq!(app.workspaces[0].tabs[0].tree.panes(), vec![PaneId(first)]);
    }
    #[test]
    fn moving_a_pane_to_another_tab_takes_its_terminal_and_drops_the_empty_tab() {
        let mut app = app();
        let first = app.focused();
        app.panes.get_mut(&first).unwrap().terminal = Some("t1".into());
        app.new_tab(None);
        let second = app.focused();
        app.panes.get_mut(&second).unwrap().terminal = Some("t2".into());
        let target = app.tab().unwrap().id;
        app.execute_action(
            "pane.move",
            json!({"pane": first.to_string(), "to": target.to_string()}),
        )
        .unwrap();
        assert_eq!(app.workspaces[0].tabs.len(), 1);
        let tab = app.workspaces[0].tab().unwrap();
        assert_eq!(tab.id, target);
        assert_eq!(tab.tree.panes(), vec![PaneId(second), PaneId(first)]);
        assert_eq!(app.focused(), first);
        assert_eq!(app.panes[&first].terminal.as_deref(), Some("t1"));
        assert!(!app
            .inflight
            .values()
            .any(|(method, _)| method == "terminal.kill"));
    }
    #[test]
    fn moving_the_only_pane_of_a_workspace_keeps_its_terminal_and_drops_the_space() {
        let mut app = app();
        let first = app.focused();
        app.panes.get_mut(&first).unwrap().terminal = Some("t1".into());
        app.new_workspace(None);
        let second = app.focused();
        let target = app.tab().unwrap().id;
        app.execute_action(
            "pane.move",
            json!({"pane": first.to_string(), "to": target.to_string()}),
        )
        .unwrap();
        assert_eq!(app.workspaces.len(), 1);
        assert_eq!(
            app.workspaces[0].tabs[0].tree.panes(),
            vec![PaneId(second), PaneId(first)]
        );
        assert_eq!(app.focused(), first);
        assert_eq!(app.panes[&first].terminal.as_deref(), Some("t1"));
    }
    #[test]
    fn moving_a_pane_before_or_after_an_anchor_places_it_on_that_side() {
        let mut app = app();
        let first = app.focused();
        app.split_focused(Axis::Horizontal);
        let second = app.focused();
        app.execute_action(
            "pane.move",
            json!({"pane": first.to_string(), "after": second.to_string()}),
        )
        .unwrap();
        assert_eq!(
            app.tab().unwrap().tree.panes(),
            vec![PaneId(second), PaneId(first)]
        );
        assert_eq!(app.focused(), first);
        app.execute_action("pane.move", json!({"before": second.to_string()}))
            .unwrap();
        assert_eq!(
            app.tab().unwrap().tree.panes(),
            vec![PaneId(first), PaneId(second)]
        );
    }
    #[test]
    fn pane_move_refuses_unknown_targets_and_conflicting_anchors() {
        let mut app = app();
        let pane = app.focused();
        let error = app
            .execute_action("pane.move", json!({"pane": pane.to_string(), "to": "999"}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown tab"), "{error}");
        let error = app
            .execute_action("pane.move", json!({"pane": "999"}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown pane"), "{error}");
        let error = app
            .execute_action("pane.move", json!({"before": "1", "after": "1"}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("before or after"), "{error}");
        assert_eq!(app.workspaces[0].tabs[0].tree.panes(), vec![PaneId(pane)]);
    }
    #[test]
    fn done_is_held_only_while_focus_stays_on_the_agent() {
        let mut app = app();
        let first = app.focused();
        let p = app.panes.get_mut(&first).unwrap();
        p.terminal = Some("t1".into());
        p.agent.set_status(Some("idle".into()));
        p.agent.observe_turn(1);
        app.mark_seen(first);
        assert_eq!(
            app.navigation_rows()
                .iter()
                .find(|r| r.pane == Some(first))
                .unwrap()
                .badge,
            Badge::Done
        );
        app.new_tab(None);
        let second = app.focused();
        app.mark_seen(second);
        assert_eq!(
            app.navigation_rows()
                .iter()
                .find(|r| r.pane == Some(first))
                .unwrap()
                .badge,
            Badge::Idle
        );
    }
    #[test]
    fn settled_tab_wakes_on_attention_without_changing_focus() {
        let mut app = app();
        let first = app.focused();
        app.panes.get_mut(&first).unwrap().terminal = Some("t1".into());
        app.new_tab(None);
        let second = app.focused();
        app.workspaces[0].tabs[0].settled = true;
        app.apply_event(TerminalEvent {
            kind: "status".into(),
            terminal: Some("t1".into()),
            seq: 1,
            data: json!({"status":{"state":"blocked"}}),
        });
        assert!(!app.workspaces[0].tabs[0].settled);
        assert_eq!(app.focused(), second);
    }
    #[test]
    fn next_and_previous_use_displayed_agent_order() {
        let mut app = app();
        let first = app.focused();
        app.panes.get_mut(&first).unwrap().terminal = Some("t1".into());
        app.panes
            .get_mut(&first)
            .unwrap()
            .agent
            .set_status(Some("working".into()));
        app.new_tab(None);
        let second = app.focused();
        app.panes.get_mut(&second).unwrap().terminal = Some("t2".into());
        app.panes
            .get_mut(&second)
            .unwrap()
            .agent
            .set_status(Some("working".into()));
        app.execute_action("agent.next", json!({})).unwrap();
        assert_eq!(app.focused(), first);
        app.execute_action("agent.prev", json!({})).unwrap();
        assert_eq!(app.focused(), second);
    }
}
