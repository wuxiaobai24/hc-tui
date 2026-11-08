//! 宿主通道：把模块的 `host.call` / `host.on` 接到 hc-tui。
//!
//! nsh 的 handler 在引擎线程上运行且必须立刻返回，因此这里只把调用连同
//! 应答通道搬到 UI 线程；[`ModHost::pump`] 在每轮循环里分发并应答。事件
//! （`state.changed` 等）由宿主在变化时 [`HostChannel::send`]。

use nsh_engine::{HostCall, HostChannel, HostReply};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

/// 模块可调用的宿主方法清单；授予按名字逐项校验。
pub const METHODS: &[&str] = &[
    "state",
    "run",
    "request",
    "exec",
    "readFile",
    "writeFile",
    "notify",
    "theme",
    "overlay",
    "closeOverlay",
    "focus",
    "surface",
    "setAgentOrder",
    "requestGrants",
    "ask",
    "answer",
    "setProps",
    "dismiss",
];

/// 一条从引擎线程搬来的调用；`reply` 为 `None` 表示 post。
pub struct PendingCall {
    pub name: String,
    pub data: Value,
    pub reply: Option<HostReply>,
}

/// 每条表面一份宿主通道。
pub struct ModHost {
    /// 表面 ID。
    pub surface: String,
    /// 模块 ID。
    pub mod_id: String,
    /// nsh 宿主通道。
    pub channel: HostChannel,
    /// UI 线程取调用的队列。
    incoming: crossbeam_channel::Receiver<PendingCall>,
    /// 单表面并发执行中的 `exec` 数量。
    pub exec_inflight: Arc<Mutex<u32>>,
}

impl ModHost {
    /// 建立通道并注册全部方法的转发 handler。
    pub fn new(surface: &str, mod_id: &str) -> ModHost {
        let channel = HostChannel::new();
        let (tx, rx) = crossbeam_channel::unbounded::<PendingCall>();
        let seq = Arc::new(AtomicU64::new(0));
        for name in METHODS {
            let tx = tx.clone();
            let mod_id = mod_id.to_string();
            let seq = seq.clone();
            channel.handle(*name, move |call: HostCall| {
                let name = call.name().to_string();
                let data = call.data().clone();
                let reply = call.into_reply();
                // 调用一到就压进诊断上下文：之后的任何 panic 都带着这三层。
                crate::diag::record_call(&mod_id, &name, seq.fetch_add(1, Ordering::Relaxed) + 1);
                // 演练崩溃报告：`HC_TUI_CRASH_ON_HOST_CALL=<方法名>` 让调用到达时 panic。
                if std::env::var("HC_TUI_CRASH_ON_HOST_CALL").as_deref() == Ok(name.as_str()) {
                    panic!("injected host-call panic: {name}");
                }
                let _ = tx.send(PendingCall { name, data, reply });
            });
        }
        ModHost {
            surface: surface.to_string(),
            mod_id: mod_id.to_string(),
            channel,
            incoming: rx,
            exec_inflight: Arc::new(Mutex::new(0)),
        }
    }

    /// 取走当前积压的调用（不阻塞）。
    pub fn drain(&self) -> Vec<PendingCall> {
        let mut calls = Vec::new();
        while let Ok(call) = self.incoming.try_recv() {
            calls.push(call);
        }
        calls
    }

    /// 设置启动参数；须在 `AppView::load` 之前调用。
    pub fn set_params(&self, params: Value) {
        if let Value::Object(map) = params {
            self.channel.set_params(map);
        }
    }

    /// 宿主 → 模块事件。
    pub fn send(&self, name: &str, data: Value) {
        let _ = self.channel.send(name, data);
    }

    /// `exec` 并发进入；超出单表面 4 条上限返回 `false`。
    pub fn exec_enter(&self) -> bool {
        let mut count = self.exec_inflight.lock().unwrap_or_else(|e| e.into_inner());
        if *count >= 4 {
            return false;
        }
        *count += 1;
        true
    }

    /// `exec` 结束。
    pub fn exec_exit(&self) {
        let mut count = self.exec_inflight.lock().unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
    }
}

/// 拒绝一个调用：统一错误形态 `{ code, message }`。
pub fn reject(reply: Option<HostReply>, code: &str, message: &str) {
    if let Some(reply) = reply {
        reply.reject(json!({"code": code, "message": message}).to_string());
    }
}

/// 组装启动参数；字段与 docs/mods.md §7 对应。
pub fn build_params(params: &ParamsContext) -> Value {
    json!({
        "mod": params.mod_id,
        "surface": params.surface,
        "generation": params.generation,
        "place": params.place,
        "props": params.props,
        "space": params.space,
        "tab": params.tab,
        "pane": params.pane,
        "machine": {
            "client": params.client,
            "core": params.core,
            "workspace": params.workspace,
        },
        "cell": params.cell,
        "locale": params.locale,
        "theme": params.theme,
        "api": {"host": 1, "hc_tui": params.hc_tui},
    })
}

/// 启动参数的宿主侧来源。
pub struct ParamsContext {
    pub mod_id: String,
    pub surface: String,
    pub generation: u64,
    pub place: &'static str,
    pub props: Value,
    pub space: Value,
    pub tab: Value,
    pub pane: Value,
    pub client: String,
    pub core: String,
    pub workspace: String,
    pub cell: [u16; 2],
    pub locale: &'static str,
    pub theme: Value,
    pub hc_tui: String,
}

/// `exec` 的本地执行：30 秒超时、8 MiB 输出上限，线程内完成并应答。
pub fn spawn_local_exec(
    reply: HostReply,
    inflight: Arc<Mutex<u32>>,
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    timeout: Duration,
    env: BTreeMap<String, String>,
) {
    std::thread::spawn(move || {
        let result = run_subprocess(argv, cwd, timeout, env);
        {
            let mut count = inflight.lock().unwrap_or_else(|e| e.into_inner());
            *count = count.saturating_sub(1);
        }
        match result {
            Ok(value) => reply.resolve(value),
            Err(code) => {
                if code == "timeout" {
                    reply.resolve(json!({"timed_out": true, "code": null, "stdout": "", "stderr": "", "truncated": false}));
                } else {
                    reject(Some(reply), code, "exec failed");
                }
            }
        }
    });
}

fn run_subprocess(
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    timeout: Duration,
    env: BTreeMap<String, String>,
) -> Result<Value, &'static str> {
    use std::process::{Command, Stdio};
    let Some((program, args)) = argv.split_first() else {
        return Err("invalid");
    };
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| "not_found")?;
    let deadline = std::time::Instant::now() + timeout;
    const MAX: usize = 8 * 1024 * 1024;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut timed_out = false;
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let mut code: Option<std::process::ExitStatus> = None;
    loop {
        if let Some(pipe) = stdout_pipe.as_mut() {
            let _ = take_upto(pipe, &mut stdout, MAX);
        }
        if let Some(pipe) = stderr_pipe.as_mut() {
            let _ = take_upto(pipe, &mut stderr, MAX);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                code = Some(status);
                break;
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
    if let Some(pipe) = stdout_pipe.as_mut() {
        let _ = take_upto(pipe, &mut stdout, MAX);
    }
    if let Some(pipe) = stderr_pipe.as_mut() {
        let _ = take_upto(pipe, &mut stderr, MAX);
    }
    let truncated = stdout.len() >= MAX || stderr.len() >= MAX;
    if timed_out {
        // 超时走 resolve(timed_out) 语义，由调用方判别。
        return Ok(json!({
            "code": null, "stdout": lossy(&stdout), "stderr": lossy(&stderr),
            "timed_out": true, "truncated": truncated,
        }));
    }
    let code = code.and_then(|status| status.code()).unwrap_or(-1);
    Ok(json!({
        "code": code,
        "stdout": lossy(&stdout),
        "stderr": lossy(&stderr),
        "timed_out": false,
        "truncated": truncated,
    }))
}

fn take_upto(pipe: &mut impl Read, buffer: &mut Vec<u8>, max: usize) -> std::io::Result<usize> {
    let mut chunk = [0u8; 8192];
    let mut total = 0;
    while buffer.len() < max {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let take = n.min(max - buffer.len());
                buffer.extend_from_slice(&chunk[..take]);
                total += take;
                if take < n {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
