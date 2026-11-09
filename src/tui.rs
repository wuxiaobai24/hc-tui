//! 外层终端的进入与恢复。
//!
//! 任何异常退出路径（正常返回、panic、SIGTERM）都必须恢复 raw mode / 鼠标 /
//! alternate screen，否则用户的 shell 会留在坏状态。见 plan v1.1 §7。

use std::io::{stdout, Stdout};

use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui_core::terminal::Terminal;
use ratatui_crossterm::CrosstermBackend;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn init() -> Result<Tui> {
    init_with(true)
}

/// `mouse` 为假时不开启鼠标捕获，把点击与拖选留给宿主终端。
pub fn init_with(mouse: bool) -> Result<Tui> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste, Hide)?;
    if mouse {
        execute!(stdout(), EnableMouseCapture)?;
    }
    let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    Ok(terminal)
}

pub fn restore() -> Result<()> {
    disable_raw_mode()?;
    execute!(
        stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        Show
    )?;
    Ok(())
}

/// Drop 时尽力恢复。放在 `init()` 之后立即创建。
pub struct Guard;

/// 终端消失后 stderr 已经没有读者：`eprintln!` 写失败会 panic（ratatui 的
/// `Terminal::drop` 就会打一行「光标恢复失败」）。把它接到 `/dev/null`，收尾期间
/// 的提示不会再变成崩溃。
pub fn detach_stderr() {
    // SAFETY: 只把本进程的 fd 2 换成 /dev/null；开不了 /dev/null 就保持原样。
    unsafe {
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if devnull == -1 {
            return;
        }
        libc::dup2(devnull, libc::STDERR_FILENO);
        libc::close(devnull);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = restore();
    }
}
