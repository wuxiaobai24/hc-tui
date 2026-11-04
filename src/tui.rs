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
use ratatui::{backend::CrosstermBackend, Terminal};

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

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = restore();
    }
}
