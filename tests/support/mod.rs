//! 对拍用例共用的模块等待与区域取样。
//!
//! 收帧条件取宿主记账的就绪状态（`props` 修订号与模块落笔修订号一致），
//! 或者取「派发之后内容真的变了」；不用连续两帧相同这类停机时间猜测。

#![allow(dead_code)]

use std::time::Duration;

use hc_tui::app::App;
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

/// 一帧里某块矩形的文本：逐行取字符。
pub fn region_text(buffer: &Buffer, area: Rect) -> String {
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

/// 画帧并泵模块通道，直到一帧同时满足：帧开始时每个模块已按交付的 props 画过
/// （就绪前这一帧会把内置帧盖回去，props 也可能就在这一帧里被推新），帧结束时
/// 仍就绪（这一帧没有推到新 props），而且这一帧真的画到了每个模块（帧预算耗尽
/// 时后面的表面会被跳过）。返回时终端上那一帧是模块自己的像素。
pub fn pump_until_ready(app: &mut App, terminal: &mut Terminal<TestBackend>, modules: &[&str]) {
    for _ in 0..1200 {
        let ready = modules.iter().all(|module| app.mod_ready(module));
        let before: Vec<u64> = modules
            .iter()
            .map(|module| app.mod_frames_drawn(module))
            .collect();
        terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
        app.pump_mods();
        let drawn = modules
            .iter()
            .zip(&before)
            .all(|(module, before)| app.mod_frames_drawn(module) > *before);
        let still_ready = modules.iter().all(|module| app.mod_ready(module));
        if ready && still_ready && drawn {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let revisions: Vec<String> = modules
        .iter()
        .map(|module| format!("{module}: {:?}", app.mod_paint_revisions(module)))
        .collect();
    panic!(
        "modules never painted with the delivered props: {}",
        revisions.join(", ")
    );
}

/// 投递输入之后画到模块落笔，最多 `budget` 帧：`before` 是投递前那块矩形的
/// 文本，画出别的内容才算这一笔落地（表面关掉也算）。超时返回 `false`，调用
/// 方可以重投；真坏了照样能判定。
pub fn pump_until_repainted_within(
    app: &mut App,
    terminal: &mut Terminal<TestBackend>,
    area: Rect,
    before: &str,
    budget: usize,
) -> bool {
    for _ in 0..budget {
        terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
        app.pump_mods();
        if region_text(terminal.backend().buffer(), area) != before {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

/// 投递输入之后画到模块落笔：`before` 是投递前那块矩形的文本，画出别的内容
/// 才算这一笔落地。表面关掉（矩形内容整体变了）同样算落地。
pub fn pump_until_repainted(
    app: &mut App,
    terminal: &mut Terminal<TestBackend>,
    area: Rect,
    before: &str,
) {
    if !pump_until_repainted_within(app, terminal, area, before, 1200) {
        panic!(
            "module never repainted after the input; frame now:\n{}",
            region_text(terminal.backend().buffer(), area)
        );
    }
}

// ---------------------------------------------------------------------------
// 隔离会话守卫：测试进程里唯一的会话入口
// ---------------------------------------------------------------------------
//
// 每个集成测试进程第一次 `IsolatedCore::new(tag)` 时建一个临时 runtime、导出
// `XDG_*` / `HC_SESSION`，进程内所有用例共用这一个会话；最后一个守卫 Drop 时
// 停会话、杀 holder、删目录，进程正常退出（含用例 panic 后被 harness 收尾）时
// `atexit` 再兜一次。两条路径都幂等：进程已死、目录已删、`hc` 不在都不报错，
// 也不往 stderr 写东西影响断言。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use std::time::Instant;

/// 隔离会话守卫。持有期间会话可用；进程内多个用例同时持有时共享同一个会话。
pub struct IsolatedCore {
    isolated: Arc<Isolated>,
}

struct Isolated {
    dir: PathBuf,
    session: String,
}

/// 存活守卫数与「当前会话是否在跑」；生命周期变更都在这把锁里做。
struct Slot {
    live: usize,
    alive: bool,
}

static SPEC: OnceLock<Arc<Isolated>> = OnceLock::new();
static SLOT: OnceLock<Mutex<Slot>> = OnceLock::new();
static ATEXIT: OnceLock<()> = OnceLock::new();

fn slot() -> &'static Mutex<Slot> {
    SLOT.get_or_init(|| {
        Mutex::new(Slot {
            live: 0,
            alive: false,
        })
    })
}

fn slot_lock() -> MutexGuard<'static, Slot> {
    slot().lock().unwrap_or_else(|poison| poison.into_inner())
}

impl IsolatedCore {
    /// 建隔离 runtime 并导出 `XDG_*` / `HC_SESSION` 到本进程。
    ///
    /// `tag` 只用于会话名与目录名，方便在 `ss` / `pgrep` 里认人；同一进程内重复
    /// 调用返回同一个会话，只有没有任何守卫存活时才会重建目录（会话名不变）。
    pub fn new(tag: &str) -> Self {
        let isolated = SPEC.get_or_init(|| Arc::new(Isolated::create(tag))).clone();
        let mut state = slot_lock();
        if !state.alive {
            isolated.prepare();
            state.alive = true;
        }
        state.live += 1;
        register_atexit();
        IsolatedCore { isolated }
    }

    /// 会话名（`HC_SESSION` 的值）。
    pub fn session(&self) -> &str {
        &self.isolated.session
    }

    /// 隔离 runtime 根目录（`XDG_RUNTIME_DIR` 是它下面的 `run`）。
    pub fn dir(&self) -> &Path {
        &self.isolated.dir
    }
}

impl Drop for IsolatedCore {
    fn drop(&mut self) {
        let mut state = slot_lock();
        state.live = state.live.saturating_sub(1);
        if state.live != 0 {
            return;
        }
        self.isolated.cleanup(false);
        state.alive = false;
    }
}

impl Isolated {
    fn create(tag: &str) -> Self {
        let pid = std::process::id();
        // socket 路径上限 107 字节（含 holder 的 `.holder.<hash>.sock` 后缀），
        // 路径里出现两次会话名，所以标签要截短。
        let room = 30usize.saturating_sub(format!("hctui--{pid}").len());
        let tag: String = tag
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .take(room)
            .collect();
        let name = format!("hctui-{tag}-{pid}");
        Self {
            dir: std::env::temp_dir().join(&name),
            session: name,
        }
    }

    fn prepare(&self) {
        for leaf in ["run", "state", "config", "cache"] {
            let _ = fs::create_dir_all(self.dir.join(leaf));
        }
        std::env::set_var("XDG_RUNTIME_DIR", self.dir.join("run"));
        std::env::set_var("XDG_STATE_HOME", self.dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", self.dir.join("config"));
        std::env::set_var("XDG_CACHE_HOME", self.dir.join("cache"));
        std::env::set_var("HC_SESSION", &self.session);
    }

    /// 停会话 → 杀 core → 杀 holder → 删目录。`exiting` 时先杀掉本进程拉起的
    /// `hc connect` 子进程，免得它在我们收尾之后又把 core 重新拉起来。
    fn cleanup(&self, exiting: bool) {
        if exiting {
            kill_connectors();
        }
        stop_session(&self.dir.join("run"), &self.session);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// 停掉某个 runtime 目录下的一个会话：先 `hc server stop`，再按 socket 路径找
/// 监听进程 TERM/KILL（`hc` 不可用时的退化路径），最后杀 `<session>.holder.*`。
///
/// `hc server stop` 自己也是一个会重试的客户端：它停掉 core 之后可能顺手把新
/// core 拉起来，所以停完以 socket 监听为准，连续静默一小段时间才收手。
/// 幂等：会话已经不在、`hc` 不在都只是返回。`runtime` 就是 core 启动时的
/// `XDG_RUNTIME_DIR`（隔离 runtime 的根目录下的 `run`）。
pub fn stop_session(runtime: &Path, session: &str) {
    let target = Session { runtime, session };
    // holder 会保活并在 core 掉线时把它拉回来：先杀 holder，再停 core。
    target.kill_holders(Duration::from_millis(400));
    target.stop();
    let deadline = Instant::now() + Duration::from_millis(1200);
    let mut quiet_since: Option<Instant> = None;
    while Instant::now() < deadline {
        target.kill_holders(Duration::from_millis(100));
        let pids = target.pids_on_socket();
        if pids.is_empty() {
            let since = *quiet_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(150) {
                return;
            }
        } else {
            quiet_since = None;
            for pid in &pids {
                signal(*pid, libc::SIGTERM);
            }
            std::thread::sleep(Duration::from_millis(40));
            for pid in target.pids_on_socket() {
                signal(pid, libc::SIGKILL);
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    target.kill_holders(Duration::from_millis(200));
}

/// 这个会话现在是否有 core 在监听（不看残留的 socket 文件）。
pub fn session_running(runtime: &Path, session: &str) -> bool {
    !Session { runtime, session }.pids_on_socket().is_empty()
}

/// 一个会话在某个 runtime 下的进程集合；core 的 cmdline 里没有会话名，
/// 会话只能从 socket 路径（core）或 `--socket` 参数（holder）读出来。
struct Session<'a> {
    runtime: &'a Path,
    session: &'a str,
}

impl Session<'_> {
    fn socket_path(&self) -> PathBuf {
        self.runtime
            .join("herdr-core")
            .join(format!("{}.sock", self.session))
    }

    fn stop(&self) {
        let _ = Command::new("hc")
            .args(["server", "stop"])
            .env("XDG_RUNTIME_DIR", self.runtime)
            .env("HC_SESSION", self.session)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    fn pids_on_socket(&self) -> Vec<i32> {
        let Some(inode) = socket_inode(&self.socket_path()) else {
            return Vec::new();
        };
        let needle = format!("socket:[{inode}]");
        let mut pids = Vec::new();
        for pid in proc_pids() {
            let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) else {
                continue;
            };
            let holds_socket = entries.flatten().any(|entry| {
                fs::read_link(entry.path())
                    .map(|target| target.to_string_lossy() == needle)
                    .unwrap_or(false)
            });
            if holds_socket {
                pids.push(pid);
            }
        }
        pids
    }

    fn holder_pids(&self) -> Vec<i32> {
        let runtime = self.runtime.to_string_lossy().into_owned();
        let holder = format!("{}.holder.", self.session);
        proc_pids()
            .into_iter()
            .filter(|pid| {
                cmdline(*pid).iter().any(|arg| {
                    let arg = String::from_utf8_lossy(arg);
                    arg.starts_with(&runtime) && arg.contains(&holder)
                })
            })
            .collect()
    }

    fn kill_socket(&self, budget: Duration) {
        kill_until_gone(budget, || self.pids_on_socket());
    }

    fn kill_holders(&self, budget: Duration) {
        kill_until_gone(budget, || self.holder_pids());
    }
}

fn kill_connectors() {
    let parent = std::process::id() as i32;
    for pid in proc_pids() {
        if parent_of(pid) != Some(parent) {
            continue;
        }
        let args = cmdline(pid);
        let is_connect = args
            .first()
            .map(|arg| arg.ends_with(b"hc"))
            .unwrap_or(false)
            && args.iter().any(|arg| arg.as_slice() == b"connect");
        if is_connect {
            signal(pid, libc::SIGTERM);
        }
    }
}

fn register_atexit() {
    ATEXIT.get_or_init(|| {
        // SAFETY: 处理函数只扫 /proc、发信号、删目录，不跨 FFI panic。
        unsafe { libc::atexit(cleanup_at_exit) };
    });
}

extern "C" fn cleanup_at_exit() {
    if let Some(isolated) = SPEC.get() {
        isolated.cleanup(true);
    }
}

fn socket_inode(socket: &Path) -> Option<String> {
    let socket = socket.to_str()?;
    let table = fs::read_to_string("/proc/net/unix").ok()?;
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 8 && fields[7..].join(" ") == socket {
            Some(fields[6].to_string())
        } else {
            None
        }
    })
}

fn proc_pids() -> Vec<i32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
        .collect()
}

fn cmdline(pid: i32) -> Vec<Vec<u8>> {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|raw| {
            raw.split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .map(<[u8]>::to_vec)
                .collect()
        })
        .unwrap_or_default()
}

fn parent_of(pid: i32) -> Option<i32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))
        .and_then(|value| value.trim().parse::<i32>().ok())
}

fn signal(pid: i32, signal: i32) {
    // SAFETY: pid 来自 /proc 扫描；进程已退出时 kill 只返回错误码。
    unsafe { libc::kill(pid, signal) };
}

fn kill_until_gone(budget: Duration, pids: impl Fn() -> Vec<i32>) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        let found = pids();
        if found.is_empty() {
            return true;
        }
        for pid in &found {
            signal(*pid, libc::SIGTERM);
        }
        if Instant::now() >= deadline {
            for pid in pids() {
                signal(pid, libc::SIGKILL);
            }
            std::thread::sleep(Duration::from_millis(30));
            return pids().is_empty();
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
