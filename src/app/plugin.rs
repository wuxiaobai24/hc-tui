//! App 侧的插件集成：动作调用、上下文冻结、结果提示、终端入口与控制通道查询。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};

use super::{App, ChooserItem, PopupPane};
use crate::config::Span;
use crate::i18n::{t, t1, t2};
use crate::layout::split::Axis;
use crate::model::{Launch, Tab};
use crate::plugin::manifest;
use crate::plugin::registry::{ActionInfo, Registry};
use crate::plugin::runner::{InvokeError, Spec};
use crate::plugin::terminal::{self, Plan, PlanError};

/// 一个落在 Pane 里的插件终端入口。
#[derive(Debug, Clone)]
pub struct PluginTerminal {
    /// 打开请求 ID（`tp3`）。
    pub request: String,
    /// `<package>.<terminal>`。
    pub name: String,
    pub package: String,
    pub terminal: String,
    pub title: String,
    pub generation: u64,
    pub root: PathBuf,
    pub machine: Option<String>,
    pub placement: String,
    /// 代际引用的键：`<name>#<pane>`。
    pub lease: String,
}

/// 一次终端入口请求的进展。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneStatus {
    Probing,
    Opening,
    Opened,
    Closed,
    Failed { kind: String, message: String },
}

impl PaneStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Probing => "probing",
            Self::Opening => "opening",
            Self::Opened => "opened",
            Self::Closed => "closed",
            Self::Failed { .. } => "failed",
        }
    }
}

/// 控制通道里可查询的一次终端入口请求。
#[derive(Debug, Clone)]
pub struct PaneRequest {
    pub request: String,
    pub name: String,
    pub machine: Option<String>,
    pub placement: String,
    pub pane: Option<u32>,
    pub terminal: Option<String>,
    pub status: PaneStatus,
}

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
        let mut env = self.plugin_env_base(&action.package, &action.root);
        env.insert("HCTUI_CONTEXT_ID".into(), request.to_string());
        env
    }

    /// 动作与终端共用的注入变量；`HCTUI_CONTEXT_ID` 由调用方补充。
    fn plugin_env_base(&self, package: &str, root: &Path) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert("HCTUI_PLUGIN_ID".into(), package.to_string());
        env.insert("HCTUI_PLUGIN_ROOT".into(), root.display().to_string());
        env.insert(
            "HCTUI_PLUGIN_CONFIG_DIR".into(),
            self.config
                .config_path
                .parent()
                .map(|parent| parent.join("plugin-data").join(package))
                .unwrap_or_else(|| std::path::PathBuf::from("plugin-data").join(package))
                .display()
                .to_string(),
        );
        env.insert(
            "HCTUI_PLUGIN_STATE_DIR".into(),
            self.config
                .plugins_state_dir()
                .join("data")
                .join(package)
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

    // ------------------------------------------------------------ 终端入口

    /// 控制通道：打开一个插件终端入口。需要目标机器探测时立即返回 `probing`。
    pub fn plugin_pane_open(&mut self, body: &Value) -> Value {
        match self.plugin_pane_open_inner(body) {
            Ok(value) => value,
            Err(error) => json!({"error": error.message, "error_kind": error.kind}),
        }
    }

    fn plugin_pane_open_inner(&mut self, body: &Value) -> std::result::Result<Value, PlanError> {
        let name = body
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(PlanError::new(
                terminal::UNAVAILABLE,
                t("plugin terminal name is required"),
            ));
        }
        let info = match self.plugins.terminal(&name) {
            Some(info) => info,
            None => {
                // 清单可能刚被改动：重载一次再判定。
                self.plugins = Registry::load(&self.config);
                self.plugins.terminal(&name).ok_or_else(|| {
                    PlanError::new(
                        terminal::UNAVAILABLE,
                        t1("unknown plugin terminal {}", name),
                    )
                })?
            }
        };
        if let Some(space) = body
            .get("space")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|space| !space.is_empty())
        {
            let index = self.plugin_workspace_index(space).ok_or_else(|| {
                PlanError::new(terminal::TARGET_GONE, t1("space {} is gone", space))
            })?;
            self.select_workspace(index);
        }
        let machine = body
            .get("machine")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                let machine = self.workspace_machine(self.active_ws);
                (machine != "local").then_some(machine)
            });
        let cwd = body
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.workspace()
                    .and_then(|workspace| workspace.folder.clone())
            });
        let placement = body
            .get("placement")
            .and_then(Value::as_str)
            .map(str::to_string);
        let plan = terminal::plan(
            &info,
            &terminal::Request {
                cwd,
                machine,
                placement,
            },
        )?;
        self.next_plugin_pane += 1;
        let request = format!("tp{}", self.next_plugin_pane);
        self.plugin_panes.insert(
            request.clone(),
            PaneRequest {
                request: request.clone(),
                name: plan.name.clone(),
                machine: plan.machine.clone(),
                placement: plan.placement.clone(),
                pane: None,
                terminal: None,
                status: PaneStatus::Probing,
            },
        );
        self.plugin_pane_order.push(request.clone());
        while self.plugin_pane_order.len() > 128 {
            if let Some(expired) = self.plugin_pane_order.first().cloned() {
                self.plugin_pane_order.remove(0);
                self.plugin_panes.remove(&expired);
            }
        }
        if let Some(machine) = &plan.machine {
            if !plan.programs.is_empty() {
                self.plugin_pending.insert(request.clone(), plan.clone());
                let tag = format!("plugin-probe:{request}");
                let params = terminal::probe_params(&plan.programs, machine, 20);
                self.call_rpc(&tag, "machine.exec", params);
                return Ok(json!({"request": request, "status": "probing"}));
            }
        } else {
            let missing = terminal::local_missing(&plan.programs);
            if !missing.is_empty() {
                let error = terminal::missing_error(None, &missing);
                self.plugin_fail(&request, &error);
                return Err(error);
            }
        }
        self.plugin_open_planned(&request, &plan)
    }

    /// 目标机器的探测结果。
    pub fn plugin_probe_reply(
        &mut self,
        request: &str,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) {
        let Some(plan) = self.plugin_pending.remove(request) else {
            return;
        };
        let machine = plan.machine.clone();
        let programs = plan.programs.clone();
        let missing = match result {
            Ok(value) => {
                let stdout = value
                    .get("stdout")
                    .and_then(Value::as_str)
                    .map(|text| {
                        STANDARD
                            .decode(text)
                            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                terminal::parse_missing(&stdout, &programs)
            }
            Err(error) => {
                let error = terminal::rpc_error(&error.kind, &error.message);
                self.plugin_fail(request, &error);
                self.note = error.message.clone();
                self.dirty = true;
                return;
            }
        };
        if !missing.is_empty() {
            let error = terminal::missing_error(machine.as_deref(), &missing);
            self.plugin_fail(request, &error);
            self.note = error.message.clone();
            self.dirty = true;
            return;
        }
        match self.plugin_open_planned(request, &plan) {
            Ok(value) => {
                self.note = t1(
                    "plugin terminal {} opened",
                    value["name"].as_str().unwrap_or(""),
                );
            }
            Err(error) => {
                self.plugin_fail(request, &error);
                self.note = error.message.clone();
            }
        }
        self.dirty = true;
    }

    /// 控制通道：查询一次终端入口请求。
    pub fn plugin_pane_status(&self, body: &Value) -> Value {
        let request = body.get("request").and_then(Value::as_str).unwrap_or("");
        match self.plugin_panes.get(request) {
            Some(pane) => json!({
                "request": pane.request,
                "name": pane.name,
                "machine": pane.machine,
                "placement": pane.placement,
                "pane": pane.pane,
                "terminal": pane.terminal,
                "status": pane.status.as_str(),
                "error": match &pane.status {
                    PaneStatus::Failed { message, .. } => json!(message),
                    _ => Value::Null,
                },
                "error_kind": match &pane.status {
                    PaneStatus::Failed { kind, .. } => json!(kind),
                    _ => Value::Null,
                },
            }),
            None => json!({"error": format!("unknown request {request}")}),
        }
    }

    /// 控制通道：按终端地址、插件名或 Pane 编号定位并聚焦。
    pub fn plugin_pane_focus(&mut self, body: &Value) -> Value {
        let target = plugin_target_argument(body);
        match self.resolve_plugin_terminal(&target) {
            Ok((pane, address)) => {
                if let Some(pane) = pane {
                    self.focus_pane(pane);
                } else if let Some(address) = address.clone() {
                    self.open_terminal(&address);
                }
                json!({"focused": true, "pane": pane, "terminal": address})
            }
            Err(error) => json!({"error": error.message, "error_kind": error.kind}),
        }
    }

    /// 控制通道：关闭承载插件终端的 Pane；程序继续在 core 里运行。
    pub fn plugin_pane_close(&mut self, body: &Value) -> Value {
        let target = plugin_target_argument(body);
        match self.resolve_plugin_terminal(&target) {
            Ok((Some(pane), address)) => {
                let request = self
                    .plugin_terminals
                    .get(&pane)
                    .map(|terminal| terminal.request.clone());
                self.detach_view_from_layout(pane);
                if let Some(request) = request {
                    if let Some(record) = self.plugin_panes.get_mut(&request) {
                        record.status = PaneStatus::Closed;
                    }
                }
                json!({"closed": true, "pane": pane, "terminal": address})
            }
            Ok((None, _)) => json!({
                "error": t1("terminal {} has no view in this client", target),
                "error_kind": terminal::UNAVAILABLE,
            }),
            Err(error) => json!({"error": error.message, "error_kind": error.kind}),
        }
    }

    fn plugin_fail(&mut self, request: &str, error: &PlanError) {
        self.plugin_pending.remove(request);
        if let Some(record) = self.plugin_panes.get_mut(request) {
            record.status = PaneStatus::Failed {
                kind: error.kind.to_string(),
                message: error.message.clone(),
            };
        }
    }

    fn plugin_open_planned(
        &mut self,
        request: &str,
        plan: &Plan,
    ) -> std::result::Result<Value, PlanError> {
        let terminal = PluginTerminal {
            request: request.to_string(),
            name: plan.name.clone(),
            package: plan.package.clone(),
            terminal: plan.terminal.clone(),
            title: plan.title.clone(),
            generation: plan.generation,
            root: plan.root.clone(),
            machine: plan.machine.clone(),
            placement: plan.placement.clone(),
            lease: String::new(),
        };
        let pane = self.spawn_plugin_terminal(&terminal, plan)?;
        let mut terminal = terminal;
        terminal.lease = format!("{}#{pane}", terminal.name);
        let _ = crate::plugin::lease::acquire(
            &self.config,
            &terminal.package,
            terminal.generation,
            &terminal.root,
            &terminal.lease,
        );
        self.plugin_terminals.insert(pane, terminal.clone());
        if let Some(record) = self.plugin_panes.get_mut(request) {
            record.pane = Some(pane);
            record.status = PaneStatus::Opening;
        }
        Ok(json!({
            "request": request,
            "status": "opening",
            "pane": pane,
            "name": terminal.name,
            "machine": terminal.machine,
            "placement": terminal.placement,
        }))
    }

    /// 按 placement 创建 Pane，再把启动参数与自有名称装上。
    fn spawn_plugin_terminal(
        &mut self,
        terminal: &PluginTerminal,
        plan: &Plan,
    ) -> std::result::Result<u32, PlanError> {
        let title = plan.title.clone();
        let pane = match plan.placement.as_str() {
            "right" | "down" => {
                let axis = if plan.placement == "down" {
                    Axis::Vertical
                } else {
                    Axis::Horizontal
                };
                let pane = self
                    .split_new(axis)
                    .ok_or_else(|| PlanError::new(terminal::UNAVAILABLE, t("no room to split")))?;
                self.spawn_pane(pane);
                pane
            }
            "float" => {
                let pane = self.split_new(Axis::Horizontal).ok_or_else(|| {
                    PlanError::new(terminal::UNAVAILABLE, t("no room to float a pane"))
                })?;
                self.spawn_pane(pane);
                let floated = self
                    .workspace_mut()
                    .and_then(crate::model::Workspace::tab_mut)
                    .map(|tab| tab.float(pane))
                    .unwrap_or(false);
                if !floated {
                    self.detach_view_from_layout(pane);
                    return Err(PlanError::new(
                        terminal::UNAVAILABLE,
                        t("no room to float a pane"),
                    ));
                }
                if let Some(tab) = self
                    .workspace_mut()
                    .and_then(crate::model::Workspace::tab_mut)
                {
                    tab.focus = pane;
                }
                self.floating_hidden = false;
                pane
            }
            "popup" => {
                let pane = self.alloc_pane();
                self.spawn_pane(pane);
                if let Some(state) = self.panes.get_mut(&pane) {
                    state.transient = true;
                }
                self.popup = Some(PopupPane {
                    pane,
                    width: Span::Percent(80),
                    height: Span::Percent(80),
                });
                pane
            }
            "zoomed" | "tab" => {
                let pane = self.alloc_pane();
                self.spawn_pane(pane);
                let id = self.alloc_tab();
                if let Some(workspace) = self.workspaces.get_mut(self.active_ws) {
                    let mut tab = Tab::new(id, pane);
                    tab.label = title.clone();
                    workspace.tabs.push(tab);
                    workspace.active = workspace.tabs.len() - 1;
                }
                self.zoom = plan.placement == "zoomed";
                pane
            }
            other => {
                return Err(PlanError::new(
                    terminal::UNAVAILABLE,
                    t1("unknown placement {}", other),
                ))
            }
        };
        // 上下文里的 Tab 必须是刚建好的这一个。
        let env = terminal::env(
            self.plugin_env_base(&plan.package, &plan.root),
            self.plugin_terminal_context(terminal, plan),
            &plan.placement,
        );
        let launch = Launch {
            argv: plan.argv.clone(),
            cwd: plan.cwd.clone(),
            on: plan.machine.clone(),
            env,
        };
        self.install_plugin_launch(pane, launch, &title);
        Ok(pane)
    }

    fn install_plugin_launch(&mut self, pane: u32, launch: Launch, title: &str) {
        if let Some(state) = self.panes.get_mut(&pane) {
            state.launch = Some(launch);
            state.name = Some(title.to_string());
        }
        self.mark_layout();
        self.dirty = true;
    }

    /// 终端程序的上下文文档：Space 为目标，附带机器身份。
    fn plugin_terminal_context(&self, terminal: &PluginTerminal, plan: &Plan) -> Value {
        let workspace = self.workspace();
        let space = workspace
            .map(|workspace| {
                json!({
                    "id": format!("sp{}", workspace.id),
                    "name": workspace.label,
                    "machine": plan.machine,
                    "folder": plan.cwd,
                })
            })
            .unwrap_or(Value::Null);
        let tab = workspace
            .and_then(|workspace| workspace.tab())
            .map(|tab| json!({"id": format!("tb{}", tab.id)}))
            .unwrap_or(Value::Null);
        json!({
            "api_version": 1,
            "package": plan.package,
            "terminal": plan.terminal,
            "request": terminal.request,
            "context": {
                "space": space,
                "tab": tab,
                "target": {"kind": "space", "space": space},
            },
            "machine": {
                "client": client_host(),
                "core": client_host(),
                "workspace": plan.machine.clone().unwrap_or_else(|| "local".to_string()),
            },
            "locale": if crate::i18n::zh() { "zh" } else { "en" },
        })
    }

    /// 终端 attach 之后写入标签；同时登记地址与状态。
    pub fn label_plugin_terminal(&mut self, pane: u32, address: &str) {
        let Some(terminal) = self.plugin_terminals.get(&pane).cloned() else {
            return;
        };
        let mut labels = json!({
            "hctui": terminal.title,
            "hctui.plugin": terminal.package,
            "hctui.terminal": terminal.terminal,
            "where": self.plugin_where(pane),
        });
        if let Some((space, tab)) = self.plugin_origin(pane) {
            labels["herdr.space"] = json!(space);
            labels["herdr.tab"] = json!(tab);
        }
        self.call_rpc(
            &format!("plugin-label:{pane}"),
            "terminal.label",
            json!({"terminal": address, "labels": labels}),
        );
        let package = terminal.package.clone();
        let id = terminal.terminal.clone();
        let title = terminal.title.clone();
        self.update_terminal(Some(address), |summary| {
            summary.plugin = Some(package);
            summary.plugin_terminal = Some(id);
            summary.name = Some(title);
        });
        if let Some(record) = self.plugin_panes.get_mut(&terminal.request) {
            record.terminal = Some(address.to_string());
            record.status = PaneStatus::Opened;
        }
    }

    /// Pane 关闭时释放代际引用；最后一个终端结束后回收已卸载包的代际。
    pub fn release_plugin_terminal(&mut self, pane: u32) {
        let Some(terminal) = self.plugin_terminals.remove(&pane) else {
            return;
        };
        if terminal.lease.is_empty() {
            return;
        }
        let removed =
            crate::plugin::lease::release(&self.config, &terminal.package, &terminal.lease);
        if !removed.is_empty() {
            self.note = t2(
                "plugin terminal closed; {} generations {} reclaimed",
                terminal.package,
                removed
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
    }

    fn plugin_where(&self, pane: u32) -> String {
        match self.locate(pane) {
            Some((workspace, tab)) => {
                let label = if self.workspaces[workspace].tabs[tab].label.is_empty() {
                    (tab + 1).to_string()
                } else {
                    self.workspaces[workspace].tabs[tab].label.clone()
                };
                format!("{} / {label}", self.workspace_label(workspace))
            }
            None => String::new(),
        }
    }

    /// 共享结构里的 Space 与 Tab 稳定 ID；没有来源时返回 `None`。
    fn plugin_origin(&self, pane: u32) -> Option<(String, String)> {
        let (workspace, tab) = self.locate(pane)?;
        let state = &self.workspaces[workspace];
        let space = state.origin.clone()?;
        let tab = state.tabs.get(tab)?.origin.clone()?;
        let local = |id: &str| {
            let machine = state.machine.as_deref().unwrap_or("");
            id.strip_prefix(&format!("{machine}/"))
                .unwrap_or(id)
                .to_string()
        };
        Some((local(&space), local(&tab)))
    }

    fn plugin_workspace_index(&self, id: &str) -> Option<usize> {
        self.workspaces.iter().position(|workspace| {
            format!("sp{}", workspace.id) == id
                || workspace.origin.as_deref() == Some(id)
                || workspace
                    .origin
                    .as_deref()
                    .and_then(|origin| origin.split_once('/'))
                    .is_some_and(|(_, local)| local == id)
        })
    }

    /// 地址、插件名或 Pane 编号 → 本实例里的 Pane 与终端地址。
    fn resolve_plugin_terminal(
        &self,
        target: &str,
    ) -> std::result::Result<(Option<u32>, Option<String>), PlanError> {
        let target = target.trim();
        if target.is_empty() {
            return Err(PlanError::new(
                terminal::UNAVAILABLE,
                t("terminal or name is required"),
            ));
        }
        if let Ok(id) = target.parse::<u32>() {
            if let Some(state) = self.panes.get(&id) {
                return Ok((Some(id), state.terminal.clone()));
            }
        }
        for (pane, terminal) in &self.plugin_terminals {
            let address = self
                .panes
                .get(pane)
                .and_then(|state| state.terminal.clone());
            if terminal.name == target
                || terminal.request == target
                || address.as_deref() == Some(target)
            {
                return Ok((Some(*pane), address));
            }
        }
        for terminal in &self.terminals {
            let name = terminal
                .plugin
                .as_ref()
                .zip(terminal.plugin_terminal.as_ref())
                .map(|(package, id)| format!("{package}.{id}"));
            if terminal.address == target || name.as_deref() == Some(target) {
                let pane = self.pane_by_terminal(&terminal.address);
                return Ok((pane, Some(terminal.address.clone())));
            }
        }
        Err(PlanError::new(
            terminal::TARGET_GONE,
            t1("plugin terminal {} is gone", target),
        ))
    }
}

/// `focus` / `close` 参数：终端地址、插件名或 Pane 编号。
fn plugin_target_argument(body: &Value) -> String {
    ["terminal", "name", "pane"]
        .iter()
        .find_map(|key| match body.get(*key) {
            Some(Value::String(text)) if !text.trim().is_empty() => Some(text.trim().to_string()),
            Some(Value::Number(number)) => Some(number.to_string()),
            _ => None,
        })
        .unwrap_or_default()
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

    fn install_terminal(app: &mut App, id: &str, placement: &str) -> String {
        let dir = app.config.plugins_dir().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "manifest_version = 1\nid = \"{id}\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[terminals]]\nid = \"ui\"\ntitle = \"Probe UI\"\nargv = [\"sh\", \"-c\", \"true\"]\nplacement = \"{placement}\"\n"
            ),
        )
        .unwrap();
        app.plugins = crate::plugin::registry::Registry::load(&app.config);
        format!("{id}.ui")
    }

    #[test]
    fn placements_create_panes_with_injected_environment() {
        for placement in ["tab", "right", "down", "float", "popup", "zoomed"] {
            let mut app = app();
            app.new_workspace(None);
            let name = install_terminal(&mut app, "dev.example.term", placement);
            let reply = app.plugin_pane_open(&json!({"name": name}));
            assert_eq!(reply["status"], "opening", "{placement}: {reply}");
            let pane = reply["pane"].as_u64().unwrap() as u32;
            let state = app.panes.get(&pane).expect("pane created");
            let launch = state.launch.as_ref().expect("launch set");
            assert_eq!(launch.env["HCTUI_PLACEMENT"], placement);
            assert_eq!(launch.env["HCTUI_PLUGIN_ID"], "dev.example.term");
            assert!(launch.env["HCTUI_CONTEXT"].contains("\"terminal\":\"ui\""));
            assert_eq!(state.name.as_deref(), Some("Probe UI"));
            if let Some((workspace, tab)) = app.locate(pane) {
                let tab_id = app.workspaces[workspace].tabs[tab].id;
                assert!(
                    launch.env["HCTUI_CONTEXT"].contains(&format!("\"tb{tab_id}\"")),
                    "{placement}: context tab mismatch"
                );
            }
            assert_eq!(
                app.plugin_panes[&format!("tp{}", app.next_plugin_pane)].placement,
                placement
            );
            let lease = app
                .config
                .plugins_state_dir()
                .join("leases")
                .join(format!("dev.example.term.{}.1.json", std::process::id()));
            assert!(lease.is_file(), "{placement} lease missing");
            assert!(app.plugin_terminals.contains_key(&pane));
            if placement == "popup" {
                assert_eq!(app.popup.map(|popup| popup.pane), Some(pane));
            }
            if placement == "zoomed" {
                assert!(app.zoom);
            }
        }
    }

    #[test]
    fn closing_a_pane_releases_the_generation_lease() {
        let mut app = app();
        app.new_workspace(None);
        let name = install_terminal(&mut app, "dev.example.lease", "right");
        let reply = app.plugin_pane_open(&json!({"name": name}));
        let pane = reply["pane"].as_u64().unwrap() as u32;
        let lease = crate::plugin::lease::live_generations(&app.config, "dev.example.lease");
        assert_eq!(lease, vec![1]);
        app.detach_view_from_layout(pane);
        assert!(!app.plugin_terminals.contains_key(&pane));
        assert!(
            crate::plugin::lease::live_generations(&app.config, "dev.example.lease").is_empty()
        );
    }

    #[test]
    fn unknown_names_and_targets_report_stable_kinds() {
        let mut app = app();
        let reply = app.plugin_pane_open(&json!({"name": "dev.example.missing.ui"}));
        assert_eq!(reply["error_kind"], "unavailable");
        let reply = app.plugin_pane_focus(&json!({"terminal": "peer/t99"}));
        assert_eq!(reply["error_kind"], "target_gone");
        let reply = app.plugin_pane_close(&json!({"terminal": "peer/t99"}));
        assert_eq!(reply["error_kind"], "target_gone");
        let reply = app.plugin_pane_open(&json!({"name": "", "space": "sp9"}));
        assert_eq!(reply["error_kind"], "unavailable");
    }

    #[test]
    fn a_stale_space_is_reported_as_target_gone() {
        let mut app = app();
        let name = install_terminal(&mut app, "dev.example.term", "tab");
        let reply = app.plugin_pane_open(&json!({"name": name, "space": "sp999"}));
        assert_eq!(reply["error_kind"], "target_gone");
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
