//! 插件动作执行：受限子进程、超时与取消、进程组回收、有界输出与并发队列。
//!
//! 每条调用在后台线程里启动独立进程，cwd 为插件目录，stdin 一次性写入
//! 上下文 JSON。stdout/stderr 由独立线程持续读取，内存只保留最近
//! [`Limits::output`] 字节，磁盘写 [`super::logs`]。超时或取消先向进程组
//! 发 SIGTERM，2 秒后 SIGKILL；槽位释放后按队列顺序启动下一条。

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::logs;

/// 全局同时运行上限。
pub const DEFAULT_GLOBAL: usize = 8;
/// 单包同时运行上限。
pub const DEFAULT_PER_PACKAGE: usize = 4;
/// 排队上限；满了返回 `busy`。
pub const DEFAULT_QUEUE: usize = 64;
/// 每条流的最近输出上限。
pub const DEFAULT_OUTPUT: usize = 64 * 1024;
/// 默认超时。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// 超时上限。
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);
/// SIGTERM 到 SIGKILL 的宽限。
const KILL_GRACE: Duration = Duration::from_secs(2);

/// 并发与输出限制；测试与受限宿主可收窄。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// 全局运行上限。
    pub global: usize,
    /// 单包运行上限。
    pub per_package: usize,
    /// 队列上限。
    pub queue: usize,
    /// 每条流保留的字节数。
    pub output: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            global: DEFAULT_GLOBAL,
            per_package: DEFAULT_PER_PACKAGE,
            queue: DEFAULT_QUEUE,
            output: DEFAULT_OUTPUT,
        }
    }
}

/// 调用状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 已排队。
    Queued,
    /// 运行中。
    Running,
    /// 成功退出。
    Done,
    /// 非零退出或启动失败。
    Failed,
    /// 被取消。
    Cancelled,
    /// 超时。
    TimedOut,
}

impl Status {
    /// 序列化名。
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
            Status::TimedOut => "timed_out",
        }
    }

    /// 是否已结束。
    pub fn finished(self) -> bool {
        matches!(
            self,
            Status::Done | Status::Failed | Status::Cancelled | Status::TimedOut
        )
    }
}

/// 一次调用的输入。
#[derive(Debug, Clone)]
pub struct Spec {
    /// 包 ID。
    pub package: String,
    /// 动作 ID。
    pub action: String,
    /// 代码根目录。
    pub root: PathBuf,
    /// 命令参数；`./` 开头相对根目录。
    pub argv: Vec<String>,
    /// 超时。
    pub timeout: Duration,
    /// stdin 的上下文文档。
    pub context: Value,
    /// 注入的宿主环境变量。
    pub env: BTreeMap<String, String>,
    /// 宿主预先分配的请求 ID；`None` 时由运行器分配。
    pub request: Option<String>,
}

/// 调用快照，供 CLI 与控制通道读取。
#[derive(Debug, Clone)]
pub struct View {
    /// 请求 ID。
    pub request: String,
    /// 包 ID。
    pub package: String,
    /// 动作 ID。
    pub action: String,
    /// 状态。
    pub status: Status,
    /// 退出码。
    pub exit_code: Option<i32>,
    /// 失败原因或超时说明。
    pub error: Option<String>,
    /// stdout 尾部。
    pub stdout: String,
    /// stderr 尾部。
    pub stderr: String,
    /// 排队到启动的毫秒数。
    pub queued_ms: u128,
    /// 运行毫秒数。
    pub runtime_ms: Option<u128>,
}

/// 调用被拒绝的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeError {
    /// 队列已满。
    Busy,
    /// 输入不合法。
    Invalid(String),
}

impl std::fmt::Display for InvokeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InvokeError::Busy => write!(f, "busy"),
            InvokeError::Invalid(message) => write!(f, "{message}"),
        }
    }
}

struct Invocation {
    spec: Spec,
    status: Status,
    cancel: Arc<AtomicBool>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    exit_code: Option<i32>,
    error: Option<String>,
    queued_at: Instant,
    started_at: Option<Instant>,
    finished_at: Option<Instant>,
}

#[derive(Default)]
struct State {
    next: u64,
    invocations: BTreeMap<String, Invocation>,
    queue: VecDeque<String>,
    running: usize,
    per_package: BTreeMap<String, usize>,
    finished: VecDeque<String>,
}

/// 动作运行器；可克隆，共享同一状态。
#[derive(Clone)]
pub struct Runner {
    state: Arc<Mutex<State>>,
    root: PathBuf,
    limits: Limits,
}

impl Runner {
    /// 以状态目录为根（日志写 `<root>/logs/<package>/`）。
    pub fn new(root: impl Into<PathBuf>) -> Runner {
        Runner::with_limits(root, Limits::default())
    }

    /// 收窄限制；测试与受限宿主使用。
    pub fn with_limits(root: impl Into<PathBuf>, limits: Limits) -> Runner {
        Runner {
            state: Arc::new(Mutex::new(State::default())),
            root: root.into(),
            limits,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// 预先分配请求 ID，供上下文与环境变量引用。
    pub fn allocate_request(&self) -> String {
        let mut state = self.lock();
        state.next += 1;
        format!("p{}", state.next)
    }

    /// 提交一条调用；立即运行或排队，队列满返回 [`InvokeError::Busy`]。
    pub fn invoke(&self, spec: Spec) -> Result<String, InvokeError> {
        if spec.argv.is_empty() {
            return Err(InvokeError::Invalid("argv must not be empty".to_string()));
        }
        let mut spec = spec;
        let timeout = spec.timeout.min(MAX_TIMEOUT);
        spec.timeout = if timeout.is_zero() {
            DEFAULT_TIMEOUT
        } else {
            timeout
        };
        let mut state = self.lock();
        state.prune();
        let request = match spec.request.take() {
            Some(request) => {
                if state.invocations.contains_key(&request) {
                    return Err(InvokeError::Invalid(format!(
                        "request {request} already exists"
                    )));
                }
                request
            }
            None => {
                state.next += 1;
                format!("p{}", state.next)
            }
        };
        state.invocations.insert(
            request.clone(),
            Invocation {
                spec: spec.clone(),
                status: Status::Queued,
                cancel: Arc::new(AtomicBool::new(false)),
                stdout: Arc::new(Mutex::new(Vec::new())),
                stderr: Arc::new(Mutex::new(Vec::new())),
                exit_code: None,
                error: None,
                queued_at: Instant::now(),
                started_at: None,
                finished_at: None,
            },
        );
        if can_start(&state, &spec.package, self.limits) {
            start_one(&self.state, &mut state, &request, &self.root, self.limits);
        } else if state.queue.len() >= self.limits.queue {
            state.invocations.remove(&request);
            return Err(InvokeError::Busy);
        } else {
            state.queue.push_back(request.clone());
        }
        Ok(request)
    }

    /// 查询状态与输出尾部。
    pub fn status(&self, request: &str) -> Option<View> {
        let state = self.lock();
        state.invocations.get(request).map(|invocation| View {
            request: request.to_string(),
            package: invocation.spec.package.clone(),
            action: invocation.spec.action.clone(),
            status: invocation.status,
            exit_code: invocation.exit_code,
            error: invocation.error.clone(),
            stdout: tail_of(&invocation.stdout),
            stderr: tail_of(&invocation.stderr),
            queued_ms: invocation
                .started_at
                .map(|started| started.duration_since(invocation.queued_at).as_millis())
                .unwrap_or_else(|| invocation.queued_at.elapsed().as_millis()),
            runtime_ms: invocation.started_at.map(|started| {
                invocation
                    .finished_at
                    .unwrap_or_else(Instant::now)
                    .duration_since(started)
                    .as_millis()
            }),
        })
    }

    /// 取消排队或运行中的调用。
    pub fn cancel(&self, request: &str) -> Result<(), String> {
        let mut state = self.lock();
        let Some(invocation) = state.invocations.get_mut(request) else {
            return Err(format!("unknown request {request}"));
        };
        match invocation.status {
            Status::Queued => {
                invocation.status = Status::Cancelled;
                invocation.finished_at = Some(Instant::now());
                invocation.error = Some("cancelled before start".to_string());
                state.queue.retain(|queued| queued != request);
                state.finished.push_back(request.to_string());
                Ok(())
            }
            Status::Running => {
                invocation.cancel.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(format!("request {request} already finished")),
        }
    }

    /// 读一次调用的日志尾部。
    pub fn log(&self, request: &str, lines: usize) -> Result<String, String> {
        let state = self.lock();
        let Some(invocation) = state.invocations.get(request) else {
            return Err(format!("unknown request {request}"));
        };
        Ok(logs::tail(
            &self.root,
            &invocation.spec.package,
            request,
            lines,
        ))
    }

    /// 取走自上次调用以来结束的请求 ID。
    pub fn take_finished(&self) -> Vec<String> {
        let mut state = self.lock();
        state.finished.drain(..).collect()
    }

    /// 当前运行中的调用数。
    pub fn running(&self) -> usize {
        self.lock().running
    }

    /// 当前排队数。
    pub fn queued(&self) -> usize {
        self.lock().queue.len()
    }
}

fn can_start(state: &State, package: &str, limits: Limits) -> bool {
    state.running < limits.global
        && state.per_package.get(package).copied().unwrap_or(0) < limits.per_package
}

/// 把一条调用置为运行并启动工作线程；调用方持锁。
fn start_one(
    shared: &Arc<Mutex<State>>,
    state: &mut State,
    request: &str,
    root: &Path,
    limits: Limits,
) {
    let Some(invocation) = state.invocations.get_mut(request) else {
        return;
    };
    invocation.status = Status::Running;
    invocation.started_at = Some(Instant::now());
    let spec = invocation.spec.clone();
    let stdout = invocation.stdout.clone();
    let stderr = invocation.stderr.clone();
    let cancel = invocation.cancel.clone();
    state.running += 1;
    *state.per_package.entry(spec.package.clone()).or_insert(0) += 1;
    let shared = shared.clone();
    let root = root.to_path_buf();
    let request = request.to_string();
    std::thread::spawn(move || {
        let outcome = execute(&request, &spec, &stdout, &stderr, &cancel, &root, limits);
        let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
        record_result(&mut state, &request, outcome);
        release(&mut state, &request);
        pump(&mut state, &shared, &root, limits);
    });
}

/// 槽位释放后按队列顺序启动下一条。
fn pump(state: &mut State, shared: &Arc<Mutex<State>>, root: &Path, limits: Limits) {
    while state.running < limits.global {
        let position = state.queue.iter().position(|request| {
            state
                .invocations
                .get(request)
                .is_some_and(|invocation| can_start(state, &invocation.spec.package, limits))
        });
        let Some(position) = position else {
            break;
        };
        let request = state
            .queue
            .remove(position)
            .expect("position came from queue");
        start_one(shared, state, &request, root, limits);
    }
}

struct Outcome {
    status: Status,
    error: Option<String>,
    exit_code: Option<i32>,
}

fn record_result(state: &mut State, request: &str, outcome: Outcome) {
    if let Some(invocation) = state.invocations.get_mut(request) {
        invocation.status = outcome.status;
        invocation.error = outcome.error;
        invocation.exit_code = outcome.exit_code;
        invocation.finished_at = Some(Instant::now());
    }
}

fn release(state: &mut State, request: &str) {
    let package = state
        .invocations
        .get(request)
        .map(|invocation| invocation.spec.package.clone());
    state.running = state.running.saturating_sub(1);
    if let Some(package) = package {
        if let Some(count) = state.per_package.get_mut(&package) {
            *count = count.saturating_sub(1);
        }
    }
    state.finished.push_back(request.to_string());
}

fn execute(
    request: &str,
    spec: &Spec,
    stdout: &Arc<Mutex<Vec<u8>>>,
    stderr: &Arc<Mutex<Vec<u8>>>,
    cancel: &Arc<AtomicBool>,
    root: &Path,
    limits: Limits,
) -> Outcome {
    let mut command = std::process::Command::new(resolve_program(&spec.argv[0], &spec.root));
    command.args(spec.argv.iter().skip(1).collect::<Vec<_>>());
    command.current_dir(&spec.root);
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setpgid only affects the child before exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Outcome {
                status: Status::Failed,
                error: Some(format!("cannot start {}: {error}", spec.argv[0])),
                exit_code: None,
            };
        }
    };
    let pid = child.id();
    if let Some(mut stdin) = child.stdin.take() {
        let text = serde_json::to_vec(&spec.context).unwrap_or_else(|_| b"{}".to_vec());
        let _ = stdin.write_all(&text);
        drop(stdin);
    }
    if let Some(pipe) = child.stdout.take() {
        spawn_reader(
            pipe,
            stdout.clone(),
            root.to_path_buf(),
            spec.package.clone(),
            request.to_string(),
            "stdout",
            limits.output,
        );
    }
    if let Some(pipe) = child.stderr.take() {
        spawn_reader(
            pipe,
            stderr.clone(),
            root.to_path_buf(),
            spec.package.clone(),
            request.to_string(),
            "stderr",
            limits.output,
        );
    }
    let deadline = Instant::now() + spec.timeout;
    let mut terminal = None;
    let mut exit_code = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code();
                break;
            }
            Ok(None) => {}
            Err(_) => {
                terminal = Some(Status::Failed);
                break;
            }
        }
        if cancel.load(Ordering::SeqCst) {
            terminal = Some(Status::Cancelled);
            break;
        }
        if Instant::now() >= deadline {
            terminal = Some(Status::TimedOut);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    match terminal {
        Some(status) => {
            kill_group(pid, libc::SIGTERM);
            let until = Instant::now() + KILL_GRACE;
            while Instant::now() < until {
                if let Ok(Some(exited)) = child.try_wait() {
                    exit_code = exited.code();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            kill_group(pid, libc::SIGKILL);
            let _ = child.wait();
            let error = if status == Status::TimedOut {
                format!("timed out after {:?}", spec.timeout)
            } else {
                "cancelled".to_string()
            };
            Outcome {
                status,
                error: Some(error),
                exit_code,
            }
        }
        None => {
            if exit_code == Some(0) {
                Outcome {
                    status: Status::Done,
                    error: None,
                    exit_code,
                }
            } else {
                Outcome {
                    status: Status::Failed,
                    error: Some(format!("exit code {}", exit_code.unwrap_or(-1))),
                    exit_code,
                }
            }
        }
    }
}

fn resolve_program(program: &str, root: &Path) -> PathBuf {
    if let Some(relative) = program.strip_prefix("./") {
        root.join(relative)
    } else {
        PathBuf::from(program)
    }
}

fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    buffer: Arc<Mutex<Vec<u8>>>,
    root: PathBuf,
    package: String,
    request: String,
    stream: &'static str,
    limit: usize,
) {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let bytes = &chunk[..read];
                    {
                        let mut tail = buffer.lock().unwrap_or_else(|error| error.into_inner());
                        tail.extend_from_slice(bytes);
                        if tail.len() > limit {
                            let overflow = tail.len() - limit;
                            tail.drain(..overflow);
                        }
                    }
                    let _ = logs::append(&root, &package, &request, stream, bytes);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
}

fn tail_of(buffer: &Arc<Mutex<Vec<u8>>>) -> String {
    let bytes = buffer
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(unix)]
fn kill_group(pid: u32, signal: i32) {
    // SAFETY: negative pid targets the child's process group.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

#[cfg(not(unix))]
fn kill_group(pid: u32, signal: i32) {
    let _ = (pid, signal);
}

impl State {
    fn prune(&mut self) {
        let now = Instant::now();
        let stale: Vec<String> = self
            .invocations
            .iter()
            .filter(|(_, invocation)| {
                invocation
                    .finished_at
                    .is_some_and(|finished| now.duration_since(finished) > Duration::from_secs(600))
            })
            .map(|(request, _)| request.clone())
            .collect();
        for request in stale {
            self.invocations.remove(&request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("hctui-plugin-runner-{tag}-{unique}"))
    }

    fn spec(script: &str, timeout_ms: u64) -> Spec {
        Spec {
            package: "dev.example.probe".to_string(),
            action: "run".to_string(),
            root: std::env::temp_dir(),
            argv: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            timeout: Duration::from_millis(timeout_ms),
            context: serde_json::json!({"api_version": 1}),
            env: BTreeMap::new(),
            request: None,
        }
    }

    fn wait(runner: &Runner, request: &str) -> View {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let view = runner.status(request).expect("request exists");
            if view.status.finished() {
                return view;
            }
            assert!(Instant::now() < deadline, "request {request} stuck");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn runner(tag: &str, limits: Limits) -> (Runner, PathBuf) {
        let root = unique_dir(tag);
        (Runner::with_limits(&root, limits), root)
    }

    #[test]
    fn timeout_terminates_the_process_group() {
        let (runner, root) = runner("timeout", Limits::default());
        let marker = root.join("marker");
        let request = runner
            .invoke(spec(&format!("sleep 2; touch {}", marker.display()), 100))
            .unwrap();
        let view = wait(&runner, &request);
        assert_eq!(view.status, Status::TimedOut, "{view:?}");
        assert!(view.runtime_ms.unwrap_or(0) < 2000);
        std::thread::sleep(Duration::from_millis(2300));
        assert!(!marker.exists(), "the process group kept running");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn output_is_truncated_to_the_tail() {
        let (runner, root) = runner("output", Limits::default());
        let request = runner.invoke(spec("yes x | head -c 200000", 5000)).unwrap();
        let view = wait(&runner, &request);
        assert_eq!(view.status, Status::Done, "{view:?}");
        assert!(view.stdout.len() <= DEFAULT_OUTPUT, "{}", view.stdout.len());
        assert!(
            view.stdout.len() > DEFAULT_OUTPUT / 2,
            "{}",
            view.stdout.len()
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn concurrency_queues_and_reports_busy_when_full() {
        let limits = Limits {
            global: 1,
            per_package: 1,
            queue: 2,
            output: 1024,
        };
        let (runner, root) = runner("queue", limits);
        let first = runner.invoke(spec("sleep 0.3", 5000)).unwrap();
        let second = runner.invoke(spec("sleep 0.3", 5000)).unwrap();
        let third = runner.invoke(spec("sleep 0.3", 5000)).unwrap();
        assert_eq!(runner.running(), 1);
        assert_eq!(runner.queued(), 2);
        assert_eq!(runner.status(&second).unwrap().status, Status::Queued);
        assert_eq!(
            runner.invoke(spec("sleep 0.3", 5000)),
            Err(InvokeError::Busy)
        );
        assert_eq!(wait(&runner, &first).status, Status::Done);
        assert_eq!(wait(&runner, &third).status, Status::Done);
        assert_eq!(runner.running(), 0);
        assert_eq!(runner.queued(), 0);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancel_stops_a_running_process_and_frees_the_slot() {
        let limits = Limits {
            global: 1,
            per_package: 1,
            queue: 1,
            output: 1024,
        };
        let (runner, root) = runner("cancel", limits);
        let request = runner.invoke(spec("sleep 30", 60000)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while runner.status(&request).unwrap().status != Status::Running {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        runner.cancel(&request).unwrap();
        assert_eq!(wait(&runner, &request).status, Status::Cancelled);
        assert_eq!(runner.running(), 0);
        let next = runner.invoke(spec("true", 5000)).unwrap();
        assert_eq!(wait(&runner, &next).status, Status::Done);
        assert!(runner.cancel("missing").is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancel_before_start_removes_it_from_the_queue() {
        let limits = Limits {
            global: 1,
            per_package: 1,
            queue: 2,
            output: 1024,
        };
        let (runner, root) = runner("cancel-queued", limits);
        let first = runner.invoke(spec("sleep 0.3", 5000)).unwrap();
        let second = runner.invoke(spec("sleep 0.3", 5000)).unwrap();
        runner.cancel(&second).unwrap();
        let view = runner.status(&second).unwrap();
        assert_eq!(view.status, Status::Cancelled);
        assert_eq!(wait(&runner, &first).status, Status::Done);
        assert_eq!(runner.queued(), 0);
        std::fs::remove_dir_all(root).ok();
    }
}
