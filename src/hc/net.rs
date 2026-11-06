//! 后台网络线程：每条 attach 流一个 Pane，帧分发、RPC、credit、重连都在这里。
//!
//! 硬约束（plan v1.1 §2.1）：
//! - **单 reader 按帧序分发**：只有 reader task 读字节，它把完整帧顺序送进一个
//!   通道；事件、各 Pane 的 attach 字节、RPC 回复都由同一个循环按到达顺序处理。
//! - **单 writer**：这个循环是唯一写子进程 stdin 的地方，帧不会交错。
//! - **UI 线程不 await**：UI 只通过 [`Command`] / [`NetEvent`] 两个通道交互。
//!
//! credit 语义（protocol-facts §3）：attach 流收到字节后**不立刻**回 CREDIT，
//! 等 UI 把字节写进本地 VT 后回 [`Command::Credit`]；这样 UI 落后时 core 停在
//! 256 KiB 窗口上，内存自然有界。events 流在本线程里就地解析，收到即回。

use std::collections::BTreeMap;
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::process::Child;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

use super::client::{self, OpenError, Parts};
use super::codec::{self, Frame, DATA};
use super::proto::{self, AttachResult, Display, Event, SubscribeResult, TerminalSummary};

/// 每条流的初始可写额度。
pub const WINDOW: usize = 256 * 1024;
/// 单帧流数据上限。
pub const CHUNK: usize = 16 * 1024;
/// RPC 回复期限。超过就判定连接已经不可用。
const RPC_TIMEOUT: Duration = Duration::from_secs(10);
/// core 每 15 s 发一次 heartbeat，留 8 倍余量判定死活。
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(120);
const BACKOFF_MIN: Duration = Duration::from_millis(200);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
/// 单个 Pane 的待发输入上限。触顶时丢最旧的字节，避免把内存拖爆。
const OUTBOX_MAX: usize = 4 << 20;
/// 本地假定的 cell 像素尺寸，FACTS 上报用。
pub const CELL_PX: [u32; 2] = [8, 16];

/// UI 发给网络线程的意图。`pane` 是 UI 侧的 Pane 编号，与 core 的终端地址无关。
#[derive(Debug)]
pub enum Command {
    /// 让这个 Pane 拥有一个 attach 流：必要时先 `terminal.run` 起一个 shell。
    Spawn {
        keep: bool,
        pane: u32,
        target: Option<String>,
        /// 首次 attach 时要起的程序；空表示 core 的 shell。
        argv: Vec<String>,
        /// 程序的起始目录；`None` 用 core 自己的。
        cwd: Option<String>,
        /// 在另一台机器上启动（`terminal.run` 的 `on`）。
        on: Option<String>,
        cols: u16,
        rows: u16,
    },
    /// Pane 内容区尺寸变化。已 attach 时立刻发 RESIZE 帧。
    Resize {
        pane: u32,
        cols: u16,
        rows: u16,
    },
    /// 键盘、鼠标、粘贴、VT 查询应答的最终字节。
    Input {
        pane: u32,
        bytes: Vec<u8>,
    },
    Focus {
        pane: u32,
        focused: bool,
    },
    /// 只断这个视图，core 里的程序继续跑。
    Close {
        pane: u32,
    },
    /// UI 已把 `bytes` 写进本地 VT，可以还给 core 了。
    Credit {
        channel: u32,
        bytes: usize,
    },
    /// 任意 JSON-RPC。`tag` 由调用方定义，答复按它关联。
    Call {
        tag: String,
        method: String,
        params: Value,
    },
    Display(Display),
    Shutdown,
}

/// 网络线程发给 UI 的通知。
#[derive(Debug)]
pub enum NetEvent {
    Hello {
        version: String,
        core: String,
    },
    Reconnecting {
        reason: String,
    },
    Failed {
        reason: String,
    },
    Snapshot {
        seq: u64,
        terminals: Vec<TerminalSummary>,
    },
    Event {
        event: Event,
    },
    Reply {
        tag: String,
        result: std::result::Result<Value, RpcFailure>,
    },
    Attached {
        pane: u32,
        channel: u32,
        /// core 里的终端地址（`t7` / `machine/t7`）。
        terminal: String,
        /// 给人看的名字：显式名字，其次标题。
        label: String,
        cols: u16,
        rows: u16,
        resize_notifications: bool,
    },
    Output {
        channel: u32,
        bytes: Vec<u8>,
    },
    AttachResized {
        channel: u32,
        cols: u16,
        rows: u16,
    },
    AttachClosed {
        pane: u32,
        channel: u32,
        reason: String,
        /// 程序自己结束（而不是连接层面断开）。
        program_exit: bool,
    },
}

#[derive(Debug, Clone)]
pub struct NetConfig {
    pub session: Option<String>,
}

/// 网络线程的句柄。Drop 会请求退出并等线程收尾。
pub struct NetHandle {
    commands: UnboundedSender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl NetHandle {
    pub fn spawn(config: NetConfig, events: Sender<NetEvent>) -> Self {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let thread = std::thread::Builder::new()
            .name("hc-net".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = events.send(NetEvent::Failed {
                            reason: format!("cannot start tokio runtime: {error}"),
                        });
                        return;
                    }
                };
                runtime.block_on(supervise(config, events, command_rx));
            })
            .ok();
        Self { commands, thread }
    }

    /// 非阻塞投递。线程已经退出时返回 `false`，调用方据此显示错误。
    pub fn send(&self, command: Command) -> bool {
        self.commands.send(command).is_ok()
    }

    pub fn shutdown(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for NetHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 每个 Pane 的 attach 意图，跨重连保留。
struct PaneIntent {
    keep: bool,
    target: Option<String>,
    argv: Vec<String>,
    cwd: Option<String>,
    on: Option<String>,
    cols: u16,
    rows: u16,
    wanted: bool,
}

impl PaneIntent {
    fn new(target: Option<String>, cols: u16, rows: u16) -> Self {
        Self {
            keep: true,
            target,
            argv: Vec::new(),
            cwd: None,
            on: None,
            cols: cols.max(1),
            rows: rows.max(1),
            wanted: true,
        }
    }

    fn size(&mut self, cols: u16, rows: u16) {
        self.cols = cols.max(1);
        self.rows = rows.max(1);
    }

    fn ready(&self) -> bool {
        self.wanted && self.cols >= 2 && self.rows >= 2
    }
}

/// RPC 失败的一侧：`kind` 用于区分 `conflict` 这类需要重试的情况。
#[derive(Debug, Clone)]
pub struct RpcFailure {
    pub kind: String,
    pub message: String,
}

enum Outcome {
    /// UI 要求退出。
    Shutdown,
    /// 重连也不会成功（代次不符、方法缺失）。
    Fatal(String),
    /// 连接断了，退避后重连。
    Lost { reason: String, was_connected: bool },
}

enum FrameMsg {
    Frame(Frame),
    End(String),
}

struct Session<'a> {
    events: Sender<NetEvent>,
    writer: tokio::process::ChildStdin,
    frames: mpsc::Receiver<FrameMsg>,
    /// 持有子进程，drop 时结束连接。
    _child: Child,
    next_id: u64,
    pending: BTreeMap<u64, (String, String, Instant)>,
    last_frame: Instant,
    events_channel: Option<u32>,
    event_lines: Vec<u8>,
    streams: Vec<Stream>,
    intents: &'a mut BTreeMap<u32, PaneIntent>,
    display: &'a mut Display,
    commands: &'a mut UnboundedReceiver<Command>,
}

/// 一条 attach 流及其记账。
struct Stream {
    pane: u32,
    channel: u32,
    cols: u16,
    rows: u16,
    /// core 给我们的可写额度，发出去的字节从这里扣。
    credit: i64,
    outbox: Vec<u8>,
}

/// 重连主循环。只有 UI 明确要求退出或出现致命错误才结束。
async fn supervise(
    config: NetConfig,
    events: Sender<NetEvent>,
    mut commands: UnboundedReceiver<Command>,
) {
    let mut intents: BTreeMap<u32, PaneIntent> = BTreeMap::new();
    let mut backoff = BACKOFF_MIN;
    let mut display = Display::with_cell(CELL_PX[0], CELL_PX[1]);

    loop {
        let outcome = serve(&config, &events, &mut commands, &mut intents, &mut display).await;
        match outcome {
            Outcome::Shutdown => return,
            Outcome::Fatal(reason) => {
                let _ = events.send(NetEvent::Failed { reason });
                return;
            }
            Outcome::Lost {
                reason,
                was_connected,
            } => {
                if was_connected {
                    backoff = BACKOFF_MIN;
                }
                let _ = events.send(NetEvent::Reconnecting { reason });
            }
        }

        // 退避期间也要响应 UI：尺寸变化要进意图，退出请求要立刻生效。
        let sleep = tokio::time::sleep(backoff);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                command = commands.recv() => match command {
                    None | Some(Command::Shutdown) => return,
                    Some(Command::Display(next)) => display = next,
                    Some(command) => apply_offline_command(command, &mut intents, &events),
                },
            }
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// 连接断开期间收到的命令：只更新意图，不需要回帧。
fn apply_offline_command(
    command: Command,
    intents: &mut BTreeMap<u32, PaneIntent>,
    events: &Sender<NetEvent>,
) {
    match command {
        Command::Spawn {
            keep,
            pane,
            target,
            argv,
            cwd,
            on,
            cols,
            rows,
        } => {
            let intent = intents
                .entry(pane)
                .or_insert_with(|| PaneIntent::new(target.clone(), cols, rows));
            if target.is_some() {
                intent.target = target;
            }
            if !argv.is_empty() {
                intent.argv = argv;
            }
            if cwd.is_some() {
                intent.cwd = cwd;
            }
            if on.is_some() {
                intent.on = on;
            }
            intent.keep = keep;
            intent.size(cols, rows);
            intent.wanted = true;
        }
        Command::Resize { pane, cols, rows } => {
            if let Some(intent) = intents.get_mut(&pane) {
                intent.size(cols, rows);
            }
        }
        Command::Close { pane } => {
            if let Some(intent) = intents.get_mut(&pane) {
                intent.wanted = false;
            }
        }
        Command::Call { tag, method, .. } => {
            let _ = events.send(NetEvent::Reply {
                tag,
                result: Err(RpcFailure {
                    kind: "offline".to_string(),
                    message: format!("{method}: 连接暂时不可用"),
                }),
            });
        }
        Command::Display(_)
        | Command::Shutdown
        | Command::Input { .. }
        | Command::Focus { .. }
        | Command::Credit { .. } => {}
    }
}

async fn serve(
    config: &NetConfig,
    events: &Sender<NetEvent>,
    commands: &mut UnboundedReceiver<Command>,
    intents: &mut BTreeMap<u32, PaneIntent>,
    display: &mut Display,
) -> Outcome {
    let parts = match client::open(config.session.as_deref()).await {
        Ok(parts) => parts,
        Err(OpenError::Fatal(error)) => return Outcome::Fatal(format!("{error:#}")),
        Err(OpenError::Retry(error)) => {
            return Outcome::Lost {
                reason: format!("{error:#}"),
                was_connected: false,
            }
        }
    };
    let Parts {
        child,
        stdin,
        stdout,
        hello,
    } = parts;
    let _ = events.send(NetEvent::Hello {
        version: hello.version.clone(),
        core: hello.core.name.clone(),
    });

    // 唯一的 reader：完整帧按到达顺序进通道，之后只有这个循环消费它。
    let (frames_tx, frames_rx) = mpsc::channel(64);
    tokio::spawn(async move {
        let mut stdout = stdout;
        loop {
            match client::read_frame(&mut stdout).await {
                Ok(frame) => {
                    if frames_tx.send(FrameMsg::Frame(frame)).await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = frames_tx.send(FrameMsg::End(format!("{error:#}"))).await;
                    return;
                }
            }
        }
    });

    let mut session = Session {
        events: events.clone(),
        writer: stdin,
        frames: frames_rx,
        _child: child,
        next_id: 0,
        pending: BTreeMap::new(),
        last_frame: Instant::now(),
        events_channel: None,
        event_lines: Vec::new(),
        streams: Vec::new(),
        intents,
        display,
        commands,
    };
    session.run().await
}

impl Session<'_> {
    async fn run(&mut self) -> Outcome {
        // 先订阅事件，再 attach：快照与后续增量之间不会漏变化。
        let subscribe = match self.rpc("events.subscribe", json!({})).await {
            Ok(value) => value,
            Err(error) => return self.lost(format!("events.subscribe failed: {error}")),
        };
        let Some(subscribe) = SubscribeResult::parse(&subscribe) else {
            return Outcome::Fatal("events.subscribe result shape changed".to_string());
        };
        self.events_channel = subscribe.channel;
        let _ = self.events.send(NetEvent::Snapshot {
            seq: subscribe.seq,
            terminals: subscribe.terminals,
        });

        // 重连后恢复每个 Pane 的 attach；缺失的终端报告错误，由 core 负责 keep 恢复。
        let panes: Vec<u32> = self
            .intents
            .iter()
            .filter(|(_, intent)| intent.ready())
            .map(|(pane, _)| *pane)
            .collect();
        for pane in panes {
            if let Err(error) = self.ensure_stream(pane).await {
                let _ = self.events.send(NetEvent::AttachClosed {
                    pane,
                    channel: 0,
                    reason: error.to_string(),
                    program_exit: false,
                });
            }
        }

        loop {
            tokio::select! {
                message = self.frames.recv() => match message {
                    None => return self.lost("core closed the connection".to_string()),
                    Some(FrameMsg::End(reason)) => return self.lost(reason),
                    Some(FrameMsg::Frame(frame)) => {
                        if let Err(error) = self.on_frame(frame).await {
                            return self.lost(format!("{error:#}"));
                        }
                    }
                },
                command = self.commands.recv() => match command {
                    None | Some(Command::Shutdown) => {
                        self.close_all().await;
                        return Outcome::Shutdown;
                    }
                    Some(command) => {
                        if let Err(error) = self.on_command(command).await {
                            return self.lost(format!("{error:#}"));
                        }
                    }
                },
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    if self.last_frame.elapsed() >= LIVENESS_TIMEOUT {return self.lost("no frames for a long time".to_string());}
                    let expired:Vec<_>=self.pending.iter().filter(|(_,(_,_,deadline))|*deadline<=Instant::now()).map(|(id,_)|*id).collect();
                    for id in expired {if let Some((tag,method,_))=self.pending.remove(&id){let _=self.events.send(NetEvent::Reply{tag,result:Err(RpcFailure{kind:"timeout".into(),message:format!("{method} timed out")})});}}
                }
            }
        }
    }

    fn lost(&self, reason: String) -> Outcome {
        for (tag, _, _) in self.pending.values() {
            let _ = self.events.send(NetEvent::Reply {
                tag: tag.clone(),
                result: Err(RpcFailure {
                    kind: "offline".into(),
                    message: reason.clone(),
                }),
            });
        }
        Outcome::Lost {
            reason,
            was_connected: true,
        }
    }

    async fn on_command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Shutdown => {}
            Command::Spawn {
                keep,
                pane,
                target,
                argv,
                cwd,
                on,
                cols,
                rows,
            } => {
                let intent = self
                    .intents
                    .entry(pane)
                    .or_insert_with(|| PaneIntent::new(target.clone(), cols, rows));
                if target.is_some() {
                    intent.target = target;
                }
                if !argv.is_empty() {
                    intent.argv = argv;
                }
                if cwd.is_some() {
                    intent.cwd = cwd;
                }
                if on.is_some() {
                    intent.on = on;
                }
                intent.keep = keep;
                intent.size(cols, rows);
                intent.wanted = true;
                if self.stream_by_pane(pane).is_none() {
                    if let Err(error) = self.ensure_stream(pane).await {
                        let _ = self.events.send(NetEvent::AttachClosed {
                            pane,
                            channel: 0,
                            reason: error.to_string(),
                            program_exit: false,
                        });
                    }
                }
            }
            Command::Display(display) => {
                *self.display = display;
                let payload = serde_json::to_vec(self.display)?;
                let channels: Vec<_> = self.streams.iter().map(|s| s.channel).collect();
                for channel in channels {
                    self.write(&codec::facts(channel, &payload)?).await?;
                }
            }
            Command::Resize { pane, cols, rows } => {
                if let Some(intent) = self.intents.get_mut(&pane) {
                    intent.size(cols, rows);
                }
                let (cols, rows) = (cols.max(2), rows.max(2));
                if let Some(index) = self.stream_by_pane(pane) {
                    if (self.streams[index].cols, self.streams[index].rows) != (cols, rows) {
                        let channel = self.streams[index].channel;
                        self.streams[index].cols = cols;
                        self.streams[index].rows = rows;
                        self.write(&codec::resize(channel, cols, rows)?).await?;
                    }
                }
            }
            Command::Input { pane, bytes } => {
                let Some(index) = self.stream_by_pane(pane) else {
                    return Ok(());
                };
                self.streams[index].outbox.extend_from_slice(&bytes);
                if self.streams[index].outbox.len() > OUTBOX_MAX {
                    let excess = self.streams[index].outbox.len() - OUTBOX_MAX;
                    self.streams[index].outbox.drain(..excess);
                }
                self.flush_stream(index).await?;
            }
            Command::Focus { pane, focused } => {
                if let Some(index) = self.stream_by_pane(pane) {
                    let channel = self.streams[index].channel;
                    self.write(&codec::focus(channel, focused)?).await?;
                }
            }
            Command::Close { pane } => {
                let closed = self
                    .stream_by_pane(pane)
                    .map(|index| self.streams.remove(index));
                if let Some(intent) = self.intents.get_mut(&pane) {
                    intent.wanted = false;
                }
                if let Some(stream) = closed {
                    // viewer→core 的 CLOSE 是单向的，core 不会回帧：本地确认。
                    self.write(&codec::close(stream.channel)?).await?;
                    let _ = self.events.send(NetEvent::AttachClosed {
                        pane,
                        channel: stream.channel,
                        reason: "detached".to_string(),
                        program_exit: false,
                    });
                }
            }
            Command::Credit { channel, bytes } => {
                self.write(&codec::credit(channel, bytes)?).await?;
            }
            Command::Call {
                tag,
                method,
                params,
            } => {
                if self.pending.len() >= 128 {
                    let _ = self.events.send(NetEvent::Reply {
                        tag,
                        result: Err(RpcFailure {
                            kind: "busy".into(),
                            message: "too many pending calls".into(),
                        }),
                    });
                    return Ok(());
                }
                self.next_id += 1;
                let id = self.next_id;
                let seconds = if method == "terminal.wait" || method == "machine.exec" {
                    params
                        .get("timeout")
                        .and_then(Value::as_f64)
                        .unwrap_or(60.0)
                        .clamp(0.0, 600.0)
                        + 5.0
                } else {
                    RPC_TIMEOUT.as_secs_f64()
                };
                let frame = codec::encode(
                    0,
                    DATA,
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                        .to_string()
                        .as_bytes(),
                )?;
                self.pending.insert(
                    id,
                    (
                        tag,
                        method,
                        Instant::now() + Duration::from_secs_f64(seconds),
                    ),
                );
                self.write(&frame).await?;
            }
        }
        Ok(())
    }

    /// 处理一帧。返回 `Some(reply)` 表示这是一条 RPC 回复，交给 `rpc` 匹配 `id`。
    async fn on_frame(&mut self, frame: Frame) -> Result<Option<Value>> {
        self.last_frame = Instant::now();
        match frame.kind {
            DATA => match frame.channel {
                0 => {
                    let reply = serde_json::from_slice::<Value>(&frame.payload).ok();
                    if let Some(value) = &reply {
                        if let Some((tag, method, _)) = value
                            .get("id")
                            .and_then(Value::as_u64)
                            .and_then(|id| self.pending.remove(&id))
                        {
                            let result = if let Some(error) = value.get("error") {
                                let error = RpcError::from_error(&method, error);
                                Err(RpcFailure {
                                    kind: error.kind,
                                    message: error.message,
                                })
                            } else {
                                Ok(value.get("result").cloned().unwrap_or(Value::Null))
                            };
                            let _ = self.events.send(NetEvent::Reply { tag, result });
                            return Ok(None);
                        }
                    }
                    Ok(reply)
                }
                channel if Some(channel) == self.events_channel => {
                    self.on_events(&frame.payload).await?;
                    Ok(None)
                }
                channel => match self.stream_index_by_channel(channel) {
                    Some(_) => {
                        // credit 等 UI 写进本地 VT 之后再还。
                        let _ = self.events.send(NetEvent::Output {
                            channel,
                            bytes: frame.payload,
                        });
                        Ok(None)
                    }
                    None => {
                        // 不认识的流：照样还 credit，否则它会静默停住。
                        self.write(&codec::credit(channel, frame.payload.len())?)
                            .await?;
                        Ok(None)
                    }
                },
            },
            codec::CREDIT => {
                if let Some(index) = self.stream_index_by_channel(frame.channel) {
                    if frame.payload.len() >= 4 {
                        let granted = u32::from_le_bytes([
                            frame.payload[0],
                            frame.payload[1],
                            frame.payload[2],
                            frame.payload[3],
                        ]);
                        self.streams[index].credit += i64::from(granted);
                    }
                    self.flush_stream(index).await?;
                }
                Ok(None)
            }
            codec::CLOSE => {
                if let Some(index) = self.stream_index_by_channel(frame.channel) {
                    let stream = self.streams.remove(index);
                    let program_exit = proto::close_is_program_exit(&frame.payload);
                    let reason = proto::close_reason(&frame.payload);
                    if let Some(intent) = self.intents.get_mut(&stream.pane) {
                        // 程序自己结束就不重来；连接或 core 层面断开时保留意图。
                        intent.wanted = !program_exit;
                    }
                    let _ = self.events.send(NetEvent::AttachClosed {
                        pane: stream.pane,
                        channel: stream.channel,
                        reason,
                        program_exit,
                    });
                    Ok(None)
                } else if frame.channel == 0 {
                    anyhow::bail!("core closed the connection");
                } else {
                    Ok(None)
                }
            }
            codec::RESIZE => {
                if frame.payload.len() >= 4 {
                    if let Some(index) = self.stream_index_by_channel(frame.channel) {
                        let cols = u16::from_le_bytes([frame.payload[0], frame.payload[1]]);
                        let rows = u16::from_le_bytes([frame.payload[2], frame.payload[3]]);
                        self.streams[index].cols = cols;
                        self.streams[index].rows = rows;
                        let _ = self.events.send(NetEvent::AttachResized {
                            channel: frame.channel,
                            cols,
                            rows,
                        });
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn on_events(&mut self, payload: &[u8]) -> Result<()> {
        self.event_lines.extend_from_slice(payload);
        while let Some(position) = self.event_lines.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.event_lines.drain(..=position).collect();
            let line = &line[..line.len() - 1];
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            // heartbeat 只是证明连接活着，不需要分发。
            if value.get("method").and_then(Value::as_str) != Some("event") {
                continue;
            }
            let Some(params) = value.get("params") else {
                continue;
            };
            let Some(event) = Event::parse(params) else {
                continue;
            };
            if event.is_resync() {
                if let Some(terminals) = params.pointer("/data/terminals").and_then(Value::as_array)
                {
                    let terminals = terminals
                        .iter()
                        .filter_map(TerminalSummary::parse)
                        .collect();
                    let _ = self.events.send(NetEvent::Snapshot {
                        seq: event.seq,
                        terminals,
                    });
                }
            }
            let _ = self.events.send(NetEvent::Event { event });
        }
        if let Some(channel) = self.events_channel {
            self.write(&codec::credit(channel, payload.len())?).await?;
        }
        Ok(())
    }

    /// 像 core 发一条 JSON-RPC，期间到达的其他帧照常分发。
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let frame = codec::encode(0, DATA, request.to_string().as_bytes())?;
        self.write(&frame).await.map_err(RpcError::io)?;

        let deadline = Instant::now() + RPC_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RpcError::new("timeout", format!("{method} timed out")));
            }
            let message = match tokio::time::timeout(remaining, self.frames.recv()).await {
                Ok(Some(message)) => message,
                Ok(None) => return Err(RpcError::new("closed", "core closed the connection")),
                Err(_) => {
                    return Err(RpcError::new("timeout", format!("{method} timed out")));
                }
            };
            let frame = match message {
                FrameMsg::Frame(frame) => frame,
                FrameMsg::End(reason) => return Err(RpcError::new("closed", reason)),
            };
            let Some(reply) = self.on_frame(frame).await.map_err(RpcError::io)? else {
                continue;
            };
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                return Err(RpcError::from_error(method, error));
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// 保证这个 Pane 有一条 attach 流。
    async fn ensure_stream(&mut self, pane: u32) -> Result<(), RpcError> {
        if self.stream_by_pane(pane).is_some() {
            return Ok(());
        }
        let Some(intent) = self.intents.get(&pane) else {
            return Ok(());
        };
        let (cols, rows) = (intent.cols, intent.rows);
        let launch = AttachLaunch {
            keep: intent.keep,
            target: intent.target.clone(),
            argv: intent.argv.clone(),
            cwd: intent.cwd.clone(),
            on: intent.on.clone(),
        };
        self.attach_once(pane, launch, cols, rows).await
    }

    async fn attach_once(
        &mut self,
        pane: u32,
        launch: AttachLaunch,
        cols: u16,
        rows: u16,
    ) -> Result<(), RpcError> {
        let target = match launch.target {
            Some(target) => target,
            None => {
                let mut params = json!({"keep":launch.keep});
                if !launch.argv.is_empty() {
                    params["argv"] = json!(launch.argv);
                }
                if let Some(cwd) = launch.cwd {
                    params["cwd"] = json!(cwd);
                }
                if let Some(on) = launch.on {
                    params["on"] = json!(on);
                }
                let run = self.rpc("terminal.run", params).await?;
                let address = run
                    .get("address")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| RpcError::new("protocol", "terminal.run returned no address"))?;
                if let Some(intent) = self.intents.get_mut(&pane) {
                    intent.target = Some(address.clone());
                }
                address
            }
        };
        let display = self.display.clone();
        let result = self
            .rpc(
                "terminal.attach",
                json!({
                    "terminal": target,
                    "cols": cols.max(2),
                    "rows": rows.max(2),
                    "history": 0,
                    "observe": false,
                    "display": display,
                }),
            )
            .await?;
        let attach = AttachResult::parse(&result)
            .ok_or_else(|| RpcError::new("protocol", "attach result shape changed"))?;

        let label = attach
            .terminal
            .as_ref()
            .map(TerminalSummary::label)
            .unwrap_or(target.as_str())
            .to_string();
        let terminal = attach
            .terminal
            .as_ref()
            .map(|terminal| terminal.address.clone())
            .unwrap_or_else(|| target.clone());
        if let Some(intent) = self.intents.get_mut(&pane) {
            intent.target = Some(terminal.clone());
        }

        self.streams.push(Stream {
            pane,
            channel: attach.channel,
            cols: cols.max(2),
            rows: rows.max(2),
            credit: WINDOW as i64,
            outbox: Vec::new(),
        });
        // attach 之后立刻上报 viewer 属性：颜色与像素尺寸查询依赖它。
        let facts = codec::facts(attach.channel, &serde_json::to_vec(&display)?)?;
        self.write(&facts).await.map_err(RpcError::io)?;
        let _ = self.events.send(NetEvent::Attached {
            pane,
            channel: attach.channel,
            terminal,
            label,
            cols: self
                .streams
                .last()
                .map(|stream| stream.cols)
                .unwrap_or(cols),
            rows: self
                .streams
                .last()
                .map(|stream| stream.rows)
                .unwrap_or(rows),
            resize_notifications: attach.resize_notifications,
        });
        Ok(())
    }

    async fn close_all(&mut self) {
        let channels: Vec<u32> = self
            .streams
            .drain(..)
            .map(|stream| stream.channel)
            .collect();
        for channel in channels {
            if let Ok(frame) = codec::close(channel) {
                let _ = self.write(&frame).await;
            }
        }
    }

    async fn flush_stream(&mut self, index: usize) -> Result<()> {
        loop {
            let (channel, mut credit) = (self.streams[index].channel, self.streams[index].credit);
            if self.streams[index].outbox.is_empty() || credit <= 0 {
                return Ok(());
            }
            let take = self.streams[index]
                .outbox
                .len()
                .min(CHUNK)
                .min(credit as usize);
            let bytes: Vec<u8> = self.streams[index].outbox.drain(..take).collect();
            credit -= take as i64;
            self.streams[index].credit = credit;
            self.write(&codec::encode(channel, DATA, &bytes)?).await?;
        }
    }

    fn stream_by_pane(&self, pane: u32) -> Option<usize> {
        self.streams.iter().position(|stream| stream.pane == pane)
    }

    fn stream_index_by_channel(&self, channel: u32) -> Option<usize> {
        self.streams
            .iter()
            .position(|stream| stream.channel == channel)
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        client::write_frame(&mut self.writer, bytes).await
    }
}

/// 一次 attach 需要的启动参数。
#[derive(Debug, Clone)]
struct AttachLaunch {
    keep: bool,
    target: Option<String>,
    argv: Vec<String>,
    cwd: Option<String>,
    on: Option<String>,
}

/// RPC 失败，保留 core 的稳定 `kind`，供界面重试或报告。
#[derive(Debug)]
pub struct RpcError {
    pub kind: String,
    pub message: String,
}

impl RpcError {
    fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            message: message.into(),
        }
    }

    fn io(error: anyhow::Error) -> Self {
        Self::new("io", format!("{error:#}"))
    }

    fn from_error(method: &str, error: &Value) -> Self {
        let kind = error
            .pointer("/data/kind")
            .and_then(Value::as_str)
            .unwrap_or("error");
        Self::new(kind, format!("{method}: {}", client::describe_error(error)))
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for RpcError {}

impl From<codec::CodecError> for RpcError {
    fn from(error: codec::CodecError) -> Self {
        Self::new("codec", error.to_string())
    }
}

impl From<serde_json::Error> for RpcError {
    fn from(error: serde_json::Error) -> Self {
        Self::new("json", error.to_string())
    }
}
