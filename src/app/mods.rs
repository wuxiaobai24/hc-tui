//! App 侧的模块运行时集成：调用分发、状态快照、事件投递与表面生命周期。
//!
//! 引擎线程发来的 `host.call` 每轮循环在 [`App::pump_mods`] 里分发应答；
//! 数据变化时递增 revision 并发 `state.changed`。目标在请求时冻结：动作与
//! 文件方法用调用时解析出的稳定 ID 与机器，后续焦点变化不影响。

use std::collections::BTreeMap;
use std::path::PathBuf;

use crossterm::event::{KeyEvent, KeyEventKind, MouseEvent};
use nsh_engine::HostReply;
use nsh_platform_tui::Handled;
use ratatui_core::layout::Rect;
use serde_json::{json, Value};

use super::App;
use crate::i18n::{t, t1, t2};
use crate::mods::host::{self, ModHost, ParamsContext, PendingCall};
use crate::mods::{ModRuntime, SurfacePlace, SurfaceState, WindowPos};

/// 快照的全部域；`state.changed` 的全集与 `state` 的 `domains` 过滤共用。
pub const MOD_DOMAINS: &[&str] = &[
    "machines",
    "machineInfo",
    "spaces",
    "tabs",
    "panes",
    "agents",
    "terminals",
    "focused",
    "theme",
    "commands",
    "keybindings",
    "settings",
    "templates",
    "notifications",
];

impl App {
    /// 每轮循环：分发积压的宿主调用，发送待发事件。
    pub fn pump_mods(&mut self) {
        if self.config.no_mods {
            return;
        }
        self.mods.prune_requests();
        self.expire_mod_asks();
        // 先取 ID 再分发：调用的副作用可能打开或关闭表面。
        let ids: Vec<(String, String)> = self
            .mods
            .surfaces
            .iter()
            .filter(|surface| surface.interactive())
            .map(|surface| (surface.id.clone(), surface.mod_id.clone()))
            .collect();
        let mut dispatched = false;
        for (surface_id, mod_id) in ids {
            let calls = match self.mods.surface(&surface_id) {
                Some(surface) => surface.host.drain(),
                None => continue,
            };
            if !calls.is_empty() {
                dispatched = true;
            }
            for call in calls {
                self.dispatch_mod_call(&surface_id, &mod_id, call);
            }
        }
        self.flush_mod_events();
        if self.mod_toast_dirty {
            self.push_toast_props();
        }
        if dispatched {
            self.dirty = true;
        }
        let exited: Vec<String> = self
            .mods
            .surfaces
            .iter()
            .filter(|surface| surface.view.exit_requested())
            .map(|surface| surface.id.clone())
            .collect();
        for id in exited {
            self.mods_close(&id);
        }
    }

    /// 分发一条宿主调用。
    fn dispatch_mod_call(&mut self, surface_id: &str, mod_id: &str, call: PendingCall) {
        let staging = self
            .mods
            .surface(surface_id)
            .is_some_and(|s| s.state == SurfaceState::Staging);
        let surface_ok = self.mods.surface(surface_id).is_some();
        let granted = self.mods.granted(mod_id, &call.name);
        let name = call.name.clone();
        let data = call.data;
        let reply = call.reply;
        match name.as_str() {
            "surface" => {
                if let Some(reply) = reply {
                    let Some(surface) = self.mods.surface(surface_id) else {
                        return host::reject(Some(reply), "target_gone", "surface closed");
                    };
                    reply.resolve(json!({
                        "id": surface.id,
                        "generation": surface.generation,
                        "place": surface.place.as_str(),
                        "cols": surface.area.width,
                        "rows": surface.area.height,
                    }));
                }
            }
            "requestGrants" => {
                let names: Vec<String> = data
                    .get("grants")
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if names.is_empty() {
                    host::reject(reply, "invalid", "grants required");
                    return;
                }
                self.mods.request_grants(mod_id, &names);
                let granted_now = self.mods.grants_for(mod_id);
                let satisfied = names.iter().all(|name| granted_now.contains(name));
                if !satisfied {
                    self.note = t1(
                        "mod {} requests grants: {}",
                        format_args!("{}: {}", mod_id, names.join(", ")),
                    );
                }
                if let Some(reply) = reply {
                    reply.resolve(json!({"granted": granted_now}));
                }
                self.mods_dirty = true;
                self.prompt_mod_grants(mod_id);
            }
            _ if staging => {
                host::reject(reply, "staging", "surface is reloading");
            }
            _ if !surface_ok => {
                host::reject(reply, "target_gone", "surface closed");
            }
            _ if !granted => {
                host::reject(
                    reply,
                    "forbidden",
                    t1("method {} not granted", name).as_str(),
                );
            }
            "state" => {
                let domains = self.mod_state_domains(&data);
                let space = data.get("space").and_then(Value::as_str);
                let snapshot = self.mod_state_snapshot_in(&domains, space);
                if let Some(reply) = reply {
                    reply.resolve(snapshot);
                }
            }
            "theme" => {
                if let Some(reply) = reply {
                    reply.resolve(self.mod_theme_value());
                }
            }
            "run" => {
                let Some(action) = data.get("name").and_then(Value::as_str).to_owned() else {
                    host::reject(reply, "invalid", "name required");
                    return;
                };
                let args = data.get("args").cloned().unwrap_or_else(|| json!({}));
                let request = self.mods.begin_request(surface_id, action, args.clone());
                // 动作若发起 core RPC，应答前不能报 done；新出现的 inflight 就是它发的。
                let before: std::collections::BTreeSet<String> =
                    self.inflight.keys().cloned().collect();
                let result = self.execute_action(action, args);
                let in_flight: Vec<String> = self
                    .inflight
                    .keys()
                    .filter(|tag| !before.contains(*tag))
                    .cloned()
                    .collect();
                let confirmation_pending = self.prompt.is_some() || self.form.is_some();
                let error = result.as_ref().err().map(|error| error.to_string());
                let status = match (&result, confirmation_pending, in_flight.is_empty()) {
                    (Err(_), _, _) => "failed",
                    (Ok(()), true, _) => "confirmation",
                    (Ok(()), false, false) => "running",
                    (Ok(()), false, true) => "done",
                };
                if let Some(record) = self.mods.requests.get_mut(&request) {
                    record.status = status.into();
                    record.error = error.clone();
                }
                if confirmation_pending {
                    self.mod_confirm = Some((request.clone(), mod_id.to_string()));
                }
                if status == "running" {
                    for tag in in_flight {
                        self.mod_action_rpc
                            .insert(tag, (request.clone(), mod_id.to_string()));
                    }
                }
                let value = match &result {
                    Ok(()) => self.mod_setting_action_value(action.as_ref()),
                    Err(_) => None,
                };
                if let Some(reply) = reply {
                    let mut payload = json!({
                        "request": request,
                        "accepted": result.is_ok(),
                        "confirmation_pending": confirmation_pending,
                    });
                    if let Some(value) = value {
                        payload["value"] = value;
                    }
                    reply.resolve(payload);
                }
                if let Some(error) = &error {
                    self.note = error.clone();
                }
                if matches!(status, "done" | "failed") {
                    let mut payload = json!({"request": request, "status": status});
                    if let Some(error) = &error {
                        payload["error"] = json!(error);
                    }
                    self.send_mod_event(mod_id, "action.finished", &payload);
                }
                self.mods_dirty = true;
            }
            "request" => {
                let Some(request) = data.get("request").and_then(Value::as_str) else {
                    host::reject(reply, "invalid", "request required");
                    return;
                };
                match self.mods.requests.get(request) {
                    Some(record) => {
                        if let Some(reply) = reply {
                            reply.resolve(json!({
                                "status": record.status,
                                "error": record.error,
                            }));
                        }
                    }
                    None => host::reject(reply, "not_found", "unknown request"),
                }
            }
            "exec" => {
                let argv: Vec<String> = data
                    .get("argv")
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if argv.is_empty() {
                    host::reject(reply, "invalid", "argv required");
                    return;
                }
                let on = data.get("on").and_then(Value::as_str);
                match self.mod_target(on) {
                    ModTarget::Local => self.dispatch_local_exec(surface_id, reply, &argv, &data),
                    ModTarget::Remote(machine) => {
                        self.dispatch_remote_exec(reply, &argv, &data, &machine)
                    }
                }
            }
            "readFile" => {
                let on = data.get("on").and_then(Value::as_str);
                if let ModTarget::Remote(machine) = self.mod_target(on) {
                    // 远端文件走 core file.read；宿主记住应答目标。
                    self.dispatch_remote_read(reply, &data, &machine);
                    return;
                }
                let Some(path) = data.get("path").and_then(Value::as_str) else {
                    host::reject(reply, "invalid", "path required");
                    return;
                };
                let max = data
                    .get("max_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(1024 * 1024) as usize;
                let path = self.mod_local_path(path, &data);
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let truncated = bytes.len() > max;
                        let data = if truncated { &bytes[..max] } else { &bytes[..] };
                        if let Some(reply) = reply {
                            reply.resolve(json!({
                                "data": String::from_utf8_lossy(data),
                                "truncated": truncated,
                            }));
                        }
                    }
                    Err(_) => host::reject(reply, "not_found", "cannot read file"),
                }
            }
            "writeFile" => {
                let (Some(path), Some(content)) = (
                    data.get("path").and_then(Value::as_str),
                    data.get("data").and_then(Value::as_str),
                ) else {
                    host::reject(reply, "invalid", "path and data required");
                    return;
                };
                if content.len() > 1024 * 1024 {
                    host::reject(reply, "invalid", "write exceeds 1 MiB");
                    return;
                }
                let on = data.get("on").and_then(Value::as_str);
                if let ModTarget::Remote(machine) = self.mod_target(on) {
                    let mut params = json!({
                        "path": path,
                        "data": content,
                        "on": machine,
                    });
                    if let Some(expected) = data.get("expected") {
                        params["expected"] = expected.clone();
                    }
                    let tag = self.begin_mod_rpc("file.write", &params, reply);
                    self.call_rpc(&tag, "file.write", params);
                    return;
                }
                let path = self.mod_local_path(path, &data);
                match std::fs::write(&path, content) {
                    Ok(()) => {
                        if let Some(reply) = reply {
                            reply.resolve(json!({"written": content.len()}));
                        }
                    }
                    Err(_) => host::reject(reply, "invalid", "cannot write file"),
                }
            }
            "notify" => {
                let Some(title) = data.get("title").and_then(Value::as_str) else {
                    host::reject(reply, "invalid", "title required");
                    return;
                };
                let body = data.get("body").and_then(Value::as_str).unwrap_or("");
                let timeout = data.get("timeout").and_then(Value::as_u64).unwrap_or(4);
                let level = data.get("level").and_then(Value::as_str).unwrap_or("info");
                let id = self.push_mod_notification(
                    title,
                    body,
                    level,
                    timeout,
                    data.get("action").cloned(),
                    data.get("target").and_then(Value::as_str),
                    data.get("surface").and_then(Value::as_str),
                    data.get("status").and_then(Value::as_bool).unwrap_or(false),
                );
                if let Some(reply) = reply {
                    reply.resolve(json!({"id": id}));
                }
            }
            "dismiss" => {
                let Some(id) = data.get("id").and_then(Value::as_str) else {
                    host::reject(reply, "invalid", "id required");
                    return;
                };
                if !self.dismiss_notification(id) {
                    host::reject(reply, "not_found", "unknown notification");
                    return;
                }
                if let Some(reply) = reply {
                    reply.resolve(json!({}));
                }
            }
            "setProps" => {
                let (Some(target), Some(props)) = (
                    data.get("surface").and_then(Value::as_str),
                    data.get("props"),
                ) else {
                    host::reject(reply, "invalid", "surface and props required");
                    return;
                };
                let props = props.clone();
                let Some(surface) = self.mods.surface_mut(target) else {
                    host::reject(reply, "not_found", "no such surface");
                    return;
                };
                surface.props = props.clone();
                surface.host.send("props.changed", json!({"props": props}));
                self.dirty = true;
                if let Some(reply) = reply {
                    reply.resolve(json!({}));
                }
            }
            "ask" => {
                let props = data.get("props").cloned().unwrap_or_else(|| json!({}));
                let timeout_ms = data
                    .get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(30_000)
                    .clamp(1, 600_000);
                let target = match data.get("surface").and_then(Value::as_str) {
                    Some(target) => {
                        let Some(surface) = self.mods.surface_mut(target) else {
                            host::reject(reply, "not_found", "no such surface");
                            return;
                        };
                        surface.props = props.clone();
                        surface.host.send("props.changed", json!({"props": props}));
                        target.to_string()
                    }
                    None => {
                        let target_mod = data
                            .get("mod")
                            .and_then(Value::as_str)
                            .unwrap_or(mod_id)
                            .to_string();
                        let place = data
                            .get("kind")
                            .or_else(|| data.get("place"))
                            .and_then(Value::as_str)
                            .and_then(crate::mods::SurfacePlace::parse)
                            .unwrap_or(crate::mods::SurfacePlace::Dialog);
                        match self.open_mod_overlay(&target_mod, place, data.clone()) {
                            Ok(id) => id,
                            Err(error) => {
                                host::reject(reply, "invalid", &error);
                                return;
                            }
                        }
                    }
                };
                let id = format!("a{}", self.mods.next_request_id());
                self.mod_asks.push(crate::mods::ModAsk {
                    id,
                    surface: target,
                    asker: mod_id.to_string(),
                    reply,
                    deadline: std::time::Instant::now()
                        + std::time::Duration::from_millis(timeout_ms),
                });
                self.dirty = true;
            }
            "answer" => {
                let value = data.get("value").cloned().unwrap_or(Value::Null);
                if let Some(index) = self
                    .mod_asks
                    .iter()
                    .position(|ask| ask.surface == surface_id)
                {
                    let ask = self.mod_asks.remove(index);
                    if let Some(pending) = ask.reply {
                        pending.resolve(json!({"value": value.clone()}));
                    }
                }
                let payload = json!({"surface": surface_id, "value": value});
                self.emit_mod_event("overlay.result", &payload);
                self.dirty = true;
                if let Some(reply) = reply {
                    reply.resolve(json!({}));
                }
            }
            "overlay" => {
                let kind = data.get("kind").and_then(Value::as_str).unwrap_or("dialog");
                let Some(place) = SurfacePlace::parse(kind) else {
                    host::reject(reply, "invalid", "kind must be dialog, popup or window");
                    return;
                };
                if !matches!(
                    place,
                    SurfacePlace::Dialog | SurfacePlace::Popup | SurfacePlace::Window
                ) {
                    host::reject(reply, "invalid", "overlay kinds are dialog, popup, window");
                    return;
                }
                let target_mod = data
                    .get("mod")
                    .and_then(Value::as_str)
                    .unwrap_or(mod_id)
                    .to_string();
                match self.open_mod_overlay(&target_mod, place, data.clone()) {
                    Ok(id) => {
                        if let Some(reply) = reply {
                            reply.resolve(json!({"surface": id}));
                        }
                        self.dirty = true;
                    }
                    Err(error) => host::reject(reply, "invalid", &error),
                }
            }
            "closeOverlay" => {
                let Some(target) = data.get("surface").and_then(Value::as_str) else {
                    host::reject(reply, "invalid", "surface required");
                    return;
                };
                self.remember_mod_window(target);
                if !self.mods.close(target) {
                    host::reject(reply, "not_found", "no such overlay");
                    return;
                }
                if self.focused_mod.as_deref() == Some(target) {
                    self.focus_mod_surface(None);
                }
                if let Some(reply) = reply {
                    reply.resolve(json!({}));
                }
                self.dirty = true;
            }
            "focus" => {
                let Some(target) = data.get("surface").and_then(Value::as_str) else {
                    host::reject(reply, "invalid", "surface required");
                    return;
                };
                let Some(surface) = self.mods.surface(target) else {
                    host::reject(reply, "not_found", "no such surface");
                    return;
                };
                if surface.place == SurfacePlace::Toast {
                    host::reject(reply, "invalid", "toast surfaces do not take focus");
                    return;
                }
                self.focus_mod_surface(Some(target.to_string()));
                if let Some(reply) = reply {
                    reply.resolve(json!({}));
                }
            }
            "setAgentOrder" => {
                let addresses: Vec<String> = data
                    .get("addresses")
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                self.mod_agent_order = (!addresses.is_empty()).then_some(addresses);
                self.dirty = true;
                self.bump_mod_revision_domains(&["agents"]);
                if let Some(reply) = reply {
                    reply.resolve(json!({}));
                }
            }
            other => host::reject(reply, "not_found", t1("unknown method {}", other).as_str()),
        }
    }

    fn dispatch_local_exec(
        &mut self,
        surface_id: &str,
        reply: Option<HostReply>,
        argv: &[String],
        data: &Value,
    ) {
        let Some(surface) = self.mods.surface(surface_id) else {
            host::reject(reply, "target_gone", "surface closed");
            return;
        };
        if !surface.host.exec_enter() {
            host::reject(reply, "busy", "too many concurrent exec calls");
            return;
        }
        let inflight = self
            .mods
            .surface(surface_id)
            .map(|s| s.host.exec_inflight.clone())
            .expect("surface exists");
        let cwd = data
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| {
                self.workspace()
                    .and_then(|ws| ws.folder.clone())
                    .map(PathBuf::from)
            });
        let env: BTreeMap<String, String> = data
            .get("env")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let timeout = std::time::Duration::from_secs(
            data.get("timeout_s")
                .and_then(Value::as_u64)
                .unwrap_or(30)
                .min(30),
        );
        let Some(reply) = reply else {
            surface_host_exit(&self.mods, surface_id);
            return;
        };
        host::spawn_local_exec(reply, inflight, argv.to_vec(), cwd, timeout, env);
    }

    /// 远端 exec：经 core 的 machine.exec；应答在 on_reply 里转发。
    fn dispatch_remote_exec(
        &mut self,
        reply: Option<HostReply>,
        argv: &[String],
        data: &Value,
        machine: &str,
    ) {
        let cwd = data.get("cwd").and_then(Value::as_str).map(str::to_string);
        let line = argv.join(" ");
        let params = json!({"line": line, "cwd": cwd, "on": machine});
        let tag = self.begin_mod_rpc("machine.exec", &params, reply);
        self.call_rpc(&tag, "machine.exec", params);
    }

    /// 远端 readFile：经 core 的 file.read。
    fn dispatch_remote_read(&mut self, reply: Option<HostReply>, data: &Value, machine: &str) {
        let Some(path) = data.get("path").and_then(Value::as_str) else {
            host::reject(reply, "invalid", "path required");
            return;
        };
        let params = json!({"path": path, "on": machine, "machine": machine});
        let tag = self.begin_mod_rpc("file.read", &params, reply);
        self.call_rpc(&tag, "file.read", params);
    }

    /// 登记一个等待 core RPC 应答的模块调用；返回 tag。
    fn begin_mod_rpc(&mut self, method: &str, params: &Value, reply: Option<HostReply>) -> String {
        let tag = format!("mod-rpc-{}", self.mods.next_request_id());
        self.inflight
            .insert(tag.clone(), (method.to_string(), params.clone()));
        self.pending_mod_rpc
            .insert(tag.clone(), (method.to_string(), reply));
        tag
    }

    /// core RPC 应达时转发给宿主调用方；在 `on_reply` 里调用。
    pub fn settle_mod_rpc(&mut self, tag: &str, result: &Result<Value, String>) {
        if let Some((method, reply)) = self.pending_mod_rpc.remove(tag) {
            match (method.as_str(), result) {
                ("machine.exec", Ok(value)) => {
                    // core 返回 exit/output 形态；映射到宿主 exec 结果。
                    let code = value.get("exit").and_then(Value::as_i64).unwrap_or(0);
                    let stdout = value
                        .get("output")
                        .or_else(|| value.get("stdout"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if let Some(reply) = reply {
                        reply.resolve(json!({
                            "code": code,
                            "stdout": stdout,
                            "stderr": "",
                            "timed_out": false,
                            "truncated": false,
                        }));
                    }
                }
                ("file.read", Ok(value)) => {
                    let data = value
                        .get("content")
                        .or_else(|| value.get("data"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if let Some(reply) = reply {
                        reply.resolve(json!({"data": data, "truncated": false}));
                    }
                }
                ("file.write", Ok(value)) => {
                    if let Some(reply) = reply {
                        reply.resolve(json!({"written": value.get("written")}));
                    }
                }
                (_, Ok(_)) => {
                    if let Some(reply) = reply {
                        reply.resolve(json!({}));
                    }
                }
                (_, Err(error)) => {
                    host::reject(reply, "unavailable", error);
                }
            }
        }
    }

    /// 状态快照：按领域裁剪；panes 上限 512。
    pub fn mod_state_snapshot(&self, domains: &[&str]) -> Value {
        self.mod_state_snapshot_in(domains, None)
    }

    /// 带 Space 过滤的快照；`space` 为 `sp<n>` 时只给该 Space 的归属项。
    pub fn mod_state_snapshot_in(&self, domains: &[&str], space: Option<&str>) -> Value {
        let want = |domain: &str| domains.is_empty() || domains.contains(&domain);
        let connection = match &self.connection {
            crate::app::Connection::Ready { .. } => "ready",
            crate::app::Connection::Connecting => "connecting",
            _ => "offline",
        };
        let in_space = |id: &str| space.is_none_or(|filter| filter == id);
        let owners = self.mod_pane_owners();
        let mut snapshot = json!({
            "generation": self.mods.generations.values().copied().max().unwrap_or(0),
            "revision": self.mod_revision,
            "connection": connection,
        });
        let map = snapshot.as_object_mut().expect("snapshot object");
        if want("machines") {
            let mut machines: Vec<String> = self
                .workspaces
                .iter()
                .filter_map(|ws| ws.machine.clone())
                .collect();
            machines.sort();
            machines.dedup();
            map.insert("machines".into(), json!(machines));
        }
        if want("machineInfo") {
            let machines: Vec<Value> = self
                .machines
                .iter()
                .map(|machine| {
                    json!({
                        "name": machine.name,
                        "state": machine.state,
                        "address": machine.target,
                        "host": machine.host,
                    })
                })
                .collect();
            map.insert("machineInfo".into(), json!(machines));
        }
        if want("spaces") {
            let spaces: Vec<Value> = self
                .workspaces
                .iter()
                .filter(|ws| in_space(&format!("sp{}", ws.id)))
                .map(|ws| {
                    json!({
                        "id": format!("sp{}", ws.id),
                        "name": ws.label,
                        "machine": ws.machine,
                        "folder": ws.folder,
                        "tabs": ws.tabs.len(),
                    })
                })
                .collect();
            map.insert("spaces".into(), json!(spaces));
        }
        if want("tabs") {
            let tabs: Vec<Value> = self
                .workspaces
                .iter()
                .filter(|ws| in_space(&format!("sp{}", ws.id)))
                .flat_map(|ws| {
                    ws.tabs.iter().map(move |tab| {
                        json!({
                            "id": format!("tb{}", tab.id),
                            "space": format!("sp{}", ws.id),
                            "name": tab.label,
                            "settled": tab.settled,
                        })
                    })
                })
                .collect();
            map.insert("tabs".into(), json!(tabs));
        }
        if want("panes") {
            let panes: Vec<Value> = self
                .panes
                .iter()
                .filter(|(id, _)| {
                    owners
                        .get(id)
                        .map(|(space_id, _)| in_space(space_id))
                        .unwrap_or(true)
                })
                .take(512)
                .map(|(id, pane)| {
                    let (space_id, tab_id) = owners.get(id).cloned().unwrap_or_default();
                    json!({
                        "id": format!("{id}"),
                        "terminal": pane.terminal,
                        "title": pane.title,
                        "space": (!space_id.is_empty()).then_some(space_id),
                        "tab": (!tab_id.is_empty()).then_some(tab_id),
                        "label": pane.label(),
                        "cwd": pane.cwd,
                        "live": pane.is_live(),
                    })
                })
                .collect();
            map.insert("panes".into(), json!(panes));
        }
        // Space 过滤时，终端类域只留该 Space 里 Pane 持有的终端。
        let allowed: Option<std::collections::BTreeSet<String>> = space.map(|filter| {
            self.panes
                .iter()
                .filter(|(id, _)| {
                    owners
                        .get(id)
                        .is_some_and(|(space_id, _)| space_id == filter)
                })
                .filter_map(|(_, pane)| pane.terminal.clone())
                .collect()
        });
        let keep = |address: &str| allowed.as_ref().is_none_or(|set| set.contains(address));
        if want("agents") {
            let agents: Vec<Value> = self
                .terminals
                .iter()
                .filter(|terminal| keep(&terminal.address))
                .map(|terminal| {
                    json!({
                        "address": terminal.address,
                        "name": terminal.name,
                        "title": terminal.title,
                        "cwd": terminal.cwd,
                        "turn": terminal.turn,
                        "pinned": terminal.pinned,
                    })
                })
                .collect();
            map.insert("agents".into(), json!(agents));
        }
        if want("terminals") {
            let terminals: Vec<Value> = self
                .terminals
                .iter()
                .filter(|terminal| keep(&terminal.address))
                .map(|terminal| {
                    json!({
                        "address": terminal.address,
                        "title": terminal.title,
                        "cwd": terminal.cwd,
                        "seq": terminal.turn,
                    })
                })
                .collect();
            map.insert("terminals".into(), json!(terminals));
        }
        if want("focused") {
            map.insert(
                "focused".into(),
                json!({
                    "space": self.workspace().map(|ws| format!("sp{}", ws.id)),
                    "tab": self
                        .workspace()
                        .and_then(|ws| ws.tab())
                        .map(|tab| format!("tb{}", tab.id)),
                    "pane": Some(format!("{}", self.focused())),
                    "mod": self.focused_mod,
                }),
            );
        }
        if want("theme") {
            map.insert("theme".into(), self.mod_theme_value());
        }
        if want("commands") {
            let mut commands: Vec<Value> = super::actions::COMMANDS
                .iter()
                .map(|(name, title)| json!({"name": name, "title": title, "source": "builtin"}))
                .collect();
            for action in self.plugins.actions() {
                commands.push(json!({
                    "name": action.name,
                    "title": action.title,
                    "source": format!("plugin:{}", action.package),
                    "description": action.description,
                    "contexts": action.contexts,
                    "args": action.args,
                }));
            }
            map.insert("commands".into(), json!(commands));
        }
        if want("keybindings") {
            let bindings: std::collections::HashMap<&str, crate::config::Binding> =
                self.config.keys.entries().into_iter().collect();
            let mut rows = Vec::new();
            for (group, names) in App::HELP_GROUPS {
                for name in names {
                    let Some(binding) = bindings.get(name) else {
                        continue;
                    };
                    rows.push(json!({
                        "name": name,
                        "keys": binding.to_string().trim_matches('"'),
                        "title": App::binding_title(name),
                        "group": group,
                        "off": binding.is_off(),
                    }));
                }
            }
            map.insert("keybindings".into(), json!(rows));
        }
        if want("settings") {
            let mut items = Vec::new();
            for (section, name) in App::SETTINGS_SECTIONS.iter().enumerate() {
                for index in 0..self.settings_len(section) {
                    let value = self.mod_setting_value(section, index);
                    let (label, active) = self.mod_setting_item(section, index);
                    items.push(json!({
                        "id": format!("{name}:{value}"),
                        "value": value,
                        "label": label,
                        "active": active,
                    }));
                }
            }
            let delivery = match self.config.notifications {
                crate::config::Delivery::Off => "off",
                crate::config::Delivery::Toast => "toast",
                crate::config::Delivery::System => "system",
            };
            map.insert(
                "settings".into(),
                json!({
                    "section": App::SETTINGS_SECTIONS[0],
                    "sections": App::SETTINGS_SECTIONS
                        .iter()
                        .map(|name| json!({"name": name, "label": t(name)}))
                        .collect::<Vec<Value>>(),
                    "items": items,
                    "current": {
                        "theme": self.config.theme.name,
                        "indicators": if self.config.indicators == crate::config::Indicators::Symbols { "symbols" } else { "dots" },
                        "notifications": delivery,
                        "sound": self.config.sound,
                        "zellij_keys": self.config.zellij_keys,
                    },
                }),
            );
        }
        if want("templates") {
            map.insert("templates".into(), json!(self.mod_templates));
        }
        if want("notifications") {
            let items: Vec<Value> = self
                .mod_notifications
                .iter()
                .map(|item| {
                    json!({
                        "id": item.id,
                        "level": item.level,
                        "text": item.text,
                        "created": item.created,
                        "action": item.action,
                        "target": item.target,
                        "surface": item.surface,
                    })
                })
                .collect();
            map.insert("notifications".into(), json!(items));
        }
        snapshot
    }

    /// Pane 归属：pane 编号 → (space ID, tab ID)；外框内与浮动 Pane 都算。
    fn mod_pane_owners(&self) -> BTreeMap<u32, (String, String)> {
        let mut owners = BTreeMap::new();
        for ws in &self.workspaces {
            let space = format!("sp{}", ws.id);
            for tab in &ws.tabs {
                let tab_id = format!("tb{}", tab.id);
                for pane in tab.panes() {
                    owners.insert(pane.0, (space.clone(), tab_id.clone()));
                }
            }
        }
        owners
    }

    /// 设置类动作执行后的新值；随 `run` 应答返回，模块不必再读一次快照。
    fn mod_setting_action_value(&self, action: &str) -> Option<Value> {
        match action {
            "app.theme" => Some(json!(self.config.theme.name)),
            "app.indicators" => Some(json!(if self.config.indicators
                == crate::config::Indicators::Symbols
            {
                "symbols"
            } else {
                "dots"
            })),
            "app.notifications" => Some(json!(match self.config.notifications {
                crate::config::Delivery::Off => "off",
                crate::config::Delivery::Toast => "toast",
                crate::config::Delivery::System => "system",
            })),
            "app.sound" => Some(json!(self.config.sound)),
            _ => None,
        }
    }

    /// 设置项的落盘值；与内置设置面板、模块拿到的域保持同一口径。
    fn mod_setting_value(&self, section: usize, index: usize) -> String {
        match section {
            0 => crate::theme::names()[index].to_string(),
            1 => if index == 1 { "symbols" } else { "dots" }.to_string(),
            2 => ["off", "toast", "system"][index.min(2)].to_string(),
            _ => if index == 1 { "on" } else { "off" }.to_string(),
        }
    }

    /// 设置项文案与生效状态；与内置设置面板、模块拿到的域保持同一口径。
    fn mod_setting_item(&self, section: usize, index: usize) -> (String, bool) {
        let value = self.mod_setting_value(section, index);
        let active = match section {
            0 => value == self.config.theme.name,
            1 => {
                (self.config.indicators == crate::config::Indicators::Symbols)
                    == (value == "symbols")
            }
            2 => matches!(
                (self.config.notifications, value.as_str()),
                (crate::config::Delivery::Off, "off")
                    | (crate::config::Delivery::Toast, "toast")
                    | (crate::config::Delivery::System, "system")
            ),
            _ => self.config.sound == (value == "on"),
        };
        let label = if section == 0 {
            value.clone()
        } else {
            let name = match section {
                1 => ["dots", "symbols"][index.min(1)],
                2 => ["off", "toast", "system"][index.min(2)],
                _ => ["off", "on"][index.min(1)],
            };
            t(name).to_string()
        };
        (label, active)
    }

    /// `state` 调用的领域过滤。
    fn mod_state_domains(&self, data: &Value) -> Vec<&'static str> {
        match data.get("domains").and_then(Value::as_array) {
            Some(list) => list
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|domain| MOD_DOMAINS.iter().copied().find(|known| *known == domain))
                .collect(),
            None => Vec::new(),
        }
    }

    /// 主题 token 与调色板。
    pub fn mod_theme_value(&self) -> Value {
        let theme = &self.config.theme;
        let tokens: serde_json::Map<String, Value> = theme
            .tokens()
            .map(|(name, color)| (name.to_string(), json!(color_hex(color))))
            .collect();
        let palette: Vec<Value> = theme
            .terminal_palette()
            .iter()
            .map(|color| json!(color_hex(*color)))
            .collect();
        json!({"name": theme.name, "tokens": tokens, "palette": palette})
    }

    /// 命令行 `notify` 的入口：与模块 `notify` 共用登记、路由与回退。
    ///
    /// 返回 `(id, 命中的提示表面)`；表面为 `None` 时已走内置 toast。
    pub(crate) fn host_notify(
        &mut self,
        text: &str,
        level: &str,
        action: Option<Value>,
        target: Option<&str>,
    ) -> (String, Option<String>) {
        let routed = self.resolve_toast_surface(None);
        let id = self.push_mod_notification(text, "", level, 4, action, target, None, false);
        (id, routed)
    }

    /// 点击内置 toast：执行它携带的动作，没有动作就跳到绑定目标。
    pub(crate) fn activate_toast(&mut self) {
        let Some(id) = self.toast_notification.clone() else {
            return;
        };
        let Some(item) = self
            .mod_notifications
            .iter()
            .find(|item| item.id == id)
            .cloned()
        else {
            return;
        };
        self.dismiss_notification(&id);
        let mut args = match &item.action {
            Some(Value::Object(map)) => match map.get("args") {
                Some(Value::Object(args)) => Value::Object(args.clone()),
                _ => json!({}),
            },
            _ => json!({}),
        };
        let name = match &item.action {
            Some(Value::String(name)) => Some(name.clone()),
            Some(Value::Object(map)) => map.get("name").and_then(Value::as_str).map(str::to_string),
            _ => None,
        };
        let name = match (name, item.target.clone()) {
            (Some(name), _) => name,
            // 没有动作时，点击后只剩「跳到绑定终端」这一种语义。
            (None, Some(target)) => {
                args["terminal"] = json!(target);
                "agent.focus".to_string()
            }
            (None, None) => return,
        };
        if args.get("terminal").is_none() {
            if let Some(target) = item.target.clone() {
                args["terminal"] = json!(target);
            }
        }
        self.note = match self.execute_action(&name, args) {
            Ok(()) => t1("notification action {} accepted", &name),
            Err(error) => t2("notification action {} failed: {}", &name, error),
        };
        self.dirty = true;
    }

    /// 通知队列：登记条目并写进现有 toast 或状态行。
    #[allow(clippy::too_many_arguments)]
    fn push_mod_notification(
        &mut self,
        title: &str,
        body: &str,
        level: &str,
        timeout: u64,
        action: Option<Value>,
        target: Option<&str>,
        surface: Option<&str>,
        status: bool,
    ) -> String {
        let text = if body.is_empty() {
            title.to_string()
        } else {
            format!("{title} · {body}")
        };
        self.register_notification(
            text,
            level,
            timeout.max(1) * 1000,
            action,
            target,
            surface,
            status,
        )
    }

    /// 宿主侧登记一条通知：解析归属表面、计时并写进队列。
    ///
    /// 命中提示表面时只走模块队列；否则保持内置 toast 或状态行行为。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn register_notification(
        &mut self,
        text: String,
        level: &str,
        timeout_ms: u64,
        action: Option<Value>,
        target: Option<&str>,
        surface: Option<&str>,
        status: bool,
    ) -> String {
        let id = format!("n{}", self.mods.next_request_id());
        let routed = self.resolve_toast_surface(surface);
        self.mod_notifications.push(crate::mods::ModNotification {
            id: id.clone(),
            level: level.to_string(),
            text: text.clone(),
            action,
            target: target.map(str::to_string),
            surface: routed.clone().or_else(|| surface.map(str::to_string)),
            created: crate::store::now_unix(),
            created_ms: crate::store::now_millis(),
            timeout_ms,
            since: std::time::Instant::now(),
        });
        // 队列只保留最近 200 条，旧条目被挤掉后 `dismiss` 返回 `not_found`。
        while self.mod_notifications.len() > 200 {
            self.mod_notifications.remove(0);
        }
        if routed.is_some() {
            self.mod_toast_dirty = true;
        } else if status {
            self.note = text;
        } else {
            self.toast = Some(crate::app::Toast {
                text,
                level: level.to_string(),
                until: std::time::Instant::now()
                    + std::time::Duration::from_millis(timeout_ms.max(1)),
            });
            self.toast_notification = Some(id.clone());
        }
        self.bump_mod_revision_domains(&["notifications"]);
        self.dirty = true;
        id
    }

    /// 按 id 撤掉一条通知；返回是否找到。文本只用于显示。
    pub(crate) fn dismiss_notification(&mut self, id: &str) -> bool {
        let Some(index) = self.mod_notifications.iter().position(|item| item.id == id) else {
            return false;
        };
        self.mod_notifications.remove(index);
        // toast 槽位按登记的通知 ID 对应，不按文本比较。
        if self.toast_notification.as_deref() == Some(id) {
            self.toast = None;
            self.toast_notification = None;
        }
        self.mod_toast_dirty = true;
        self.bump_mod_revision_domains(&["notifications"]);
        self.dirty = true;
        true
    }

    /// 提示表面：外框声明了 `toasts` 节点且表面可交互。
    fn toast_surfaces(&self) -> Vec<(String, String)> {
        if self.config.no_mods || !self.chrome.has_toasts() {
            return Vec::new();
        }
        self.mods
            .surfaces
            .iter()
            .filter(|surface| surface.place == SurfacePlace::Toast && surface.interactive())
            .map(|surface| (surface.id.clone(), surface.mod_id.clone()))
            .collect()
    }

    /// 通知归属的提示表面：`surface` 按表面 ID 或模块 ID 命中；未指定取默认。
    fn resolve_toast_surface(&self, requested: Option<&str>) -> Option<String> {
        let surfaces = self.toast_surfaces();
        match requested {
            Some(name) => surfaces
                .iter()
                .find(|(id, mod_id)| id == name || mod_id == name)
                .map(|(id, _)| id.clone()),
            None => surfaces.first().map(|(id, _)| id.clone()),
        }
    }

    /// 把队列写进每条提示表面的 props；通道与 `setProps` 相同。
    pub(crate) fn push_toast_props(&mut self) {
        let targets = self.toast_surfaces();
        if targets.is_empty() {
            self.mod_toast_dirty = false;
            return;
        }
        let limit = self.config.toast_limit.max(1);
        let default = targets[0].0.clone();
        for (surface_id, _) in targets {
            let mut toasts: Vec<Value> = self
                .mod_notifications
                .iter()
                .filter(|item| match &item.surface {
                    Some(id) => id == &surface_id,
                    None => surface_id == default,
                })
                .map(|item| {
                    let mut value = json!({
                        "id": item.id,
                        "level": item.level,
                        "text": item.text,
                        "created_ms": item.created_ms,
                        "timeout_ms": item.timeout_ms,
                    });
                    if let Some(action) = &item.action {
                        value["action"] = action.clone();
                    }
                    if let Some(target) = &item.target {
                        value["target"] = json!(target);
                    }
                    value
                })
                .collect();
            // 只留最新几条，显示顺序由模块决定。
            if toasts.len() > limit {
                toasts.drain(..toasts.len() - limit);
            }
            let props = json!({"toasts": toasts, "max": limit});
            if let Some(surface) = self.mods.surface_mut(&surface_id) {
                surface.props = props.clone();
                surface.host.send("props.changed", json!({"props": props}));
            }
        }
        self.mod_toast_dirty = false;
        self.dirty = true;
    }

    /// 超时的通知由宿主撤掉，模块只负责显示。
    pub(crate) fn expire_mod_notifications(&mut self, now: std::time::Instant) {
        let expired: Vec<String> = self
            .mod_notifications
            .iter()
            .filter(|item| {
                item.timeout_ms > 0
                    && now.saturating_duration_since(item.since).as_millis() as u64
                        >= item.timeout_ms
            })
            .map(|item| item.id.clone())
            .collect();
        if expired.is_empty() {
            return;
        }
        self.mod_notifications
            .retain(|item| !expired.contains(&item.id));
        if self
            .toast_notification
            .as_ref()
            .is_some_and(|id| expired.contains(id))
        {
            self.toast = None;
            self.toast_notification = None;
        }
        self.mod_toast_dirty = true;
        self.bump_mod_revision_domains(&["notifications"]);
        self.dirty = true;
    }

    /// `ask` 超时或目标表面已关：应答错误，不静默丢弃。
    fn expire_mod_asks(&mut self) {
        if self.mod_asks.is_empty() {
            return;
        }
        let now = std::time::Instant::now();
        let mut kept = Vec::new();
        for ask in std::mem::take(&mut self.mod_asks) {
            if now >= ask.deadline {
                host::reject(ask.reply, "timeout", "ask timed out");
            } else if self.mods.surface(&ask.surface).is_none() {
                host::reject(ask.reply, "target_gone", "ask surface closed");
            } else {
                kept.push(ask);
            }
        }
        self.mod_asks = kept;
    }

    /// `run` 打开的确认结束：写终态并发 `action.finished`。
    pub(crate) fn settle_mod_confirm(&mut self, status: &str) {
        let Some((request, mod_id)) = self.mod_confirm.take() else {
            return;
        };
        if let Some(record) = self.mods.requests.get_mut(&request) {
            record.status = status.to_string();
        }
        self.send_mod_event(
            &mod_id,
            "action.finished",
            &json!({"request": request, "status": status}),
        );
        self.dirty = true;
    }

    /// `run` 发起的 core RPC 应达：写终态并发 `action.finished`。
    pub(crate) fn settle_mod_action_rpc(&mut self, tag: &str, result: &Result<Value, String>) {
        let Some((request, mod_id)) = self.mod_action_rpc.remove(tag) else {
            return;
        };
        // 同一动作可能有并发 RPC：一条应答就收尾，其余 tag 一并丢弃。
        self.mod_action_rpc.retain(|_, (id, _)| id != &request);
        let (status, error) = match result {
            Ok(_) => ("done".to_string(), None),
            Err(message) => ("failed".to_string(), Some(message.clone())),
        };
        if let Some(record) = self.mods.requests.get_mut(&request) {
            record.status = status.clone();
            record.error = error.clone();
        }
        let mut payload = json!({"request": request, "status": status});
        if let Some(error) = &error {
            payload["error"] = json!(error);
            self.note = error.clone();
        }
        self.send_mod_event(&mod_id, "action.finished", &payload);
        self.dirty = true;
    }

    /// 把焦点交给一条模块表面（`None` 回到 Pane）。
    pub fn focus_mod_surface(&mut self, surface: Option<String>) {
        if self.focused_mod == surface {
            return;
        }
        // 提示表面不抢焦点：请求无效，当前焦点保持不变。
        if let Some(id) = &surface {
            if self
                .mods
                .surface(id)
                .is_some_and(|target| target.place == SurfacePlace::Toast)
            {
                return;
            }
        }
        let targets: Vec<String> = self.mods.surfaces.iter().map(|s| s.id.clone()).collect();
        for id in targets {
            if let Some(surface) = self.mods.surface_mut(&id) {
                let was = surface.focused;
                surface.focused = false;
                if was {
                    surface.view.set_focused(false);
                }
            }
        }
        self.focused_mod = None;
        if let Some(id) = surface {
            if let Some(target) = self.mods.surface_mut(&id) {
                target.focused = true;
                target.view.set_focused(true);
                self.focused_mod = Some(id);
            }
        }
        self.dirty = true;
        self.bump_mod_revision_domains(&["focused"]);
        self.send_focus_changed();
    }

    /// `mods.open`：为模块开一条表面。
    pub fn mods_open(
        &mut self,
        mod_id: &str,
        place: SurfacePlace,
        props: Value,
        size: Option<(u16, u16)>,
        at: Option<(u16, u16)>,
        key: Option<String>,
    ) -> Result<String, String> {
        if self.config.no_mods {
            return Err(t("mods are disabled (--no-mods)").to_string());
        }
        if place == SurfacePlace::Toast && !self.chrome.has_toasts() {
            return Err(t("chrome has no toasts block").to_string());
        }
        let package = self.mod_package(mod_id)?;
        if !package.enabled || !self.mods.is_enabled(mod_id) {
            return Err(t1("mod {} is disabled", mod_id));
        }
        // 首次加载：按包记录的 `[grants]` 批准本地可信模块的授予；已批准的
        // 记录以 state.json 为准，模块不能自己扩大范围。
        if !self.mods.grants.contains_key(mod_id) && !package.grants.is_empty() {
            self.mods.approve_grants(mod_id, &package.grants);
            self.seen_dirty = true;
            self.persist_mod_grants(mod_id);
        }
        let bundle = Self::read_bundle(&package)?;
        let id = self.mods.next_surface_id();
        let generation = *self.mods.generations.get(mod_id).unwrap_or(&0) + 1;
        let host = ModHost::new(&id, mod_id);
        let params = ParamsContext {
            mod_id: mod_id.to_string(),
            surface: id.clone(),
            generation,
            place: place.as_str(),
            props: props.clone(),
            space: self
                .workspace()
                .map(|ws| {
                    json!({
                        "id": format!("sp{}", ws.id),
                        "name": ws.label,
                        "machine": ws.machine,
                        "folder": ws.folder,
                    })
                })
                .unwrap_or(Value::Null),
            tab: self
                .workspace()
                .and_then(|ws| ws.tab())
                .map(|tab| json!({"id": format!("tb{}", tab.id)}))
                .unwrap_or(Value::Null),
            pane: if matches!(place, SurfacePlace::Pane) {
                json!({"id": format!("{}", self.focused())})
            } else {
                Value::Null
            },
            client: client_host(),
            core: client_host(),
            workspace: self
                .workspace()
                .and_then(|ws| ws.machine.clone())
                .unwrap_or_default(),
            cell: [8, 16],
            locale: if crate::i18n::zh() { "zh" } else { "en" },
            theme: self.mod_theme_value(),
            hc_tui: env!("CARGO_PKG_VERSION").to_string(),
        };
        host.set_params(host::build_params(&params));
        let mut options = self.mods.options(Some(self.mod_wake_callback()));
        options.host = host.channel.clone();
        let mut view = nsh_platform_tui::AppView::new(options);
        let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = errors.clone();
        view.on_error(move |message| {
            sink.lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(message.to_string());
        });
        view.load(bundle).map_err(|error| error.to_string())?;
        let area = self.initial_surface_area(place, size, at, key.as_deref());
        let store_generation = self.mod_store_generation(mod_id);
        let surface = self
            .mods
            .open_surface(id.clone(), mod_id, place, view, key, host);
        surface.area = area;
        surface.errors = errors;
        surface.state = SurfaceState::Running;
        surface.store_generation = store_generation;
        surface.props = props;
        if place == SurfacePlace::Toast {
            // 提示层固定形态：不抢焦点、不是模态、不参与 `Esc`、不吃鼠标。
            surface.modal = false;
            surface.autofocus = false;
            surface.esc_to_host = false;
            surface.click_through = true;
            self.mod_toast_dirty = true;
        } else {
            self.focus_mod_surface(Some(id.clone()));
        }
        if place == SurfacePlace::Toast {
            // 模块接手绘制：内置槽位里属于这条表面的那条不再另画一份。
            let default = self.resolve_toast_surface(None);
            let taken = self.toast_notification.as_deref().is_some_and(|id| {
                self.mod_notifications
                    .iter()
                    .find(|item| item.id == id)
                    .is_some_and(|item| match &item.surface {
                        None => true,
                        Some(surface) => Some(surface) == default.as_ref(),
                    })
            });
            if taken {
                self.toast = None;
                self.toast_notification = None;
            }
        }
        self.dirty = true;
        Ok(id)
    }

    /// 关闭一条表面，或某模块的全部表面。
    pub fn mods_close(&mut self, target: &str) -> bool {
        let targets: Vec<(String, String, SurfacePlace)> = self
            .mods
            .surfaces
            .iter()
            .filter(|surface| surface.id == target || surface.mod_id == target)
            .map(|surface| (surface.id.clone(), surface.mod_id.clone(), surface.place))
            .collect();
        let mut closed = false;
        let mut closed_mods: Vec<String> = Vec::new();
        for (id, mod_id, place) in targets {
            self.remember_mod_window(&id);
            if self.mods.close(&id) {
                closed = true;
                closed_mods.push(mod_id.clone());
                // 关闭停靠区表面同时解除外框绑定，区域不留空占位。
                if place == SurfacePlace::Region && self.chrome.undock(&mod_id) {
                    self.seen_dirty = true;
                }
            }
        }
        if closed {
            if self
                .focused_mod
                .as_deref()
                .is_some_and(|id| self.mods.surface(id).is_none())
            {
                self.focus_mod_surface(None);
            }
            self.dirty = true;
            self.bump_mod_revision_domains(&["focused"]);
            for mod_id in closed_mods {
                self.prune_mod_generations(&mod_id);
            }
        }
        closed
    }

    /// 当前安装代际；未安装的模块返回 `None`。
    fn mod_store_generation(&self, mod_id: &str) -> Option<u64> {
        for state in [
            self.config.plugins_state_dir(),
            self.config.mods_state_dir(),
        ] {
            if let Some(record) = crate::store::Store::open(state).record(mod_id) {
                return Some(record.generation);
            }
        }
        None
    }

    /// 回收没有表面引用的旧代际；当前代际总保留。
    ///
    /// 插件商店的代际由 [`crate::plugin::lease`] 按 pin 回收：正在运行的终端与
    /// 已排队的动作都持有 pin，这里若一并 prune 会删掉它们仍在读的代码。
    fn prune_mod_generations(&self, mod_id: &str) {
        let live: Vec<u64> = self
            .mods
            .surfaces
            .iter()
            .filter(|surface| surface.mod_id == mod_id)
            .filter_map(|surface| surface.store_generation)
            .collect();
        crate::store::Store::open(self.config.mods_state_dir()).prune(mod_id, &live);
    }

    /// 授予写回持久化文件，供 CLI 与下次启动使用。
    pub fn persist_mod_grants(&self, mod_id: &str) {
        let grants = self.mods.grants_for(mod_id);
        let _ = crate::mods::store::save_grants(&self.config, mod_id, &grants);
    }

    /// staging 重载：新实例加载成功并完成离屏首帧后原子替换。
    pub fn mods_reload(&mut self, mod_id: &str) -> Result<(), String> {
        let Some(surface_id) = self
            .mods
            .surfaces
            .iter()
            .find(|s| s.mod_id == mod_id)
            .map(|s| s.id.clone())
        else {
            return Err(t1("mod {} has no surface", mod_id));
        };
        let package = self.mod_package(mod_id)?;
        let bundle = Self::read_bundle(&package)?;
        let host_channel = self
            .mods
            .surface(&surface_id)
            .map(|s| s.host.channel.clone())
            .expect("surface exists");
        let mut options = self.mods.options(Some(self.mod_wake_callback()));
        options.host = host_channel;
        let mut staging = nsh_platform_tui::AppView::new(options);
        let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = errors.clone();
        staging.on_error(move |message| {
            sink.lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(message.to_string());
        });
        staging.load(bundle).map_err(|error| error.to_string())?;
        // 离屏首帧：TestBackend 上渲染一次，成功才替换。
        let area = self
            .mods
            .surface(&surface_id)
            .map(|s| s.area)
            .unwrap_or(ratatui_core::layout::Rect::new(0, 0, 40, 12));
        let rect = ratatui_core::layout::Rect::new(0, 0, area.width.max(4), area.height.max(3));
        let mut terminal = ratatui_core::terminal::Terminal::new(
            ratatui_core::backend::TestBackend::new(rect.width, rect.height),
        )
        .map_err(|error| error.to_string())?;
        terminal
            .draw(|frame| staging.render(frame, rect))
            .map_err(|error| error.to_string())?;
        let generation = {
            let generation = self.mods.generations.entry(mod_id.to_string()).or_insert(0);
            *generation += 1;
            *generation
        };
        let store_generation = self.mod_store_generation(mod_id);
        let Some(surface) = self.mods.surface_mut(&surface_id) else {
            return Err(t1("mod {} has no surface", mod_id));
        };
        surface.generation = generation;
        surface.view = staging;
        surface.errors = errors;
        surface.state = SurfaceState::Running;
        surface.fails = 0;
        surface.last_error = None;
        surface.store_generation = store_generation;
        self.dirty = true;
        Ok(())
    }

    /// 按包记录解析出的运行期形态：代码根、能力、模块与授予。
    fn mod_package(&self, mod_id: &str) -> Result<crate::pkg::Resolved, String> {
        crate::pkg::resolve(&self.config, mod_id)?.ok_or_else(|| t1("no package {}", mod_id))
    }

    /// 从记录里的模块摘要读 bundle；相对路径按该代际目录解析。
    fn read_bundle(package: &crate::pkg::Resolved) -> Result<nsh_platform_tui::Bundle, String> {
        let bundle_path = package
            .bundle_path()
            .ok_or_else(|| t1("package {} declares no module", &package.id))?;
        if !bundle_path.is_file() {
            return Err(t1(
                "mod {} has no bundle; run `nsh build -o dist` in its directory",
                &package.id,
            ));
        }
        nsh_platform_tui::Bundle::read(&bundle_path).map_err(|error| error.to_string())
    }

    fn initial_surface_area(
        &self,
        place: SurfacePlace,
        size: Option<(u16, u16)>,
        at: Option<(u16, u16)>,
        key: Option<&str>,
    ) -> ratatui_core::layout::Rect {
        let content = self.content;
        match place {
            SurfacePlace::Region | SurfacePlace::Pane | SurfacePlace::Toast => content,
            SurfacePlace::Dialog => {
                let (w, h) = size.unwrap_or((64, 16));
                crate::mods::render::dialog_rect(content, w, h)
            }
            SurfacePlace::Popup => {
                let (w, h) = size.unwrap_or((40, 10));
                crate::mods::render::popup_rect(
                    content,
                    at.unwrap_or((content.x + 2, content.y + 2)),
                    w,
                    h,
                )
            }
            SurfacePlace::Window => {
                let (w, h) = size.unwrap_or((50, 14));
                if let Some(pos) = key.and_then(|key| self.mods.windows.get(key)) {
                    return ratatui_core::layout::Rect::new(pos.x, pos.y, pos.cols, pos.rows);
                }
                let mut rect = crate::mods::render::dialog_rect(content, w, h);
                // 级联偏移，避免叠死在中心。
                let offset = (self.mods.windows.len() as u16 * 2) % 20;
                rect.x = rect.x.saturating_add(offset);
                rect.y = rect.y.saturating_add(offset);
                rect
            }
        }
    }

    /// 打开参数：`modal`、`autofocus`、`onEsc`、`clickThrough`；默认与既有行为一致。
    pub fn apply_surface_options(&mut self, surface_id: &str, data: &Value) {
        let toast = self
            .mods
            .surface(surface_id)
            .is_some_and(|surface| surface.place == SurfacePlace::Toast);
        if toast {
            return;
        }
        let mut unfocus = false;
        if let Some(surface) = self.mods.surface_mut(surface_id) {
            if let Some(modal) = data.get("modal").and_then(Value::as_bool) {
                surface.modal = modal;
            }
            if let Some(autofocus) = data.get("autofocus").and_then(Value::as_bool) {
                surface.autofocus = autofocus;
                unfocus = !autofocus;
            }
            if let Some(on_esc) = data.get("onEsc").and_then(Value::as_str) {
                surface.esc_to_host = on_esc != "module";
            }
            if let Some(click_through) = data.get("clickThrough").and_then(Value::as_bool) {
                surface.click_through = click_through;
            }
        }
        if unfocus && self.focused_mod.as_deref() == Some(surface_id) {
            self.focus_mod_surface(None);
        }
        self.dirty = true;
    }

    /// `mods.open` 的覆盖层入口（dialog/popup/window）。
    pub fn open_mod_overlay(
        &mut self,
        mod_id: &str,
        place: SurfacePlace,
        data: Value,
    ) -> Result<String, String> {
        let size = match (
            data.get("width").and_then(Value::as_u64),
            data.get("height").and_then(Value::as_u64),
        ) {
            (Some(w), Some(h)) => {
                Some((w.min(u16::MAX as u64) as u16, h.min(u16::MAX as u64) as u16))
            }
            _ => None,
        };
        let at = data
            .get("at")
            .and_then(Value::as_array)
            .filter(|list| list.len() == 2)
            .map(|list| {
                (
                    list[0].as_u64().unwrap_or(0).min(u16::MAX as u64) as u16,
                    list[1].as_u64().unwrap_or(0).min(u16::MAX as u64) as u16,
                )
            });
        let key = data.get("key").and_then(Value::as_str).map(str::to_string);
        let props = data.get("props").cloned();
        // 覆盖层的 props 走 props.changed 事件（模块已按启动参数收到首份）。
        let id = self.mods_open(
            mod_id,
            place,
            props.clone().unwrap_or_else(|| json!({})),
            size,
            at,
            key,
        )?;
        self.apply_surface_options(&id, &data);
        if let (Some(props), Some(surface)) = (props, self.mods.surface(&id)) {
            surface.host.send("props.changed", json!({"props": props}));
        }
        Ok(id)
    }

    /// 数据变化：递增 revision，节流发送 `state.changed`。
    pub fn bump_mod_revision(&mut self) {
        self.mod_all_domains = true;
        self.mod_revision += 1;
        self.mods_dirty = true;
    }

    /// 只有部分域变化：递增 revision 并记下域，`state.changed` 如实上报。
    pub fn bump_mod_revision_domains(&mut self, domains: &[&'static str]) {
        self.mod_changed_domains.extend(domains.iter().copied());
        self.mod_revision += 1;
        self.mods_dirty = true;
    }

    /// 断线重连完成：发 `app.ready`，模块重新取快照。
    pub fn notify_mods_ready(&mut self) {
        let payload = json!({"revision": self.mod_revision});
        self.emit_mod_event("app.ready", &payload);
    }

    /// 主题变化事件。
    pub fn notify_mods_theme(&mut self) {
        let theme = self.mod_theme_value();
        self.emit_mod_event("theme.changed", &theme);
    }

    /// 投递给声明了该事件的表面；未列出的不投递。
    pub fn emit_mod_event(&self, name: &str, payload: &Value) {
        for surface in &self.mods.surfaces {
            if self.mods.granted(&surface.mod_id, name) {
                surface.host.send(name, payload.clone());
            }
        }
    }

    pub fn send_focus_changed(&mut self) {
        let payload = json!({
            "space": self.workspace().map(|ws| format!("sp{}", ws.id)),
            "tab": self.workspace().and_then(|ws| ws.tab()).map(|tab| format!("tb{}", tab.id)),
            "pane": format!("{}", self.focused()),
            "mod": self.focused_mod,
        });
        self.emit_mod_event("focus.changed", &payload);
    }

    /// 发一条定向事件；表面不存在时丢弃。
    pub fn send_mod_event(&mut self, mod_id: &str, name: &str, payload: &Value) {
        if !self.mods.granted(mod_id, name) {
            return;
        }
        for surface in &self.mods.surfaces {
            if surface.mod_id == mod_id {
                surface.host.send(name, payload.clone());
            }
        }
    }

    /// 把积压的 `state.changed` 发出去；一轮只发最新 revision。
    fn flush_mod_events(&mut self) {
        if !self.mods_dirty {
            return;
        }
        self.mods_dirty = false;
        if !self.mod_all_domains && self.mod_changed_domains.is_empty() {
            // 只有 revision 变化（如授予变更）：不发空域的 `state.changed`。
            return;
        }
        let domains: Vec<&str> = if self.mod_all_domains {
            MOD_DOMAINS.to_vec()
        } else {
            self.mod_changed_domains.iter().copied().collect()
        };
        self.mod_all_domains = false;
        self.mod_changed_domains.clear();
        let payload = json!({
            "revision": self.mod_revision,
            "domains": domains,
        });
        self.emit_mod_event("state.changed", &payload);
    }

    /// 引擎线程的唤醒回调：只置位，不触碰 App 状态。
    pub fn mod_wake_callback(&self) -> nsh_platform_tui::WakeCallback {
        let flag = self.mod_wake.clone();
        std::sync::Arc::new(move || {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        })
    }

    /// 焦点表面的按键投递；模块未处理的键也不落到后台 PTY。
    pub fn route_key_to_mod(&mut self, key: KeyEvent) -> bool {
        let Some(id) = self.focused_mod.clone() else {
            return false;
        };
        let release = key.kind == KeyEventKind::Release;
        let Some(input) = crate::mods::input::translate_key(&key, release) else {
            return true;
        };
        let Some(surface) = self.mods.surface_mut(&id) else {
            self.focused_mod = None;
            return false;
        };
        if !surface.interactive() {
            return true;
        }
        if surface.view.handle(input) == Handled::Yes {
            self.dirty = true;
        }
        true
    }

    /// 焦点表面的粘贴投递。
    pub fn route_paste_to_mod(&mut self, text: String) -> bool {
        let Some(id) = self.focused_mod.clone() else {
            return false;
        };
        let Some(surface) = self.mods.surface_mut(&id) else {
            self.focused_mod = None;
            return false;
        };
        if !surface.interactive() {
            return true;
        }
        if surface.view.handle(crate::mods::input::paste(text)) == Handled::Yes {
            self.dirty = true;
        }
        true
    }

    /// 鼠标按 z 序选择表面；覆盖层内操作不穿透。
    pub fn route_mouse_to_mod(&mut self, mouse: MouseEvent) -> bool {
        if self.config.no_mods || self.mods.surfaces.is_empty() {
            return false;
        }
        let mut overlays: Vec<(u8, String, Rect, bool, bool)> = self
            .mods
            .surfaces
            .iter()
            .filter(|surface| {
                matches!(
                    surface.place,
                    SurfacePlace::Dialog | SurfacePlace::Popup | SurfacePlace::Window
                ) && surface.interactive()
            })
            .map(|surface| {
                let z = match surface.place {
                    SurfacePlace::Dialog => 1,
                    SurfacePlace::Popup => 2,
                    _ => 3,
                };
                (
                    z,
                    surface.id.clone(),
                    surface.area,
                    surface.modal,
                    surface.click_through,
                )
            })
            .collect();
        overlays.sort_by_key(|(z, _, _, _, _)| std::cmp::Reverse(*z));
        for (_, id, area, modal, click_through) in overlays {
            if click_through || !super::contains(area, mouse.column, mouse.row) {
                continue;
            }
            self.focus_mod_surface(Some(id.clone()));
            self.deliver_mouse(&id, mouse);
            if modal {
                return true;
            }
        }
        // 提示层覆盖整个内容区却不抢焦点：只吞掉模块自己认下的那一下，
        // 点在空白处仍落到下面的 Pane。
        let toasts: Vec<(String, Rect)> = self
            .mods
            .surfaces
            .iter()
            .filter(|surface| surface.place == SurfacePlace::Toast && surface.interactive())
            .map(|surface| (surface.id.clone(), surface.area))
            .collect();
        for (id, area) in toasts {
            if super::contains(area, mouse.column, mouse.row) && self.deliver_mouse(&id, mouse) {
                return true;
            }
        }
        // 非模态覆盖层结束传播后，继续按外框区域命中。
        let hit = self
            .mod_regions
            .iter()
            .find(|region| !region.folded && super::contains(region.rect, mouse.column, mouse.row))
            .map(|region| region.id.clone());
        let Some(mod_id) = hit else {
            return false;
        };
        let surface_id = self
            .mods
            .surfaces
            .iter()
            .find(|surface| {
                surface.mod_id == mod_id
                    && matches!(surface.place, SurfacePlace::Region | SurfacePlace::Pane)
                    && surface.interactive()
            })
            .map(|surface| surface.id.clone());
        let Some(surface_id) = surface_id else {
            return false;
        };
        self.focus_mod_surface(Some(surface_id.clone()));
        self.deliver_mouse(&surface_id, mouse);
        true
    }

    /// 把鼠标交给一条表面；返回模块是否认下了这个事件。
    fn deliver_mouse(&mut self, surface_id: &str, mouse: MouseEvent) -> bool {
        let Some(input) = crate::mods::input::translate_mouse(&mouse) else {
            return false;
        };
        let Some(surface) = self.mods.surface_mut(surface_id) else {
            return false;
        };
        let handled = surface.view.handle(input) == Handled::Yes;
        if handled {
            self.dirty = true;
        }
        handled
    }

    /// 关闭 z 序最高的覆盖层（window > popup > dialog）；`onEsc: "module"` 的不由宿主关。
    /// 返回是否关闭。
    pub fn close_top_mod_overlay(&mut self) -> bool {
        let target = self
            .mods
            .surfaces
            .iter()
            .filter(|surface| {
                surface.interactive()
                    && surface.esc_to_host
                    && matches!(
                        surface.place,
                        SurfacePlace::Dialog | SurfacePlace::Popup | SurfacePlace::Window
                    )
            })
            .max_by_key(|surface| match surface.place {
                SurfacePlace::Dialog => 1,
                SurfacePlace::Popup => 2,
                _ => 3,
            })
            .map(|surface| surface.id.clone());
        let Some(target) = target else {
            return false;
        };
        self.mods_close(&target)
    }

    /// 终端焦点变化投递给全部表面。
    pub fn route_focus_to_mods(&mut self, focused: bool) {
        for surface in &mut self.mods.surfaces {
            surface.view.handle(crate::mods::input::focus(focused));
        }
        self.dirty = true;
    }

    /// 让外框里的模块区域与包记录的启用状态一致：停用的包撤下区域并关闭表面，
    /// 新启用（或首次出现）的包按其 `[module].place` 停靠。用户主动 `mods.undock`
    /// 过的包留在 `mods_docked` 里，不再自动停靠。
    pub fn sync_package_modules(&mut self) {
        if self.config.no_mods {
            return;
        }
        let enabled = self.plugins.modules();
        let enabled_ids: std::collections::BTreeSet<String> =
            enabled.iter().map(|entry| entry.id.clone()).collect();
        let mut changed = false;
        let docked: Vec<String> = self.chrome.mods().into_iter().map(|(id, _)| id).collect();
        for id in &docked {
            if enabled_ids.contains(id) {
                continue;
            }
            if self.chrome.undock(id) {
                changed = true;
            }
            if self.mods_docked.remove(id) {
                changed = true;
            }
            if self.mods_close(id) {
                changed = true;
            }
        }
        // 提示表面不在外框 `mod` 块里，停用时单独收起。
        let dropped: Vec<String> = self
            .mods
            .surfaces
            .iter()
            .filter(|surface| {
                surface.place == SurfacePlace::Toast && !enabled_ids.contains(&surface.mod_id)
            })
            .map(|surface| surface.mod_id.clone())
            .collect();
        for id in dropped {
            if self.mods_close(&id) {
                changed = true;
            }
        }
        for entry in enabled {
            // 提示模块不占停靠区、不进 `mods_docked`：外框有 `toasts` 节点就始终有一条表面。
            if entry.module.place == "toast" {
                if !self.chrome.has_toasts() {
                    continue;
                }
                let open = self.mods.surfaces.iter().any(|surface| {
                    surface.mod_id == entry.id && surface.place == SurfacePlace::Toast
                });
                if !open {
                    match self.mods_open(
                        &entry.id,
                        SurfacePlace::Toast,
                        json!({}),
                        None,
                        None,
                        None,
                    ) {
                        Ok(_) => changed = true,
                        Err(error) => self.note = error,
                    }
                }
                continue;
            }
            if self.chrome.mods().iter().any(|(id, _)| id == &entry.id) {
                changed |= self.mods_docked.insert(entry.id.clone());
                continue;
            }
            if self.mods_docked.contains(&entry.id) {
                continue;
            }
            let Some(place) = dock_place(&entry.module.place) else {
                continue;
            };
            let horizontal = matches!(
                place,
                crate::mods::chrome::DockPlace::Left | crate::mods::chrome::DockPlace::Right
            );
            let size = entry.module.size.and_then(|size| {
                if size <= 1.0 {
                    Some(crate::mods::chrome::ChromeSize {
                        cols: None,
                        rows: None,
                        ratio: Some(size as f32),
                    })
                } else if horizontal {
                    crate::mods::chrome::ChromeSize::cols(size.round() as u16)
                } else {
                    crate::mods::chrome::ChromeSize::rows(size.round() as u16)
                }
            });
            self.chrome.dock(&entry.id, place, size, None, json!({}));
            self.mods_docked.insert(entry.id.clone());
            changed = true;
        }
        if changed {
            self.seen_dirty = true;
            self.dirty = true;
            self.save_local_state();
        }
    }

    /// 让外框里的模块区域与活表面一致；树未变时直接返回。
    pub fn sync_mod_surfaces(&mut self) {
        if self.config.no_mods {
            return;
        }
        let key = self.chrome.to_state().to_string();
        if self.mod_chrome_sync.as_deref() == Some(key.as_str()) {
            return;
        }
        self.mod_chrome_sync = Some(key);
        let wanted = self.chrome.mods();
        let focused = self.focused_mod.clone();
        let stale: Vec<String> = self
            .mods
            .surfaces
            .iter()
            .filter(|surface| {
                surface.place == SurfacePlace::Region
                    && !wanted.iter().any(|(id, _)| id == &surface.mod_id)
            })
            .map(|surface| surface.id.clone())
            .collect();
        for id in stale {
            self.remember_mod_window(&id);
            self.mods.close(&id);
        }
        for (mod_id, props) in wanted {
            if self
                .mods
                .surfaces
                .iter()
                .any(|surface| surface.mod_id == mod_id && surface.place == SurfacePlace::Region)
            {
                continue;
            }
            if let Err(error) =
                self.mods_open(&mod_id, SurfacePlace::Region, props, None, None, None)
            {
                self.note = error;
            }
        }
        self.focus_mod_surface(focused);
        self.dirty = true;
    }

    /// `window` 覆盖层关闭前记住位置。
    pub fn remember_mod_window(&mut self, surface_id: &str) {
        let Some(surface) = self.mods.surface(surface_id) else {
            return;
        };
        if surface.place != SurfacePlace::Window {
            return;
        }
        let Some(key) = surface.key.clone() else {
            return;
        };
        if surface.area.width == 0 || surface.area.height == 0 {
            return;
        }
        let pos = WindowPos {
            x: surface.area.x,
            y: surface.area.y,
            cols: surface.area.width,
            rows: surface.area.height,
        };
        self.mods.windows.insert(key.clone(), pos);
        self.mods_windows.insert(key, pos.to_value());
        self.seen_dirty = true;
    }

    /// 从 state.json 恢复 `window` 位置。
    pub fn load_mod_windows(&mut self) {
        for (key, value) in self.mods_windows.clone() {
            if let Some(pos) = WindowPos::from_value(&value) {
                self.mods.windows.insert(key, pos);
            }
        }
    }

    /// 相对路径按 `cwd` 或工作区目录解析；绝对路径原样使用。
    fn mod_local_path(&self, path: &str, data: &Value) -> PathBuf {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return path;
        }
        let base = data
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| {
                self.workspace()
                    .and_then(|ws| ws.folder.clone())
                    .map(PathBuf::from)
            });
        match base {
            Some(base) => base.join(path),
            None => path,
        }
    }

    /// 文件与命令的目标：`client` 与本机名在本进程执行，工作区在远端时
    /// 经 core 路由；显式 `on` 优先于工作区。
    fn mod_target(&self, on: Option<&str>) -> ModTarget {
        let machine = self.workspace_machine(self.active_ws);
        let client = client_host();
        let target = match on {
            Some("client") => client.clone(),
            Some(name) => name.to_string(),
            None => machine,
        };
        if target == "local" || target == client {
            ModTarget::Local
        } else {
            ModTarget::Remote(target)
        }
    }

    /// 待确认授予：列出每一项，确认后追加授予。
    pub fn prompt_mod_grants(&mut self, mod_id: &str) {
        let pending = self.mods.pending_grants_for(mod_id);
        if pending.is_empty() {
            return;
        }
        let mut items: Vec<super::ChooserItem> = pending
            .iter()
            .map(|name| super::ChooserItem {
                label: t1("Approve {}", name),
                value: json!({"mod": mod_id, "name": name}).to_string(),
            })
            .collect();
        items.push(super::ChooserItem {
            label: t("Approve all").to_string(),
            value: json!({"mod": mod_id}).to_string(),
        });
        self.open_chooser(super::Chooser {
            title: t1("Grants for {}", mod_id),
            selected: 0,
            kind: super::ChooserKind::Grants,
            items,
        });
    }

    /// 批准一条或全部授予申请，并发 `grant.changed`。
    pub fn approve_mod_grant(&mut self, value: &str) {
        let Ok(value) = serde_json::from_str::<Value>(value) else {
            return;
        };
        let Some(mod_id) = value.get("mod").and_then(Value::as_str) else {
            return;
        };
        let names: Vec<String> = match value.get("name").and_then(Value::as_str) {
            Some(name) => vec![name.to_string()],
            None => self.mods.pending_grants_for(mod_id),
        };
        if names.is_empty() {
            return;
        }
        let added = self.mods.approve_grants(mod_id, &names);
        if let Some(pending) = self.mods.pending_grants.get_mut(mod_id) {
            for name in &names {
                pending.remove(name);
            }
        }
        if !added.is_empty() {
            let grants = self.mods.grants_for(mod_id);
            self.send_mod_event(mod_id, "grant.changed", &json!({"grants": grants}));
            self.persist_mod_grants(mod_id);
            self.seen_dirty = true;
            self.bump_mod_revision_domains(&[]);
        }
        self.dirty = true;
    }

    /// `mods.reload`：重载一个模块或全部模块的停靠表面。
    pub fn mods_reload_target(&mut self, target: Option<&str>) -> Result<(), String> {
        let mods: Vec<String> = self
            .mods
            .surfaces
            .iter()
            .filter(|surface| target.is_none_or(|target| surface.mod_id == target))
            .map(|surface| surface.mod_id.clone())
            .collect();
        if mods.is_empty() {
            return match target {
                Some(target) => Err(t1("mod {} has no surface", target)),
                None => Ok(()),
            };
        }
        for mod_id in mods {
            self.mods_reload(&mod_id)?;
        }
        Ok(())
    }

    /// 记录运行期异常：写 `logs/<id>/engine.log` 并计数；连续三次进入 Failed。
    pub fn record_mod_errors(&mut self, surface_id: &str, mod_id: &str, errors: Vec<String>) {
        let store = crate::store::Store::open(self.config.mods_state_dir());
        for error in &errors {
            let line = format!("[{}] {error}", crate::store::now_unix());
            let _ = store.append_log(mod_id, &line);
            eprintln!("{}", t2("mod {}: {}", mod_id, error));
        }
        if let Some(surface) = self.mods.surface_mut(surface_id) {
            for error in errors {
                surface.record_failure(error);
            }
        }
        self.dirty = true;
    }

    /// 终端事件转发：只发模块声明的事件名，并递增 revision。
    pub fn on_mod_terminal_event(&mut self, kind: &str, terminal: Option<&str>, data: &Value) {
        if self.mods.surfaces.is_empty() {
            return;
        }
        let revision = self.mod_revision + 1;
        let event = match kind {
            "status" => Some((
                "terminal.status",
                json!({
                    "address": terminal,
                    "status": data.get("status"),
                    "revision": revision,
                }),
            )),
            "turn" => Some((
                "terminal.turn",
                json!({
                    "address": terminal,
                    "turn": data.get("turn"),
                    "revision": revision,
                }),
            )),
            "title" => Some((
                "terminal.title",
                json!({
                    "address": terminal,
                    "title": data.get("title"),
                    "revision": revision,
                }),
            )),
            "finished" | "removed" | "lost" => Some((
                "terminal.finished",
                json!({
                    "address": terminal,
                    "program_exit": kind != "removed",
                    "revision": revision,
                }),
            )),
            _ => None,
        };
        self.bump_mod_revision_domains(&["agents", "terminals"]);
        if let Some((name, payload)) = event {
            self.emit_mod_event(name, &payload);
        }
    }
}

/// 模块命令与文件方法的目标位置。
enum ModTarget {
    /// hc-tui 进程所在主机。
    Local,
    /// 经 core 转发的机器名。
    Remote(String),
}

/// 主题颜色转 `#rrggbb`，供模块与调色板使用。
fn color_hex(color: ratatui_core::style::Color) -> String {
    use ratatui_core::style::Color;
    let (r, g, b) = match color {
        Color::Reset => (0, 0, 0),
        Color::Black => (0x00, 0x00, 0x00),
        Color::Red => (0x80, 0x00, 0x00),
        Color::Green => (0x00, 0x80, 0x00),
        Color::Yellow => (0x80, 0x80, 0x00),
        Color::Blue => (0x00, 0x00, 0x80),
        Color::Magenta => (0x80, 0x00, 0x80),
        Color::Cyan => (0x00, 0x80, 0x80),
        Color::Gray => (0xc0, 0xc0, 0xc0),
        Color::DarkGray => (0x80, 0x80, 0x80),
        Color::LightRed => (0xff, 0x00, 0x00),
        Color::LightGreen => (0x00, 0xff, 0x00),
        Color::LightYellow => (0xff, 0xff, 0x00),
        Color::LightBlue => (0x00, 0x00, 0xff),
        Color::LightMagenta => (0xff, 0x00, 0xff),
        Color::LightCyan => (0x00, 0xff, 0xff),
        Color::White => (0xff, 0xff, 0xff),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => ansi256(index),
    };
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// xterm 256 色表里的 RGB。
fn ansi256(index: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0x80, 0x00, 0x00),
        (0x00, 0x80, 0x00),
        (0x80, 0x80, 0x00),
        (0x00, 0x00, 0x80),
        (0x80, 0x00, 0x80),
        (0x00, 0x80, 0x80),
        (0xc0, 0xc0, 0xc0),
        (0x80, 0x80, 0x80),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x00, 0x00, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    match index {
        0..=15 => BASE[index as usize],
        16..=231 => {
            let index = index - 16;
            let step = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
            (step(index / 36), step((index % 36) / 6), step(index % 6))
        }
        _ => {
            let value = 8 + (index - 232) * 10;
            (value, value, value)
        }
    }
}

/// `[module].place` 到外框停靠位置的映射；`none` 与覆盖层不自动停靠。
fn dock_place(place: &str) -> Option<crate::mods::chrome::DockPlace> {
    match place {
        "left" => Some(crate::mods::chrome::DockPlace::Left),
        "right" => Some(crate::mods::chrome::DockPlace::Right),
        "top" => Some(crate::mods::chrome::DockPlace::Top),
        "bottom" => Some(crate::mods::chrome::DockPlace::Bottom),
        _ => None,
    }
}

fn surface_host_exit(runtime: &ModRuntime, surface_id: &str) {
    if let Some(surface) = runtime.surface(surface_id) {
        surface.host.exec_exit();
    }
}

fn client_host() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| {
        std::fs::read_to_string("/etc/hostname")
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod base_tests;

#[cfg(test)]
mod toast_tests;

#[cfg(test)]
mod notify_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use ratatui_core::backend::TestBackend;
    use ratatui_core::terminal::Terminal;

    const PROBE_TSX: &str = r#"
import { host, mount, signal, Text, View } from "nsh";
const result = signal("pending");
host.call("exec", { argv: ["echo", "hi"] }).then(
  () => result.set("done"),
  (err: Error) => result.set("error:" + err.message),
);
mount(() => <View><Text>{result()}</Text></View>);
"#;

    fn app() -> App {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("hctui-mods-{}-{unique}", std::process::id()));
        let (_, events) = std::sync::mpsc::channel();
        App::with_network(
            Config {
                restore: false,
                prompt_new_tab: false,
                config_path: dir.join("config.toml"),
                state_path: dir.join("state.json"),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap()
    }

    /// 在配置目录写一个模块：bundle、capabilities 与可选的 hctui.toml。
    fn write_mod(app: &App, id: &str, code: String, capabilities: &str) {
        let dir = app.config.mods_dir().join(id).join("dist");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bundle.js"), code).unwrap();
        std::fs::write(dir.join("capabilities.json"), capabilities).unwrap();
    }

    fn write_manifest(app: &App, id: &str, grants: &[&str]) {
        let dir = app.config.mods_dir().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let list = grants
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            dir.join("hctui.toml"),
            format!("[mod]\nid = \"{id}\"\nhost_api = 1\n\n[grants]\nhost = [{list}]\n"),
        )
        .unwrap();
    }

    fn probe(app: &mut App) -> String {
        let code = nsh_bundler::compile_module("App.tsx", PROBE_TSX, &[]).unwrap();
        write_mod(app, "probe", code, r#"{"host":["exec"]}"#);
        app.mods_open(
            "probe",
            SurfacePlace::Dialog,
            json!({}),
            Some((60, 10)),
            None,
            None,
        )
        .unwrap()
    }

    fn frames(app: &mut App, count: usize) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        for _ in 0..count {
            terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
            app.pump_mods();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    #[test]
    fn unapproved_exec_is_forbidden_and_has_no_side_effect() {
        let mut app = app();
        let surface = probe(&mut app);
        let screen = frames(&mut app, 40);
        assert!(screen.contains("forbidden"), "screen: {screen}");
        assert!(!screen.contains("done"), "screen: {screen}");
        let surface = app.mods.surface(&surface).expect("surface stays");
        assert!(surface.state == SurfaceState::Running);
    }

    #[test]
    fn approved_exec_runs_the_program() {
        let mut app = app();
        write_manifest(&app, "probe", &["exec"]);
        probe(&mut app);
        let screen = frames(&mut app, 60);
        assert!(screen.contains("done"), "screen: {screen}");
    }

    #[test]
    fn reload_swaps_generation_and_keeps_focus() {
        let mut app = app();
        let surface_id = probe(&mut app);
        let before = app
            .mods
            .surface(&surface_id)
            .map(|surface| surface.generation)
            .unwrap();
        app.mods_reload("probe").unwrap();
        let surface = app.mods.surface(&surface_id).expect("surface stays");
        assert_eq!(surface.generation, before + 1);
        assert_eq!(surface.state, SurfaceState::Running);
        assert_eq!(app.focused_mod.as_deref(), Some(surface_id.as_str()));
    }

    #[test]
    fn manifest_grants_are_approved_once_and_persist() {
        let mut app = app();
        write_manifest(&app, "probe", &["exec"]);
        let surface = probe(&mut app);
        assert!(app.mods.granted("probe", "exec"));
        let _ = surface;
        app.save_local_state();
        let mut restored = app_config_only(&app);
        restored.load_local_state();
        assert!(restored.mods.granted("probe", "exec"));
        // 已批准记录不以清单为准：撤销后重新加载不恢复。
        restored.mods.revoke_grants("probe", &["exec".to_string()]);
        restored.save_local_state();
        let mut again = app_config_only(&restored);
        again.load_local_state();
        assert!(!again.mods.granted("probe", "exec"));
    }

    fn app_config_only(source: &App) -> App {
        let (_, events) = std::sync::mpsc::channel();
        App::with_network(
            Config {
                restore: false,
                prompt_new_tab: false,
                config_path: source.config.config_path.clone(),
                state_path: source.config.state_path.clone(),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap()
    }

    #[test]
    fn failed_reload_keeps_the_old_surface_and_generation() {
        let mut app = app();
        let surface_id = probe(&mut app);
        let before = app
            .mods
            .surface(&surface_id)
            .map(|surface| surface.generation)
            .unwrap();
        let dir = app.config.mods_dir().join("probe").join("dist");
        std::fs::remove_file(dir.join("bundle.js")).unwrap();
        let result = app.mods_reload("probe");
        assert!(result.is_err());
        let surface = app.mods.surface(&surface_id).expect("old surface stays");
        assert_eq!(surface.generation, before);
        assert_eq!(surface.state, SurfaceState::Running);
        assert!(surface.last_error.is_none());
    }

    #[test]
    fn overlays_close_top_down_and_remember_window_positions() {
        let mut app = app();
        probe(&mut app);
        frames(&mut app, 10);
        let window = app
            .mods_open(
                "probe",
                SurfacePlace::Window,
                json!({}),
                Some((30, 8)),
                None,
                Some("prs".into()),
            )
            .unwrap();
        frames(&mut app, 2);
        if let Some(surface) = app.mods.surface_mut(&window) {
            surface.area = Rect::new(5, 4, 30, 8);
        }
        assert!(app.close_top_mod_overlay());
        assert!(app.mods.surface(&window).is_none());
        assert_eq!(
            app.mods_windows.get("prs"),
            Some(&json!({"x":5,"y":4,"cols":30,"rows":8}))
        );
        let mut restored = app_config_only(&app);
        restored.mods_windows = app.mods_windows.clone();
        restored.load_mod_windows();
        assert_eq!(
            restored.mods.windows.get("prs").copied(),
            Some(WindowPos {
                x: 5,
                y: 4,
                cols: 30,
                rows: 8
            })
        );
    }

    fn module_source(app: &App, marker: &str) -> PathBuf {
        let dir = app
            .config
            .state_path
            .parent()
            .unwrap()
            .join(format!("mod-source-{marker}"));
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(
            dir.join("hctui.toml"),
            "[mod]\nid = \"storeprobe\"\nhost_api = 1\n",
        )
        .unwrap();
        std::fs::write(dir.join("nsh.json"), "{\"name\":\"Store Probe\"}").unwrap();
        let tsx = format!(
            "import {{ mount, Text, View }} from \"nsh\";\nmount(() => <View><Text>{marker}</Text></View>);"
        );
        let code = nsh_bundler::compile_module("App.tsx", &tsx, &[]).unwrap();
        std::fs::write(dir.join("dist").join("bundle.js"), code).unwrap();
        dir
    }

    #[test]
    fn linked_module_reloads_source_changes() {
        let mut app = app();
        let source = module_source(&app, "link-one");
        crate::mods::store::link(&app.config, &source).unwrap();
        app.mods_open(
            "storeprobe",
            SurfacePlace::Dialog,
            json!({}),
            Some((60, 10)),
            None,
            None,
        )
        .unwrap();
        let screen = frames(&mut app, 30);
        assert!(screen.contains("link-one"), "screen: {screen}");

        let tsx =
            "import { mount, Text, View } from \"nsh\";\nmount(() => <View><Text>link-two</Text></View>);";
        let code = nsh_bundler::compile_module("App.tsx", tsx, &[]).unwrap();
        std::fs::write(source.join("dist").join("bundle.js"), code).unwrap();
        app.mods_reload("storeprobe").unwrap();
        let screen = frames(&mut app, 30);
        assert!(screen.contains("link-two"), "screen: {screen}");
    }

    #[test]
    fn installed_module_loads_from_store_and_reload_takes_new_generation() {
        let mut app = app();
        let source = module_source(&app, "gen-one");
        crate::mods::store::install_local(&app.config, &source).unwrap();
        let surface = app
            .mods_open(
                "storeprobe",
                SurfacePlace::Dialog,
                json!({}),
                Some((60, 10)),
                None,
                None,
            )
            .unwrap();
        let screen = frames(&mut app, 30);
        assert!(screen.contains("gen-one"), "screen: {screen}");

        let source = module_source(&app, "gen-two");
        crate::mods::store::update(&app.config, Some("storeprobe"), &source).unwrap();
        app.mods_reload("storeprobe").unwrap();
        let screen = frames(&mut app, 30);
        assert!(screen.contains("gen-two"), "screen: {screen}");

        let store = crate::store::Store::open(app.config.mods_state_dir());
        assert!(store.generation_dir("storeprobe", 1).exists());
        assert!(app.mods_close(&surface));
        assert!(!store.generation_dir("storeprobe", 1).exists());
        assert!(store.generation_dir("storeprobe", 2).exists());
    }

    #[test]
    fn state_snapshot_filters_domains_and_caps_panes() {
        let app = app();
        let snapshot = app.mod_state_snapshot(&["focused"]);
        assert!(snapshot.get("focused").is_some());
        assert!(snapshot.get("spaces").is_none());
        let all = app.mod_state_snapshot(&[]);
        assert!(all.get("spaces").is_some());
        assert!(all.get("connection").is_some());
    }
}
