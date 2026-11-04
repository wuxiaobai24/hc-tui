//! 应用状态与事件循环。
//!
//! 骨架版本：不连 core，Pane 里跑一段演示 VT 字节，用来验证
//! 「字节 → 本地 VT → Ratatui Buffer」这条链路。接 core 在 G1。

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{config::Config, term::view::TerminalView, tui, ui};

/// 演示内容：颜色、真彩、宽字符、反显、下划线。
const DEMO: &[u8] = b"\x1b[1;36mhc-tui\x1b[0m  skeleton\r\n\
\r\n\
  core   : \x1b[33mnot connected\x1b[0m   (\x1b[2mhc-tui probe\x1b[0m)\r\n\
  pane   : \x1b[32mlocal VT\x1b[0m \xc2\xb7 ghostty-vt\r\n\
  wide   : \xe5\xae\xbd\xe5\xad\x97\xe7\xac\xa6 \xe6\xbc\xa2\xe5\xad\x97 \xed\x85\x8c\xec\x8a\xa4\xed\x8a\xb8 \xe2\x9c\x93\r\n\
  color  : \x1b[38;2;255;128;0mtruecolor\x1b[0m \x1b[7mreverse\x1b[0m \x1b[4munderline\x1b[0m\r\n\
\r\n\
  \x1b[2mctrl+b q\x1b[0m  quit\r\n\
  \x1b[2mctrl+b b\x1b[0m  toggle sidebar\r\n";

pub struct App {
    pub view: TerminalView,
    pub config: Config,
    pub quit: bool,
    pub sidebar: bool,
    pub status: String,
    prefix: bool,
}

impl App {
    pub fn new() -> Result<Self> {
        let config = Config::default();
        let mut view = TerminalView::new(80, 20, config.scrollback_bytes)?;
        view.write(DEMO);
        Ok(Self {
            view,
            sidebar: config.sidebar,
            status: "skeleton · demo buffer".to_string(),
            config,
            quit: false,
            prefix: false,
        })
    }

    pub fn prefix_active(&self) -> bool {
        self.prefix
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if self.prefix {
            self.prefix = false;
            match key.code {
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Char('b') => self.sidebar = !self.sidebar,
                _ => {}
            }
            return;
        }
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('b')) => self.prefix = true,
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => self.quit = true,
            _ => {}
        }
    }
}

pub fn run() -> Result<()> {
    let mut terminal = tui::init()?;
    let _guard = tui::Guard;
    let mut app = App::new()?;

    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                app.on_key(key);
            }
        }
        if app.quit {
            break;
        }
    }
    Ok(())
}
