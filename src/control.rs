//! Local command transport. The UI thread alone mutates application state.
use crate::config::Config;
use crate::i18n::t;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

pub struct Request {
    pub body: Value,
    pub reply: Sender<Value>,
}

pub fn socket_path(config: &Config) -> PathBuf {
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

#[cfg(unix)]
pub struct Server {
    path: PathBuf,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
#[cfg(not(unix))]
pub struct Server;

#[cfg(unix)]
impl Server {
    pub fn start(config: &Config) -> Result<(Self, Receiver<Request>)> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        };
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;
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
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let shutdown = stop.clone();
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
                let response = match read
                    .ok()
                    .filter(|n| *n > 0)
                    .and_then(|_| serde_json::from_str::<Value>(&line).ok())
                {
                    Some(body) => {
                        let (reply, answer) = mpsc::channel();
                        if sender.try_send(Request { body, reply }).is_err() {
                            json!({"error": t("command queue is full")})
                        } else {
                            answer
                                .recv_timeout(Duration::from_secs(5))
                                .unwrap_or_else(|_| json!({"error": t("UI did not answer")}))
                        }
                    }
                    None => json!({"error": t("expected one JSON request line")}),
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
        ))
    }
}
#[cfg(unix)]
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}
#[cfg(not(unix))]
impl Server {
    pub fn start(_config: &Config) -> Result<(Self, Receiver<Request>)> {
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
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket_path(config))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    writeln!(stream, "{request}")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}
#[cfg(not(unix))]
pub fn call(_config: &Config, _request: Value) -> Result<Value> {
    anyhow::bail!(t("local command sockets require Unix"))
}
