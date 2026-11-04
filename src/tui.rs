//! 外层终端的进入与恢复。
//!
//! 任何异常退出路径（正常返回、panic、SIGTERM）都必须恢复 raw mode / 鼠标 /
//! alternate screen，否则用户的 shell 会留在坏状态。见 plan v1.1 §7。

use std::io::{stdout, Stdout};

use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn init() -> Result<Tui> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture, Hide)?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    Ok(terminal)
}

pub fn restore() -> Result<()> {
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture, Show)?;
    Ok(())
}

/// Drop 时尽力恢复。放在 `init()` 之后立即创建。
pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = restore();
    }
}
