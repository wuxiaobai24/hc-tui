//! 到 core 的连接：`hc connect` 子进程 + framed 协议。
//!
//! 这是 P0 的骨架版本：握手、单条 RPC、帧读取、credit 发送。
//! 事件分发、并发 RPC、自动重连在 G1 补齐（见 plan v1.1 §3）。

use std::process::Stdio;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::codec::{self, Frame, DATA};

#[derive(Debug, Clone, Deserialize)]
pub struct CoreInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Hello {
    pub generation: u32,
    pub version: String,
    pub core: CoreInfo,
    #[serde(default)]
    pub framed: bool,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

/// 一条到 core 的连接。
///
/// 子进程必须活着：它持有管道，drop 会结束连接（`kill_on_drop`）。
pub struct Conn {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    pub hello: Hello,
}

impl Conn {
    /// 启动 `hc connect` 并完成 hello 握手。
    ///
    /// core 未运行时 `hc connect` 会自动拉起（`connect_or_start`）。
    pub async fn connect(session: Option<&str>) -> Result<Self> {
        let mut command = Command::new("hc");
        command.arg("connect");
        if let Some(session) = session {
            command.arg("--session").arg(session);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .with_context(|| "cannot start `hc connect`; is herdr-core installed and on PATH?")?;
        let stdin = child.stdin.take().context("hc connect has no stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("hc connect has no stdout")?);

        let mut conn = Self {
            _child: child,
            stdin,
            stdout,
            next_id: 0,
            hello: placeholder_hello(),
        };
        conn.handshake().await?;
        Ok(conn)
    }

    async fn handshake(&mut self) -> Result<()> {
        // hello 是明文一行；回复也是明文。之后切到帧。
        let request = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "hello",
            "params": { "generation": 1, "framed": true },
        });
        self.stdin.write_all(request.to_string().as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).await?;
        if read == 0 {
            bail!("core closed the connection during hello");
        }
        let reply: Value = serde_json::from_str(&line).context("hello reply is not json")?;
        if let Some(error) = reply.get("error") {
            bail!("hello failed: {}", describe_error(error));
        }
        let result = reply.get("result").context("hello reply has no result")?;
        let hello: Hello =
            serde_json::from_value(result.clone()).context("hello result shape changed")?;

        if hello.generation != 1 {
            bail!(
                "unsupported protocol generation {} (this client speaks 1); run `hc update`",
                hello.generation
            );
        }
        for required in [
            "terminal.run",
            "terminal.attach",
            "terminal.kill",
            "events.subscribe",
        ] {
            if !hello.methods.iter().any(|m| m == required) {
                bail!("core is missing `{required}`; run `hc update`");
            }
        }

        self.hello = hello;
        Ok(())
    }

    /// 发一条 JSON-RPC 并等它的回复。其他帧（事件、attach 流）暂时丢弃。
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write_data(0, request.to_string().as_bytes()).await?;

        loop {
            let frame = self.next_frame().await?;
            if frame.channel != 0 || frame.kind != DATA {
                continue;
            }
            let reply: Value = match serde_json::from_slice(&frame.payload) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                bail!("{method} failed: {}", describe_error(error));
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// 读下一帧。连接断开时返回错误。
    pub async fn next_frame(&mut self) -> Result<Frame> {
        let mut header = [0u8; codec::HEADER];
        self.stdout
            .read_exact(&mut header)
            .await
            .context("core closed the connection")?;
        let (channel, kind, len) = codec::decode_header(&header)?;
        let mut payload = vec![0u8; len];
        self.stdout
            .read_exact(&mut payload)
            .await
            .context("core closed the connection mid-frame")?;
        Ok(Frame {
            channel,
            kind,
            payload,
        })
    }

    pub async fn write_data(&mut self, channel: u32, payload: &[u8]) -> Result<()> {
        let bytes = codec::encode(channel, DATA, payload)?;
        self.stdin.write_all(&bytes).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// 确认消费字节数。**每条流都要发**，否则 256 KiB 后静默停住。
    pub async fn send_credit(&mut self, channel: u32, bytes: usize) -> Result<()> {
        let frame = codec::credit(channel, bytes)?;
        self.stdin.write_all(&frame).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    pub async fn send_resize(&mut self, channel: u32, cols: u16, rows: u16) -> Result<()> {
        let frame = codec::resize(channel, cols, rows)?;
        self.stdin.write_all(&frame).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

fn placeholder_hello() -> Hello {
    Hello {
        generation: 1,
        version: String::new(),
        core: CoreInfo {
            id: String::new(),
            name: String::new(),
        },
        framed: false,
        methods: Vec::new(),
        features: Vec::new(),
    }
}

fn describe_error(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    let kind = error
        .pointer("/data/kind")
        .and_then(Value::as_str)
        .unwrap_or("error");
    match error.pointer("/data/hint").and_then(Value::as_str) {
        Some(hint) if !hint.is_empty() => format!("{kind}: {message} ({hint})"),
        _ => format!("{kind}: {message}"),
    }
}
