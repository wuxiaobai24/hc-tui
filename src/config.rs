//! 本地配置：`~/.config/hc-tui/config.toml`。
//!
//! 文件是可选的；缺字段一律取默认值，解析失败的问题收集到 [`Config::warnings`]，
//! 启动后显示在状态栏，不阻止启动。
//!
//! 只实现本程序用到的 TOML 子集：`[section]`、`key = value`、字符串 / 布尔 /
//! 整数 / 浮点 / 一行内数组、`#` 注释。需要更多语法时再升级解析器。

use std::collections::BTreeMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::theme::{self, Theme};

/// 文件里的位置，1 起算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// 一条配置诊断：文件内的行列与原因。
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// 1 起算的行；0 表示问题不在某一行上。
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl Diagnostic {
    fn at(position: Position, message: impl Into<String>) -> Self {
        Self {
            line: position.line,
            column: position.column,
            message: message.into(),
        }
    }

    fn whole(message: impl Into<String>) -> Self {
        Self {
            line: 0,
            column: 0,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(formatter, "{}", self.message)
        } else {
            write!(formatter, "{}:{}: {}", self.line, self.column, self.message)
        }
    }
}

/// `config check` 的一行：错误带 `文件:行:列`。
pub fn describe(path: &std::path::Path, diagnostic: &Diagnostic) -> String {
    if diagnostic.line == 0 {
        format!("{}: {}", path.display(), diagnostic.message)
    } else {
        format!("{}:{}", path.display(), diagnostic)
    }
}

/// 分隔线绘制策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneBorders {
    /// 只有分屏时才画边框，单个 Pane 不画。
    Auto,
    /// 永远画。
    Always,
    /// 只画顶上的标题行，其余三边不画。
    Titles,
    /// 从不画。
    Off,
}

/// 新建 Pane 的起始目录策略（与 herdr 的 `new_cwd` 同名同义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewCwd {
    /// 沿用来源 Pane 的前台目录（默认）。
    Follow,
    /// `$HOME`。
    Home,
    /// hc-tui 进程自己的工作目录。
    Current,
    /// 固定路径。
    Path(String),
}

/// Tab 栏位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBarPosition {
    Top,
    Bottom,
}

/// 侧栏里 Agent 的排序方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSort {
    /// 按工作区分组。
    Spaces,
    /// 按注意力优先级：blocked > error > done > working > idle。
    Priority,
}

/// 状态图标样式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indicators {
    Dots,
    Symbols,
}

/// 侧栏收起时的形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollapsedSidebar {
    /// 保留一条窄栏，显示各工作区的待处理数。
    Compact,
    /// 完全隐藏。
    Hidden,
}

/// 宿主光标策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCursor {
    /// 交给宿主终端画光标（默认）。
    Native,
    /// 由 TUI 把光标画成单元格内容，避免闪烁。
    Drawn,
}

/// 界面文案语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// 跟随 `LC_ALL` / `LC_MESSAGES` / `LANG`。
    Auto,
    En,
    Zh,
}

impl Lang {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "en" => Some(Self::En),
            "zh" => Some(Self::Zh),
            _ => None,
        }
    }
}

/// 通知投递方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Off,
    /// 应用内 toast。
    Toast,
    /// 交给系统通知服务（`notify-send`）。
    System,
}

/// popup 或自定义尺寸：格子数或内容区的百分比。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    Cells(u16),
    Percent(u16),
}

impl Span {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Some(percent) = text.strip_suffix('%') {
            let percent = percent.trim().parse::<u16>().ok()?;
            return (percent > 0 && percent <= 100).then_some(Self::Percent(percent));
        }
        text.parse::<u16>().ok().map(Self::Cells)
    }

    /// 换算成实际格数，永远不超过 `total`。
    pub fn resolve(self, total: u16) -> u16 {
        match self {
            Self::Cells(cells) => cells.min(total),
            Self::Percent(percent) => ((u32::from(total) * u32::from(percent) + 50) / 100) as u16,
        }
    }
}

/// 自定义命令跑在哪一层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// 后台 detach 跑，不占界面。
    Shell,
    /// 在当前 Tab 开一个临时 Pane，命令退出后 Pane 自动消失。
    Pane,
    /// 会话级浮层终端，不改 Tab 布局。
    Popup,
    Action,
}

impl CommandKind {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "shell" => Some(Self::Shell),
            "pane" => Some(Self::Pane),
            "popup" => Some(Self::Popup),
            "action" => Some(Self::Action),
            _ => None,
        }
    }
}

/// 一条 `[[keys.command]]`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommand {
    pub key: Binding,
    pub kind: CommandKind,
    /// 交给 shell 执行的命令行。
    pub command: String,
    pub width: Option<Span>,
    pub height: Option<Span>,
}

/// 一次按键绑定。`prefix+` 需要先按前缀键；直接绑定在终端模式生效。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Off,
    Prefix(KeyCode, KeyModifiers),
    Direct(KeyCode, KeyModifiers),
}

impl Binding {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() || text == "off" {
            return Ok(Self::Off);
        }
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let prefix = parts.first() == Some(&"prefix");
        if prefix {
            parts.remove(0);
        }
        if parts.is_empty() {
            return Err(format!("{text}: 缺少按键"));
        }
        let mut modifiers = KeyModifiers::NONE;
        let mut code = None;
        for part in parts {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                "alt" | "option" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                "cmd" | "super" => modifiers |= KeyModifiers::SUPER,
                other => {
                    if code.is_some() {
                        return Err(format!("{text}: 多个按键"));
                    }
                    code = Some(parse_key(other)?);
                }
            }
        }
        let code = code.ok_or_else(|| format!("{text}: 缺少按键"))?;
        Ok(if prefix {
            Self::Prefix(code, modifiers)
        } else {
            Self::Direct(code, modifiers)
        })
    }

    /// 前缀模式下匹配一条绑定。
    pub fn matches_prefix(&self, key: &KeyEvent) -> bool {
        match self {
            Self::Prefix(code, modifiers) => key_matches(*code, *modifiers, key),
            _ => false,
        }
    }

    pub fn matches_mode(&self, key: &KeyEvent, prefix: bool) -> bool {
        if prefix {
            self.matches_prefix(key)
        } else {
            self.matches_direct(key)
        }
    }

    /// 直接绑定（含前缀键本身）在终端模式下匹配。
    pub fn matches_direct(&self, key: &KeyEvent) -> bool {
        match self {
            Self::Direct(code, modifiers) => key_matches(*code, *modifiers, key),
            _ => false,
        }
    }

    pub fn is_off(&self) -> bool {
        matches!(self, Self::Off)
    }
}

impl std::fmt::Display for Binding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (prefix, code, modifiers) = match self {
            Self::Off => return write!(formatter, "\"\""),
            Self::Prefix(code, modifiers) => (true, code, modifiers),
            Self::Direct(code, modifiers) => (false, code, modifiers),
        };
        let mut parts: Vec<String> = Vec::new();
        if prefix {
            parts.push("prefix".to_string());
        }
        if modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("ctrl".to_string());
        }
        if modifiers.contains(KeyModifiers::ALT) {
            parts.push("alt".to_string());
        }
        if modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("shift".to_string());
        }
        if modifiers.contains(KeyModifiers::SUPER) {
            parts.push("super".to_string());
        }
        parts.push(name_of(*code));
        write!(formatter, "\"{}\"", parts.join("+"))
    }
}

fn name_of(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char('-') => "minus".to_string(),
        KeyCode::Char('+') => "plus".to_string(),
        KeyCode::Char(',') => "comma".to_string(),
        KeyCode::Char('.') => "period".to_string(),
        KeyCode::Char('/') => "slash".to_string(),
        KeyCode::Char(';') => "semicolon".to_string(),
        KeyCode::Char(':') => "colon".to_string(),
        KeyCode::Char('&') => "ampersand".to_string(),
        KeyCode::Char('`') => "backtick".to_string(),
        KeyCode::Char('?') => "?".to_string(),
        KeyCode::Char('[') => "leftbracket".to_string(),
        KeyCode::Char(']') => "rightbracket".to_string(),
        KeyCode::Char(character) => character.to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "backtab".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::F(number) => format!("f{number}"),
        other => format!("{other:?}"),
    }
}

/// 比较按键与绑定。Shift 只对字母大小写有意义，其余按修改键集合比较。
fn key_matches(code: KeyCode, modifiers: KeyModifiers, key: &KeyEvent) -> bool {
    match (code, key.code) {
        (KeyCode::Char(expected), KeyCode::Char(actual)) => {
            if !expected.eq_ignore_ascii_case(&actual) {
                return false;
            }
            // 字母的大小写已经编码了 Shift，两边都折算成「是否需要 Shift」再比。
            let wants_shift = modifiers.contains(KeyModifiers::SHIFT) || expected.is_uppercase();
            let has_shift = key.modifiers.contains(KeyModifiers::SHIFT) || actual.is_uppercase();
            if wants_shift != has_shift {
                return false;
            }
            let strip = |modifiers: KeyModifiers| modifiers - KeyModifiers::SHIFT;
            strip(modifiers) == strip(key.modifiers)
        }
        _ => code == key.code && modifiers == key.modifiers,
    }
}

fn parse_key(name: &str) -> Result<KeyCode, String> {
    let named = match name {
        "enter" | "return" => Some(KeyCode::Enter),
        "esc" | "escape" => Some(KeyCode::Esc),
        "tab" => Some(KeyCode::Tab),
        "backtab" => Some(KeyCode::BackTab),
        "backspace" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        "insert" | "ins" => Some(KeyCode::Insert),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" => Some(KeyCode::PageUp),
        "pagedown" => Some(KeyCode::PageDown),
        "space" => Some(KeyCode::Char(' ')),
        "minus" => Some(KeyCode::Char('-')),
        "plus" => Some(KeyCode::Char('+')),
        "comma" => Some(KeyCode::Char(',')),
        "period" => Some(KeyCode::Char('.')),
        "slash" => Some(KeyCode::Char('/')),
        "semicolon" => Some(KeyCode::Char(';')),
        "colon" => Some(KeyCode::Char(':')),
        "ampersand" => Some(KeyCode::Char('&')),
        "backtick" => Some(KeyCode::Char('`')),
        "question" => Some(KeyCode::Char('?')),
        "leftbracket" => Some(KeyCode::Char('[')),
        "rightbracket" => Some(KeyCode::Char(']')),
        "f1" => Some(KeyCode::F(1)),
        "f2" => Some(KeyCode::F(2)),
        "f3" => Some(KeyCode::F(3)),
        "f4" => Some(KeyCode::F(4)),
        "f5" => Some(KeyCode::F(5)),
        "f6" => Some(KeyCode::F(6)),
        "f7" => Some(KeyCode::F(7)),
        "f8" => Some(KeyCode::F(8)),
        "f9" => Some(KeyCode::F(9)),
        "f10" => Some(KeyCode::F(10)),
        "f11" => Some(KeyCode::F(11)),
        "f12" => Some(KeyCode::F(12)),
        other => {
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(KeyCode::Char(c)),
                _ => None,
            }
        }
    };
    named.ok_or_else(|| format!("不认识的按键：{name}"))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PanelDefinition {
    pub name: String,
    pub command: String,
    pub refresh: u64,
}

/// 全部可配置的动作。默认值与原硬编码键位一致。
#[derive(Debug, Clone)]
pub struct Keys {
    pub prefix: Binding,
    pub help: Binding,
    pub settings: Binding,
    pub detach: Binding,
    pub reload_config: Binding,
    pub open_notification_target: Binding,
    pub goto: Binding,
    pub new_tab: Binding,
    pub rename_tab: Binding,
    pub next_tab: Binding,
    pub previous_tab: Binding,
    pub move_tab_previous: Binding,
    pub move_tab_next: Binding,
    pub close_tab: Binding,
    pub new_workspace: Binding,
    pub rename_workspace: Binding,
    pub close_workspace: Binding,
    pub workspace_picker: Binding,
    pub next_workspace: Binding,
    pub previous_workspace: Binding,
    pub split_vertical: Binding,
    pub split_horizontal: Binding,
    pub focus_pane_left: Binding,
    pub focus_pane_down: Binding,
    pub focus_pane_up: Binding,
    pub focus_pane_right: Binding,
    pub cycle_pane_next: Binding,
    pub cycle_pane_previous: Binding,
    pub last_pane: Binding,
    pub zoom: Binding,
    pub stack_panes: Binding,
    pub layout_next: Binding,
    pub layout_previous: Binding,
    pub float_pane: Binding,
    pub float_pin: Binding,
    pub float_hide: Binding,
    pub close_pane: Binding,
    pub rename_pane: Binding,
    pub kill_pane: Binding,
    pub move_pane: Binding,
    pub resize_mode: Binding,
    pub toggle_sidebar: Binding,
    pub scroll_bottom: Binding,
    pub copy_mode: Binding,
    pub next_agent: Binding,
    pub chat: Binding,
    pub send_text: Binding,
    pub interrupt: Binding,
    pub toggle_keep: Binding,
    pub remove_terminal: Binding,
    pub open_link: Binding,
    pub edit_history: Binding,
    pub read_pane: Binding,
    pub send_keys: Binding,
    pub wait_output: Binding,
    pub put_file: Binding,
    pub templates: Binding,
    pub machines: Binding,
    pub paste_image: Binding,
}

impl Default for Keys {
    fn default() -> Self {
        let prefix = Binding::parse("ctrl+b").expect("default prefix parses");
        let bind = |text: &str| Binding::parse(text).expect("default binding parses");
        Self {
            prefix,
            help: bind("prefix+?"),
            settings: bind("prefix+s"),
            detach: bind("prefix+q"),
            reload_config: bind("prefix+shift+r"),
            open_notification_target: bind("prefix+o"),
            goto: bind("prefix+g"),
            new_tab: bind("prefix+c"),
            rename_tab: bind("prefix+shift+t"),
            next_tab: bind("prefix+n"),
            previous_tab: bind("prefix+p"),
            move_tab_previous: Binding::Off,
            move_tab_next: Binding::Off,
            close_tab: bind("prefix+shift+x"),
            new_workspace: bind("prefix+shift+n"),
            rename_workspace: bind("prefix+shift+w"),
            close_workspace: bind("prefix+shift+d"),
            workspace_picker: bind("prefix+w"),
            next_workspace: Binding::Off,
            previous_workspace: Binding::Off,
            split_vertical: bind("prefix+v"),
            split_horizontal: bind("prefix+minus"),
            focus_pane_left: bind("prefix+h"),
            focus_pane_down: bind("prefix+j"),
            focus_pane_up: bind("prefix+k"),
            focus_pane_right: bind("prefix+l"),
            cycle_pane_next: bind("prefix+tab"),
            cycle_pane_previous: bind("prefix+backtab"),
            last_pane: Binding::Off,
            zoom: bind("prefix+z"),
            stack_panes: bind("prefix+t"),
            layout_next: bind("prefix+shift+l"),
            layout_previous: bind("prefix+shift+j"),
            float_pane: bind("prefix+f"),
            float_pin: bind("prefix+shift+f"),
            float_hide: bind("prefix+shift+h"),
            close_pane: bind("prefix+x"),
            rename_pane: bind("prefix+shift+p"),
            kill_pane: bind("prefix+shift+k"),
            move_pane: bind("prefix+m"),
            resize_mode: bind("prefix+r"),
            toggle_sidebar: bind("prefix+b"),
            scroll_bottom: Binding::Off,
            copy_mode: bind("prefix+leftbracket"),
            next_agent: bind("prefix+a"),
            chat: bind("prefix+shift+a"),
            send_text: bind("prefix+i"),
            interrupt: bind("prefix+shift+c"),
            toggle_keep: bind("prefix+shift+o"),
            remove_terminal: bind("prefix+shift+u"),
            open_link: Binding::Off,
            edit_history: bind("prefix+e"),
            read_pane: bind("prefix+slash"),
            send_keys: bind("prefix+shift+i"),
            wait_output: bind("prefix+shift+w"),
            put_file: bind("prefix+shift+p"),
            templates: bind("prefix+shift+t"),
            machines: bind("prefix+shift+m"),
            paste_image: bind("prefix+shift+v"),
        }
    }
}

impl Keys {
    /// 全部已支持的动作与当前绑定，供 `--default-config` 输出。
    pub fn entries(&self) -> Vec<(&'static str, Binding)> {
        vec![
            ("prefix", self.prefix),
            ("help", self.help),
            ("settings", self.settings),
            ("detach", self.detach),
            ("reload_config", self.reload_config),
            ("open_notification_target", self.open_notification_target),
            ("goto", self.goto),
            ("new_tab", self.new_tab),
            ("rename_tab", self.rename_tab),
            ("next_tab", self.next_tab),
            ("previous_tab", self.previous_tab),
            ("move_tab_previous", self.move_tab_previous),
            ("move_tab_next", self.move_tab_next),
            ("close_tab", self.close_tab),
            ("new_workspace", self.new_workspace),
            ("rename_workspace", self.rename_workspace),
            ("close_workspace", self.close_workspace),
            ("workspace_picker", self.workspace_picker),
            ("next_workspace", self.next_workspace),
            ("previous_workspace", self.previous_workspace),
            ("split_vertical", self.split_vertical),
            ("split_horizontal", self.split_horizontal),
            ("focus_pane_left", self.focus_pane_left),
            ("focus_pane_down", self.focus_pane_down),
            ("focus_pane_up", self.focus_pane_up),
            ("focus_pane_right", self.focus_pane_right),
            ("cycle_pane_next", self.cycle_pane_next),
            ("cycle_pane_previous", self.cycle_pane_previous),
            ("last_pane", self.last_pane),
            ("zoom", self.zoom),
            ("stack_panes", self.stack_panes),
            ("layout_next", self.layout_next),
            ("layout_previous", self.layout_previous),
            ("float_pane", self.float_pane),
            ("float_pin", self.float_pin),
            ("float_hide", self.float_hide),
            ("close_pane", self.close_pane),
            ("rename_pane", self.rename_pane),
            ("kill_pane", self.kill_pane),
            ("move_pane", self.move_pane),
            ("resize_mode", self.resize_mode),
            ("toggle_sidebar", self.toggle_sidebar),
            ("scroll_bottom", self.scroll_bottom),
            ("copy_mode", self.copy_mode),
            ("next_agent", self.next_agent),
            ("chat", self.chat),
            ("send_text", self.send_text),
            ("interrupt", self.interrupt),
            ("toggle_keep", self.toggle_keep),
            ("remove_terminal", self.remove_terminal),
            ("open_link", self.open_link),
            ("edit_history", self.edit_history),
            ("read_pane", self.read_pane),
            ("send_keys", self.send_keys),
            ("wait_output", self.wait_output),
            ("put_file", self.put_file),
            ("templates", self.templates),
            ("machines", self.machines),
            ("paste_image", self.paste_image),
        ]
    }

    fn path(
        &mut self,
        name: &str,
        value: &str,
        warnings: &mut Vec<String>,
        errors: &mut Vec<Diagnostic>,
        position: Position,
    ) {
        let slot = match name {
            "prefix" => &mut self.prefix,
            "help" => &mut self.help,
            "settings" => &mut self.settings,
            "detach" => &mut self.detach,
            "reload_config" => &mut self.reload_config,
            "open_notification_target" => &mut self.open_notification_target,
            "goto" => &mut self.goto,
            "new_tab" => &mut self.new_tab,
            "rename_tab" => &mut self.rename_tab,
            "next_tab" => &mut self.next_tab,
            "previous_tab" => &mut self.previous_tab,
            "move_tab_previous" => &mut self.move_tab_previous,
            "move_tab_next" => &mut self.move_tab_next,
            "close_tab" => &mut self.close_tab,
            "new_workspace" => &mut self.new_workspace,
            "rename_workspace" => &mut self.rename_workspace,
            "close_workspace" => &mut self.close_workspace,
            "workspace_picker" => &mut self.workspace_picker,
            "next_workspace" => &mut self.next_workspace,
            "previous_workspace" => &mut self.previous_workspace,
            "split_vertical" => &mut self.split_vertical,
            "split_horizontal" => &mut self.split_horizontal,
            "focus_pane_left" => &mut self.focus_pane_left,
            "focus_pane_down" => &mut self.focus_pane_down,
            "focus_pane_up" => &mut self.focus_pane_up,
            "focus_pane_right" => &mut self.focus_pane_right,
            "cycle_pane_next" => &mut self.cycle_pane_next,
            "cycle_pane_previous" => &mut self.cycle_pane_previous,
            "last_pane" => &mut self.last_pane,
            "zoom" => &mut self.zoom,
            "stack_panes" => &mut self.stack_panes,
            "layout_next" => &mut self.layout_next,
            "layout_previous" => &mut self.layout_previous,
            "float_pane" => &mut self.float_pane,
            "float_pin" => &mut self.float_pin,
            "float_hide" => &mut self.float_hide,
            "close_pane" => &mut self.close_pane,
            "rename_pane" => &mut self.rename_pane,
            "kill_pane" => &mut self.kill_pane,
            "move_pane" => &mut self.move_pane,
            "resize_mode" => &mut self.resize_mode,
            "toggle_sidebar" => &mut self.toggle_sidebar,
            "scroll_bottom" => &mut self.scroll_bottom,
            "copy_mode" => &mut self.copy_mode,
            "next_agent" => &mut self.next_agent,
            "chat" => &mut self.chat,
            "send_text" => &mut self.send_text,
            "interrupt" => &mut self.interrupt,
            "toggle_keep" => &mut self.toggle_keep,
            "remove_terminal" => &mut self.remove_terminal,
            "open_link" => &mut self.open_link,
            "edit_history" | "edit_scrollback" => &mut self.edit_history,
            "read_pane" => &mut self.read_pane,
            "send_keys" => &mut self.send_keys,
            "wait_output" => &mut self.wait_output,
            "put_file" => &mut self.put_file,
            "templates" => &mut self.templates,
            "machines" => &mut self.machines,
            "paste_image" => &mut self.paste_image,
            other => {
                errors.push(Diagnostic::at(position, format!("[keys] 未知动作 {other}")));
                return;
            }
        };
        match Binding::parse(value) {
            Ok(binding) => *slot = binding,
            Err(error) => warnings.push(format!("[keys] {error}")),
        }
    }
}

/// 冲突比较用的按键规范式：字母大小写与 shift 折算成同一种写法。
fn binding_key(binding: Binding) -> Option<String> {
    let (prefix, code, modifiers) = match binding {
        Binding::Off => return None,
        Binding::Prefix(code, modifiers) => (true, code, modifiers),
        Binding::Direct(code, modifiers) => (false, code, modifiers),
    };
    let (code, modifiers) = match code {
        KeyCode::Char(character) => {
            let shift = modifiers.contains(KeyModifiers::SHIFT) || character.is_uppercase();
            let modifiers = if shift {
                modifiers | KeyModifiers::SHIFT
            } else {
                modifiers - KeyModifiers::SHIFT
            };
            (KeyCode::Char(character.to_ascii_lowercase()), modifiers)
        }
        _ => (code, modifiers),
    };
    let binding = if prefix {
        Binding::Prefix(code, modifiers)
    } else {
        Binding::Direct(code, modifiers)
    };
    Some(binding.to_string().trim_matches('"').to_string())
}

#[derive(Debug, Clone)]
pub struct Config {
    /// 目标 hc session（`None` = 使用环境里的 `HC_SESSION` / 默认）。
    pub session: Option<String>,
    /// 启动时直接 attach 的终端（`t7` / 名字 / `machine/t7`）。
    pub attach: Option<String>,
    pub sidebar: bool,
    pub sidebar_width: u16,
    pub sidebar_min_width: u16,
    pub sidebar_max_width: u16,
    /// 终端宽度不超过这个值时进入手机单列布局；`0` 关闭。
    pub mobile_width_threshold: u16,
    /// 界面文案语言。
    pub lang: Lang,
    /// 每个终端保留的 scrollback 字节数。
    pub scrollback_bytes: usize,
    /// 新建 Pane 的起始目录。
    pub new_cwd: NewCwd,
    /// 新建 Pane 用的可执行文件；空表示交给 core 的默认 shell。
    pub default_shell: String,
    pub render_fps: u32,
    /// Optional modal keyboard preset; explicit bindings remain available.
    pub zellij_keys: bool,
    pub keys: Keys,
    pub theme: Theme,
    /// `[theme] source`：`auto` / `ghostty` / `omarchy` / `builtin`。
    pub theme_source: String,
    /// `[theme.custom]` 的原始键值，热重载来源后重新套用。
    pub theme_custom: Vec<(String, String)>,
    pub mouse_capture: bool,
    pub mouse_scroll_lines: usize,
    pub copy_on_select: bool,
    pub confirm_close: bool,
    pub collapsed_sidebar: CollapsedSidebar,
    pub host_cursor: HostCursor,
    pub pane_borders: PaneBorders,
    pub pane_gaps: bool,
    /// 分隔线是否沿着 Pane 区域的外沿绘制。
    pub pane_outer_borders: bool,
    /// 是否在焦点 Pane 右缘画滚动条。
    pub pane_scrollbars: bool,
    /// 边框上是否显示检测到的 Agent 名字。
    pub show_agent_labels_on_pane_borders: bool,
    /// 只有一个 Tab 时隐藏 Tab 栏。
    pub hide_tab_bar_when_single_tab: bool,
    pub tab_bar_position: TabBarPosition,
    pub agent_sort: AgentSort,
    pub indicators: Indicators,
    pub notifications: Delivery,
    /// 提示表面上同时保留的最大条数；内置 toast 不受影响。
    pub toast_limit: usize,
    pub sound: bool,
    pub sound_path: Option<String>,
    pub sound_done_path: Option<String>,
    pub sound_request_path: Option<String>,
    /// 启动时从 core 的 doc 恢复布局。
    pub restore: bool,
    /// 关闭界面模块运行时：使用内置外框，不加载任何模块（`--no-mods`）。
    pub no_mods: bool,
    /// 模块签名包的信任根：base64 Ed25519 公钥或 `.pub` 文件路径。
    pub mods_trust_roots: Vec<String>,
    /// 签名包撤销列表 JSON；`None` 时读配置目录的 mods-revocations.json。
    pub mods_revocations: Option<String>,
    /// 新建 Tab 时先问名字。
    pub prompt_new_tab: bool,
    /// 新建工作区时先问名字。
    pub prompt_new_workspace: bool,
    pub tab_bar_right: Vec<String>,
    pub tab_bar_right_separator: String,
    /// 写给宿主终端的窗口标题模板；空字符串表示不改标题。
    pub window_title: String,
    /// `[[keys.command]]` 解析结果，按文件顺序执行第一个命中的绑定。
    pub commands: Vec<CustomCommand>,
    pub panels: Vec<PanelDefinition>,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
    /// 加载期发现的问题，启动后显示一次。
    pub warnings: Vec<String>,
    /// `config check` 用的错误：语法、未知键、重复键，带行列。
    pub errors: Vec<Diagnostic>,
    /// 配置文件里出现过的键及位置：键是 `section.key`。
    pub positions: std::collections::BTreeMap<String, Position>,
    /// `[theme] name` 在文件里的位置；`None` 表示没写过这个键。
    pub theme_name_position: Option<Position>,
}

/// XDG 目录：变量存在且非空时用它，否则退回 `$HOME/<fallback>`。
fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            home.join(fallback)
        })
}

impl Config {
    /// 开发中的模块目录：`mods link` 与本地构建的模块放在配置目录的 mods/ 下。
    pub fn mods_dir(&self) -> std::path::PathBuf {
        self.config_path
            .parent()
            .map(|parent| parent.join("mods"))
            .unwrap_or_else(|| std::path::PathBuf::from("mods"))
    }

    /// 模块代际存储根：`<state>/mods/`。
    pub fn mods_state_dir(&self) -> std::path::PathBuf {
        self.state_path
            .parent()
            .map(|parent| parent.join("mods"))
            .unwrap_or_else(|| std::path::PathBuf::from("mods"))
    }

    /// 开发中的插件目录：`<config>/plugins/`。
    pub fn plugins_dir(&self) -> std::path::PathBuf {
        self.config_path
            .parent()
            .map(|parent| parent.join("plugins"))
            .unwrap_or_else(|| std::path::PathBuf::from("plugins"))
    }

    /// 插件代际存储与日志根：`<state>/plugins/`。
    pub fn plugins_state_dir(&self) -> std::path::PathBuf {
        self.state_path
            .parent()
            .map(|parent| parent.join("plugins"))
            .unwrap_or_else(|| std::path::PathBuf::from("plugins"))
    }

    /// 同一按键绑到两个动作。默认值里已知的重键（`docs/usage.md`「默认绑定冲突」）
    /// 不算配置引入的冲突；至少一侧被配置改过才算。
    pub fn key_conflicts(&self) -> Vec<Diagnostic> {
        let defaults: BTreeMap<&str, Binding> = Keys::default().entries().into_iter().collect();
        let mut groups: BTreeMap<String, Vec<(String, bool, Option<Position>)>> = BTreeMap::new();
        for (name, binding) in self.keys.entries() {
            let Some(key) = binding_key(binding) else {
                continue;
            };
            let changed = defaults.get(name) != Some(&binding);
            let position = self.positions.get(&format!("keys.{name}")).copied();
            groups
                .entry(key)
                .or_default()
                .push((name.to_string(), changed, position));
        }
        for (index, command) in self.commands.iter().enumerate() {
            let Some(key) = binding_key(command.key) else {
                continue;
            };
            groups.entry(key).or_default().push((
                format!("[[keys.command]] 第 {} 条", index + 1),
                true,
                None,
            ));
        }
        groups
            .into_iter()
            .filter(|(_, actions)| actions.len() > 1 && actions.iter().any(|action| action.1))
            .map(|(key, actions)| {
                let position = actions
                    .iter()
                    .filter_map(|action| action.2)
                    .min_by_key(|position| (position.line, position.column))
                    .unwrap_or_default();
                let labels: Vec<&str> = actions.iter().map(|action| action.0.as_str()).collect();
                Diagnostic::at(position, format!("键位冲突 {key}：{}", labels.join("、")))
            })
            .collect()
    }

    /// 撤销列表路径。
    pub fn mods_revocations_path(&self) -> std::path::PathBuf {
        self.mods_revocations
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                self.config_path
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join("mods-revocations.json")
            })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            session: None,
            attach: None,
            sidebar: true,
            sidebar_width: 26,
            sidebar_min_width: 18,
            sidebar_max_width: 36,
            mobile_width_threshold: 64,
            lang: Lang::Auto,
            scrollback_bytes: 8 << 20,
            new_cwd: NewCwd::Follow,
            default_shell: String::new(),
            render_fps: 30,
            zellij_keys: false,
            keys: Keys::default(),
            theme: theme::builtin("default").expect("default theme exists"),
            theme_source: theme::source::AUTO.to_string(),
            theme_custom: Vec::new(),
            mouse_capture: true,
            mouse_scroll_lines: 3,
            copy_on_select: false,
            confirm_close: true,
            collapsed_sidebar: CollapsedSidebar::Compact,
            host_cursor: HostCursor::Native,
            pane_borders: PaneBorders::Auto,
            pane_gaps: true,
            pane_outer_borders: true,
            pane_scrollbars: true,
            show_agent_labels_on_pane_borders: false,
            hide_tab_bar_when_single_tab: false,
            tab_bar_position: TabBarPosition::Top,
            agent_sort: AgentSort::Spaces,
            indicators: Indicators::Dots,
            notifications: Delivery::Toast,
            toast_limit: 3,
            sound: false,
            sound_path: None,
            sound_done_path: None,
            sound_request_path: None,
            restore: true,
            no_mods: false,
            mods_trust_roots: Vec::new(),
            mods_revocations: None,
            prompt_new_tab: true,
            prompt_new_workspace: false,
            tab_bar_right: Vec::new(),
            tab_bar_right_separator: " ".to_string(),
            window_title: "{hostname}: {workspace}".to_string(),
            commands: Vec::new(),
            panels: Vec::new(),
            config_path: xdg_dir("XDG_CONFIG_HOME", ".config").join("hc-tui/config.toml"),
            state_path: xdg_dir("XDG_STATE_HOME", ".local/state").join("hc-tui/state.json"),
            warnings: Vec::new(),
            errors: Vec::new(),
            positions: std::collections::BTreeMap::new(),
            theme_name_position: None,
        }
    }
}

/// `auto` 语言下，环境里的 locale 是否中文。
pub fn detect_zh() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("zh"))
}

impl Config {
    /// 界面文案是否用中文；`lang = "auto"` 时看环境变量里的 locale。
    pub fn zh(&self) -> bool {
        match self.lang {
            Lang::Zh => true,
            Lang::En => false,
            Lang::Auto => detect_zh(),
        }
    }

    /// 读配置文件。文件不存在时只是用默认值。
    pub fn load(path: Option<PathBuf>) -> Self {
        let mut config = Self::default();
        let path = path.unwrap_or_else(|| config.config_path.clone());
        config.config_path = path.clone();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                config.apply(&Document {
                    values: Table::new(),
                    arrays: Vec::new(),
                    array_positions: Vec::new(),
                    positions: BTreeMap::new(),
                    duplicates: Vec::new(),
                });
                config.load_panel_files();
                return config;
            }
            Err(error) => {
                let diagnostic = Diagnostic::whole(format!("读取文件失败：{error}"));
                config.warnings.push(describe(&path, &diagnostic));
                config.errors.push(diagnostic);
                return config;
            }
        };
        let document = match parse_toml(&text) {
            Ok(document) => document,
            Err(error) => {
                config.warnings.push(describe(&path, &error));
                config.errors.push(error);
                return config;
            }
        };
        config.errors = document
            .duplicates
            .iter()
            .map(|(position, message)| Diagnostic::at(*position, message.clone()))
            .collect();
        config.positions = document.positions.clone();
        let errors = config.apply(&document);
        config.errors.extend(errors);
        // 运行时没有 `config check`，错误也要进状态栏提示一次。
        let notes: Vec<String> = config
            .errors
            .iter()
            .map(|error| describe(&path, error))
            .collect();
        config.warnings.extend(notes);
        config.load_panel_files();
        config
    }

    fn load_panel_files(&mut self) {
        let path = self
            .config_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("panels");
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut paths: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();
        for path in paths {
            match std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<PanelDefinition>(&s).ok())
            {
                Some(mut panel) if !self.panels.iter().any(|p| p.name == panel.name) => {
                    panel.refresh = panel.refresh.clamp(1, 3600);
                    self.panels.push(panel)
                }
                Some(_) => {}
                None => self
                    .warnings
                    .push(crate::i18n::t1("Invalid panel {}", path.display())),
            }
        }
    }

    fn apply(&mut self, document: &Document) -> Vec<Diagnostic> {
        let mut warnings = Vec::new();
        let mut errors = Vec::new();
        let mut theme_name = self.theme.name.clone();
        let mut theme_name_position = None;
        let mut theme_source = self.theme_source.clone();
        let mut custom: Vec<(String, String)> = Vec::new();

        for (key, value) in &document.values {
            let path = key.as_str();
            match path {
                "keys.preset" => match value.text() {
                    Some("default") => self.zellij_keys = false,
                    Some("zellij") => self.zellij_keys = true,
                    _ => warnings.push(format!("{path}: expected default / zellij")),
                },
                "ui.sidebar" => {
                    self.sidebar = value.boolean(&mut warnings, path).unwrap_or(self.sidebar)
                }
                "ui.sidebar_width" => {
                    self.sidebar_width = value
                        .number(&mut warnings, path)
                        .unwrap_or(self.sidebar_width as i64)
                        as u16
                }
                "ui.sidebar_min_width" => {
                    self.sidebar_min_width = value
                        .number(&mut warnings, path)
                        .unwrap_or(self.sidebar_min_width as i64)
                        as u16
                }
                "ui.sidebar_max_width" => {
                    self.sidebar_max_width = value
                        .number(&mut warnings, path)
                        .unwrap_or(self.sidebar_max_width as i64)
                        as u16
                }
                "ui.mobile_width_threshold" => {
                    self.mobile_width_threshold = value
                        .number(&mut warnings, path)
                        .unwrap_or(self.mobile_width_threshold as i64)
                        .clamp(0, u16::MAX as i64)
                        as u16
                }
                "ui.lang" => match value.text() {
                    Some(text) => match Lang::parse(text) {
                        Some(lang) => self.lang = lang,
                        None => warnings.push(format!("{path}: 只支持 auto / en / zh")),
                    },
                    None => warnings.push(format!("{path}: 只支持 auto / en / zh")),
                },
                "ui.sidebar_start_collapsed" => {
                    self.sidebar = !value.boolean(&mut warnings, path).unwrap_or(true)
                }
                "ui.tab_bar_position" => match value.text() {
                    Some("top") => self.tab_bar_position = TabBarPosition::Top,
                    Some("bottom") => self.tab_bar_position = TabBarPosition::Bottom,
                    _ => warnings.push(format!("{path}: 只支持 top / bottom")),
                },
                "ui.pane_borders" => match value.text() {
                    Some("auto" | "true") => self.pane_borders = PaneBorders::Auto,
                    Some("always") => self.pane_borders = PaneBorders::Always,
                    Some("titles") => self.pane_borders = PaneBorders::Titles,
                    Some("off" | "false") => self.pane_borders = PaneBorders::Off,
                    _ => warnings.push(format!("{path}: 只支持 auto / always / off")),
                },
                "ui.pane_gaps" => {
                    self.pane_gaps = value.boolean(&mut warnings, path).unwrap_or(self.pane_gaps)
                }
                "ui.pane_outer_borders" => {
                    self.pane_outer_borders = value
                        .boolean(&mut warnings, path)
                        .unwrap_or(self.pane_outer_borders)
                }
                "ui.pane_scrollbars" => {
                    self.pane_scrollbars = value
                        .boolean(&mut warnings, path)
                        .unwrap_or(self.pane_scrollbars)
                }
                "ui.show_agent_labels_on_pane_borders" => {
                    self.show_agent_labels_on_pane_borders = value
                        .boolean(&mut warnings, path)
                        .unwrap_or(self.show_agent_labels_on_pane_borders)
                }
                "ui.hide_tab_bar_when_single_tab" => {
                    self.hide_tab_bar_when_single_tab = value
                        .boolean(&mut warnings, path)
                        .unwrap_or(self.hide_tab_bar_when_single_tab)
                }
                "ui.window_title" => match value.text() {
                    Some(text) => self.window_title = text.to_string(),
                    None => warnings.push(format!("{path}: 需要字符串")),
                },
                "ui.tab_bar_right_separator" => match value.text() {
                    Some(text) => self.tab_bar_right_separator = text.to_string(),
                    None => warnings.push(format!("{path}: 需要字符串")),
                },
                "ui.mouse_capture" => {
                    self.mouse_capture = value
                        .boolean(&mut warnings, path)
                        .unwrap_or(self.mouse_capture)
                }
                "ui.mouse_scroll_lines" => {
                    self.mouse_scroll_lines = value
                        .number(&mut warnings, path)
                        .unwrap_or(self.mouse_scroll_lines as i64)
                        .max(1) as usize
                }
                "ui.copy_on_select" => {
                    self.copy_on_select = value
                        .boolean(&mut warnings, path)
                        .unwrap_or(self.copy_on_select)
                }
                "ui.sidebar_collapsed_mode" => match value.text() {
                    Some("compact") => self.collapsed_sidebar = CollapsedSidebar::Compact,
                    Some("hidden") => self.collapsed_sidebar = CollapsedSidebar::Hidden,
                    _ => warnings.push(format!("{path}: 只支持 compact / hidden")),
                },
                "ui.host_cursor" => match value.text() {
                    Some("auto" | "native") => self.host_cursor = HostCursor::Native,
                    Some("drawn") => self.host_cursor = HostCursor::Drawn,
                    _ => warnings.push(format!("{path}: 只支持 native / drawn")),
                },
                "ui.confirm_close" => {
                    self.confirm_close = value
                        .boolean(&mut warnings, path)
                        .unwrap_or(self.confirm_close)
                }
                "ui.status_indicators" => match value.text() {
                    Some("dots") => self.indicators = Indicators::Dots,
                    Some("symbols") => self.indicators = Indicators::Symbols,
                    _ => warnings.push(format!("{path}: 只支持 dots / symbols")),
                },
                "ui.agent_panel_sort" => match value.text() {
                    Some("spaces" | "workspaces") => self.agent_sort = AgentSort::Spaces,
                    Some("priority") => self.agent_sort = AgentSort::Priority,
                    _ => warnings.push(format!("{path}: 只支持 spaces / priority")),
                },
                "ui.tab_bar_right" => match value.strings() {
                    Some(items) => self.tab_bar_right = items,
                    None => warnings.push(format!("{path}: 需要字符串数组")),
                },
                "ui.prompt_new_tab_name" => {
                    self.prompt_new_tab = value
                        .boolean(&mut warnings, path)
                        .unwrap_or(self.prompt_new_tab)
                }
                "ui.prompt_new_workspace_name" => {
                    self.prompt_new_workspace = value
                        .boolean(&mut warnings, path)
                        .unwrap_or(self.prompt_new_workspace)
                }
                "terminal.new_cwd" => match value.text() {
                    Some("follow") => self.new_cwd = NewCwd::Follow,
                    Some("home") => self.new_cwd = NewCwd::Home,
                    Some("current") => self.new_cwd = NewCwd::Current,
                    Some(path) => self.new_cwd = NewCwd::Path(path.to_string()),
                    None => warnings.push(format!("{path}: 需要字符串")),
                },
                "terminal.default_shell" => {
                    self.default_shell = value.text().unwrap_or_default().to_string()
                }
                "terminal.shell_mode" => {
                    // herdr 的登录 shell 开关；core 用 argv 决定，这里只接受该键。
                    let _ = value.text();
                }
                "terminal.scrollback_bytes" => {
                    self.scrollback_bytes = value
                        .number(&mut warnings, path)
                        .unwrap_or(self.scrollback_bytes as i64)
                        .max(0) as usize
                }
                "render.fps" => {
                    self.render_fps = value
                        .number(&mut warnings, path)
                        .unwrap_or(self.render_fps as i64)
                        .clamp(1, 240) as u32
                }
                "theme.name" => match value.text() {
                    Some(name) => {
                        theme_name = name.to_string();
                        theme_name_position = Some(document.position(path));
                    }
                    None => warnings.push(format!("{path}: 需要字符串")),
                },
                "theme.source" => match value.text() {
                    Some(text @ ("auto" | "ghostty" | "omarchy" | "builtin")) => {
                        theme_source = text.to_string()
                    }
                    _ => {
                        warnings.push(format!("{path}: 只支持 auto / ghostty / omarchy / builtin"))
                    }
                },
                "notifications.delivery" => match value.text() {
                    Some("off") => self.notifications = Delivery::Off,
                    Some("toast" | "herdr") => self.notifications = Delivery::Toast,
                    Some("system" | "terminal") => self.notifications = Delivery::System,
                    _ => warnings.push(format!("{path}: 只支持 off / toast / system")),
                },
                "notifications.delay_seconds" => {
                    // 保留键位兼容：toast 目前立即出现，不按延迟排队。
                    let _ = value.number(&mut warnings, path);
                }
                "notifications.toast_limit" => {
                    self.toast_limit = value
                        .number(&mut warnings, path)
                        .unwrap_or(self.toast_limit as i64)
                        .clamp(1, 20) as usize
                }
                "notifications.sound" => {
                    self.sound = value.boolean(&mut warnings, path).unwrap_or(self.sound)
                }
                "notifications.sound_path" => self.sound_path = value.text().map(str::to_string),
                "notifications.done_path" => {
                    self.sound_done_path = value.text().map(str::to_string)
                }
                "notifications.request_path" => {
                    self.sound_request_path = value.text().map(str::to_string)
                }
                "session.restore" | "persistence.restore" => {
                    self.restore = value.boolean(&mut warnings, path).unwrap_or(self.restore)
                }
                "session.session" => self.session = value.text().map(str::to_string),
                "mods.trust_roots" => match value.strings() {
                    Some(list) => self.mods_trust_roots = list,
                    None => warnings.push(format!("{path}: 需要字符串数组")),
                },
                "mods.revocations" => match value.text() {
                    Some(text) => self.mods_revocations = Some(text.to_string()),
                    None => warnings.push(format!("{path}: 需要字符串")),
                },
                _ => {
                    if let Some(name) = path.strip_prefix("keys.") {
                        if let Some(text) = value.text() {
                            self.keys.path(
                                name,
                                text,
                                &mut warnings,
                                &mut errors,
                                document.position(path),
                            );
                        } else {
                            warnings.push(format!("{path}: 需要字符串"));
                        }
                    } else if let Some(name) = path.strip_prefix("theme.custom.") {
                        match value.text() {
                            Some(text) => custom.push((name.to_string(), text.to_string())),
                            None => warnings.push(format!("{path}: 需要颜色字符串")),
                        }
                    } else {
                        errors.push(Diagnostic::at(
                            document.position(path),
                            format!("未知配置键 {path}"),
                        ));
                    }
                }
            }
        }

        let (theme, theme_warnings) = theme::source::apply(&theme_name, &theme_source, &custom);
        self.theme = theme;
        self.theme_source = theme_source;
        self.theme_name_position = theme_name_position;
        self.theme_custom = custom;
        warnings.extend(theme_warnings);
        self.parse_commands(document, &mut warnings, &mut errors);
        self.warnings.extend(warnings);
        errors
    }
}

impl Config {
    /// 解析 `[[keys.command]]` 数组表。坏条目只警告，不影响其它条目。
    fn parse_commands(
        &mut self,
        document: &Document,
        warnings: &mut Vec<String>,
        errors: &mut Vec<Diagnostic>,
    ) {
        for (index, (name, table)) in document.arrays.iter().enumerate() {
            let position = document
                .array_positions
                .get(index)
                .copied()
                .unwrap_or_default();
            if name == "panels" {
                if let (Some(name), Some(command)) = (
                    table.get("name").and_then(Value::text),
                    table.get("command").and_then(Value::text),
                ) {
                    self.panels.push(PanelDefinition {
                        name: name.to_string(),
                        command: command.to_string(),
                        refresh: table
                            .get("refresh")
                            .and_then(|value| value.number(warnings, "panels.refresh"))
                            .unwrap_or(5)
                            .clamp(1, 3600) as u64,
                    });
                } else {
                    warnings
                        .push(crate::i18n::t("[[panels]] requires name and command").to_string());
                }
                continue;
            }
            if name != "keys.command" {
                errors.push(Diagnostic::at(position, format!("未知的数组表 [[{name}]]")));
                continue;
            }
            let position = index + 1;
            let key_text = table.get("key").and_then(Value::text);
            let Some(key_text) = key_text else {
                warnings.push(format!("第 {position} 条自定义命令缺少 key"));
                continue;
            };
            let key = match Binding::parse(key_text) {
                Ok(Binding::Off) => {
                    warnings.push(format!("第 {position} 条自定义命令的 key 为空"));
                    continue;
                }
                Ok(binding) => binding,
                Err(error) => {
                    warnings.push(format!("第 {position} 条自定义命令：{error}"));
                    continue;
                }
            };
            let Some(command) = table.get("command").and_then(Value::text) else {
                warnings.push(format!("第 {position} 条自定义命令缺少 command"));
                continue;
            };
            let kind = match table.get("type").and_then(Value::text) {
                None => CommandKind::Pane,
                Some(text) => match CommandKind::parse(text) {
                    Some(kind) => kind,
                    None => {
                        warnings.push(format!(
                            "第 {position} 条自定义命令：type 只支持 shell / pane / popup / action"
                        ));
                        continue;
                    }
                },
            };
            let mut span = |key: &str| -> Option<Span> {
                let value = table.get(key)?;
                let span = match value {
                    Value::Text(text) => Span::parse(text),
                    Value::Number(number) if *number >= 0.0 => Some(Span::Cells(*number as u16)),
                    _ => None,
                };
                if span.is_none() {
                    warnings.push(format!(
                        "第 {position} 条自定义命令：{key} 需要格数或百分比，例如 40 或 \"80%\""
                    ));
                }
                span
            };
            let width = span("width");
            let height = span("height");
            self.commands.push(CustomCommand {
                key,
                kind,
                command: command.to_string(),
                width,
                height,
            });
        }
    }
}

type Table = BTreeMap<String, Value>;

/// 解析结果：`[section]` 里的键值，以及 `[[name]]` 数组表。
#[derive(Debug)]
struct Document {
    values: Table,
    arrays: Vec<(String, Table)>,
    /// 数组表头部的位置，与 `arrays` 对齐。
    array_positions: Vec<Position>,
    /// 每个键在文件里的位置：键是 `section.key`。
    positions: BTreeMap<String, Position>,
    /// 重复定义的键：第二次出现的位置与原因。
    duplicates: Vec<(Position, String)>,
}

impl Document {
    /// 某个键在文件里的位置；没记下来时返回零位置。
    fn position(&self, path: &str) -> Position {
        self.positions.get(path).copied().unwrap_or_default()
    }
}

/// `[keys]` 段的重置计划。
#[derive(Debug, Clone)]
pub struct KeysReset {
    /// 替换后的全文。
    pub text: String,
    /// 被替换的第一行与最后一行（1 起算，含）。
    pub first_line: usize,
    pub last_line: usize,
    /// 文件里其余的表名，按出现顺序。
    pub sections: Vec<String>,
}

/// `[keys]` 段的默认内容：`preset` 加全部默认绑定。
fn default_keys_section() -> String {
    let mut out = String::from("[keys]\npreset = \"default\"   # default | zellij\n");
    for (name, binding) in Keys::default().entries() {
        out.push_str(&format!("{name} = {binding}\n"));
    }
    out
}

/// 一行是表头时返回表名（`[name]` 与 `[[name]]` 都算）。
fn header_name(line: &str) -> Option<&str> {
    let trimmed = strip_comment(line).trim();
    let inner = if let Some(inner) = trimmed.strip_prefix("[[") {
        inner.strip_suffix("]]")?
    } else {
        let inner = trimmed.strip_prefix('[')?;
        inner.strip_suffix(']')?
    };
    Some(inner.trim())
}

/// 把 `[keys]` 段换成默认键位，其它行原样保留；没有 `[keys]` 段时返回 `None`。
pub fn reset_keys(text: &str) -> Option<KeysReset> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let start = lines
        .iter()
        .position(|line| header_name(line) == Some("keys"))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| header_name(line).is_some())
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let sections: Vec<String> = lines
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != start)
        .filter_map(|(_, line)| header_name(line).map(str::to_string))
        .collect();
    // 段尾的空行是段落分隔，不跟着换成默认键位。
    let mut last = end;
    while last > start + 1 && strip_comment(lines[last - 1]).trim().is_empty() {
        last -= 1;
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut block = default_keys_section();
    if eol == "\r\n" {
        block = block.replace('\n', eol);
    }
    if last == lines.len() && !text.ends_with('\n') {
        let keep = block.trim_end_matches(eol).len();
        block.truncate(keep);
    }
    let mut output = String::new();
    for line in &lines[..start] {
        output.push_str(line);
    }
    if start > 0 && !output.ends_with('\n') {
        output.push_str(eol);
    }
    output.push_str(&block);
    for line in &lines[last..] {
        output.push_str(line);
    }
    Some(KeysReset {
        text: output,
        first_line: start + 1,
        last_line: last,
        sections,
    })
}

/// `--default-config` 的输出：当前支持的每个字段。
/// 把 `key = value` 写回配置文件。已有行就地替换，没有就追加到对应
/// section 末尾；section 不存在时新建。写盘失败只返回错误文本。
pub fn write_setting(path: &std::path::Path, key: &str, value: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (section, name) = match key.rsplit_once('.') {
        Some((section, name)) => (Some(section.to_string()), name.to_string()),
        None => (None, key.to_string()),
    };
    let wanted = format!("{name} = {value}");
    let mut current = String::new();
    let mut replaced = false;
    let mut insert_at = None;
    for (index, line) in lines.iter_mut().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if Some(current.as_str()) == section.as_deref() {
                insert_at = Some(index);
            }
            current = trimmed
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            continue;
        }
        if let Some((lhs, _)) = trimmed.split_once('=') {
            if lhs.trim() == name && Some(current.as_str()) == section.as_deref() {
                *line = wanted.clone();
                replaced = true;
                break;
            }
        }
    }
    if !replaced {
        match section {
            None => lines.insert(0, wanted),
            Some(section) => {
                if let Some(index) = insert_at {
                    lines.insert(index, wanted);
                } else {
                    if !lines.is_empty() && !lines.last().is_some_and(|line| line.is_empty()) {
                        lines.push(String::new());
                    }
                    lines.push(format!("[{section}]"));
                    lines.push(wanted);
                }
            }
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    std::fs::write(path, out).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn default_config_text() -> String {
    let config = Config::default();
    let mut out = String::new();
    out.push_str("# hc-tui configuration\n# Place at ~/.config/hc-tui/config.toml\n\n[ui]\n");
    out.push_str(&format!("sidebar = {}\n", config.sidebar));
    out.push_str(&format!("sidebar_width = {}\n", config.sidebar_width));
    out.push_str(&format!(
        "sidebar_min_width = {}\n",
        config.sidebar_min_width
    ));
    out.push_str(&format!(
        "sidebar_max_width = {}\n",
        config.sidebar_max_width
    ));
    out.push_str(&format!(
        "mobile_width_threshold = {}   # 宽度 <= 此值走手机单列布局，0 关闭\n",
        config.mobile_width_threshold
    ));
    out.push_str(&format!("sidebar_start_collapsed = {}\n", !config.sidebar));
    out.push_str("lang = \"auto\"    # auto | en | zh，auto 跟随 locale\n");
    out.push_str("tab_bar_position = \"top\"   # top | bottom\n");
    out.push_str("pane_borders = \"auto\"      # auto | always | titles | off\n");
    out.push_str(&format!("pane_gaps = {}\n", config.pane_gaps));
    out.push_str(&format!(
        "pane_outer_borders = {}\n",
        config.pane_outer_borders
    ));
    out.push_str(&format!("pane_scrollbars = {}\n", config.pane_scrollbars));
    out.push_str(&format!(
        "show_agent_labels_on_pane_borders = {}\n",
        config.show_agent_labels_on_pane_borders
    ));
    out.push_str(&format!(
        "hide_tab_bar_when_single_tab = {}\n",
        config.hide_tab_bar_when_single_tab
    ));
    out.push_str(&format!("mouse_capture = {}\n", config.mouse_capture));
    out.push_str(&format!(
        "mouse_scroll_lines = {}\n",
        config.mouse_scroll_lines
    ));
    out.push_str(&format!("copy_on_select = {}\n", config.copy_on_select));
    out.push_str(&format!("confirm_close = {}\n", config.confirm_close));
    out.push_str("sidebar_collapsed_mode = \"compact\" # compact | hidden\n");
    out.push_str("host_cursor = \"native\"      # native | drawn\n");
    out.push_str("status_indicators = \"dots\"   # dots | symbols\n");
    out.push_str(&format!(
        "agent_panel_sort = \"{}\" # spaces | priority\n",
        if config.agent_sort == AgentSort::Spaces {
            "spaces"
        } else {
            "priority"
        }
    ));
    out.push_str(&format!(
        "prompt_new_tab_name = {}\n",
        config.prompt_new_tab
    ));
    out.push_str(&format!(
        "prompt_new_workspace_name = {}\n",
        config.prompt_new_workspace
    ));
    out.push_str(&format!("window_title = \"{}\"\n", config.window_title));
    out.push_str("tab_bar_right = [");
    out.push_str(
        &config
            .tab_bar_right
            .iter()
            .map(|token| format!("\"{token}\""))
            .collect::<Vec<_>>()
            .join(", "),
    );
    out.push_str("]\n");
    out.push_str(&format!(
        "tab_bar_right_separator = \"{}\"\n",
        config.tab_bar_right_separator
    ));
    out.push_str("\n[terminal]\n");
    out.push_str(&format!("scrollback_bytes = {}\n", config.scrollback_bytes));
    let new_cwd = match &config.new_cwd {
        NewCwd::Follow => "follow".to_string(),
        NewCwd::Home => "home".to_string(),
        NewCwd::Current => "current".to_string(),
        NewCwd::Path(path) => path.clone(),
    };
    out.push_str(&format!(
        "new_cwd = \"{new_cwd}\"   # follow | home | current | 固定路径\n"
    ));
    out.push_str(&format!("default_shell = \"{}\"\n", config.default_shell));
    out.push_str("\n[theme]\n");
    out.push_str("# name 是内置主题名，或 auto / omarchy / ghostty 跟随终端主题。\n");
    out.push_str(&format!("name = \"{}\"\n", config.theme.name));
    out.push_str("# source 只在 name = \"default\" 时生效：auto 先 Ghostty 后 Omarchy。\n");
    out.push_str(&format!("source = \"{}\"\n", config.theme_source));
    out.push_str("# [theme.custom]\n# accent = \"#89b4fa\"\n");
    out.push_str("\n[notifications]\n");
    out.push_str("delivery = \"toast\"   # off | toast | system\n");
    out.push_str("toast_limit = 3      # 提示表面同时保留的条数\n");
    out.push_str("sound = false\n");
    out.push_str("# sound_path = \"/path/to/notification.mp3\"\n");
    out.push_str("# done_path = \"/path/to/done.mp3\"\n");
    out.push_str("# request_path = \"/path/to/request.mp3\"\n");
    out.push_str("\n[session]\n");
    out.push_str(&format!("restore = {}\n", config.restore));
    out.push('\n');
    out.push_str(&default_keys_section());
    out.push_str(
        "\n# 自定义命令：key 支持 prefix+... 或 ctrl+alt+...；type 为 shell / pane / popup / action\n\
         # [[keys.command]]\n\
         # key = \"prefix+alt+g\"\n\
         # type = \"popup\"\n\
         # command = \"lazygit\"\n\
         # width = \"80%\"\n\
         # height = \"80%\"\n",
    );
    out.push_str("\n# 内置主题：");
    out.push_str(
        &[
            "default",
            "catppuccin",
            "catppuccin-latte",
            "tokyo-night",
            "dracula",
            "nord",
            "gruvbox",
            "one-dark",
            "solarized",
            "kanagawa",
            "rose-pine",
            "vesper",
        ]
        .join(", "),
    );
    out.push('\n');
    out.push_str("# 跟随终端主题：name = \"auto\" / \"omarchy\" / \"ghostty\"\n");
    out.push_str("# 主题 token：");
    out.push_str(
        &config
            .theme
            .tokens()
            .map(|(name, _)| name.to_string())
            .collect::<Vec<_>>()
            .join(", "),
    );
    out.push('\n');
    out.push_str("\n[mods]\n");
    out.push_str("# 签名包信任根：base64 Ed25519 公钥或 .pub 文件路径。\n");
    out.push_str("trust_roots = []\n");
    out.push_str("# 撤销列表 JSON；默认读配置目录的 mods-revocations.json。\n");
    out.push_str("# revocations = \"/path/to/mods-revocations.json\"\n");
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    Bool(bool),
    Number(f64),
    Array(Vec<Value>),
}

impl Value {
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    fn boolean(&self, warnings: &mut Vec<String>, path: &str) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => {
                warnings.push(format!("{path}: 需要 true / false"));
                None
            }
        }
    }

    fn number(&self, warnings: &mut Vec<String>, path: &str) -> Option<i64> {
        match self {
            Self::Number(value) => Some(*value as i64),
            _ => {
                warnings.push(format!("{path}: 需要数字"));
                None
            }
        }
    }

    fn strings(&self) -> Option<Vec<String>> {
        match self {
            Self::Array(items) => items
                .iter()
                .map(|item| item.text().map(str::to_string))
                .collect(),
            _ => None,
        }
    }
}

/// 解析 TOML 子集。`[section]` 的键合并成 `section.key`；`[[name]]` 每次开一条新表。
fn parse_toml(text: &str) -> Result<Document, Diagnostic> {
    let mut values = Table::new();
    let mut arrays: Vec<(String, Table)> = Vec::new();
    let mut array_positions: Vec<Position> = Vec::new();
    let mut positions: BTreeMap<String, Position> = BTreeMap::new();
    let mut duplicates: Vec<(Position, String)> = Vec::new();
    let mut section = String::new();
    let mut current: Option<(String, Table, Position)> = None;
    for (index, raw) in text.lines().enumerate() {
        let without_comment = strip_comment(raw);
        let line = without_comment.trim();
        if line.is_empty() {
            continue;
        }
        let position = Position {
            line: index + 1,
            column: without_comment[..without_comment.len() - line.len()]
                .chars()
                .count()
                + 1,
        };
        if let Some(inner) = line.strip_prefix("[[") {
            let name = inner
                .strip_suffix("]]")
                .ok_or_else(|| Diagnostic::at(position, "数组表缺少 ]]"))?
                .trim();
            if name.is_empty() {
                return Err(Diagnostic::at(position, "数组表名为空"));
            }
            if let Some(entry) = current.take() {
                arrays.push((entry.0, entry.1));
                array_positions.push(entry.2);
            }
            current = Some((name.to_string(), Table::new(), position));
            section.clear();
            continue;
        }
        if let Some(inner) = line.strip_prefix('[') {
            let name = inner
                .strip_suffix(']')
                .ok_or_else(|| Diagnostic::at(position, "section 缺少 ]"))?
                .trim();
            if name.is_empty() {
                return Err(Diagnostic::at(position, "section 名为空"));
            }
            if let Some(entry) = current.take() {
                arrays.push((entry.0, entry.1));
                array_positions.push(entry.2);
            }
            section = name.to_string();
            continue;
        }
        let (key, rest) = line
            .split_once('=')
            .ok_or_else(|| Diagnostic::at(position, "缺少 ="))?;
        let key = key.trim().to_string();
        let label = match &current {
            Some((name, _, _)) => format!("{name}.{key}"),
            None if section.is_empty() => key.clone(),
            None => format!("{section}.{key}"),
        };
        let value_start = line.len() - rest.len() + (rest.len() - rest.trim_start().len());
        let value_position = Position {
            line: position.line,
            column: position.column + line[..value_start].chars().count(),
        };
        let value = parse_value(rest.trim())
            .map_err(|error| Diagnostic::at(value_position, format!("{label}: {error}")))?;
        match &mut current {
            Some((name, table, header)) => {
                if table.insert(key.clone(), value).is_some() {
                    duplicates.push((
                        value_position,
                        format!(
                            "[[{name}]]（第 {} 行开始）里的键 {key} 重复定义",
                            header.line
                        ),
                    ));
                }
            }
            None => {
                let path = if section.is_empty() {
                    key.clone()
                } else {
                    format!("{section}.{key}")
                };
                if values.insert(path.clone(), value).is_some() {
                    let table = if section.is_empty() {
                        "顶层".to_string()
                    } else {
                        format!("[{section}]")
                    };
                    duplicates.push((value_position, format!("{table} 里的键 {key} 重复定义")));
                }
                positions.insert(path, position);
            }
        }
    }
    if let Some(entry) = current.take() {
        arrays.push((entry.0, entry.1));
        array_positions.push(entry.2);
    }
    Ok(Document {
        values,
        arrays,
        array_positions,
        positions,
        duplicates,
    })
}

fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (index, character) in line.char_indices() {
        match character {
            '"' | '\'' => match quote {
                None => quote = Some(character),
                Some(open) if open == character => quote = None,
                Some(_) => {}
            },
            '#' if quote.is_none() => return &line[..index],
            _ => {}
        }
    }
    line
}

fn parse_value(text: &str) -> Result<Value, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("缺少值".to_string());
    }
    if text.starts_with('"') || text.starts_with('\'') {
        let (value, rest) = parse_string(text)?;
        if !rest.trim().is_empty() {
            return Err(format!("字符串后有额外内容：{}", rest.trim()));
        }
        return Ok(Value::Text(value));
    }
    if let Some(inner) = text.strip_prefix('[') {
        let inner = inner
            .strip_suffix(']')
            .ok_or_else(|| "数组缺少 ]".to_string())?;
        let mut items = Vec::new();
        let mut rest = inner.trim();
        while !rest.is_empty() {
            let (value, tail) = if rest.starts_with('"') || rest.starts_with('\'') {
                let (value, tail) = parse_string(rest)?;
                (Value::Text(value), tail)
            } else {
                let (head, tail) = match rest.split_once(',') {
                    Some((head, tail)) => (head, tail),
                    None => (rest, ""),
                };
                (parse_scalar(head.trim())?, tail)
            };
            items.push(value);
            let tail = tail.trim();
            if let Some(next) = tail.strip_prefix(',') {
                rest = next.trim();
            } else if tail.is_empty() {
                rest = "";
            } else {
                return Err(format!("数组元素后有多余内容：{tail}"));
            }
        }
        return Ok(Value::Array(items));
    }
    let (head, tail) = match text.split_once('#') {
        Some((head, _)) => (head, ""),
        None => (text, ""),
    };
    if !tail.is_empty() {
        return Err("多余内容".to_string());
    }
    parse_scalar(head.trim())
}

fn parse_scalar(text: &str) -> Result<Value, String> {
    match text {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    let cleaned = text.replace('_', "");
    if let Ok(number) = cleaned.parse::<i64>() {
        return Ok(Value::Number(number as f64));
    }
    if let Ok(number) = cleaned.parse::<f64>() {
        return Ok(Value::Number(number));
    }
    Err(format!("无法解析的值：{text}"))
}

fn parse_string(text: &str) -> Result<(String, &str), String> {
    let quote = text
        .chars()
        .next()
        .ok_or_else(|| "缺少字符串".to_string())?;
    let mut out = String::new();
    let mut chars = text.char_indices().skip(1);
    while let Some((index, character)) = chars.next() {
        if character == quote {
            return Ok((out, &text[index + character.len_utf8()..]));
        }
        if character == '\\' && quote == '"' {
            match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, escaped)) => out.push(escaped),
                None => return Err("字符串以转义符结尾".to_string()),
            }
            continue;
        }
        out.push(character);
    }
    Err("字符串缺少结束引号".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: crossterm::event::KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn zellij_preset_is_optional_and_preserves_explicit_bindings() {
        let mut config = Config::default();
        assert!(!config.zellij_keys);
        let document = parse_toml("[keys]\npreset = \"zellij\"\nnew_tab = \"prefix+t\"\n").unwrap();
        config.apply(&document);
        assert!(config.zellij_keys);
        assert_eq!(config.keys.new_tab, Binding::parse("prefix+t").unwrap());
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        let document = parse_toml(&default_config_text()).unwrap();
        config.apply(&document);
        assert!(!config.zellij_keys);
    }

    #[test]
    fn parses_mods_trust_roots_and_revocations() {
        let mut config = Config::default();
        let document = parse_toml(
            "[mods]\ntrust_roots = [\"abc\", \"/tmp/root.pub\"]\nrevocations = \"/tmp/revocations.json\"\n",
        )
        .unwrap();
        config.apply(&document);
        assert_eq!(config.mods_trust_roots, vec!["abc", "/tmp/root.pub"]);
        assert_eq!(
            config.mods_revocations.as_deref(),
            Some("/tmp/revocations.json")
        );
        assert_eq!(
            config.mods_revocations_path(),
            std::path::PathBuf::from("/tmp/revocations.json")
        );
        assert!(config.mods_state_dir().ends_with("mods"));
    }

    #[test]
    fn parses_the_supported_subset() {
        let document = parse_toml(
            r#"
            # comment
            [ui]
            sidebar = false
            sidebar_width = 30   # trailing
            tab_bar_right = ["zoom", "datetime"]
            [theme]
            name = "nord"
            [keys]
            new_tab = "prefix+t"
            "prefix+?" = "unused"
            "#,
        )
        .unwrap();
        let table = &document.values;
        assert!(document.arrays.is_empty());
        assert_eq!(table.get("ui.sidebar"), Some(&Value::Bool(false)));
        assert_eq!(table.get("ui.sidebar_width"), Some(&Value::Number(30.0)));
        assert_eq!(
            table.get("ui.tab_bar_right"),
            Some(&Value::Array(vec![
                Value::Text("zoom".to_string()),
                Value::Text("datetime".to_string())
            ]))
        );
        assert_eq!(
            table.get("theme.name"),
            Some(&Value::Text("nord".to_string()))
        );
        assert_eq!(
            table.get("keys.new_tab"),
            Some(&Value::Text("prefix+t".to_string()))
        );
    }

    #[test]
    fn array_tables_collect_one_entry_per_header() {
        let document = parse_toml(
            r#"
            [ui]
            sidebar = true

            [[keys.command]]
            key = "prefix+alt+g"
            type = "popup"
            command = "lazygit"
            width = "80%"
            height = "80%"

            [[keys.command]]
            key = "ctrl+alt+t"
            command = "just test"
            "#,
        )
        .unwrap();
        assert_eq!(document.arrays.len(), 2);
        assert_eq!(document.arrays[0].0, "keys.command");
        assert_eq!(
            document.arrays[0].1.get("type"),
            Some(&Value::Text("popup".to_string()))
        );
        assert_eq!(
            document.arrays[1].1.get("key").and_then(Value::text),
            Some("ctrl+alt+t")
        );
        assert_eq!(document.values.get("ui.sidebar"), Some(&Value::Bool(true)));
    }

    #[test]
    fn custom_commands_parse_with_defaults_and_spans() {
        let path = temp_file(
            "commands",
            r##"
            [[keys.command]]
            key = "prefix+g"
            type = "popup"
            command = "lazygit"
            width = "80%"
            height = 20

            [[keys.command]]
            key = "prefix+r"
            command = "./run.sh"

            [[keys.command]]
            key = "prefix+bad"
            type = "tab"
            command = "nope"

            [[keys.command]]
            command = "missing key"
            "##,
        );
        let config = Config::load(Some(path));
        assert_eq!(config.commands.len(), 2);
        assert_eq!(config.commands[0].kind, CommandKind::Popup);
        assert_eq!(config.commands[0].command, "lazygit");
        assert_eq!(config.commands[0].width, Some(Span::Percent(80)));
        assert_eq!(config.commands[0].height, Some(Span::Cells(20)));
        // 省略 type 时是临时 Pane。
        assert_eq!(config.commands[1].kind, CommandKind::Pane);
        assert_eq!(config.commands[1].width, None);
        assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);
    }

    #[test]
    fn spans_resolve_against_the_content_size() {
        assert_eq!(Span::Percent(80).resolve(100), 80);
        assert_eq!(Span::Percent(50).resolve(31), 16);
        assert_eq!(Span::Cells(200).resolve(40), 40);
        assert_eq!(Span::parse("40"), Some(Span::Cells(40)));
        assert_eq!(Span::parse("80%"), Some(Span::Percent(80)));
        assert_eq!(Span::parse("0%"), None);
        assert_eq!(Span::parse("abc"), None);
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        assert_eq!(
            parse_value("\"#89b4fa\"").unwrap(),
            Value::Text("#89b4fa".to_string())
        );
    }

    #[test]
    fn bindings_match_modifiers_and_case() {
        let binding = Binding::parse("prefix+shift+r").unwrap();
        assert!(binding.matches_prefix(&event(KeyCode::Char('R'), KeyModifiers::SHIFT)));
        assert!(binding.matches_prefix(&event(KeyCode::Char('R'), KeyModifiers::NONE)));
        assert!(!binding.matches_prefix(&event(KeyCode::Char('r'), KeyModifiers::NONE)));

        let direct = Binding::parse("ctrl+b").unwrap();
        assert!(direct.matches_direct(&event(KeyCode::Char('b'), KeyModifiers::CONTROL)));
        assert!(!direct.matches_direct(&event(KeyCode::Char('b'), KeyModifiers::NONE)));
    }

    #[test]
    fn named_punctuation_parses() {
        assert_eq!(
            Binding::parse("prefix+minus").unwrap(),
            Binding::Prefix(KeyCode::Char('-'), KeyModifiers::NONE)
        );
        assert_eq!(
            Binding::parse("prefix+question").unwrap(),
            Binding::Prefix(KeyCode::Char('?'), KeyModifiers::NONE)
        );
        assert_eq!(Binding::parse("").unwrap(), Binding::Off);
    }

    fn temp_file(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hctui-config-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn the_default_config_text_parses_back_without_warnings() {
        let path = temp_file("default", &default_config_text());
        let config = Config::load(Some(path));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        let defaults = Keys::default();
        for (name, binding) in defaults.entries() {
            let parsed = config
                .keys
                .entries()
                .into_iter()
                .find(|(key, _)| *key == name)
                .map(|(_, binding)| binding);
            assert_eq!(parsed, Some(binding), "{name}");
        }
        assert_eq!(config.theme.name, "default");
    }

    #[test]
    fn mobile_width_threshold_is_configurable() {
        let path = temp_file("mobile", "[ui]\nmobile_width_threshold = 40\n");
        let config = Config::load(Some(path));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.mobile_width_threshold, 40);
    }

    #[test]
    fn lang_is_configurable() {
        let path = temp_file("lang", "[ui]\nlang = \"zh\"\n");
        let config = Config::load(Some(path));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.lang, Lang::Zh);
        assert!(config.zh());
    }

    #[test]
    fn a_real_file_overrides_defaults() {
        let path = temp_file(
            "custom",
            r##"
            [ui]
            sidebar = false
            sidebar_width = 31
            pane_borders = "off"
            pane_gaps = false
            tab_bar_position = "bottom"
            status_indicators = "symbols"
            agent_panel_sort = "spaces"
            mouse_scroll_lines = 7

            [theme]
            name = "gruvbox"
            [theme.custom]
            accent = "#55aa77"

            [notifications]
            delivery = "off"

            [session]
            restore = false

            [keys]
            prefix = "ctrl+a"
            new_tab = "prefix+t"
            kill_pane = ""
            "##,
        );
        let config = Config::load(Some(path));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert!(!config.sidebar);
        assert_eq!(config.sidebar_width, 31);
        assert_eq!(config.mobile_width_threshold, 64);
        assert_eq!(config.pane_borders, PaneBorders::Off);
        assert_eq!(
            Config::load(Some(temp_file(
                "titles",
                "[ui]\npane_borders = \"titles\"\n"
            )))
            .pane_borders,
            PaneBorders::Titles
        );
        assert!(!config.pane_gaps);
        assert_eq!(config.tab_bar_position, TabBarPosition::Bottom);
        assert_eq!(config.indicators, Indicators::Symbols);
        assert_eq!(config.agent_sort, AgentSort::Spaces);
        assert_eq!(config.mouse_scroll_lines, 7);
        assert_eq!(config.theme.name, "gruvbox");
        assert_eq!(
            config.theme.color("accent"),
            ratatui_core::style::Color::Rgb(85, 170, 119)
        );
        assert_eq!(config.notifications, Delivery::Off);
        assert!(!config.restore);
        assert_eq!(
            config.keys.prefix,
            Binding::Direct(KeyCode::Char('a'), KeyModifiers::CONTROL)
        );
        assert_eq!(
            config.keys.new_tab,
            Binding::Prefix(KeyCode::Char('t'), KeyModifiers::NONE)
        );
        assert_eq!(config.keys.kill_pane, Binding::Off);
    }

    #[test]
    fn unknown_keys_are_reported_not_fatal() {
        let mut config = Config::default();
        let mut warnings = Vec::new();
        let mut errors = Vec::new();
        config.keys.path(
            "new_tab",
            "prefix+t",
            &mut warnings,
            &mut errors,
            Position::default(),
        );
        config.keys.path(
            "nonsense",
            "prefix+y",
            &mut warnings,
            &mut errors,
            Position::default(),
        );
        config.keys.path(
            "close_tab",
            "prefix+notakey",
            &mut warnings,
            &mut errors,
            Position::default(),
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("未知动作 nonsense"));
        assert_eq!(
            config.keys.new_tab,
            Binding::Prefix(KeyCode::Char('t'), KeyModifiers::NONE)
        );
    }

    #[test]
    fn syntax_errors_carry_line_and_column() {
        let error = parse_toml("[theme]\nname = \"nord\n").unwrap_err();
        assert_eq!((error.line, error.column), (2, 8), "{error}");
        assert!(error.message.contains("结束引号"), "{error}");
        let error = parse_toml("[ui\nsidebar = true\n").unwrap_err();
        assert_eq!((error.line, error.column), (1, 1), "{error}");
        assert!(error.message.contains("section 缺少 ]"), "{error}");
    }

    #[test]
    fn unknown_and_duplicate_keys_are_errors() {
        let path = temp_file("unknown", "[ui]\nsiderbar = false\n");
        let config = Config::load(Some(path));
        assert_eq!(config.errors.len(), 1, "{:?}", config.errors);
        assert_eq!((config.errors[0].line, config.errors[0].column), (2, 1));
        assert!(
            config.errors[0].message.contains("未知配置键 ui.siderbar"),
            "{:?}",
            config.errors
        );
        // 运行时没有 `config check`：同一个问题进状态栏提示。
        assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
        assert!(config.warnings[0].contains("未知配置键 ui.siderbar"));

        let path = temp_file("duplicate", "[ui]\nsidebar = true\nsidebar = false\n");
        let config = Config::load(Some(path));
        assert_eq!(config.errors.len(), 1, "{:?}", config.errors);
        assert_eq!((config.errors[0].line, config.errors[0].column), (3, 11));
        assert!(
            config.errors[0].message.contains("重复定义"),
            "{:?}",
            config.errors
        );
        // 重复键只报一次，不得连累运行时取值。
        assert!(!config.sidebar);
    }

    #[test]
    fn key_conflicts_name_the_introducing_binding() {
        let path = temp_file("conflict", "[keys]\nnew_tab = \"prefix+q\"\n");
        let config = Config::load(Some(path));
        assert!(config.errors.is_empty(), "{:?}", config.errors);
        let conflicts = config.key_conflicts();
        assert_eq!(conflicts.len(), 1, "{:?}", conflicts);
        assert!(conflicts[0].message.contains("prefix+q"), "{:?}", conflicts);
        assert!(conflicts[0].message.contains("new_tab"), "{:?}", conflicts);
        assert!(conflicts[0].message.contains("detach"), "{:?}", conflicts);
        assert_eq!((conflicts[0].line, conflicts[0].column), (2, 1));

        // 默认值里已知的三对重键不是配置引入的。
        let path = temp_file("default-conflicts", &default_config_text());
        assert!(Config::load(Some(path)).key_conflicts().is_empty());
    }

    #[test]
    fn reset_keys_replaces_only_the_keys_section() {
        let text = "[ui]\nsidebar = false\n\n[keys]\nnew_tab = \"prefix+q\"\npreset = \"zellij\"\n\n[theme]\nname = \"nord\"\n";
        let plan = reset_keys(text).unwrap();
        assert_eq!((plan.first_line, plan.last_line), (4, 6));
        assert_eq!(plan.sections, vec!["ui", "theme"]);
        assert!(plan.text.contains("sidebar = false"));
        assert!(plan.text.contains("name = \"nord\""));
        assert!(!plan.text.contains("new_tab = \"prefix+q\""));
        assert!(plan.text.contains("new_tab = \"prefix+c\""));
        assert!(plan.text.contains("preset = \"default\""));
        assert!(plan.text.contains("\n\n[theme]"));
        let document = parse_toml(&plan.text).unwrap();
        assert!(document.values.contains_key("ui.sidebar"));
        assert!(document.values.contains_key("theme.name"));
        assert!(reset_keys("[ui]\nsidebar = true\n").is_none());
    }
}
