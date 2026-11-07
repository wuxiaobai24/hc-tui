//! App 侧的插件集成：动作调用、上下文冻结、结果提示与控制通道查询。

use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{json, Value};

use super::{App, ChooserItem};
use crate::i18n::{t1, t2};
use crate::plugin::manifest;
use crate::plugin::registry::ActionInfo;
use crate::plugin::runner::{InvokeError, Spec};

impl App {
    /// 插件动作的菜单项；`target` 会冻结进调用上下文。
    pub fn plugin_menu_items(&self, context: &str, target: Value) -> Vec<ChooserItem> {
        self.plugins
            .actions()
            .into_iter()
            .filter(|action| action.contexts.iter().any(|entry| entry == context))
            .map(|action| ChooserItem {
                label: format!("{} · {}", action.title, action.name),
                value: format!("{} {}", action.name, json!({"_target": target})),
            })
            .collect()
    }

    /// 执行插件动作：校验参数、冻结目标、注入环境并提交给运行器。
    pub fn invoke_plugin(&mut self, name: &str, mut args: Value) -> Result<()> {
        let Some(action) = self.plugins.action(name) else {
            anyhow::bail!(t1("unknown plugin action {}", name));
        };
        let target_arg = args.as_object_mut().and_then(|map| map.remove("_target"));
        let args = match &action.args {
            Some(schema) => manifest::validate_args(schema, &args).map_err(anyhow::Error::msg)?,
            None => args,
        };
        let request = self.plugin_runner.allocate_request();
        let (target, pane) = self.plugin_target(&action, target_arg);
        let context = self.plugin_context(&action, &request, args, target, pane);
        self.prepare_plugin_dirs(&action);
        let env = self.plugin_env(&action, &request);
        let spec = Spec {
            package: action.package.clone(),
            action: action.action.clone(),
            root: action.root.clone(),
            argv: action.argv.clone(),
            timeout: action.timeout,
            context,
            env,
            request: Some(request.clone()),
        };
        match self.plugin_runner.invoke(spec) {
            Ok(request) => {
                self.plugin_request = Some(request.clone());
                self.note = t1("plugin {} queued", &action.name);
                self.dirty = true;
                Ok(())
            }
            Err(InvokeError::Busy) => anyhow::bail!("busy: the plugin queue is full"),
            Err(InvokeError::Invalid(message)) => anyhow::bail!("{message}"),
        }
    }

    fn plugin_target(&self, action: &ActionInfo, requested: Option<Value>) -> (Value, Option<u32>) {
        let mut pane = None;
        if let Some(requested) = requested {
            let pick = requested
                .get("pane")
                .and_then(Value::as_u64)
                .map(|id| id as u32)
                .or_else(|| {
                    requested
                        .get("pane_or_terminal")
                        .and_then(Value::as_str)
                        .and_then(|value| {
                            value.parse::<u32>().ok().or_else(|| {
                                self.panes
                                    .iter()
                                    .find(|(_, state)| state.terminal.as_deref() == Some(value))
                                    .map(|(id, _)| *id)
                            })
                        })
                });
            pane = pick;
        }
        if pane.is_none() && action.contexts.iter().any(|context| context == "pane") {
            let focused = self.focused();
            if self.panes.contains_key(&focused) {
                pane = Some(focused);
            }
        }
        let space = self
            .workspace()
            .map(|workspace| {
                json!({
                    "id": format!("sp{}", workspace.id),
                    "name": workspace.label,
                    "machine": workspace.machine,
                    "folder": workspace.folder,
                })
            })
            .unwrap_or(Value::Null);
        let tab = self
            .workspace()
            .and_then(|workspace| workspace.tab())
            .map(|tab| json!({"id": format!("tb{}", tab.id)}))
            .unwrap_or(Value::Null);
        let Some(pane) = pane else {
            return (
                json!({
                    "space": space,
                    "tab": tab,
                    "pane": Value::Null,
                    "terminal": Value::Null,
                    "kind": "space",
                }),
                None,
            );
        };
        let state = self.panes.get(&pane);
        let target = json!({
            "space": space,
            "tab": tab,
            "pane": json!(pane),
            "terminal": state.and_then(|state| state.terminal.clone()),
            "kind": "pane",
        });
        (target, Some(pane))
    }

    fn plugin_context(
        &self,
        action: &ActionInfo,
        request: &str,
        args: Value,
        target: Value,
        _pane: Option<u32>,
    ) -> Value {
        json!({
            "api_version": 1,
            "request": request,
            "package": action.package,
            "action": action.action,
            "args": args,
            "context": {
                "space": target.get("space"),
                "tab": target.get("tab"),
                "pane": target.get("pane"),
                "terminal": target.get("terminal"),
                "target": target,
            },
            "machine": {
                "client": client_host(),
                "core": client_host(),
                "workspace": self.workspace_machine(self.active_ws),
            },
            "locale": if crate::i18n::zh() { "zh" } else { "en" },
        })
    }

    fn prepare_plugin_dirs(&self, action: &ActionInfo) {
        let config_dir = self
            .config
            .config_path
            .parent()
            .map(|parent| parent.join("plugin-data").join(&action.package))
            .unwrap_or_else(|| std::path::PathBuf::from("plugin-data").join(&action.package));
        let state_dir = self
            .config
            .plugins_state_dir()
            .join("data")
            .join(&action.package);
        for dir in [config_dir, state_dir] {
            let _ = std::fs::create_dir_all(&dir);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            }
        }
    }

    fn plugin_env(&self, action: &ActionInfo, request: &str) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert("HCTUI_PLUGIN_ID".into(), action.package.clone());
        env.insert(
            "HCTUI_PLUGIN_ROOT".into(),
            action.root.display().to_string(),
        );
        env.insert(
            "HCTUI_PLUGIN_CONFIG_DIR".into(),
            self.config
                .config_path
                .parent()
                .map(|parent| parent.join("plugin-data").join(&action.package))
                .unwrap_or_else(|| std::path::PathBuf::from("plugin-data").join(&action.package))
                .display()
                .to_string(),
        );
        env.insert(
            "HCTUI_PLUGIN_STATE_DIR".into(),
            self.config
                .plugins_state_dir()
                .join("data")
                .join(&action.package)
                .display()
                .to_string(),
        );
        env.insert(
            "HCTUI_BIN".into(),
            std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "hc-tui".to_string()),
        );
        env.insert(
            "HCTUI_SOCKET".into(),
            crate::control::socket_path(&self.config)
                .display()
                .to_string(),
        );
        env.insert(
            "HCTUI_SESSION".into(),
            self.config
                .session
                .clone()
                .or_else(|| std::env::var("HC_SESSION").ok())
                .unwrap_or_default(),
        );
        env.insert(
            "HCTUI_CONFIG".into(),
            self.config.config_path.display().to_string(),
        );
        env.insert("HCTUI_CONTEXT_ID".into(), request.to_string());
        env.insert("HCTUI_HOST_API".into(), "1".to_string());
        env
    }

    /// 把结束的调用写进提示；失败附 stderr 末尾。
    pub fn poll_plugins(&mut self) {
        let finished = self.plugin_runner.take_finished();
        for request in finished {
            let Some(view) = self.plugin_runner.status(&request) else {
                continue;
            };
            let name = format!("{}.{}", view.package, view.action);
            let text = match view.status {
                crate::plugin::runner::Status::Done => t1("plugin {} done", &name),
                crate::plugin::runner::Status::Failed => {
                    let tail = trailing(&view.stderr, 3);
                    if tail.is_empty() {
                        t2(
                            "plugin {} failed (exit {})",
                            &name,
                            view.exit_code.unwrap_or(-1),
                        )
                    } else {
                        format!(
                            "{} · {}",
                            t2(
                                "plugin {} failed (exit {})",
                                &name,
                                view.exit_code.unwrap_or(-1)
                            ),
                            tail
                        )
                    }
                }
                crate::plugin::runner::Status::Cancelled => t1("plugin {} cancelled", &name),
                crate::plugin::runner::Status::TimedOut => t1("plugin {} timed out", &name),
                _ => continue,
            };
            self.note = text;
            self.dirty = true;
        }
    }

    /// 控制通道：查询一次调用。
    pub fn plugin_status(&self, body: &Value) -> Value {
        let request = body.get("request").and_then(Value::as_str).unwrap_or("");
        match self.plugin_runner.status(request) {
            Some(view) => json!({
                "request": view.request,
                "package": view.package,
                "action": view.action,
                "status": view.status.as_str(),
                "exit_code": view.exit_code,
                "detail": view.error,
                "stdout": view.stdout,
                "stderr": view.stderr,
                "queued_ms": view.queued_ms,
                "runtime_ms": view.runtime_ms,
            }),
            None => json!({"error": format!("unknown request {request}")}),
        }
    }

    /// 控制通道：取消一次调用。
    pub fn plugin_cancel(&self, body: &Value) -> Value {
        let request = body.get("request").and_then(Value::as_str).unwrap_or("");
        match self.plugin_runner.cancel(request) {
            Ok(()) => json!({"cancelled": true, "request": request}),
            Err(error) => json!({"error": error}),
        }
    }

    /// 控制通道：读取一次调用的日志。
    pub fn plugin_log(&self, body: &Value) -> Value {
        let request = body.get("request").and_then(Value::as_str).unwrap_or("");
        let lines = body
            .get("lines")
            .and_then(Value::as_u64)
            .map(|lines| lines as usize)
            .unwrap_or(100);
        match self.plugin_runner.log(request, lines) {
            Ok(log) => json!({"request": request, "log": log}),
            Err(error) => json!({"error": error}),
        }
    }
}

fn trailing(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join(" · ")
}

fn client_host() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| {
        std::fs::read_to_string("/etc/hostname")
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn app() -> App {
        let (_, events) = std::sync::mpsc::channel();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let root =
            std::env::temp_dir().join(format!("hctui-plugin-app-{}-{unique}", std::process::id()));
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

    fn install(app: &mut App, id: &str, script: &str) -> String {
        let dir = app.config.plugins_dir().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "manifest_version = 1\nid = \"{id}\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\ncontexts = [\"global\", \"pane\"]\nargv = [\"sh\", \"-c\", \"{script}\"]\ntimeout_ms = 5000\n"
            ),
        )
        .unwrap();
        app.plugins = crate::plugin::registry::Registry::load(&app.config);
        format!("{id}.run")
    }

    #[test]
    fn invoking_an_action_queues_and_reports_through_the_runner() {
        let mut app = app();
        let name = install(&mut app, "dev.example.probe", "echo hello-from-plugin");
        app.invoke_plugin(&name, json!({})).unwrap();
        let request = app.plugin_request.clone().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let view = app.plugin_runner.status(&request).unwrap();
            if view.status.finished() {
                assert_eq!(view.status.as_str(), "done");
                assert!(view.stdout.contains("hello-from-plugin"));
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.poll_plugins();
        assert!(app.note.contains("done"), "{}", app.note);
    }

    #[test]
    fn menu_items_carry_the_frozen_target_and_disappear_with_the_plugin() {
        let mut app = app();
        install(&mut app, "dev.example.probe", "true");
        let items = app.plugin_menu_items("pane", json!({"pane": 7, "terminal": "box/t7"}));
        assert_eq!(items.len(), 1);
        assert!(items[0].value.contains("\"pane\":7"), "{}", items[0].value);
        std::fs::remove_dir_all(app.config.plugins_dir().join("dev.example.probe")).unwrap();
        app.plugins = crate::plugin::registry::Registry::load(&app.config);
        assert!(app.plugin_menu_items("pane", json!({})).is_empty());
    }

    #[test]
    fn unknown_actions_are_rejected() {
        let mut app = app();
        assert!(app
            .invoke_plugin("dev.example.missing.run", json!({}))
            .is_err());
    }
}
