//! 本地配置。首版只定义结构，文件加载在 P2 接。

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    /// 目标 hc session（`None` = 使用环境里的 `HC_SESSION` / 默认）。
    pub session: Option<String>,
    pub sidebar: bool,
    pub sidebar_width: u16,
    /// 每个终端保留的 scrollback 字节数。
    pub scrollback_bytes: usize,
    pub render_fps: u32,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            session: None,
            sidebar: true,
            sidebar_width: 26,
            scrollback_bytes: 8 << 20,
            render_fps: 30,
            config_path: home.join(".config/hc-tui/config.toml"),
            state_path: home.join(".local/state/hc-tui/state.json"),
        }
    }
}
