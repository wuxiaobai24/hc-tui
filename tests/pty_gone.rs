//! pty 被关闭（终端关掉、父进程消失）之后 hc-tui 必须自己退出。
//!
//! 关闭 master 后，从端的 `read` 返回 EOF（Linux 上部分是 `EIO`）。事件循环把它当成
//! 「暂时读不到」继续读，就会在紧循环里打满单核、永不退出——这个用例盯的就是那条路。

#![cfg(unix)]

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_hc-tui");
/// 关闭 master 之后，进程自行退出允许的时间。
const EXIT_BUDGET: Duration = Duration::from_secs(5);
/// 从起进程到画出第一帧允许的时间（调试构建首帧要建连、加载模块）。
const PAINT_BUDGET: Duration = Duration::from_secs(30);

fn open_pty() -> (File, File) {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    // SAFETY: openpty 只写 master / slave 两个出参。
    let status = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(status, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: 两个 fd 都刚由 openpty 交出，所有权转给 File。
    let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    cloexec(&master);
    cloexec(&slave);
    // 40 行 120 列：给 TUI 一个真实终端大小，布局才有东西可画。
    let size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: ioctl 只把窗口大小写进本 pty 的从端。
    unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, &size) };
    (master, slave)
}

/// master 读改成非阻塞：等第一帧与持续排空都要能自己掌握节奏。
fn nonblocking(file: &File) {
    let fd = file.as_raw_fd();
    // SAFETY: fcntl 只读改本 fd 的 O_NONBLOCK 标志位。
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert!(flags != -1, "fcntl: {}", std::io::Error::last_os_error());
    let status = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    assert!(status != -1, "fcntl: {}", std::io::Error::last_os_error());
}

/// openpty 交出的 fd 默认不带 CLOEXEC：不清掉的话子进程会继承 master，
/// 关掉自己那份 master 之后 pty 依然不断开（用例就永远等不到退出）。
fn cloexec(file: &File) {
    let fd = file.as_raw_fd();
    // SAFETY: fcntl 只读改本 fd 的 FD_CLOEXEC 标志位。
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert!(flags != -1, "fcntl: {}", std::io::Error::last_os_error());
    let status = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    assert!(status != -1, "fcntl: {}", std::io::Error::last_os_error());
}

/// 进程累计 CPU 时间（ticks），用来在超时时区分「卡住」与「空转」。
fn cpu_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = stat.rsplit_once(") ")?.1;
    let fields: Vec<&str> = tail.split_whitespace().collect();
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    Some(utime + stime)
}

/// master 端的读循环：一直排空输出（真实终端也这样），收到关闭信号就把 master 关掉。
struct PtyReader {
    close: Sender<()>,
    painted: Receiver<()>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl PtyReader {
    fn start(mut master: File) -> Self {
        nonblocking(&master);
        let (close, closing) = channel();
        let (painted_tx, painted) = channel();
        let handle = std::thread::spawn(move || {
            let mut buffer = [0u8; 65536];
            // 首帧的标志是同步刷新（`CSI ? 2026 h`）；它之后事件循环肯定已经在读输入。
            let frame = b"\x1b[?2026h";
            let mut tail: Vec<u8> = Vec::new();
            let mut announced = false;
            loop {
                match master.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        tail.extend_from_slice(&buffer[..read]);
                        if !announced && tail.windows(frame.len()).any(|window| window == frame) {
                            announced = true;
                            let _ = painted_tx.send(());
                        }
                        let keep = tail.len().saturating_sub(frame.len() - 1);
                        tail.drain(..keep);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => match closing.try_recv()
                    {
                        Ok(()) | Err(TryRecvError::Disconnected) => break,
                        Err(TryRecvError::Empty) => {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                    },
                    // 从端已经不在：没有可读的了。
                    Err(_) => break,
                }
            }
            // 关掉 master：从端此后读不到东西，正是要复现的场景。
            drop(master);
        });
        Self {
            close,
            painted,
            handle: Some(handle),
        }
    }

    /// 等到 TUI 画出第一帧：此时事件循环已经在读输入。
    fn wait_until_painted(&self) {
        match self.painted.recv_timeout(PAINT_BUDGET) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => panic!("the TUI never painted a frame"),
            Err(RecvTimeoutError::Disconnected) => panic!("the TUI died before painting"),
        }
    }

    fn close(&self) {
        let _ = self.close.send(());
    }
}

impl Drop for PtyReader {
    fn drop(&mut self) {
        let _ = self.close.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// pty 上跑着的 TUI。`Drop` 里一定 kill + wait：用例失败也不会留下孤儿。
struct PtyTui {
    reader: PtyReader,
    child: Child,
    root: std::path::PathBuf,
}

impl PtyTui {
    fn spawn(tag: &str) -> Self {
        let (master, slave) = open_pty();
        let root =
            std::env::temp_dir().join(format!("hctui-pty-gone-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["run", "state", "config", "cache"] {
            std::fs::create_dir_all(root.join(dir)).expect("temp root");
        }
        let mut command = Command::new(BIN);
        command
            .arg("--no-restore")
            // 用例只看 pty 这条路径：隔离会话与 XDG，PATH 里没有 `hc`，不会顺手拉起 core。
            .env(
                "HC_SESSION",
                format!("hctui-pty-gone-{tag}-{}", std::process::id()),
            )
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("PATH", &root)
            .env("TERM", "xterm-256color")
            .env_remove("HC_TUI_GRAPHICS")
            .stdin(Stdio::from(slave.try_clone().expect("dup slave")))
            .stdout(Stdio::from(slave.try_clone().expect("dup slave")))
            .stderr(Stdio::from(slave));
        // SAFETY: 这里只调 setsid 与 TIOCSCTTY；stdio 已经重定向到从端。
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Self {
            reader: PtyReader::start(master),
            child: command.spawn().expect("hc-tui starts"),
            root,
        }
    }

    fn wait_within(&mut self, budget: Duration) -> ExitStatus {
        let pid = self.child.id();
        let started = cpu_ticks(pid);
        let deadline = Instant::now() + budget;
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            if Instant::now() >= deadline {
                let ticks = cpu_ticks(pid).zip(started).map(|(now, then)| now - then);
                panic!(
                    "the TUI is still running {budget:?} after the pty was closed \
                     (cpu ticks spent while waiting: {ticks:?})"
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for PtyTui {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn closing_the_pty_exits_the_tui() {
    let mut pty = PtyTui::spawn("exit");
    pty.reader.wait_until_painted();
    pty.reader.close();
    let started = Instant::now();
    let status = pty.wait_within(EXIT_BUDGET);
    assert!(
        status.success(),
        "the TUI did not exit through the normal path: {status:?} after {:?}",
        started.elapsed()
    );
}
