//! Script-backed TUI modules and built-in workspace panels.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};

pub(super) fn builtin(name: &str) -> Option<&'static str> {
    match name{
    "files"=>Some("if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then git ls-files --cached --others --exclude-standard; else ls -Ap; fi"),
    "changes"=>Some("git -c color.ui=false status --short; git -c color.ui=false diff --no-ext-diff; git -c color.ui=false diff --cached --no-ext-diff"),_=>None}
}

impl App {
    pub(super) fn refresh_panels(&mut self, now: Instant) {
        let slots = self.slots.clone();
        for slot in slots {
            let Some(raw) = self.panes.get(&slot.pane).and_then(|p| p.module.clone()) else {
                continue;
            };
            let name = raw
                .pointer("/content/mod")
                .and_then(Value::as_str)
                .unwrap_or("module");
            let custom = self.config.panels.iter().find(|p| p.name == name);
            let interval = custom.map(|p| p.refresh).unwrap_or(5);
            let tag = format!("panel:{}", slot.pane);
            if self.inflight.contains_key(&tag)
                || self
                    .panel_refresh
                    .get(&slot.pane)
                    .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(interval))
            {
                continue;
            }
            let located = self
                .locate(slot.pane)
                .map(|p| p.0)
                .unwrap_or(self.active_ws);
            let cwd = self.workspace_cwd(located);
            let machine = self.workspace_machine(located);
            let on = (machine != "local").then_some(machine);
            let command = custom
                .map(|p| p.command.clone())
                .or_else(|| builtin(name).map(str::to_string));
            self.panel_refresh.insert(slot.pane, now);
            if let Some(command) = command {
                self.call_rpc(&tag,"machine.exec",json!({"argv":["sh","-c",command],"cwd":cwd,"on":on,"env":{"HC_TUI_PANEL_PROPS":raw.pointer("/content/props").cloned().unwrap_or(json!({})).to_string()},"timeout":30}));
            } else {
                let text=format!("{name}\r\n\r\nRegister a [[panels]] command with this name in hc-tui/config.toml to display this GUI module in the terminal.\r\nIts content and props are preserved in the shared layout.");
                if let Some(pane) = self.panes.get_mut(&slot.pane) {
                    pane.view.write(b"\x1b[2J\x1b[H");
                    pane.view.write(text.as_bytes());
                }
                self.dirty = true;
            }
        }
    }
    pub(super) fn panel_reply(
        &mut self,
        id: u32,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) {
        let Some(pane) = self.panes.get_mut(&id) else {
            return;
        };
        let text = match result {
            Ok(value) => {
                let decode = |key: &str| {
                    STANDARD
                        .decode(value[key].as_str().unwrap_or(""))
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default()
                };
                let mut text = format!("{}{}", decode("stdout"), decode("stderr"));
                if value["timed_out"] == true {
                    text.push_str("\nCommand timed out");
                }
                if value["truncated"] == true {
                    text.push_str("\nOutput truncated");
                }
                text
            }
            Err(error) => error.message,
        };
        pane.view.write(b"\x1b[2J\x1b[H");
        pane.view.write(text.replace('\n', "\r\n").as_bytes());
        self.dirty = true;
    }
}
