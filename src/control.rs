//! Local command transport. The UI thread alone mutates application state.
//!
//! 两个入口共用同一个 Unix socket：`method` 请求一问一答；`method = "events"`
//! 升级为长连接，按 `plugin` 与 `kinds` 过滤后逐行输出 NDJSON。每个订阅者一条
//! 有界队列（[`EVENT_QUEUE`]），满了丢弃并补发 `events.dropped`；客户端断开
//! 即停止推送。
//!
//! 每条事件带单调递增的 `revision`；`events` 请求可带 `since`，首帧推当前快照
//! （带 `revision`），此后只推更新的修订号；`since` 已被 [`HISTORY`] 裁剪掉时
//! 首帧标 `resync: true`。
use crate::config::Config;
use crate::i18n::t;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// 每个事件订阅者的队列上限。
pub const EVENT_QUEUE: usize = 256;
/// 给 `--since` 补发保留的事件条数；更旧的 revision 回落到快照。
pub const HISTORY: usize = 1024;

pub struct Request {
    pub body: Value,
    pub reply: mpsc::Sender<Value>,
}

pub fn socket_path(config: &Config) -> PathBuf {
    if let Ok(path) = std::env::var("HCTUI_SOCKET") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    use std::hash::{Hash, Hasher};
    let session = config
        .session
        .clone()
        .or_else(|| std::env::var("HC_SESSION").ok())
        .unwrap_or_default();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    session.hash(&mut hash);
    config
        .state_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(format!("control-{:x}.sock", hash.finish()))
}

/// 一个已登记的事件订阅者。
struct Subscriber {
    id: u64,
    /// 只看这个插件的事件；`None` 表示不过滤。
    plugin: Option<String>,
    /// 只看这些 kind；空表示全部。
    kinds: Vec<String>,
    sender: mpsc::SyncSender<Value>,
    /// 队列满而丢弃的条数，由推送线程补发成 `events.dropped`。
    dropped: Arc<AtomicU64>,
    /// 推送线程看到客户端断开或界面退出时置位。
    closed: Arc<AtomicBool>,
}

#[derive(Default)]
struct PublisherInner {
    next: AtomicU64,
    /// 最新事件的 revision；起点由 core 的 `events.subscribe` 序号垫高。
    revision: AtomicU64,
    /// 最近 [`HISTORY`] 条事件，供 `--since` 补发。
    history: Mutex<VecDeque<Value>>,
    subscribers: Mutex<Vec<Subscriber>>,
}

/// 事件发布端：App 在状态变化时调用 [`Publisher::publish`]。
#[derive(Clone, Default)]
pub struct Publisher {
    inner: Arc<PublisherInner>,
}

impl Publisher {
    /// 推一条事件；不符合某个订阅者过滤条件时不入队。
    ///
    /// 每条事件先打上递增的 `revision` 再分发，因此事件流、补发与快照用同一
    /// 把尺子。
    pub fn publish(&self, event: &Value) {
        let revision = self.inner.revision.fetch_add(1, Ordering::SeqCst) + 1;
        let mut event = event.clone();
        event["revision"] = json!(revision);
        {
            let mut history = self
                .inner
                .history
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            history.push_back(event.clone());
            while history.len() > HISTORY {
                history.pop_front();
            }
        }
        let event = &event;
        let mut subscribers = self
            .inner
            .subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        subscribers.retain(|subscriber| !subscriber.closed.load(Ordering::Relaxed));
        for subscriber in subscribers.iter() {
            if !matches(subscriber.plugin.as_deref(), &subscriber.kinds, event) {
                continue;
            }
            if subscriber.sender.try_send(event.clone()).is_err() {
                subscriber.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// 当前订阅者数量；测试与诊断用。
    pub fn subscribers(&self) -> usize {
        let subscribers = self
            .inner
            .subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        subscribers
            .iter()
            .filter(|subscriber| !subscriber.closed.load(Ordering::Relaxed))
            .count()
    }

    /// 当前快照该报的 revision：最新事件号。
    pub fn revision(&self) -> u64 {
        self.inner.revision.load(Ordering::SeqCst)
    }

    /// 把 revision 的起点垫到 core 的状态序号；只升不降，重连不会让 revision 回退。
    pub fn seed_revision(&self, seq: u64) {
        self.inner.revision.fetch_max(seq, Ordering::SeqCst);
    }

    /// `since` 是否已经接不上：早于保留区间的第一条，或快于当前 revision
    /// （上一个进程留下的号）。两种情况都由首帧的 `resync` 标出来，不静默。
    fn stale(&self, since: u64) -> bool {
        let history = self
            .inner
            .history
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let oldest = history
            .front()
            .and_then(|event| event.get("revision").and_then(Value::as_u64));
        match oldest {
            Some(oldest) => since + 1 < oldest || since > self.revision(),
            None => since > self.revision(),
        }
    }

    fn subscribe(
        &self,
        plugin: Option<String>,
        kinds: Vec<String>,
    ) -> (u64, mpsc::Receiver<Value>, Arc<AtomicU64>, Arc<AtomicBool>) {
        let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        self.inner
            .subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(Subscriber {
                id,
                plugin,
                kinds,
                sender,
                dropped: Arc::clone(&dropped),
                closed: Arc::clone(&closed),
            });
        (id, receiver, dropped, closed)
    }

    fn unsubscribe(&self, id: u64) {
        self.inner
            .subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|subscriber| subscriber.id != id);
    }
}

#[cfg(unix)]
pub struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
#[cfg(not(unix))]
pub struct Server;

#[cfg(unix)]
impl Server {
    pub fn start(config: &Config) -> Result<(Self, mpsc::Receiver<Request>, Publisher)> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        };
        let path = socket_path(config);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                anyhow::bail!(t("another TUI already accepts commands for this session"));
            }
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let (sender, receiver) = mpsc::sync_channel::<Request>(32);
        let stop = Arc::new(AtomicBool::new(false));
        let publisher = Publisher::default();
        let shutdown = Arc::clone(&stop);
        let events = publisher.clone();
        let worker = std::thread::spawn(move || {
            while !shutdown.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => break,
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                let read =
                    std::io::Read::take(BufReader::new(&stream), 1024 * 1024).read_line(&mut line);
                let Some(body) = read
                    .ok()
                    .filter(|n| *n > 0)
                    .and_then(|_| serde_json::from_str::<Value>(&line).ok())
                else {
                    let _ = writeln!(
                        stream,
                        "{}",
                        json!({"error": t("expected one JSON request line")})
                    );
                    continue;
                };
                if body.get("method").and_then(Value::as_str) == Some("events") {
                    let filter = EventFilter::from_request(&body);
                    let events = events.clone();
                    let shutdown = Arc::clone(&shutdown);
                    // 长连接不阻塞其它控制命令。
                    let requests = sender.clone();
                    std::thread::spawn(move || {
                        stream_events(stream, &events, &shutdown, filter, &requests)
                    });
                    continue;
                }
                let (reply, answer) = mpsc::channel();
                let response = if sender.try_send(Request { body, reply }).is_err() {
                    json!({"error": t("command queue is full")})
                } else {
                    answer
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap_or_else(|_| json!({"error": t("UI did not answer")}))
                };
                let _ = writeln!(stream, "{response}");
            }
        });
        Ok((
            Self {
                path,
                stop,
                worker: Some(worker),
            },
            receiver,
            publisher,
        ))
    }
}

/// `method = "events"` 的过滤条件。
#[derive(Debug, Clone, Default)]
struct EventFilter {
    plugin: Option<String>,
    kinds: Vec<String>,
    /// 只推 revision 大于它的新事件；`None` 表示不补发。
    since: Option<u64>,
}

impl EventFilter {
    fn from_request(body: &Value) -> EventFilter {
        let plugin = body
            .get("plugin")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|plugin| !plugin.is_empty())
            .map(str::to_string);
        let kinds = body
            .get("kinds")
            .map(|kinds| match kinds {
                Value::Array(items) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .flat_map(|kind| kind.split(','))
                    .map(str::trim)
                    .filter(|kind| !kind.is_empty())
                    .map(str::to_string)
                    .collect(),
                Value::String(text) => text
                    .split(',')
                    .map(str::trim)
                    .filter(|kind| !kind.is_empty())
                    .map(str::to_string)
                    .collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default();
        let since = body.get("since").and_then(Value::as_u64);
        EventFilter {
            plugin,
            kinds,
            since,
        }
    }
}

/// 一条事件是否命中 `plugin` 与 `kinds` 过滤；空条件表示全收。
fn matches(plugin: Option<&str>, kinds: &[String], event: &Value) -> bool {
    let kind = event.get("kind").and_then(Value::as_str).unwrap_or("");
    if !kinds.is_empty() && !kinds.iter().any(|wanted| wanted == kind) {
        return false;
    }
    match plugin {
        Some(wanted) => event.get("plugin").and_then(Value::as_str) == Some(wanted),
        None => true,
    }
}

/// 一个订阅连接的生命周期：先回一条确认，再逐行推送，断开或界面退出即结束。
/// 结束时记一行 `outcome="stream_closed"`。
#[cfg(unix)]
fn stream_events(
    stream: std::os::unix::net::UnixStream,
    publisher: &Publisher,
    stop: &Arc<AtomicBool>,
    filter: EventFilter,
    requests: &mpsc::SyncSender<Request>,
) {
    let log = crate::api_log::Request::new(
        "control",
        crate::api_log::next_request_id("stream"),
        "events.subscribe",
    );
    stream_events_once(stream, publisher, stop, filter, requests);
    log.finish(crate::api_log::Outcome::StreamClosed);
}

/// 向界面要一份当前快照；命令队列满或超时时把原因当帧报出去，不静默。
#[cfg(unix)]
fn snapshot_frame(
    requests: &mpsc::SyncSender<Request>,
    publisher: &Publisher,
    resync: bool,
) -> Value {
    let (reply, answer) = mpsc::channel();
    let body = json!({"method": "snapshot"});
    if requests.try_send(Request { body, reply }).is_err() {
        return json!({
            "kind": "events.snapshot",
            "revision": publisher.revision(),
            "resync": true,
            "error": t("command queue is full"),
        });
    }
    let snapshot = answer
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| json!({"error": t("UI did not answer")}));
    let revision = snapshot
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| publisher.revision());
    match snapshot.get("error") {
        Some(error) => json!({
            "kind": "events.snapshot",
            "revision": revision,
            "resync": resync,
            "error": error,
        }),
        None => json!({
            "kind": "events.snapshot",
            "revision": revision,
            "resync": resync,
            "snapshot": snapshot,
        }),
    }
}

#[cfg(unix)]
fn stream_events_once(
    mut stream: std::os::unix::net::UnixStream,
    publisher: &Publisher,
    stop: &Arc<AtomicBool>,
    filter: EventFilter,
    requests: &mpsc::SyncSender<Request>,
) {
    use std::io::Read;
    let (id, receiver, dropped, closed) =
        publisher.subscribe(filter.plugin.clone(), filter.kinds.clone());
    let _ = stream.set_nonblocking(true);
    if write_line(
        &mut stream,
        &json!({"subscribed": true, "queue": EVENT_QUEUE}),
    )
    .is_err()
    {
        closed.store(true, Ordering::Relaxed);
        publisher.unsubscribe(id);
        return;
    }
    // `--since`：首帧总是当前快照（带 revision）。快照已经把更早的状态包含进去，
    // 所以此后只推 revision 比快照与客户端手里的值都大的事件。
    let mut floor = filter.since;
    if let Some(since) = filter.since {
        let resync = publisher.stale(since);
        let frame = snapshot_frame(requests, publisher, resync);
        if write_line(&mut stream, &frame).is_err() {
            closed.store(true, Ordering::Relaxed);
            publisher.unsubscribe(id);
            return;
        }
        let revision = frame
            .get("revision")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| publisher.revision());
        floor = Some(since.max(revision));
    }
    while !stop.load(Ordering::Relaxed) && !closed.load(Ordering::Relaxed) {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                let revision = event.get("revision").and_then(Value::as_u64).unwrap_or(0);
                // 快照覆盖过的 revision 不再重复推。
                if floor.is_some_and(|floor| revision <= floor) {
                    continue;
                }
                if write_line(&mut stream, &event).is_err() {
                    break;
                }
                floor = floor.map(|floor| floor.max(revision));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let lost = dropped.swap(0, Ordering::Relaxed);
        if lost > 0
            && write_line(
                &mut stream,
                &json!({"kind": "events.dropped", "count": lost}),
            )
            .is_err()
        {
            break;
        }
        // 没有积压时探一下对端：读到 0 就是客户端已经断开。
        let mut probe = [0u8; 64];
        match stream.read(&mut probe) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    closed.store(true, Ordering::Relaxed);
    publisher.unsubscribe(id);
}

/// 非阻塞 socket 上的整行写入；缓冲区满时小睡重试，直到成功或停止。
#[cfg(unix)]
fn write_line(stream: &mut std::os::unix::net::UnixStream, value: &Value) -> std::io::Result<()> {
    use std::io::Write;
    let mut text = serde_json::to_vec(value)?;
    text.push(b'\n');
    let mut rest = text.as_slice();
    while !rest.is_empty() {
        match stream.write(rest) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    t("client went away"),
                ))
            }
            Ok(written) => rest = &rest[written..],
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(unix)]
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}
#[cfg(not(unix))]
impl Server {
    pub fn start(_config: &Config) -> Result<(Self, mpsc::Receiver<Request>, Publisher)> {
        anyhow::bail!(t("local command sockets require Unix"))
    }
}

#[cfg(unix)]
pub fn call(config: &Config, request: Value) -> Result<Value> {
    let result = call_raw(config, request)?;
    if let Some(error) = result.get("error") {
        anyhow::bail!("{error}");
    }
    Ok(result)
}

/// 与 [`call`] 相同，但把带 `error` 的应答也原样返回给调用方。
#[cfg(unix)]
pub fn call_raw(config: &Config, request: Value) -> Result<Value> {
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let request_id = request
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| crate::api_log::next_request_id("req"));
    let log = crate::api_log::Request::new("control", request_id, method);
    let answer = call_raw_once(config, request);
    log.finish(match &answer {
        Ok(value) if value.get("error").is_none() => crate::api_log::Outcome::Ok,
        _ => crate::api_log::Outcome::Err,
    });
    answer
}

#[cfg(unix)]
fn call_raw_once(config: &Config, request: Value) -> Result<Value> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket_path(config))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    writeln!(stream, "{request}")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}
#[cfg(not(unix))]
pub fn call(_config: &Config, _request: Value) -> Result<Value> {
    anyhow::bail!(t("local command sockets require Unix"))
}

#[cfg(not(unix))]
pub fn call_raw(_config: &Config, _request: Value) -> Result<Value> {
    anyhow::bail!(t("local command sockets require Unix"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_by_kind_and_plugin_and_counts_overflow() {
        let publisher = Publisher::default();
        let (_, all, _, _) = publisher.subscribe(None, Vec::new());
        let (_, status_only, _, _) = publisher.subscribe(None, vec!["terminal.status".into()]);
        let (_, plugin_only, _, _) = publisher.subscribe(Some("dev.example".into()), Vec::new());
        let (_, never_read, _, _) = publisher.subscribe(None, Vec::new());
        publisher
            .publish(&json!({"kind":"terminal.status","plugin":"dev.example","terminal":"t1"}));
        publisher.publish(&json!({"kind":"terminal.turn","terminal":"t1"}));
        assert_eq!(all.try_recv().unwrap()["kind"], "terminal.status");
        assert_eq!(all.try_recv().unwrap()["kind"], "terminal.turn");
        assert_eq!(status_only.try_recv().unwrap()["kind"], "terminal.status");
        assert!(status_only.try_recv().is_err());
        assert_eq!(plugin_only.try_recv().unwrap()["kind"], "terminal.status");
        assert!(plugin_only.try_recv().is_err());
        // 未读的订阅者队列满了以后只能丢弃并计数。
        for _ in 0..EVENT_QUEUE + 10 {
            publisher.publish(&json!({"kind":"terminal.turn","terminal":"t1"}));
        }
        assert!(never_read.try_recv().is_ok());
        assert_eq!(publisher.subscribers(), 4);
    }

    #[test]
    fn event_filter_accepts_arrays_and_comma_lists() {
        let from_array = EventFilter::from_request(&json!({
            "method": "events",
            "plugin": "dev.example",
            "kinds": ["terminal.status", "terminal.turn"],
        }));
        assert_eq!(from_array.plugin.as_deref(), Some("dev.example"));
        assert_eq!(from_array.kinds, vec!["terminal.status", "terminal.turn"]);
        let from_text = EventFilter::from_request(&json!({
            "method": "events",
            "kinds": "terminal.status, terminal.turn",
        }));
        assert_eq!(from_text.plugin, None);
        assert_eq!(from_text.kinds, vec!["terminal.status", "terminal.turn"]);
        let empty = EventFilter::from_request(&json!({"method":"events"}));
        assert!(empty.kinds.is_empty() && empty.plugin.is_none());
    }
}
