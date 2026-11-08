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
use crate::plugin::lease::{
    self, GenerationPin, ResourceOwner, RetentionEvidence, TerminalAddress,
};
use crate::plugin::manifest;
use crate::plugin::registry::{ActionInfo, Registry};
use crate::plugin::runner::{InvocationSpec, InvokeError};
use crate::plugin::terminal::{
    self, ExecutionTarget, Plan, PlanError, PreparedTerminal, TargetSelector,
};

/// 主动扫描注册表磁盘指纹的间隔；控制请求前还会无节流对齐一次。
const PLUGIN_SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

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
    /// 代际的 pin；Pane 关闭不释放，只有 core 报告终端结束才释放。
    pub pin: Option<GenerationPin>,
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
    /// 每帧调用：节流比较注册表指纹，变了就重新加载。
    pub fn poll_plugin_registry(&mut self) {
        let now = std::time::Instant::now();
        if self
            .plugin_scan_at
            .is_some_and(|last| now.duration_since(last) < PLUGIN_SCAN_INTERVAL)
        {
            return;
        }
        self.plugin_scan_at = Some(now);
        self.refresh_plugins_if_changed();
    }

    /// 磁盘指纹变化时重新加载注册表：`plugins enable|disable` 无需重启。
    pub fn refresh_plugins_if_changed(&mut self) {
        let revision = crate::plugin::registry::revision(&self.config);
        if revision == self.plugin_revision {
            return;
        }
        self.plugin_revision = revision;
        self.plugins = Registry::load(&self.config);
        // 模块撤下/恢复与动作、终端走同一次重载：先对齐外框，再对齐表面。
        self.sync_package_modules();
        self.sync_mod_surfaces();
        self.recover_region_modules();
        let at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        self.event_publisher.publish(&json!({
            "kind": "registry.changed",
            "at_ms": at_ms,
            "data": {
                "plugins": self.plugins.plugins.len(),
                "actions": self.plugins.actions().len(),
                "terminals": self.plugins.terminals().len(),
            },
        }));
        self.dirty = true;
    }

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
        if let Some(package) = crate::plugin::registry::disabled_package(&self.config, name) {
            anyhow::bail!(t1("plugin {} is disabled", &package));
        }
        let Some(action) = self.plugins.action(name) else {
            anyhow::bail!(t1("unknown plugin action {}", name));
        };
        let target_arg = args.as_object_mut().and_then(|map| map.remove("_target"));
        let args = match &action.args {
            Some(schema) => manifest::validate_args(schema, &args).map_err(anyhow::Error::msg)?,
            None => args,
        };
        let request = self.plugin_runner.allocate_request();
        let pin = lease::pin(
            &self.config,
            &action.package,
            action.generation,
            &action.root,
            ResourceOwner::Invocation(crate::plugin::lease::InvocationOwner {
                request: request.clone(),
            }),
        )
        .map_err(|error| anyhow::anyhow!(t1("cannot pin plugin generation: {}", error)))?;
        let (target, _pane) = self
            .plugin_target(&action, target_arg)
            .map_err(anyhow::Error::msg)?;
        let context = self.plugin_context(&action, &request, args, target);
        self.prepare_plugin_dirs(&action);
        let env = self.plugin_env(&action, &request);
        let spec = InvocationSpec {
            package: action.package.clone(),
            action: action.action.clone(),
            root: action.root.clone(),
            argv: action.argv.clone(),
            timeout: action.timeout,
            context,
            env,
            request: Some(request.clone()),
            pin: Some(pin),
            stdin_limit: crate::plugin::runner::MAX_STDIN,
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

    /// 磁盘上仍被本会话终端 pin 持有的地址（上一个进程留下的）。
    pub(crate) fn plugin_terminals_on_disk(&self) -> Vec<String> {
        let session = self.plugin_session();
        let mut out = Vec::new();
        for package in lease::packages(&self.config) {
            for pin in lease::pins(&self.config, &package) {
                if let ResourceOwner::Terminal(address) = &pin.owner {
                    // 别的会话的 pin 不参与本会话的终端列表对账。
                    if address.session != session {
                        continue;
                    }
                    out.push(if address.machine == "local" {
                        address.address.clone()
                    } else {
                        format!("{}/{}", address.machine, address.address)
                    });
                }
            }
        }
        out
    }

    /// 本实例的 core 会话名；pin 与回收证据都用它作身份。
    pub(crate) fn plugin_session(&self) -> String {
        self.config
            .session
            .clone()
            .or_else(|| std::env::var("HC_SESSION").ok())
            .unwrap_or_default()
    }

    /// core 报告某条终端结束：释放它的代际 pin，并回收没有持有者的代际。
    pub fn plugin_terminal_ended(&mut self, address: &str) {
        let session = self.plugin_session();
        let mut evidence = RetentionEvidence::default();
        evidence.ended(&TerminalAddress::from_core(&session, address));
        let report = lease::gc_all(&self.config, &evidence);
        let generations: Vec<u64> = report
            .iter()
            .flat_map(|(_, report)| report.generations.iter().copied())
            .collect();
        if !generations.is_empty() {
            self.note = t2(
                "plugin terminal ended; {} generations {} reclaimed",
                generations.len(),
                generations
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            );
            self.dirty = true;
        }
    }

    /// 上一个进程留下的终端 pin 与 core 的权威终端列表对账：
    /// 列表里已经不存在的终端才算结束，其余保守保留。
    pub fn reconcile_plugin_pins(&mut self, live: &std::collections::BTreeSet<String>) {
        if self.plugin_startup_pins.is_empty() {
            return;
        }
        let session = self.plugin_session();
        let mut evidence = RetentionEvidence::default();
        for address in std::mem::take(&mut self.plugin_startup_pins) {
            let address = TerminalAddress::from_core(&session, &address);
            if !live.contains(&address.key()) {
                evidence.ended(&address);
            }
        }
        if !evidence.is_empty() {
            lease::gc_all(&self.config, &evidence);
        }
    }

    /// 目标解析：请求里给了目标就只看目标，绝不回退到当前焦点。
    pub fn resolve_plugin_target(
        &self,
        selector: &TargetSelector,
    ) -> std::result::Result<ExecutionTarget, PlanError> {
        if let Some(raw) = selector.pane_or_terminal.as_deref() {
            let raw = raw.trim();
            let pane = raw
                .parse::<u32>()
                .ok()
                .filter(|pane| self.panes.contains_key(pane))
                .or_else(|| {
                    self.panes
                        .iter()
                        .find(|(_, state)| state.terminal.as_deref() == Some(raw))
                        .map(|(pane, _)| *pane)
                });
            if let Some(pane) = pane {
                return self.target_of_pane(pane, selector);
            }
            // 本实例没有视图：退回到 core 的终端列表，仍然只认这条终端。
            if let Some(summary) = self.terminals.iter().find(|item| item.address == raw) {
                let machine = raw
                    .split_once('/')
                    .map(|(machine, _)| machine.to_string())
                    .filter(|machine| machine != "local");
                // 本实例没有这条终端的视图：Space 只能取自 core 的归属，
                // 拿不到就不编造（动作上下文仍可用机器与 cwd，放置则会失败）。
                let mapped = summary
                    .space_id
                    .as_deref()
                    .and_then(|id| self.plugin_workspace_index(id));
                let (index, space, space_name) = match mapped {
                    Some(index) => (
                        Some(index),
                        format!("sp{}", self.workspaces[index].id),
                        self.workspaces[index].label.clone(),
                    ),
                    None => (
                        None,
                        summary.space_id.clone().unwrap_or_default(),
                        String::new(),
                    ),
                };
                return Ok(ExecutionTarget {
                    space,
                    space_name,
                    index,
                    tab: summary
                        .tab_id
                        .as_deref()
                        .map(|id| format!("tb{id}"))
                        .unwrap_or_default(),
                    machine: selector.machine.clone().or(machine),
                    cwd: selector.cwd.clone().or_else(|| summary.cwd.clone()),
                    pane: None,
                    terminal: Some(summary.address.clone()),
                });
            }
            return Err(PlanError::new(
                terminal::TARGET_GONE,
                t1("terminal or pane {} is gone", raw),
            ));
        }
        if let Some(space) = selector.space.as_deref() {
            let index = self.plugin_workspace_index(space).ok_or_else(|| {
                PlanError::new(terminal::TARGET_GONE, t1("space {} is gone", space))
            })?;
            return self.target_of_space(index, selector);
        }
        if selector.is_empty() {
            return Err(PlanError::new(
                terminal::TARGET_GONE,
                t("no target was given"),
            ));
        }
        let index = self.active_ws.min(self.workspaces.len().saturating_sub(1));
        self.target_of_space(index, selector)
    }

    fn target_of_pane(
        &self,
        pane: u32,
        selector: &TargetSelector,
    ) -> std::result::Result<ExecutionTarget, PlanError> {
        let (index, tab_index) = self.locate(pane).ok_or_else(|| {
            PlanError::new(
                terminal::TARGET_GONE,
                t1("pane {} is not in any space", pane),
            )
        })?;
        let workspace = &self.workspaces[index];
        let mut target = ExecutionTarget {
            space: format!("sp{}", workspace.id),
            space_name: workspace.label.clone(),
            index: Some(index),
            tab: workspace
                .tabs
                .get(tab_index)
                .map(|tab| format!("tb{}", tab.id))
                .unwrap_or_default(),
            machine: None,
            cwd: None,
            pane: Some(pane),
            terminal: self
                .panes
                .get(&pane)
                .and_then(|state| state.terminal.clone()),
        };
        target.machine = machine_of(selector.machine.as_deref(), workspace.machine.clone());
        target.cwd = selector.cwd.clone().or_else(|| workspace.folder.clone());
        Ok(target)
    }

    fn target_of_space(
        &self,
        index: usize,
        selector: &TargetSelector,
    ) -> std::result::Result<ExecutionTarget, PlanError> {
        let workspace = self
            .workspaces
            .get(index)
            .ok_or_else(|| PlanError::new(terminal::TARGET_GONE, t("the target space is gone")))?;
        let mut target = ExecutionTarget {
            space: format!("sp{}", workspace.id),
            space_name: workspace.label.clone(),
            index: Some(index),
            tab: workspace
                .tab()
                .map(|tab| format!("tb{}", tab.id))
                .unwrap_or_default(),
            machine: None,
            cwd: None,
            pane: None,
            terminal: None,
        };
        target.machine = machine_of(selector.machine.as_deref(), workspace.machine.clone());
        target.cwd = selector.cwd.clone().or_else(|| workspace.folder.clone());
        Ok(target)
    }

    fn plugin_target(
        &self,
        action: &ActionInfo,
        requested: Option<Value>,
    ) -> std::result::Result<(Value, Option<u32>), PlanError> {
        let selector = requested.as_ref().map(target_selector).unwrap_or_default();
        let target = if selector.is_empty() {
            let pane_context = action.contexts.iter().any(|context| context == "pane");
            if pane_context {
                let focused = self.focused();
                if !self.panes.contains_key(&focused) {
                    return Err(PlanError::new(
                        terminal::TARGET_GONE,
                        t("there is no focused pane to target"),
                    ));
                }
                self.target_of_pane(focused, &selector)?
            } else {
                let index = self.active_ws.min(self.workspaces.len().saturating_sub(1));
                self.target_of_space(index, &selector)?
            }
        } else {
            self.resolve_plugin_target(&selector)?
        };
        let space = json!({
            "id": target.space,
            "name": target.space_name,
            "machine": target.machine,
            "folder": target.cwd,
        });
        let tab = json!({"id": target.tab});
        let kind = if target.pane.is_some() {
            "pane"
        } else if target.terminal.is_some() {
            "terminal"
        } else {
            "space"
        };
        let value = json!({
            "space": space,
            "tab": tab,
            "pane": target.pane,
            "terminal": target.terminal,
            "kind": kind,
        });
        Ok((value, target.pane))
    }

    fn plugin_context(
        &self,
        action: &ActionInfo,
        request: &str,
        args: Value,
        target: Value,
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
                "workspace": target
                    .pointer("/space/machine")
                    .and_then(Value::as_str)
                    .unwrap_or("local"),
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
    pub(super) fn plugin_env_base(&self, package: &str, root: &Path) -> BTreeMap<String, String> {
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
        if let Some(package) = crate::plugin::registry::disabled_package(&self.config, &name) {
            return Err(PlanError::new(
                terminal::DISABLED,
                t1("plugin {} is disabled", &package),
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
        let text = |key: &str| {
            body.get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let selector = TargetSelector {
            pane_or_terminal: text("pane").or_else(|| text("terminal")),
            space: text("space"),
            machine: text("machine"),
            cwd: text("cwd"),
        };
        // 没给目标就用当前 Space；给了目标就只认目标，不回退。
        let target = if selector.is_empty() {
            let index = self.active_ws.min(self.workspaces.len().saturating_sub(1));
            self.target_of_space(index, &selector)?
        } else {
            self.resolve_plugin_target(&selector)?
        };
        let placement = text("placement");
        let prepared = terminal::prepare(&info, target, placement)?;
        self.next_plugin_pane += 1;
        let request = format!("tp{}", self.next_plugin_pane);
        let plan = prepared.plan.clone();
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
        // 程序是否存在由实际执行位置（core）回答，TUI 环境不参与判决。
        if !plan.programs.is_empty() {
            self.plugin_pending.insert(request.clone(), prepared);
            let tag = format!("plugin-probe:{request}");
            let params = terminal::probe_params(&plan.programs, plan.machine.as_deref(), 20);
            self.call_rpc(&tag, "machine.exec", params);
            return Ok(json!({"request": request, "status": "probing"}));
        }
        self.open_plugin_terminal(&request, prepared)
    }

    /// 目标机器的探测结果。
    pub fn plugin_probe_reply(
        &mut self,
        request: &str,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) {
        let Some(prepared) = self.plugin_pending.remove(request) else {
            return;
        };
        let machine = prepared.plan.machine.clone();
        let programs = prepared.plan.programs.clone();
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
        match self.open_plugin_terminal(request, prepared) {
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

    /// 放置冻结的目标：只验证它仍然存在，不重新解析。
    fn open_plugin_terminal(
        &mut self,
        request: &str,
        prepared: PreparedTerminal,
    ) -> std::result::Result<Value, PlanError> {
        let target = prepared.target.clone();
        let wanted = target.space.clone();
        let matches = |workspace: &crate::model::Workspace| format!("sp{}", workspace.id) == wanted;
        // 只验证冻结的 Space 还在：先看原下标，再看稳定 ID。
        let index = target
            .index
            .filter(|index| self.workspaces.get(*index).is_some_and(matches))
            .or_else(|| self.workspaces.iter().position(matches))
            .ok_or_else(|| PlanError::new(terminal::TARGET_GONE, t1("space {} is gone", wanted)))?;
        self.active_ws = index;
        self.zoom = false;
        if let Some(tab) = self.workspaces[index]
            .tabs
            .iter()
            .position(|tab| format!("tb{}", tab.id) == target.tab)
        {
            self.workspaces[index].active = tab;
        }
        let plan = prepared.plan;
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
            pin: None,
        };
        let pane = self.spawn_plugin_terminal(&terminal, &plan, &target)?;
        let mut terminal = terminal;
        // 地址要等 attach 才知道；先按请求登记，拿到 core 地址后 repin。
        if let Ok(pin) = lease::pin(
            &self.config,
            &terminal.package,
            terminal.generation,
            &terminal.root,
            self.pending_owner(request),
        ) {
            terminal.pin = Some(pin);
        }
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
        target: &ExecutionTarget,
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
        // 上下文里的 Tab 必须是 Pane 实际所在的那一个：`tab`/`zoomed` 放置会建新 Tab。
        let placed_tab = self
            .locate(pane)
            .map(|(index, tab)| format!("tb{}", self.workspaces[index].tabs[tab].id))
            .unwrap_or_else(|| target.tab.clone());
        let env = terminal::env(
            self.plugin_env_base(&plan.package, &plan.root),
            self.plugin_terminal_context(terminal, plan, target, &placed_tab),
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

    /// 终端程序的上下文文档：每个字段都来自冻结的目标。
    fn plugin_terminal_context(
        &self,
        terminal: &PluginTerminal,
        plan: &Plan,
        target: &ExecutionTarget,
        placed_tab: &str,
    ) -> Value {
        let space = json!({
            "id": target.space,
            "name": target.space_name,
            "machine": target.machine,
            "folder": target.cwd,
        });
        let tab = json!({"id": placed_tab});
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
                "workspace": target.machine.clone().unwrap_or_else(|| "local".to_string()),
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
        // 终端身份从“请求占位”升级为 core 地址；之后只有 core 证据能释放它。
        let target = ResourceOwner::Terminal(self.terminal_address(address));
        if let Some(entry) = self.plugin_terminals.get_mut(&pane) {
            if let Some(pin) = entry.pin.as_ref() {
                if pin.owner != target {
                    let from = pin.owner.clone();
                    if let Ok(pin) = lease::repin(
                        &self.config,
                        &entry.package,
                        entry.generation,
                        &entry.root,
                        &from,
                        target,
                    ) {
                        entry.pin = Some(pin);
                    }
                }
            }
        }
        if let Some(record) = self.plugin_panes.get_mut(&terminal.request) {
            record.terminal = Some(address.to_string());
            record.status = PaneStatus::Opened;
        }
    }

    /// 按请求 ID 构造 attach 之前的占位终端身份。
    fn pending_owner(&self, request: &str) -> ResourceOwner {
        ResourceOwner::Terminal(TerminalAddress {
            session: self.plugin_session(),
            machine: "local".to_string(),
            address: format!("pending/{request}"),
        })
    }

    /// core 地址 → 带会话与机器身份的终端地址。
    pub(crate) fn terminal_address(&self, address: &str) -> TerminalAddress {
        TerminalAddress::from_core(&self.plugin_session(), address)
    }

    /// 终端程序还没拿到地址就结束：占位 pin 已经可以释放。
    pub fn release_pending_plugin_pin(&mut self, pane: u32) {
        let Some(terminal) = self.plugin_terminals.get(&pane) else {
            return;
        };
        if let Some(pin) = terminal.pin.as_ref() {
            pin.release();
        }
    }

    /// Pane 关闭只断视图，终端 pin 保留到 core 报告结束。
    pub fn release_plugin_terminal(&mut self, pane: u32) {
        let Some(terminal) = self.plugin_terminals.remove(&pane) else {
            return;
        };
        let address = self
            .panes
            .get(&pane)
            .and_then(|state| state.terminal.clone());
        if let Some(address) = address.as_deref() {
            self.publish_event(
                "terminal.closed",
                Some(address),
                json!({"reason": "pane_closed"}),
            );
        }
        if terminal.pin.is_none() {
            return;
        }
        self.note = t1(
            "plugin terminal {} detached; its generation stays until the core ends the terminal",
            terminal.name,
        );
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

/// `_target` 参数 → 目标选择器。
fn target_selector(value: &Value) -> TargetSelector {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    TargetSelector {
        pane_or_terminal: text("pane_or_terminal").or_else(|| {
            value
                .get("pane")
                .and_then(Value::as_u64)
                .map(|pane| pane.to_string())
        }),
        space: text("space"),
        machine: text("machine"),
        cwd: text("cwd"),
    }
}

/// 显式机器参数优先；`local` 与空值都表示本机。
fn machine_of(explicit: Option<&str>, fallback: Option<String>) -> Option<String> {
    explicit
        .map(|machine| machine.trim().to_string())
        .or(fallback)
        .filter(|machine| !machine.is_empty() && machine != "local")
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

    /// 临时根目录：`<root>/config/config.toml` 的上一级。
    fn scratch(app: &App) -> PathBuf {
        app.config
            .config_path
            .parent()
            .and_then(Path::parent)
            .expect("config path has two levels")
            .to_path_buf()
    }

    /// 写一个安装来源目录；`terminal` 与 `action` 为 argv 数组的 TOML 文本。
    fn write_source(root: &Path, id: &str, terminal: &str, action: &str) -> PathBuf {
        let dir = root.join("source").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("payload.txt"), "payload-v1\n").unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "manifest_version = 1\nid = \"{id}\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[[actions]]\nid = \"run\"\ntitle = \"Run\"\ncontexts = [\"global\", \"pane\"]\nargv = {action}\ntimeout_ms = 20000\n\n[[terminals]]\nid = \"ui\"\ntitle = \"Probe UI\"\nargv = {terminal}\nplacement = \"right\"\n"
            ),
        )
        .unwrap();
        dir
    }

    fn installed_generation(app: &App, id: &str, generation: u64) -> PathBuf {
        crate::store::Store::open(app.config.plugins_state_dir()).generation_dir(id, generation)
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

    fn wait_for_file(path: &Path) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.is_empty() {
                    return text;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{} never appeared",
                path.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn placements_create_panes_with_injected_environment() {
        for placement in ["tab", "right", "down", "float", "popup", "zoomed"] {
            let mut app = app();
            app.new_workspace(None);
            let name = install_terminal(&mut app, "dev.example.term", placement);
            let reply = app.plugin_pane_open(&json!({"name": name}));
            let request = reply["request"].as_str().unwrap().to_string();
            // 程序检查交给 core：本机入口同样先进入探测。
            if reply["status"] == "probing" {
                app.plugin_probe_reply(&request, Ok(json!({"stdout": ""})));
            }
            assert_eq!(
                app.plugin_panes[&request].status.as_str(),
                "opening",
                "{placement}: {reply}"
            );
            let pane = app.plugin_panes[&request].pane.expect("pane created");
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
            assert_eq!(app.plugin_panes[&request].placement, placement);
            let pins = crate::plugin::lease::pins(&app.config, "dev.example.term");
            assert!(
                pins.iter().any(|pin| matches!(
                    &pin.owner,
                    crate::plugin::lease::ResourceOwner::Terminal(_)
                )),
                "{placement} terminal pin missing"
            );
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
    fn closing_a_pane_keeps_the_terminal_pin_until_core_ends_the_terminal() {
        let mut app = app();
        app.new_workspace(None);
        let name = install_terminal(&mut app, "dev.example.lease", "right");
        let reply = app.plugin_pane_open(&json!({"name": name}));
        let request = reply["request"].as_str().unwrap().to_string();
        if reply["status"] == "probing" {
            app.plugin_probe_reply(&request, Ok(json!({"stdout": ""})));
        }
        let pane = app.plugin_panes[&request].pane.expect("pane opened");
        assert_eq!(
            crate::plugin::lease::live_generations(&app.config, "dev.example.lease"),
            vec![1]
        );
        app.detach_view_from_layout(pane);
        assert!(!app.plugin_terminals.contains_key(&pane));
        // 关闭视图不是终端结束的证据。
        assert_eq!(
            crate::plugin::lease::live_generations(&app.config, "dev.example.lease"),
            vec![1]
        );
        // core 说终端结束：pin 才释放。
        app.plugin_terminal_ended("local/t7");
        assert!(crate::plugin::lease::pins(&app.config, "dev.example.lease").len() < 2);
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
    fn a_pane_close_does_not_release_the_generation_of_a_running_terminal() {
        let mut app = app();
        app.new_workspace(None);
        let root = scratch(&app);
        let trigger = root.join("trigger");
        let out = root.join("out.txt");
        let script = format!(
            "#!/bin/sh\nwhile [ ! -f {trigger} ]; do sleep 0.02; done\ncat ./payload.txt > {out}\n",
            trigger = trigger.display(),
            out = out.display()
        );
        let source = write_source(&root, "dev.example.keep", "[\"./run.sh\"]", "[\"true\"]");
        std::fs::write(source.join("run.sh"), script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                source.join("run.sh"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let record = crate::plugin::registry::install(
            &app.config,
            &source,
            crate::plugin::registry::EnablePolicy::Enable,
        )
        .unwrap();
        app.plugins = Registry::load(&app.config);
        let dir = installed_generation(&app, "dev.example.keep", record.generation);
        let reply = app.plugin_pane_open(&json!({"name": "dev.example.keep.ui"}));
        assert_ne!(reply["status"], "failed", "{reply}");
        let pane = reply["pane"].as_u64().expect("pane opened") as u32;

        // core 里的终端：真实进程，cwd 是代际目录。
        let mut child = std::process::Command::new(dir.join("run.sh"))
            .current_dir(&dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();

        // 只断视图，程序继续跑；随后卸载插件。
        app.release_plugin_terminal(pane);
        crate::plugin::registry::uninstall(&app.config, "dev.example.keep").unwrap();
        assert!(
            dir.is_dir(),
            "generation removed although the terminal still runs"
        );

        std::fs::write(&trigger, "go").unwrap();
        assert_eq!(wait_for_file(&out), "payload-v1\n");
        let _ = child.wait();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_queued_action_still_runs_after_the_plugin_is_uninstalled() {
        let mut app = app();
        let root = scratch(&app);
        let source = write_source(
            &root,
            "dev.example.queue",
            "[\"true\"]",
            "[\"sh\", \"-c\", \"sleep 0.5; cat ./payload.txt\"]",
        );
        let record = crate::plugin::registry::install(
            &app.config,
            &source,
            crate::plugin::registry::EnablePolicy::Enable,
        )
        .unwrap();
        app.plugins = Registry::load(&app.config);
        let dir = installed_generation(&app, "dev.example.queue", record.generation);
        app.plugin_runner = crate::plugin::runner::Runner::with_limits(
            app.config.plugins_state_dir(),
            crate::plugin::runner::Limits {
                global: 1,
                per_package: 1,
                queue: 4,
                output: 4096,
            },
        );
        app.invoke_plugin("dev.example.queue.run", json!({}))
            .unwrap();
        let running = app.plugin_request.clone().unwrap();
        app.invoke_plugin("dev.example.queue.run", json!({}))
            .unwrap();
        let queued = app.plugin_request.clone().unwrap();
        assert_eq!(
            app.plugin_runner.status(&queued).unwrap().status,
            crate::plugin::runner::Status::Queued
        );

        crate::plugin::registry::uninstall(&app.config, "dev.example.queue").unwrap();

        let view = wait_for_status(&app, &queued);
        assert_eq!(view.status, crate::plugin::runner::Status::Done, "{view:?}");
        assert!(view.stdout.contains("payload-v1"), "{:?}", view.stdout);
        assert!(dir.is_dir(), "queued action lost its cwd");
        let _ = wait_for_status(&app, &running);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn uninstalling_during_a_run_keeps_the_action_directory_contents() {
        let mut app = app();
        let root = scratch(&app);
        let source = write_source(
            &root,
            "dev.example.live",
            "[\"true\"]",
            "[\"sh\", \"-c\", \"sleep 0.5; pwd; cat ./payload.txt\"]",
        );
        let record = crate::plugin::registry::install(
            &app.config,
            &source,
            crate::plugin::registry::EnablePolicy::Enable,
        )
        .unwrap();
        app.plugins = Registry::load(&app.config);
        let dir = installed_generation(&app, "dev.example.live", record.generation);
        app.invoke_plugin("dev.example.live.run", json!({}))
            .unwrap();
        let request = app.plugin_request.clone().unwrap();
        crate::plugin::registry::uninstall(&app.config, "dev.example.live").unwrap();
        let view = wait_for_status(&app, &request);
        assert_eq!(view.status, crate::plugin::runner::Status::Done, "{view:?}");
        assert!(
            view.stdout.contains(dir.to_string_lossy().as_ref()),
            "{:?}",
            view.stdout
        );
        assert!(view.stdout.contains("payload-v1"), "{:?}", view.stdout);
        std::fs::remove_dir_all(root).ok();
    }

    /// 宿主退出要取消运行中的动作并清理进程组，而不是等三秒就走。
    #[test]
    fn shutdown_cancels_running_plugins_and_cleans_the_group() {
        let mut app = app();
        let root = scratch(&app);
        let source = write_source(
            &root,
            "dev.example.stop",
            "[\"true\"]",
            "[\"sh\", \"-c\", \"sleep 60 & echo $! > ./child.pid; sleep 60\"]",
        );
        let record = crate::plugin::registry::install(
            &app.config,
            &source,
            crate::plugin::registry::EnablePolicy::Enable,
        )
        .unwrap();
        app.plugins = Registry::load(&app.config);
        let dir = installed_generation(&app, "dev.example.stop", record.generation);
        app.invoke_plugin("dev.example.stop.run", json!({}))
            .unwrap();
        let request = app.plugin_request.clone().unwrap();
        let pidfile = dir.join("child.pid");
        let pid: u32 = wait_for_file(&pidfile).trim().parse().unwrap();
        app.shutdown();
        let view = app.plugin_runner.status(&request).expect("request exists");
        assert!(view.status.finished(), "{view:?}");
        assert!(!process_alive(pid), "descendant {pid} survived shutdown");
        std::fs::remove_dir_all(root).ok();
    }

    fn process_alive(pid: u32) -> bool {
        // SAFETY: signal 0 only checks for existence.
        unsafe { libc::kill(pid as i32, 0) == 0 }
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

    /// 造出两个 Space：A 本机，B 在 peer 上有 folder，并在 B 里放一个终端。
    fn two_spaces(app: &mut App) -> (u32, u32) {
        app.new_workspace(None);
        app.new_workspace(None);
        let local = 0;
        let remote = 1;
        app.workspaces[local].machine = None;
        app.workspaces[local].folder = Some("/home/local".into());
        app.workspaces[remote].machine = Some("peer".into());
        app.workspaces[remote].folder = Some("/srv/remote".into());
        let pane = app.workspaces[remote].tabs[0].panes()[0].0;
        if let Some(state) = app.panes.get_mut(&pane) {
            state.terminal = Some("peer/t7".into());
        }
        app.select_workspace(local);
        (pane, app.workspaces[remote].id)
    }

    #[test]
    fn an_action_targeted_at_another_space_uses_that_space() {
        let mut app = app();
        let (pane, _space) = two_spaces(&mut app);
        let root = scratch(&app);
        let source = write_source(
            &root,
            "dev.example.target",
            "[\"true\"]",
            "[\"sh\", \"-c\", \"cat\"]",
        );
        crate::plugin::registry::install(
            &app.config,
            &source,
            crate::plugin::registry::EnablePolicy::Enable,
        )
        .unwrap();
        app.plugins = Registry::load(&app.config);
        app.invoke_plugin(
            "dev.example.target.run",
            json!({"_target": {"pane_or_terminal": "peer/t7"}}),
        )
        .unwrap();
        let request = app.plugin_request.clone().unwrap();
        let view = wait_for_status(&app, &request);
        assert_eq!(view.status, crate::plugin::runner::Status::Done, "{view:?}");
        let context: serde_json::Value =
            serde_json::from_str(view.stdout.trim()).expect("context json");
        assert_eq!(context["machine"]["workspace"], "peer", "{context}");
        assert_eq!(context["context"]["space"]["folder"], "/srv/remote");
        assert_eq!(context["context"]["space"]["machine"], "peer");
        assert_eq!(context["context"]["pane"], pane);
        assert_eq!(context["context"]["terminal"], "peer/t7");
        std::fs::remove_dir_all(root).ok();
    }

    fn action_for_a_target(app: &mut App) -> String {
        let root = scratch(app);
        let source = write_source(
            &root,
            "dev.example.miss",
            "[\"true\"]",
            "[\"sh\", \"-c\", \"cat\"]",
        );
        crate::plugin::registry::install(
            &app.config,
            &source,
            crate::plugin::registry::EnablePolicy::Enable,
        )
        .unwrap();
        app.plugins = Registry::load(&app.config);
        "dev.example.miss.run".to_string()
    }

    #[test]
    fn a_missing_terminal_target_is_reported_as_target_gone() {
        let mut app = app();
        two_spaces(&mut app);
        let name = action_for_a_target(&mut app);
        let missing =
            app.invoke_plugin(&name, json!({"_target": {"pane_or_terminal": "peer/t404"}}));
        let error = format!("{}", missing.expect_err("unknown target must fail"));
        assert!(error.contains("target_gone"), "{error}");
        std::fs::remove_dir_all(scratch(&app)).ok();
    }

    #[test]
    fn a_missing_numeric_pane_is_rejected_instead_of_using_the_focus() {
        let mut app = app();
        two_spaces(&mut app);
        let name = action_for_a_target(&mut app);
        let missing = app.invoke_plugin(&name, json!({"_target": {"pane": 9999}}));
        let error = format!("{}", missing.expect_err("missing pane must fail"));
        assert!(error.contains("target_gone"), "{error}");
        std::fs::remove_dir_all(scratch(&app)).ok();
    }

    #[test]
    fn a_probe_keeps_the_target_space_even_if_the_user_switches() {
        let mut app = app();
        let (_pane, space) = two_spaces(&mut app);
        let name = install_terminal(&mut app, "dev.example.space", "right");
        let reply = app.plugin_pane_open(&json!({
            "name": name,
            "space": format!("sp{space}"),
            "machine": "peer",
        }));
        assert_eq!(reply["status"], "probing", "{reply}");
        let request = reply["request"].as_str().unwrap().to_string();
        // 探测期间切回本地 Space A。
        app.select_workspace(0);
        app.plugin_probe_reply(&request, Ok(json!({"stdout": ""})));
        let record = app.plugin_panes.get(&request).expect("record exists");
        let pane = record.pane.expect("pane opened");
        let (workspace, _) = app.locate(pane).expect("pane placed");
        assert_eq!(
            workspace, 1,
            "pane landed in Space {} instead of the target",
            workspace
        );
        std::fs::remove_dir_all(scratch(&app)).ok();
    }

    #[test]
    fn unknown_actions_are_rejected() {
        let mut app = app();
        assert!(app
            .invoke_plugin("dev.example.missing.run", json!({}))
            .is_err());
    }

    #[test]
    fn disabling_a_package_removes_its_actions_without_restart() {
        let mut app = app();
        let name = install(&mut app, "dev.example.gate", "true");
        assert!(app.plugins.owns(&name));
        assert!(!app.plugin_menu_items("global", json!({})).is_empty());

        // 另一个进程里执行 `hc-tui plugins disable`：只落盘，不重启 TUI。
        crate::plugin::registry::set_enabled(&app.config, "dev.example.gate", false).unwrap();
        app.refresh_plugins_if_changed();

        assert!(!app.plugins.owns(&name));
        assert!(app.plugin_menu_items("global", json!({})).is_empty());
        assert!(app.execute_action(&name, json!({})).is_err());

        crate::plugin::registry::set_enabled(&app.config, "dev.example.gate", true).unwrap();
        app.refresh_plugins_if_changed();
        assert!(app.plugins.owns(&name), "enable did not take effect");
        std::fs::remove_dir_all(scratch(&app)).ok();
    }

    #[test]
    fn invoking_a_disabled_package_returns_disabled() {
        let mut app = app();
        let name = install(&mut app, "dev.example.off", "true");
        crate::plugin::registry::set_enabled(&app.config, "dev.example.off", false).unwrap();
        app.refresh_plugins_if_changed();
        let reply = app.control_reply(&json!({"method": "do", "command": name, "args": {}}));
        assert_eq!(reply["error_kind"], json!("disabled"), "{reply}");
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or_default()
                .contains("disabled"),
            "{reply}"
        );

        // 终端入口闸门同样返回 disabled，而不是 unknown terminal。
        let terminal = install_terminal(&mut app, "dev.example.offterm", "tab");
        crate::plugin::registry::set_enabled(&app.config, "dev.example.offterm", false).unwrap();
        app.refresh_plugins_if_changed();
        let reply = app.plugin_pane_open(&json!({"name": terminal}));
        assert_eq!(reply["error_kind"], json!("disabled"), "{reply}");
        std::fs::remove_dir_all(scratch(&app)).ok();
    }

    #[test]
    fn an_invocation_already_queued_runs_to_completion_after_disable() {
        let mut app = app();
        let name = install(&mut app, "dev.example.inflight", "sleep 0.3; true");
        app.execute_action(&name, json!({})).unwrap();
        let request = app.plugin_request.clone().expect("request queued");

        crate::plugin::registry::set_enabled(&app.config, "dev.example.inflight", false).unwrap();
        app.refresh_plugins_if_changed();

        let view = wait_for_status(&app, &request);
        assert!(view.status.finished(), "running invocation was cut short");
        std::fs::remove_dir_all(scratch(&app)).ok();
    }
}
