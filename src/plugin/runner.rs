//! 插件动作执行：受限子进程、超时与取消、进程组回收、有界输出与并发队列。
//!
//! 每条调用在后台线程里启动独立进程，cwd 为插件目录，stdin 一次性写入
//! 上下文 JSON。deadline 从启动阶段开始计时；stdin 写入与两个 reader 都在
//! 独立线程里，同时受取消与 deadline 管理，父进程不会堵在管道上。
//!
//! 收尾覆盖完整生命周期：直接子进程退出后仍会终止进程组里的后代、等管道
//! 关闭、等 reader 收完，全部完成才公布最终状态。宿主退出走同一条终止路径
//! （[`Runner::shutdown`]）。超时或取消先向进程组发 SIGTERM，2 秒后 SIGKILL；
//! 槽位释放后按队列顺序启动下一条。

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::lease::GenerationPin;
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
/// 收尾时等 reader 排空管道的上限。
const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// 一次调用允许写入 stdin 的上下文上限。
pub const MAX_STDIN: usize = 256 * 1024;

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
pub struct InvocationSpec {
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
    /// 代码代际的 pin；排队中与运行中都持有，调用结束后由运行器释放。
    pub pin: Option<GenerationPin>,
    /// stdin 上下文的字节上限；超过 [`MAX_STDIN`] 时按上限拒绝。
    pub stdin_limit: usize,
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

/// 关闭失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownError {
    /// deadline 到期仍有调用没结束。
    Unfinished(usize),
}

impl std::fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShutdownError::Unfinished(count) => write!(f, "{count} invocations unfinished"),
        }
    }
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
    spec: InvocationSpec,
    status: Status,
    cancel: Arc<AtomicBool>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    exit_code: Option<i32>,
    error: Option<String>,
    queued_at: Instant,
    started_at: Option<Instant>,
    finished_at: Option<Instant>,
    /// 直接子进程的 PID；进程组终止用。
    pid: Option<u32>,
}

#[derive(Default)]
struct State {
    /// 宿主正在退出：不再启动排队项。
    shutting_down: bool,
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
    pub fn invoke(&self, spec: InvocationSpec) -> Result<String, InvokeError> {
        if spec.argv.is_empty() {
            return Err(InvokeError::Invalid("argv must not be empty".to_string()));
        }
        let stdin_limit = spec.stdin_limit.min(MAX_STDIN);
        let context_bytes = serde_json::to_vec(&spec.context)
            .map(|bytes| bytes.len())
            .unwrap_or(0);
        if context_bytes > stdin_limit {
            return Err(InvokeError::Invalid(format!(
                "context is {context_bytes} bytes; the stdin limit is {stdin_limit}"
            )));
        }
        let mut spec = spec;
        let timeout = spec.timeout.min(MAX_TIMEOUT);
        spec.timeout = if timeout.is_zero() {
            DEFAULT_TIMEOUT
        } else {
            timeout
        };
        let mut state = self.lock();
        if state.shutting_down {
            return Err(InvokeError::Invalid(
                "the runner is shutting down".to_string(),
            ));
        }
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
                pid: None,
            },
        );
        if can_start(&state, &spec.package, self.limits) {
            start_one(&self.state, &mut state, &request, &self.root, self.limits);
        } else if state.queue.len() >= self.limits.queue {
            release_pin(&state, &request);
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
                let pin = invocation.spec.pin.clone();
                invocation.status = Status::Cancelled;
                invocation.finished_at = Some(Instant::now());
                invocation.error = Some("cancelled before start".to_string());
                state.queue.retain(|queued| queued != request);
                state.finished.push_back(request.to_string());
                if let Some(pin) = pin {
                    pin.release();
                }
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

    /// 宿主正常退出：取消排队与运行中的调用，终止它们的进程组，等收尾完成。
    ///
    /// 与 [`Runner::cancel`] 走同一条终止路径；deadline 到期仍没结束时向进程组
    /// 补 SIGKILL，返回 [`ShutdownError::Unfinished`]。
    pub fn shutdown(&self, deadline: Instant) -> Result<(), ShutdownError> {
        {
            let mut state = self.lock();
            state.shutting_down = true;
            let requests: Vec<String> = state
                .invocations
                .iter()
                .filter(|(_, invocation)| {
                    matches!(invocation.status, Status::Queued | Status::Running)
                })
                .map(|(request, _)| request.clone())
                .collect();
            state.queue.clear();
            let mut released = Vec::new();
            for request in &requests {
                let Some(invocation) = state.invocations.get_mut(request) else {
                    continue;
                };
                match invocation.status {
                    Status::Queued => {
                        invocation.status = Status::Cancelled;
                        invocation.finished_at = Some(Instant::now());
                        invocation.error = Some("cancelled at shutdown".to_string());
                        released.extend(invocation.spec.pin.clone());
                    }
                    Status::Running => invocation.cancel.store(true, Ordering::SeqCst),
                    _ => {}
                }
            }
            for pin in released {
                pin.release();
            }
        }
        while self.running() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let leftovers: Vec<u32> = {
            let state = self.lock();
            state
                .invocations
                .iter()
                .filter(|(_, invocation)| invocation.status == Status::Running)
                .filter_map(|(_, invocation)| invocation.pid)
                .collect()
        };
        for pid in leftovers {
            #[cfg(unix)]
            kill_group(pid, libc::SIGKILL);
            #[cfg(not(unix))]
            let _ = pid;
        }
        let grace = Instant::now() + KILL_GRACE;
        while self.running() > 0 && Instant::now() < grace {
            std::thread::sleep(Duration::from_millis(20));
        }
        let running = self.running();
        if running > 0 {
            Err(ShutdownError::Unfinished(running))
        } else {
            Ok(())
        }
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
    let worker = shared.clone();
    std::thread::spawn(move || {
        let outcome = execute(
            &request, &spec, &stdout, &stderr, &cancel, &root, limits, &worker,
        );
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
    release_pin(state, request);
    state.running = state.running.saturating_sub(1);
    if let Some(package) = package {
        if let Some(count) = state.per_package.get_mut(&package) {
            *count = count.saturating_sub(1);
        }
    }
    state.finished.push_back(request.to_string());
}

/// 释放一条调用持有的代码 pin；调用结束后代际才能被回收。
fn release_pin(state: &State, request: &str) {
    if let Some(invocation) = state.invocations.get(request) {
        if let Some(pin) = invocation.spec.pin.as_ref() {
            pin.release();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute(
    request: &str,
    spec: &InvocationSpec,
    stdout: &Arc<Mutex<Vec<u8>>>,
    stderr: &Arc<Mutex<Vec<u8>>>,
    cancel: &Arc<AtomicBool>,
    root: &Path,
    limits: Limits,
    shared: &Arc<Mutex<State>>,
) -> Outcome {
    // deadline 从启动阶段开始算，不是启动完成之后。
    let started = Instant::now();
    let deadline = started + spec.timeout;
    let context = serde_json::to_vec(&spec.context).unwrap_or_else(|_| b"{}".to_vec());
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
        let parent = std::process::id();
        // SAFETY: these calls only affect the child between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // 父进程消失（含 SIGKILL）时由内核回收直接子进程，不依赖宿主还活着。
                #[cfg(target_os = "linux")]
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // fork 与 prctl 之间父进程已经死掉的话，PDEATHSIG 不会再触发。
                if libc::getppid() != parent as libc::pid_t {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
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
    {
        let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(invocation) = state.invocations.get_mut(request) {
            invocation.pid = Some(pid);
        }
    }

    // stdin 写入独立线程：父进程不再可能堵在管道上。
    let writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&context);
            let _ = stdin.flush();
            drop(stdin);
        })
    });

    // reader 完成后回报，收尾以管道真正关闭为准。
    let (drained, drain_rx) = std::sync::mpsc::channel();
    let mut readers = 0;
    if let Some(pipe) = child.stdout.take() {
        spawn_reader(
            pipe,
            stdout.clone(),
            root.to_path_buf(),
            spec.package.clone(),
            request.to_string(),
            "stdout",
            limits.output,
            drained.clone(),
        );
        readers += 1;
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
            drained.clone(),
        );
        readers += 1;
    }
    drop(drained);

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
        std::thread::sleep(Duration::from_millis(10));
    }

    // 收尾：直接子进程退出后仍要终止进程组里的后代并等管道关闭。
    terminate_group(pid, &mut child, &mut exit_code);
    drain(readers, &drain_rx);
    if let Some(writer) = writer {
        let _ = writer.join();
    }

    match terminal {
        Some(status) => {
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

/// 终止整个进程组并回收直接子进程；后代可能还持有管道。
fn terminate_group(pid: u32, child: &mut std::process::Child, exit_code: &mut Option<i32>) {
    if !group_alive(pid) {
        let _ = child.wait();
        return;
    }
    #[cfg(unix)]
    kill_group(pid, libc::SIGTERM);
    let until = Instant::now() + KILL_GRACE;
    while Instant::now() < until && group_alive(pid) {
        if let Ok(Some(exited)) = child.try_wait() {
            *exit_code = exited.code();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    #[cfg(unix)]
    kill_group(pid, libc::SIGKILL);
    if let Ok(Some(exited)) = child.try_wait() {
        *exit_code = exited.code();
    }
    let _ = child.wait();
}

/// 进程组里是否还有成员；非 Unix 上按已结束处理。
#[cfg(unix)]
fn group_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence of the process group.
    let error = unsafe { libc::kill(-(pid as i32), 0) };
    error == 0
}

#[cfg(not(unix))]
fn group_alive(pid: u32) -> bool {
    let _ = pid;
    false
}

/// 等 reader 排空管道；超过 [`DRAIN_GRACE`] 就不再等。
fn drain(readers: usize, rx: &std::sync::mpsc::Receiver<()>) {
    let until = Instant::now() + DRAIN_GRACE;
    for _ in 0..readers {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || rx.recv_timeout(left).is_err() {
            break;
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

#[allow(clippy::too_many_arguments)]
fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    buffer: Arc<Mutex<Vec<u8>>>,
    root: PathBuf,
    package: String,
    request: String,
    stream: &'static str,
    limit: usize,
    drained: std::sync::mpsc::Sender<()>,
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
        let _ = drained.send(());
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

    fn spec(script: &str, timeout_ms: u64) -> InvocationSpec {
        InvocationSpec {
            package: "dev.example.probe".to_string(),
            action: "run".to_string(),
            root: std::env::temp_dir(),
            argv: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            timeout: Duration::from_millis(timeout_ms),
            context: serde_json::json!({"api_version": 1}),
            env: BTreeMap::new(),
            request: None,
            pin: None,
            stdin_limit: MAX_STDIN,
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
        std::fs::create_dir_all(&root).unwrap();
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

    /// 子进程先大量输出、父进程还在写 stdin 时不得死锁。
    #[test]
    fn a_large_context_does_not_deadlock_a_child_that_writes_first() {
        let (runner, root) = runner("deadlock", Limits::default());
        let mut stalled = spec("head -c 200000 /dev/zero; sleep 30", 300);
        stalled.context = serde_json::json!({"padding": "x".repeat(96 * 1024)});
        let request = runner.invoke(stalled).unwrap();
        let view = wait(&runner, &request);
        assert!(
            matches!(view.status, Status::TimedOut | Status::Cancelled),
            "{view:?}"
        );
        assert_eq!(runner.running(), 0);
        std::fs::remove_dir_all(root).ok();
    }

    /// 直接子进程退出后仍要收尾后代，收尾完成才公布最终状态。
    #[test]
    fn a_normal_exit_does_not_leave_descendants_behind() {
        let (runner, root) = runner("descendants", Limits::default());
        let pidfile = root.join("child.pid");
        let script = format!(
            "sleep 60 & echo $! > {}; sleep 0.05; exit 0",
            pidfile.display()
        );
        let request = runner.invoke(spec(&script, 5000)).unwrap();
        let view = wait(&runner, &request);
        assert_eq!(view.status, Status::Done, "{view:?}");
        let pid: u32 = std::fs::read_to_string(&pidfile)
            .expect("child pid")
            .trim()
            .parse()
            .unwrap();
        assert!(!alive(pid), "descendant {pid} survived a normal exit");
        std::fs::remove_dir_all(root).ok();
    }

    /// 八个死锁型调用不得永久占满运行槽。
    #[test]
    fn stalled_invocations_release_their_slots() {
        let limits = Limits {
            global: 8,
            per_package: 8,
            queue: 32,
            output: 1024,
        };
        let (runner, root) = runner("slots", limits);
        let mut requests = Vec::new();
        for _ in 0..8 {
            let mut stalled = spec("head -c 200000 /dev/zero; sleep 30", 200);
            stalled.context = serde_json::json!({"padding": "x".repeat(96 * 1024)});
            requests.push(runner.invoke(stalled).unwrap());
        }
        for request in &requests {
            let view = wait(&runner, request);
            assert!(view.status.finished(), "{view:?}");
        }
        assert_eq!(runner.running(), 0);
        let next = runner.invoke(spec("true", 5000)).unwrap();
        assert_eq!(wait(&runner, &next).status, Status::Done);
        std::fs::remove_dir_all(root).ok();
    }

    /// 关闭时排队项的 pin 必须释放，否则卸载后的代际永远回收不掉。
    #[test]
    fn shutdown_releases_the_pins_of_queued_invocations() {
        let limits = Limits {
            global: 1,
            per_package: 1,
            queue: 2,
            output: 1024,
        };
        let (runner, root) = runner("shutdown-pins", limits);
        let config = crate::config::Config {
            config_path: root.join("config").join("config.toml"),
            state_path: root.join("state").join("state.json"),
            ..Default::default()
        };
        let generation_dir = root.join("gen");
        std::fs::create_dir_all(&generation_dir).unwrap();
        let running = runner.invoke(spec("sleep 30", 60000)).unwrap();
        let mut queued_spec = spec("true", 5000);
        queued_spec.pin = Some(
            super::super::lease::pin(
                &config,
                "dev.example.probe",
                1,
                &generation_dir,
                super::super::lease::ResourceOwner::Invocation(
                    super::super::lease::InvocationOwner {
                        request: "p2".to_string(),
                    },
                ),
            )
            .unwrap(),
        );
        let queued = runner.invoke(queued_spec).unwrap();
        assert_eq!(runner.status(&queued).unwrap().status, Status::Queued);
        runner
            .shutdown(Instant::now() + Duration::from_secs(10))
            .unwrap();
        assert!(
            super::super::lease::pins(&config, "dev.example.probe").is_empty(),
            "queued pin survived shutdown"
        );
        let _ = runner.status(&running);
        std::fs::remove_dir_all(root).ok();
    }

    fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 only checks for existence.
        unsafe { libc::kill(pid as i32, 0) == 0 }
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
