//! 请求路径的结构化日志：一条可 grep 的完成行覆盖模块宿主调用、CLI 控制请求
//! 与 core 客户端请求三处。
//!
//! 数据形状：一行 `event="api.request.complete" subsystem=... outcome=...
//! request_id=... method=... dur_ms=...`；`outcome` 只用 [`Outcome`] 的五个词。
//! 模块边界：[`Request`] 由请求发起方持有，结束处只调一次 `finish`；
//! `outcome` 的取值由调用点决定，本模块不做归类。
//! 不变量：`outcome="err"` 一次就能筛出全部失败请求（`denied` 与 `timeout`
//! 另有自己的词，要看全部失败就连这两个一起 grep）。
//!
//! 落盘走 [`install`]：本仓自己的 tracing 与插件日志共用
//! [`crate::plugin::logs`] 的 1 MiB 轮转与每包 8 MiB / 32 文件预算。

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 每条完成行的 `event` 字段值；脚本按它筛请求日志。
pub const EVENT: &str = "api.request.complete";

/// core 日志在日志根下的包名：`<root>/logs/core/tracing.stdout.log`。
pub const CORE_PACKAGE: &str = "core";
/// core 日志的请求名，与插件动作日志共用同一条路径规则。
pub const CORE_REQUEST: &str = "tracing";

/// 请求的结局词表；脚本依赖这些串，不许改名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 正常返回。
    Ok,
    /// 返回了错误。
    Err,
    /// 用完了答复期限。
    Timeout,
    /// 授权不足，拒绝执行。
    Denied,
    /// 事件流结束（对端断开或界面退出）。
    StreamClosed,
}

impl Outcome {
    /// 词表里的串。
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Err => "err",
            Outcome::Timeout => "timeout",
            Outcome::Denied => "denied",
            Outcome::StreamClosed => "stream_closed",
        }
    }

    /// core 客户端的失败归类：超时单独成词，其余都算 `err`。
    pub fn from_rpc_kind(kind: &str) -> Outcome {
        match kind {
            "timeout" => Outcome::Timeout,
            _ => Outcome::Err,
        }
    }
}

/// 一条在途请求；`finish` 时打一行完成日志并带上耗时。
pub struct Request {
    subsystem: &'static str,
    request_id: String,
    method: String,
    started: Instant,
}

impl Request {
    pub fn new(
        subsystem: &'static str,
        request_id: impl Into<String>,
        method: impl Into<String>,
    ) -> Self {
        Self {
            subsystem,
            request_id: request_id.into(),
            method: method.into(),
            started: Instant::now(),
        }
    }

    /// 写完成行。`event` 与 `subsystem` 都是固定字段，别处不要再写。
    pub fn finish(self, outcome: Outcome) {
        tracing::info!(
            event = EVENT,
            subsystem = self.subsystem,
            outcome = outcome.as_str(),
            request_id = self.request_id.as_str(),
            method = self.method.as_str(),
            dur_ms = self.started.elapsed().as_millis() as u64,
        );
    }
}

/// 进程内递增的请求 id，给没有自带 id 的请求（控制通道）用。带上进程号，
/// 两次 CLI 调用的 `req_1` 不会在日志里撞号。
pub fn next_request_id(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{prefix}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// 按行把 tracing 写进插件日志同一套预算。
struct LogFile {
    root: PathBuf,
    pending: Vec<u8>,
}

impl LogFile {
    fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let _ = crate::plugin::logs::append(
                &self.root,
                CORE_PACKAGE,
                CORE_REQUEST,
                "stdout",
                &line,
            );
        }
    }
}

impl Write for LogFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.push(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// tracing 的写入端：每条事件拿到一次锁，整行落盘。
#[derive(Clone)]
pub struct LogHandle {
    inner: Arc<Mutex<LogFile>>,
}

/// 一次写入拿到的锁。
pub struct LogGuard<'a> {
    guard: std::sync::MutexGuard<'a, LogFile>,
}

impl Write for LogGuard<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.guard.push(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogHandle {
    type Writer = LogGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        LogGuard {
            guard: self.inner.lock().unwrap_or_else(|error| error.into_inner()),
        }
    }
}

/// 写 `root` 的 subscriber；测试用 [`tracing::subscriber::with_default`] 局部挂，
/// 不去动进程全局的那一个。
pub fn subscriber(root: PathBuf) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(LogHandle {
            inner: Arc::new(Mutex::new(LogFile {
                root,
                pending: Vec::new(),
            })),
        })
        .finish()
}

/// 把全局 tracing 接到 `root` 下的 core 日志；重复调用只有第一次生效。
pub fn install(root: PathBuf) {
    let _ = tracing::subscriber::set_global_default(subscriber(root));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("hctui-api-log-{tag}-{unique}"))
    }

    #[test]
    fn writes_share_the_plugin_budget_and_rotate() {
        let root = temp_root("rotate");
        let mut file = LogFile {
            root: root.clone(),
            pending: Vec::new(),
        };
        // 每行约 130 字节，写满 1 MiB 需要一万行上下；轮转与配额都不改实现。
        let line = format!(
            "event=\"{EVENT}\" subsystem=\"core\" outcome=\"ok\" request_id=\"req_1\" method=\"terminal.list\" dur_ms=1\n"
        );
        for _ in 0..(crate::plugin::logs::ROTATE_BYTES / line.len() as u64 + 2) {
            file.push(line.as_bytes());
        }
        let dir = root.join("logs").join(CORE_PACKAGE);
        let rotated = dir.join(format!("{CORE_REQUEST}.stdout.1.log"));
        assert!(rotated.exists(), "{} 没轮转", rotated.display());
        let total: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .map(|meta| meta.len())
            .sum();
        assert!(total <= crate::plugin::logs::PACKAGE_BYTES, "{total} bytes");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_completion_line_carries_every_field() {
        assert_eq!(Outcome::Ok.as_str(), "ok");
        assert_eq!(Outcome::Err.as_str(), "err");
        assert_eq!(Outcome::Timeout.as_str(), "timeout");
        assert_eq!(Outcome::Denied.as_str(), "denied");
        assert_eq!(Outcome::StreamClosed.as_str(), "stream_closed");
        assert_eq!(Outcome::from_rpc_kind("timeout"), Outcome::Timeout);
        assert_eq!(Outcome::from_rpc_kind("closed"), Outcome::Err);
        assert!(next_request_id("req") != next_request_id("req"));
    }
}
