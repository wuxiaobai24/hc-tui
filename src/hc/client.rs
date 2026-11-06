//! 到 core 的连接：`hc connect` 子进程 + framed 协议。
//!
//! 这一层只提供「打开连接」和「读写一帧」的原语：
//! - 明文 `hello` 握手完成后所有帧都按 [`codec`] 解析；
//! - 读端与写端可以分开持有，读帧绝不与写帧交错（见 plan v1.1 §2.1）；
//! - `hc connect` 在 core 未运行时自动拉起（`connect_or_start`）。

use std::process::Stdio;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::codec::{self, Frame, DATA};
use crate::i18n::{t, t1, t2};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct CoreInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
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

/// 握手阶段失败时区分「换 core 也没用」和「等一下再试」。
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// 协议代次不匹配或必需方法缺失：重连只会重复失败。
    #[error("{0}")]
    Fatal(anyhow::Error),
    /// 子进程起不来、管道断了：可以退避重连。
    #[error("{0}")]
    Retry(anyhow::Error),
}

/// 一条已经完成握手的连接，读端与写端交给调用方各自持有。
pub struct Parts {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: BufReader<ChildStdout>,
    pub hello: Hello,
}

/// 本客户端要求的 core 方法，缺一个就报版本不匹配。
const REQUIRED_METHODS: [&str; 4] = [
    "terminal.run",
    "terminal.attach",
    "terminal.kill",
    "events.subscribe",
];

/// 启动 `hc connect` 并完成明文 hello，此后连接进入 framed 模式。
pub async fn open(session: Option<&str>) -> Result<Parts, OpenError> {
    let mut command = Command::new("hc");
    command.arg("connect");
    if let Some(session) = session {
        command.arg("--session").arg(session);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|error| {
        OpenError::Retry(anyhow::Error::new(error).context(t(
            "cannot start `hc connect`; is herdr-core installed and on PATH?",
        )))
    })?;
    let mut stdin = child
        .stdin
        .take()
        .context(t("hc connect has no stdin"))
        .map_err(OpenError::Retry)?;
    let mut stdout = BufReader::new(
        child
            .stdout
            .take()
            .context(t("hc connect has no stdout"))
            .map_err(OpenError::Retry)?,
    );

    let hello = hello(&mut stdin, &mut stdout, std::time::Duration::from_secs(10)).await?;
    Ok(Parts {
        child,
        stdin,
        stdout,
        hello,
    })
}

/// 明文一行 `hello` 请求，明文一行回复；之后全是帧。
async fn hello<W: AsyncWrite + Unpin, R: AsyncBufRead + Unpin>(
    stdin: &mut W,
    stdout: &mut R,
    timeout: std::time::Duration,
) -> Result<Hello, OpenError> {
    let line = tokio::time::timeout(timeout, exchange_hello(stdin, stdout))
        .await
        .map_err(|_| OpenError::Retry(anyhow::anyhow!(t("core hello timed out"))))?
        .map_err(OpenError::Retry)?;
    parse_hello(&line).map_err(OpenError::Fatal)
}

async fn exchange_hello<W: AsyncWrite + Unpin, R: AsyncBufRead + Unpin>(
    stdin: &mut W,
    stdout: &mut R,
) -> Result<String> {
    // features 是「另一个 core 拨入」时声明能力用的，客户端不发；能力看返回值。
    let request = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "hello",
        "params": { "generation": 1, "framed": true },
    });
    stdin.write_all(request.to_string().as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;

    let mut line = String::new();
    let read = stdout.read_line(&mut line).await?;
    if read == 0 {
        bail!(t("core closed the connection during hello"));
    }
    Ok(line)
}

fn parse_hello(line: &str) -> Result<Hello> {
    let reply: Value = serde_json::from_str(line).context(t("hello reply is not json"))?;
    if let Some(error) = reply.get("error") {
        bail!(t1("hello failed: {}", describe_error(error)));
    }
    let result = reply
        .get("result")
        .context(t("hello reply has no result"))?;
    let hello: Hello =
        serde_json::from_value(result.clone()).context(t("hello result shape changed"))?;

    if hello.generation != 1 {
        bail!(t1(
            "unsupported protocol generation {} (this client speaks 1); run `hc update`",
            hello.generation
        ));
    }
    if !hello.framed {
        bail!(t("core refused framed mode; run `hc update`"));
    }
    for required in REQUIRED_METHODS {
        if !hello.methods.iter().any(|method| method == required) {
            bail!(t1("core is missing `{}`; run `hc update`", required));
        }
    }
    Ok(hello)
}

/// 读下一帧。半包由 `read_exact` 处理，粘包是它自己多读的部分留在 `BufReader` 里。
pub async fn read_frame(stdout: &mut BufReader<ChildStdout>) -> Result<Frame> {
    let mut header = [0u8; codec::HEADER];
    stdout
        .read_exact(&mut header)
        .await
        .context(t("core closed the connection"))?;
    let (channel, kind, len) = codec::decode_header(&header)?;
    let mut payload = vec![0u8; len];
    stdout
        .read_exact(&mut payload)
        .await
        .context(t("core closed the connection mid-frame"))?;
    Ok(Frame {
        channel,
        kind,
        payload,
    })
}

pub async fn write_frame(stdin: &mut ChildStdin, bytes: &[u8]) -> Result<()> {
    stdin.write_all(bytes).await?;
    stdin.flush().await?;
    Ok(())
}

/// 顺序读写的简单连接，供 `hc-tui probe` 这类一次性诊断使用。
pub struct Conn {
    parts: Parts,
    next_id: u64,
    pub hello: Hello,
}

impl Conn {
    pub async fn connect(session: Option<&str>) -> Result<Self> {
        let parts = match open(session).await {
            Ok(parts) => parts,
            Err(OpenError::Fatal(error) | OpenError::Retry(error)) => return Err(error),
        };
        let hello = parts.hello.clone();
        Ok(Self {
            parts,
            next_id: 0,
            hello,
        })
    }

    /// 发一条 JSON-RPC 并等它的回复。途中到达的其他帧丢弃。
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
                bail!(t2("{} failed: {}", method, describe_error(error)));
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    pub async fn next_frame(&mut self) -> Result<Frame> {
        read_frame(&mut self.parts.stdout).await
    }

    pub async fn write_data(&mut self, channel: u32, payload: &[u8]) -> Result<()> {
        let bytes = codec::encode(channel, DATA, payload)?;
        write_frame(&mut self.parts.stdin, &bytes).await
    }
}

pub fn describe_error(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(t("unknown error"));
    let kind = error
        .pointer("/data/kind")
        .and_then(Value::as_str)
        .unwrap_or("error");
    match error.pointer("/data/hint").and_then(Value::as_str) {
        Some(hint) if !hint.is_empty() => format!("{kind}: {message} ({hint})"),
        _ => format!("{kind}: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn hello_transport_failures_are_retryable() {
        let mut input = tokio::io::sink();
        let mut eof = BufReader::new(tokio::io::empty());
        assert!(matches!(
            hello(&mut input, &mut eof, Duration::from_secs(1)).await,
            Err(OpenError::Retry(_))
        ));
        let (_peer, blocked) = tokio::io::duplex(128);
        let mut blocked = BufReader::new(blocked);
        assert!(matches!(
            hello(&mut input, &mut blocked, Duration::from_millis(20)).await,
            Err(OpenError::Retry(_))
        ));
    }

    #[tokio::test]
    async fn hello_incompatible_protocol_is_fatal() {
        let reply = json!({"result": {
            "generation": 2, "version": "test", "core": {"id": "test", "name": "test"},
            "framed": true, "methods": REQUIRED_METHODS
        }})
        .to_string()
            + "\n";
        let mut input = tokio::io::sink();
        let mut output = BufReader::new(reply.as_bytes());
        assert!(matches!(
            hello(&mut input, &mut output, Duration::from_secs(1)).await,
            Err(OpenError::Fatal(_))
        ));
    }
}
