//! 应用状态与事件循环：Workspace → Tab → BSP Pane。
//!
//! 线程分工（plan v1.1 §2.1）：
//! - 这个文件里的循环跑在 UI 线程：crossterm 读取、布局、渲染；
//! - [`NetHandle`] 跑在后台线程：协议、credit、attach 生命周期、重连；
//! - 每个 Pane 一个本地 VT，只在 UI 线程推进，跨线程只传字节与状态。

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::Rect;
use serde_json::{json, Value};

use crate::clipboard;
use crate::config::{AgentSort, CommandKind, Config, Delivery, PaneBorders, Span};
use crate::copy::CopyMode;
use crate::hc::net::{Command, NetConfig, NetEvent, NetHandle};
use crate::hc::proto::{Event as TerminalEvent, TerminalSummary};
use crate::layout::split::{neighbor, Axis, Direction, PaneId, SplitNode};
use crate::model::{
    renumber_node, Badge, Launch, Pane, PaneState, SavedNode, SavedPane, SavedState, SavedTab,
    SavedWorkspace, Slot, Tab, Workspace,
};
use crate::term::view::TerminalView;
use crate::tui;
use crate::ui;

/// 一个 Pane 能正常使用的最小内容区。
const MIN_PANE_WIDTH: u16 = 20;
const MIN_PANE_HEIGHT: u16 = 5;
/// 每个 Pane 上报给 FACTS 的 cell 像素尺寸。
const CELL_PX: (u32, u32) = (8, 16);
/// 布局写入 core doc 的防抖窗口。
const LAYOUT_DEBOUNCE: Duration = Duration::from_millis(400);
/// 等待 `doc.get` 的期限，超时就用本地默认布局。
const RESTORE_TIMEOUT: Duration = Duration::from_millis(1500);
/// 共享的布局文档名。
const DOC_NAME: &str = "hctui-layout-v1";
const TOAST_LIFETIME: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Ready { version: String, core: String },
    Reconnecting { reason: String },
    Failed { reason: String },
}

impl Connection {
    pub fn label(&self) -> String {
        match self {
            Self::Connecting => "connecting".to_string(),
            Self::Ready { version, core } => format!("hc {version} · {core}"),
            Self::Reconnecting { .. } => "reconnecting".to_string(),
            Self::Failed { .. } => "disconnected".to_string(),
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Reconnecting { reason } | Self::Failed { reason } => Some(reason),
            _ => None,
        }
    }
}

/// 需要用户输入一行的动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    NewTab,
    NewWorkspace,
    RenameTab { workspace: usize, tab: u32 },
    RenameWorkspace(usize),
    RenamePane(u32),
    SendText(u32),
    SendKeys(u32),
    WaitOutput(u32),
    PutFile(u32),
    AddMachine,
    RenameMachine(String),
    CloseTab(usize),
    CloseWorkspace(usize),
    KillPane(u32),
}

impl PromptKind {
    /// 只有这些动作需要输入 `y` 再回车。
    fn needs_confirm(&self) -> bool {
        matches!(
            self,
            Self::CloseTab(_) | Self::CloseWorkspace(_) | Self::KillPane(_)
        )
    }
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub label: String,
    pub buffer: String,
    /// 光标位置（字节偏移，永远落在字符边界上）。
    pub cursor: usize,
    /// `Up`/`Down` 翻到第几条历史；`None` 表示正在输入新内容。
    pub history_pos: Option<usize>,
    pub kind: PromptKind,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub until: Instant,
}

/// 会话级浮层终端。
#[derive(Debug, Clone, Copy)]
pub struct PopupPane {
    pub pane: u32,
    pub width: Span,
    pub height: Span,
}

/// 只读文本浮层（`terminal.read` 的结果）。
#[derive(Debug, Clone)]
pub struct Reader {
    pub title: String,
    pub lines: Vec<String>,
    pub scroll: usize,
    pub complete: bool,
    pub how: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChooserKind {
    Templates,
    Machines,
}

#[derive(Debug, Clone)]
pub struct ChooserItem {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct Chooser {
    pub title: String,
    pub items: Vec<ChooserItem>,
    pub selected: usize,
    pub kind: ChooserKind,
}

#[derive(Debug, Clone)]
pub struct MachineInfo {
    pub name: String,
    pub target: String,
    pub state: String,
    pub host: String,
}

/// 断线期间失败、连接恢复后要重放的 RPC。
#[derive(Debug, Clone)]
struct PendingCall {
    tag: String,
    method: String,
    params: Value,
}

/// 本地状态文件：`seen_turn` 按终端地址保存，重启后不把旧完成当新完成。
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct LocalState {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    seen: BTreeMap<String, u64>,
}

/// 普通模式下鼠标拖选。
#[derive(Debug, Clone, Copy)]
pub struct Drag {
    pub pane: u32,
    pub start: (u16, u16),
    pub current: (u16, u16),
}

pub struct App {
    pub config: Config,
    pub connection: Connection,
    pub workspaces: Vec<Workspace>,
    pub active_ws: usize,
    pub panes: HashMap<u32, Pane>,
    pub terminals: Vec<TerminalSummary>,
    pub sidebar: bool,
    pub sidebar_width: u16,
    pub note: String,
    pub slots: Vec<Slot>,
    /// 上一次绘制算出的 Pane 区域（不含 Tab 栏、侧栏、状态栏）。
    pub content: Rect,
    pub tab_bar: Rect,
    /// Tab 栏上每一段的点击区域，值是当前工作区里的 Tab 下标。
    pub tab_hits: Vec<(usize, Rect)>,
    /// 侧栏里 Agent 行的点击区域。
    pub agent_hits: Vec<(u32, Rect)>,
    /// 侧栏里工作区行的点击区域。
    pub workspace_hits: Vec<(usize, Rect)>,
    pub zoom: bool,
    pub resize_mode: bool,
    pub move_mode: bool,
    pub workspace_mode: bool,
    pub prompt: Option<Prompt>,
    pub copy: Option<CopyMode>,
    pub drag: Option<Drag>,
    pub toast: Option<Toast>,
    pub popup: Option<PopupPane>,
    pub reader: Option<Reader>,
    pub chooser: Option<Chooser>,
    pub machines: Vec<MachineInfo>,
    /// `tab_bar_right` 里 `cmd:` 条目的最近一次输出。
    pub command_output: HashMap<String, String>,
    /// 侧栏滚动偏移（行）。
    pub sidebar_scroll: usize,
    /// 侧栏区域，鼠标滚轮用。
    pub sidebar_area: Rect,
    pub events_seen: u64,
    /// core doc 的版本；`0` 表示从未写过。
    pub doc_version: u64,
    pub doc_loaded: bool,
    pub last_pane: Option<u32>,
    prefix: bool,
    dirty: bool,
    quit: bool,
    next_pane: u32,
    next_tab: u32,
    next_workspace: u32,
    restore_pending: bool,
    restore_since: Instant,
    layout_dirty: bool,
    layout_since: Option<Instant>,
    /// 待重放的 RPC（断线时发不出去的那些）。
    pending_calls: Vec<PendingCall>,
    inflight: HashMap<String, (String, Value)>,
    /// 已读轮次：终端地址 → 看过的 turn。
    seen: BTreeMap<String, u64>,
    seen_dirty: bool,
    seen_since: Option<Instant>,
    prompt_history: Vec<String>,
    command_rx: Receiver<(String, String)>,
    command_tx: std::sync::mpsc::Sender<(String, String)>,
    command_since: HashMap<String, Instant>,
    net: Option<NetHandle>,
    events: Receiver<NetEvent>,
}

impl App {
    pub fn new(config: Config) -> Result<Self> {
        let (sender, events) = std::sync::mpsc::channel();
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let net = NetHandle::spawn(
            NetConfig {
                session: config.session.clone(),
            },
            sender,
        );
        let note = if config.warnings.is_empty() {
            "starting hc connect".to_string()
        } else {
            config.warnings.join(" · ")
        };
        let restore_pending = config.restore && config.attach.is_none();
        let mut app = Self {
            sidebar: config.sidebar,
            sidebar_width: config
                .sidebar_width
                .clamp(config.sidebar_min_width, config.sidebar_max_width),
            connection: Connection::Connecting,
            workspaces: Vec::new(),
            active_ws: 0,
            panes: HashMap::new(),
            terminals: Vec::new(),
            note,
            slots: Vec::new(),
            content: Rect::new(0, 0, 80, 24),
            tab_bar: Rect::new(0, 0, 0, 0),
            tab_hits: Vec::new(),
            agent_hits: Vec::new(),
            workspace_hits: Vec::new(),
            zoom: false,
            resize_mode: false,
            move_mode: false,
            workspace_mode: false,
            prompt: None,
            copy: None,
            drag: None,
            toast: None,
            popup: None,
            reader: None,
            chooser: None,
            machines: Vec::new(),
            command_output: HashMap::new(),
            sidebar_scroll: 0,
            sidebar_area: Rect::new(0, 0, 0, 0),
            events_seen: 0,
            doc_version: 0,
            doc_loaded: false,
            last_pane: None,
            prefix: false,
            dirty: true,
            quit: false,
            next_pane: 0,
            next_tab: 0,
            next_workspace: 0,
            restore_pending,
            restore_since: Instant::now(),
            layout_dirty: false,
            layout_since: None,
            pending_calls: Vec::new(),
            inflight: HashMap::new(),
            seen: BTreeMap::new(),
            seen_dirty: false,
            seen_since: None,
            prompt_history: Vec::new(),
            command_rx,
            command_tx,
            command_since: HashMap::new(),
            net: Some(net),
            events,
            config,
        };
        app.load_local_state();
        if !app.restore_pending {
            app.ensure_layout();
        }
        Ok(app)
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn prefix_active(&self) -> bool {
        self.prefix
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn focused(&self) -> u32 {
        self.workspaces
            .get(self.active_ws)
            .and_then(Workspace::focus)
            .unwrap_or_default()
    }

    pub fn focused_pane(&self) -> Option<&Pane> {
        self.panes.get(&self.focused())
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active_ws)
    }

    pub fn tab(&self) -> Option<&Tab> {
        self.workspace().and_then(Workspace::tab)
    }

    /// 供侧栏与 UI 使用的定位查询。
    pub fn locate_public(&self, pane: u32) -> Option<(usize, usize)> {
        self.locate(pane)
    }

    /// 当前 Pane 是否在可见的工作区与 Tab 里。
    pub fn is_visible(&self, pane: u32) -> bool {
        let Some(workspace) = self.workspaces.get(self.active_ws) else {
            return false;
        };
        let Some(index) = workspace.tab_of(pane) else {
            return false;
        };
        index == workspace.active && (!self.zoom || self.focused() == pane)
    }

    fn ensure_layout(&mut self) {
        if self.workspaces.is_empty() {
            let pane = self.alloc_pane();
            let tab = self.alloc_tab();
            let workspace = self.alloc_workspace();
            self.workspaces
                .push(Workspace::new(workspace, String::new(), pane, tab));
            self.active_ws = 0;
            self.spawn_pane(pane);
            let target = self.config.attach.clone();
            if let Some(target) = target {
                if let Some(pane) = self.panes.get_mut(&pane) {
                    pane.terminal = Some(target);
                }
            }
        }
    }

    pub fn pump(&mut self) {
        let mut credits: Vec<(u32, usize)> = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                NetEvent::Output { channel, bytes } => {
                    if let Some(pane_id) = self.pane_by_channel(channel) {
                        let responses = match self.panes.get_mut(&pane_id) {
                            Some(pane) => {
                                pane.view.write(&bytes);
                                pane.view.take_responses()
                            }
                            None => Vec::new(),
                        };
                        if !responses.is_empty() {
                            self.send(Command::Input {
                                pane: pane_id,
                                bytes: responses,
                            });
                        }
                        self.dirty = true;
                    }
                    credits.push((channel, bytes.len()));
                }
                other => self.on_net(other),
            }
        }
        for (channel, bytes) in credits {
            self.send(Command::Credit { channel, bytes });
        }
    }

    /// 防抖持久化、toast 过期、恢复超时。
    pub fn tick(&mut self) {
        let now = Instant::now();
        if self.restore_pending && now.duration_since(self.restore_since) >= RESTORE_TIMEOUT {
            self.restore_pending = false;
            self.ensure_layout();
            self.note = "core 没有返回布局，使用默认布局".to_string();
            self.dirty = true;
        }
        if self.toast.as_ref().is_some_and(|toast| now >= toast.until) {
            self.toast = None;
            self.dirty = true;
        }
        self.refresh_tab_commands(now);
        if self.seen_dirty
            && self
                .seen_since
                .is_some_and(|since| now.duration_since(since) >= Duration::from_millis(500))
        {
            self.save_local_state();
        }
        if self.layout_dirty
            && self.doc_loaded
            && self
                .layout_since
                .is_some_and(|since| now.duration_since(since) >= LAYOUT_DEBOUNCE)
        {
            self.push_layout();
        }
    }

    /// 轮询 `tab_bar_right` 里 `cmd:` 条目的输出，最多每 5 秒跑一次。
    fn refresh_tab_commands(&mut self, now: Instant) {
        while let Ok((key, output)) = self.command_rx.try_recv() {
            self.command_output.insert(key, output);
            self.dirty = true;
        }
        // key 用完整 token（含 `cmd:` 前缀），渲染层按同一个键查。
        let commands: Vec<(String, String)> = self
            .config
            .tab_bar_right
            .iter()
            .filter_map(|token| {
                token
                    .strip_prefix("cmd:")
                    .map(|command| (token.clone(), command.to_string()))
            })
            .collect();
        for (key, command) in commands {
            let due = self
                .command_since
                .get(&key)
                .is_none_or(|since| now.duration_since(*since) >= Duration::from_secs(5));
            if !due {
                continue;
            }
            self.command_since.insert(key.clone(), now);
            let sender = self.command_tx.clone();
            std::thread::spawn(move || {
                let text = match std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&command)
                    .output()
                {
                    Ok(output) => String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                    Err(error) => format!("err: {error}"),
                };
                let _ = sender.send((key, text));
            });
        }
    }

    /// lifecycle 通知的提示音；没有播放器时退回宿主终端响铃。
    fn play_sound(&self, badge: Badge) {
        if !self.config.sound {
            return;
        }
        let path = if badge == Badge::Done {
            self.config
                .sound_done_path
                .clone()
                .or_else(|| self.config.sound_path.clone())
        } else {
            self.config
                .sound_request_path
                .clone()
                .or_else(|| self.config.sound_path.clone())
        };
        if let Some(path) = path {
            let players: &[(&str, &[&str])] = &[
                ("paplay", &[]),
                ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
                ("afplay", &[]),
                ("mpv", &["--no-video", "--really-quiet"]),
            ];
            for (program, args) in players {
                let spawned = std::process::Command::new(program)
                    .args(*args)
                    .arg(&path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                if spawned.is_ok() {
                    return;
                }
            }
        }
        let _ = std::io::stdout().write_all(b"\x07");
    }

    /// 下一次必须醒来处理内部状态的时间点。
    pub fn wake_at(&self) -> Option<Instant> {
        let mut deadlines = Vec::new();
        if self.restore_pending {
            deadlines.push(self.restore_since + RESTORE_TIMEOUT);
        }
        if self.layout_dirty {
            if let Some(since) = self.layout_since {
                deadlines.push(since + LAYOUT_DEBOUNCE);
            }
        }
        if let Some(toast) = &self.toast {
            deadlines.push(toast.until);
        }
        if self.seen_dirty {
            if let Some(since) = self.seen_since {
                deadlines.push(since + Duration::from_millis(500));
            }
        }
        deadlines.into_iter().min()
    }

    /// 按当前 Tab 的树算出每个 Pane 的矩形，并让本地 VT 跟上尺寸。
    pub fn reflow(&mut self, content: Rect) {
        self.content = content;
        self.slots.clear();
        if content.width < 3 || content.height < 3 {
            return;
        }
        let focus = self.focused();
        let leaves = if self.zoom {
            vec![(PaneId(focus), content)]
        } else {
            self.workspace()
                .and_then(Workspace::tab)
                .map(|tab| tab.tree.layout(content))
                .unwrap_or_default()
        };
        let mut slots = Vec::with_capacity(leaves.len() + 1);
        for (pane, outer) in &leaves {
            let borders = borders_for(
                *outer,
                &leaves,
                self.config.pane_borders,
                self.config.pane_gaps,
            );
            slots.push(Slot {
                pane: pane.0,
                outer: *outer,
                inner: inner_rect(*outer, borders),
                borders,
                overlay: false,
            });
        }
        // popup 画在所有分屏之上，尺寸由配置的格数或百分比决定。
        if let Some(popup) = self.popup {
            let width = popup.width.resolve(content.width).max(8).min(content.width);
            let height = popup
                .height
                .resolve(content.height)
                .max(3)
                .min(content.height);
            let outer = Rect::new(
                content.x + (content.width - width) / 2,
                content.y + (content.height - height) / 2,
                width,
                height,
            );
            slots.push(Slot {
                pane: popup.pane,
                outer,
                inner: inner_rect(outer, ratatui::widgets::Borders::ALL),
                borders: ratatui::widgets::Borders::ALL,
                overlay: true,
            });
        }
        for slot in &slots {
            if let Some(pane) = self.panes.get_mut(&slot.pane) {
                if slot.inner.width >= 1 && slot.inner.height >= 1 {
                    let _ =
                        pane.view
                            .resize(slot.inner.width, slot.inner.height, CELL_PX.0, CELL_PX.1);
                }
            }
        }
        self.slots = slots;
    }

    /// 尺寸与 attach 状态对齐。必须在绘制之后调用（`slots` 才是新的）。
    pub fn reconcile(&mut self) {
        if self.restore_pending {
            return;
        }
        let mut spawns = Vec::new();
        let mut resizes = Vec::new();
        for slot in self.slots.clone() {
            if slot.inner.width < 2 || slot.inner.height < 2 {
                continue;
            }
            let Some(pane) = self.panes.get_mut(&slot.pane) else {
                continue;
            };
            let size = (slot.inner.width, slot.inner.height);
            if pane.needs_spawn() {
                pane.mark_spawned(size);
                let (argv, cwd, on) = pane
                    .launch
                    .as_ref()
                    .map(|launch| (launch.argv.clone(), launch.cwd.clone(), launch.on.clone()))
                    .unwrap_or_default();
                spawns.push((slot.pane, pane.terminal.clone(), argv, cwd, on, size));
            } else if pane.mark_resized(size) {
                resizes.push((slot.pane, size));
            }
        }
        for (pane, target, argv, cwd, on, (cols, rows)) in spawns {
            self.send(Command::Spawn {
                pane,
                target,
                argv,
                cwd,
                on,
                cols,
                rows,
            });
        }
        for (pane, (cols, rows)) in resizes {
            self.send(Command::Resize { pane, cols, rows });
        }
    }

    // ---------------------------------------------------------------- 输入

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.prompt.is_some() {
            self.on_prompt_key(key);
            return;
        }
        if self.reader.is_some() {
            self.on_reader_key(key);
            return;
        }
        if self.chooser.is_some() {
            self.on_chooser_key(key);
            return;
        }
        if self.copy.is_some() {
            self.on_copy_key(key);
            return;
        }
        if self.resize_mode {
            if !self.on_resize_key(key) {
                self.resize_mode = false;
                self.dirty = true;
            }
            return;
        }
        if self.move_mode || self.workspace_mode {
            self.on_mode_key(key);
            return;
        }
        if self.popup.is_some() && key.kind == KeyEventKind::Press && key.code == KeyCode::Esc {
            self.close_popup();
            return;
        }
        if self.prefix {
            self.prefix = false;
            self.dirty = true;
            if key.kind == KeyEventKind::Press {
                self.on_prefix_key(key);
            }
            return;
        }
        let keys = &self.config.keys;
        if key.kind == KeyEventKind::Press && keys.prefix.matches_direct(&key) {
            self.prefix = true;
            self.dirty = true;
            return;
        }
        if key.kind == KeyEventKind::Press {
            if let Some(index) = self.custom_command(&key, false) {
                self.run_custom(index);
                return;
            }
        }
        self.forward_key(key);
    }

    /// 把一次按键按本地 VT 的当前模式编码后送给输入目标。
    fn forward_key(&mut self, key: KeyEvent) {
        let pane_id = self.input_pane();
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };
        if !pane.is_live() {
            return;
        }
        if let Some(bytes) = pane.view.encode_key(key) {
            self.send(Command::Input {
                pane: pane_id,
                bytes,
            });
        }
    }

    pub fn on_paste(&mut self, text: String) {
        if let Some(prompt) = &mut self.prompt {
            prompt.buffer.insert_str(prompt.cursor, &text);
            prompt.cursor += text.len();
            self.dirty = true;
            return;
        }
        if self.copy.is_some() {
            return;
        }
        let pane_id = self.input_pane();
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        if !pane.is_live() {
            return;
        }
        let bytes = pane.view.encode_paste(&text);
        self.send(Command::Input {
            pane: pane_id,
            bytes,
        });
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        // popup 打开时独占鼠标：点在外面不改变下面分屏的焦点。
        if let Some(popup) = self.popup {
            let slot = self
                .slots
                .iter()
                .find(|slot| slot.pane == popup.pane)
                .copied();
            let Some(slot) = slot else {
                return;
            };
            if !contains(slot.outer, mouse.column, mouse.row) {
                return;
            }
            let Some(pane) = self.panes.get_mut(&slot.pane) else {
                return;
            };
            if pane.view.mouse_tracking() {
                if let Some(bytes) = pane.view.encode_mouse(mouse, (slot.inner.x, slot.inner.y)) {
                    self.send(Command::Input {
                        pane: slot.pane,
                        bytes,
                    });
                }
            } else if mouse.kind == MouseEventKind::ScrollUp {
                pane.view.scroll(self.config.mouse_scroll_lines as isize);
                self.dirty = true;
            } else if mouse.kind == MouseEventKind::ScrollDown {
                pane.view.scroll(-(self.config.mouse_scroll_lines as isize));
                self.dirty = true;
            }
            return;
        }

        if contains(self.sidebar_area, mouse.column, mouse.row) {
            let step = self.config.mouse_scroll_lines;
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.sidebar_scroll = self.sidebar_scroll.saturating_sub(step);
                    self.dirty = true;
                }
                MouseEventKind::ScrollDown => {
                    self.sidebar_scroll = self.sidebar_scroll.saturating_add(step);
                    self.dirty = true;
                }
                _ => {}
            }
            if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
                return;
            }
        }

        let pressed = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if pressed {
            if let Some((index, _)) = self
                .tab_hits
                .iter()
                .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
            {
                self.select_tab(*index);
                return;
            }
            if let Some((index, _)) = self
                .workspace_hits
                .iter()
                .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
            {
                self.select_workspace(*index);
                return;
            }
            if let Some((pane, _)) = self
                .agent_hits
                .iter()
                .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
            {
                self.focus_pane(*pane);
                return;
            }
        }

        let Some(slot) = self
            .slots
            .iter()
            .find(|slot| contains(slot.outer, mouse.column, mouse.row))
            .copied()
        else {
            return;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.set_focus(slot.pane);
                if !self
                    .panes
                    .get(&slot.pane)
                    .is_some_and(|pane| pane.view.mouse_tracking())
                {
                    self.drag = Some(Drag {
                        pane: slot.pane,
                        start: to_inner(slot.inner, mouse),
                        current: to_inner(slot.inner, mouse),
                    });
                    self.dirty = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(drag) = &mut self.drag {
                    drag.current = to_inner(slot.inner, mouse);
                    self.dirty = true;
                    return;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(drag) = self.drag.take() {
                    if self.config.copy_on_select && drag.start != drag.current {
                        let text = self
                            .panes
                            .get(&drag.pane)
                            .map(|pane| {
                                let (start, end) = order(drag.start, drag.current);
                                pane.view.selection_text(start, end, false)
                            })
                            .unwrap_or_default();
                        self.note = clipboard::copy(&text);
                    }
                    self.dirty = true;
                    return;
                }
            }
            _ => {}
        }

        let Some(pane) = self.panes.get_mut(&slot.pane) else {
            return;
        };
        if pane.view.mouse_tracking() {
            if let Some(bytes) = pane.view.encode_mouse(mouse, (slot.inner.x, slot.inner.y)) {
                self.send(Command::Input {
                    pane: slot.pane,
                    bytes,
                });
            }
            return;
        }
        let lines = self.config.mouse_scroll_lines as isize;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                pane.view.scroll(lines);
                self.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                pane.view.scroll(-lines);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn on_prefix_key(&mut self, key: KeyEvent) {
        let keys = self.config.keys.clone();

        // 前缀键再按一次：把这次按键原样交给程序（发送字面 ctrl+b）。
        if keys.prefix.matches_direct(&key) {
            self.forward_key(key);
            return;
        }
        if let Some(index) = self.custom_command(&key, true) {
            self.run_custom(index);
            return;
        }
        if keys.help.matches_prefix(&key) {
            self.note = "c 新 tab · v/- 分屏 · hjkl 焦点 · z 缩放 · x 关闭视图 · , 命名 tab · K 命名 pane · D kill · m 移动 pane · w 工作区 · [ 复制 · a 下一个待处理 · & 关 tab · q 退出".to_string();
            self.dirty = true;
            return;
        }
        if keys.detach.matches_prefix(&key) {
            self.quit = true;
            return;
        }
        if keys.reload_config.matches_prefix(&key) {
            self.reload_config();
            return;
        }
        if keys.new_tab.matches_prefix(&key) {
            if self.config.prompt_new_tab {
                self.ask(PromptKind::NewTab, "新 tab 名字");
            } else {
                self.new_tab(None);
            }
            return;
        }
        if keys.rename_tab.matches_prefix(&key) {
            if let Some(tab) = self.tab() {
                let (workspace, id) = (self.active_ws, tab.id);
                self.ask(
                    PromptKind::RenameTab { workspace, tab: id },
                    "tab 名字（留空恢复默认）",
                );
            }
            return;
        }
        if keys.next_tab.matches_prefix(&key) {
            self.cycle_tab(1);
            return;
        }
        if keys.previous_tab.matches_prefix(&key) {
            self.cycle_tab(-1);
            return;
        }
        if keys.move_tab_previous.matches_prefix(&key) {
            self.move_tab(-1);
            return;
        }
        if keys.move_tab_next.matches_prefix(&key) {
            self.move_tab(1);
            return;
        }
        if keys.close_tab.matches_prefix(&key) {
            if self.config.confirm_close {
                self.ask(
                    PromptKind::CloseTab(self.active_ws),
                    "关闭整个 tab 连同其中的 pane 视图？(y/N)",
                );
            } else {
                let workspace = self.active_ws;
                self.close_tab_in(workspace);
            }
            return;
        }
        if keys.new_workspace.matches_prefix(&key) {
            self.ask(PromptKind::NewWorkspace, "新工作区名字");
            return;
        }
        if keys.close_workspace.matches_prefix(&key) {
            self.ask(
                PromptKind::CloseWorkspace(self.active_ws),
                "关闭当前工作区及其全部 tab？(y/N)",
            );
            return;
        }
        if keys.workspace_picker.matches_prefix(&key) {
            self.workspace_mode = true;
            self.note = format!(
                "workspace: {} 个，按 1-9 切换，Esc 取消",
                self.workspaces.len()
            );
            self.dirty = true;
            return;
        }
        if keys.next_workspace.matches_prefix(&key) {
            self.select_workspace((self.active_ws + 1) % self.workspaces.len().max(1));
            return;
        }
        if keys.previous_workspace.matches_prefix(&key) {
            let index = self
                .active_ws
                .checked_sub(1)
                .unwrap_or(self.workspaces.len().saturating_sub(1));
            self.select_workspace(index);
            return;
        }
        if keys.split_vertical.matches_prefix(&key) {
            self.split_focused(Axis::Horizontal);
            return;
        }
        if keys.split_horizontal.matches_prefix(&key) {
            self.split_focused(Axis::Vertical);
            return;
        }
        if keys.focus_pane_left.matches_prefix(&key) {
            self.move_focus(Direction::Left);
            return;
        }
        if keys.focus_pane_down.matches_prefix(&key) {
            self.move_focus(Direction::Down);
            return;
        }
        if keys.focus_pane_up.matches_prefix(&key) {
            self.move_focus(Direction::Up);
            return;
        }
        if keys.focus_pane_right.matches_prefix(&key) {
            self.move_focus(Direction::Right);
            return;
        }
        if keys.cycle_pane_next.matches_prefix(&key) {
            self.cycle_pane(1);
            return;
        }
        if keys.cycle_pane_previous.matches_prefix(&key) {
            self.cycle_pane(-1);
            return;
        }
        if keys.last_pane.matches_prefix(&key) {
            self.toggle_last_pane();
            return;
        }
        if keys.zoom.matches_prefix(&key) {
            self.zoom = !self.zoom;
            self.note = if self.zoom {
                "zoomed".to_string()
            } else {
                "unzoomed".to_string()
            };
            self.dirty = true;
            return;
        }
        if keys.close_pane.matches_prefix(&key) {
            self.close_focused_pane();
            return;
        }
        if keys.rename_pane.matches_prefix(&key) {
            self.ask(
                PromptKind::RenamePane(self.focused()),
                "pane 名字（留空清除）",
            );
            return;
        }
        if keys.kill_pane.matches_prefix(&key) {
            let terminal = self
                .focused_pane()
                .and_then(|pane| pane.terminal.clone())
                .unwrap_or_default();
            self.ask(
                PromptKind::KillPane(self.focused()),
                &format!("结束 {terminal} 里的程序？(y/N)"),
            );
            return;
        }
        if keys.move_pane.matches_prefix(&key) {
            self.move_mode = true;
            self.note = format!(
                "move: 1-{} 移到目标 tab，t 移成新 tab，Esc 取消",
                self.tabs_in_workspace()
            );
            self.dirty = true;
            return;
        }
        if keys.resize_mode.matches_prefix(&key) {
            self.resize_mode = true;
            self.note = "resize: h/j/k/l 调整，其他键退出".to_string();
            self.dirty = true;
            return;
        }
        if keys.toggle_sidebar.matches_prefix(&key) {
            self.sidebar = !self.sidebar;
            self.dirty = true;
            return;
        }
        if keys.scroll_bottom.matches_prefix(&key) {
            if let Some(pane) = self.panes.get_mut(&self.focused()) {
                pane.view.scroll_bottom();
            }
            self.note = "scrolled to bottom".to_string();
            self.dirty = true;
            return;
        }
        if keys.copy_mode.matches_prefix(&key) {
            self.enter_copy_mode();
            return;
        }
        if keys.next_agent.matches_prefix(&key) {
            self.focus_next_attention();
            return;
        }
        if keys.send_text.matches_prefix(&key) {
            let pane = self.input_pane();
            let terminal = self
                .panes
                .get(&pane)
                .and_then(|state| state.terminal.clone())
                .unwrap_or_else(|| format!("pane {pane}"));
            self.ask(PromptKind::SendText(pane), &format!("发给 {terminal}"));
            return;
        }
        if keys.interrupt.matches_prefix(&key) {
            let terminal = self
                .panes
                .get(&self.input_pane())
                .and_then(|state| state.terminal.clone());
            if let Some(terminal) = terminal {
                self.call_rpc(
                    "interrupt",
                    "terminal.interrupt",
                    json!({"terminal": terminal}),
                );
                self.note = format!("正在中断 {terminal}");
            }
            return;
        }
        if keys.toggle_keep.matches_prefix(&key) {
            self.toggle_keep();
            return;
        }
        if keys.remove_terminal.matches_prefix(&key) {
            self.remove_finished_terminal();
            return;
        }
        if keys.open_link.matches_prefix(&key) {
            self.open_link_at_cursor();
            return;
        }
        if keys.edit_history.matches_prefix(&key) {
            self.edit_history_popup();
            return;
        }
        if keys.read_pane.matches_prefix(&key) {
            self.open_reader();
            return;
        }
        if keys.send_keys.matches_prefix(&key) {
            let pane = self.input_pane();
            self.ask(PromptKind::SendKeys(pane), "按下按键：enter, ctrl+c, esc");
            return;
        }
        if keys.wait_output.matches_prefix(&key) {
            let pane = self.input_pane();
            self.ask(PromptKind::WaitOutput(pane), "等待输出包含");
            return;
        }
        if keys.put_file.matches_prefix(&key) {
            let pane = self.input_pane();
            self.ask(PromptKind::PutFile(pane), "发送文件路径");
            return;
        }
        if keys.templates.matches_prefix(&key) {
            self.call_rpc("templates", "templates.list", json!({}));
            return;
        }
        if keys.machines.matches_prefix(&key) {
            self.call_rpc("machines", "machine.list", json!({}));
            return;
        }
        if keys.paste_image.matches_prefix(&key) {
            self.paste_image();
            return;
        }
        if let KeyCode::Char(digit @ '1'..='9') = key.code {
            let index = digit as usize - '1' as usize;
            if index < self.tabs_in_workspace() {
                self.select_tab(index);
            }
            return;
        }
        self.note = "prefix: ? 帮助 · c 新 tab · v/- 分屏 · hjkl 焦点 · z 缩放".to_string();
        self.dirty = true;
    }

    fn on_mode_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
            self.move_mode = false;
            self.workspace_mode = false;
            self.note = String::new();
            self.dirty = true;
            return;
        }
        if self.workspace_mode {
            match key.code {
                KeyCode::Char(digit @ '1'..='9') => {
                    let index = digit as usize - '1' as usize;
                    self.workspace_mode = false;
                    self.select_workspace(index);
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    let next = (self.active_ws + 1) % self.workspaces.len().max(1);
                    self.select_workspace(next);
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    let index = self
                        .active_ws
                        .checked_sub(1)
                        .unwrap_or(self.workspaces.len().saturating_sub(1));
                    self.select_workspace(index);
                }
                _ => {
                    self.note = "workspace: 1-9 切换 · h/l 前后 · Esc 取消".to_string();
                    self.dirty = true;
                }
            }
            return;
        }
        match key.code {
            KeyCode::Char('t') => {
                self.move_mode = false;
                self.move_pane_to_new_tab();
            }
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                self.move_mode = false;
                self.move_pane_to_tab(index);
            }
            _ => {
                self.note = "move: 1-9 目标 tab · t 新 tab · Esc 取消".to_string();
                self.dirty = true;
            }
        }
    }

    fn on_prompt_key(&mut self, key: KeyEvent) {
        let history = self.prompt_history.clone();
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.note = "已取消".to_string();
            }
            KeyCode::Enter => {
                let prompt = self.prompt.take().expect("prompt exists");
                if !prompt.kind.needs_confirm()
                    && !prompt.buffer.trim().is_empty()
                    && self.prompt_history.last() != Some(&prompt.buffer)
                {
                    self.prompt_history.push(prompt.buffer.clone());
                    if self.prompt_history.len() > 50 {
                        self.prompt_history.remove(0);
                    }
                }
                self.commit_prompt(prompt);
            }
            KeyCode::Backspace => {
                if prompt.cursor > 0 {
                    let from = prev_char_boundary(&prompt.buffer, prompt.cursor);
                    prompt.buffer.replace_range(from..prompt.cursor, "");
                    prompt.cursor = from;
                }
            }
            KeyCode::Delete => {
                if prompt.cursor < prompt.buffer.len() {
                    let to = next_char_boundary(&prompt.buffer, prompt.cursor);
                    prompt.buffer.replace_range(prompt.cursor..to, "");
                }
            }
            KeyCode::Left => {
                prompt.cursor = prev_char_boundary(&prompt.buffer, prompt.cursor);
            }
            KeyCode::Right => {
                prompt.cursor = next_char_boundary(&prompt.buffer, prompt.cursor);
            }
            KeyCode::Home => prompt.cursor = 0,
            KeyCode::End => prompt.cursor = prompt.buffer.len(),
            KeyCode::Up => {
                if let Some(position) = prompt.history_pos {
                    if position > 0 {
                        prompt.history_pos = Some(position - 1);
                        prompt.buffer = history[position - 1].clone();
                        prompt.cursor = prompt.buffer.len();
                    }
                } else if let Some(last) = history.last() {
                    prompt.history_pos = Some(history.len() - 1);
                    prompt.buffer = last.clone();
                    prompt.cursor = prompt.buffer.len();
                }
            }
            KeyCode::Down => {
                if let Some(position) = prompt.history_pos {
                    let next = position + 1;
                    if next < history.len() {
                        prompt.history_pos = Some(next);
                        prompt.buffer = history[next].clone();
                    } else {
                        prompt.history_pos = None;
                        prompt.buffer.clear();
                    }
                    prompt.cursor = prompt.buffer.len();
                }
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.buffer.insert(prompt.cursor, character);
                prompt.cursor += character.len_utf8();
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn on_resize_key(&mut self, key: KeyEvent) -> bool {
        let direction = match key.code {
            KeyCode::Char('h') | KeyCode::Left => Some((Axis::Horizontal, -0.05)),
            KeyCode::Char('l') | KeyCode::Right => Some((Axis::Horizontal, 0.05)),
            KeyCode::Char('k') | KeyCode::Up => Some((Axis::Vertical, -0.05)),
            KeyCode::Char('j') | KeyCode::Down => Some((Axis::Vertical, 0.05)),
            _ => None,
        };
        let Some((axis, delta)) = direction else {
            return false;
        };
        let pane = self.focused();
        if let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) {
            if tab.tree.adjust_ratio(PaneId(pane), axis, delta) {
                self.mark_layout();
                self.dirty = true;
            }
        }
        true
    }

    // ------------------------------------------------------------ Copy Mode

    fn enter_copy_mode(&mut self) {
        let pane = self.focused();
        let Some(state) = self.panes.get(&pane) else {
            return;
        };
        let size = state.view.size();
        let lines = state.view.viewport_lines();
        let cursor = state.view.cursor();
        self.copy = Some(CopyMode::new(pane, size, lines, cursor));
        self.note = "copy: hjkl 移动 · w/b 词 · v 选区 · y 复制 · / 搜索 · q 退出".to_string();
        self.dirty = true;
    }

    fn on_copy_key(&mut self, key: KeyEvent) {
        let Some(mut copy) = self.copy.take() else {
            return;
        };
        if copy.typing {
            match key.code {
                KeyCode::Enter => {
                    copy.typing = false;
                    let found = copy.find_next();
                    self.note = if found {
                        format!("搜索：{}", copy.search.clone().unwrap_or_default())
                    } else {
                        "没有找到".to_string()
                    };
                }
                KeyCode::Esc => {
                    copy.typing = false;
                    copy.search = None;
                    self.note = "copy".to_string();
                }
                KeyCode::Backspace => {
                    if let Some(search) = &mut copy.search {
                        search.pop();
                    }
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    copy.search.get_or_insert_with(String::new).push(character);
                }
                _ => {}
            }
            self.copy = Some(copy);
            self.dirty = true;
            return;
        }

        let mut scroll: isize = 0;
        let mut done = false;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.note = "copy 结束".to_string();
                done = true;
            }
            KeyCode::Char('h') | KeyCode::Left => copy.move_by(-1, 0),
            KeyCode::Char('l') | KeyCode::Right => copy.move_by(1, 0),
            KeyCode::Char('j') | KeyCode::Down => {
                if copy.cursor.1 + 1 >= copy.size.1 {
                    scroll = -1;
                } else {
                    copy.move_by(0, 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if copy.cursor.1 == 0 {
                    scroll = 1;
                } else {
                    copy.move_by(0, -1);
                }
            }
            KeyCode::Char('w') => copy.word_next(),
            KeyCode::Char('b') => copy.word_previous(),
            KeyCode::Char('0') | KeyCode::Home => copy.line_start(),
            KeyCode::Char('$') | KeyCode::End => copy.line_end(),
            KeyCode::Char('g') => copy.to_top(),
            KeyCode::Char('G') => copy.to_bottom(),
            KeyCode::Char('v') | KeyCode::Char(' ') => copy.toggle_anchor(),
            KeyCode::Char('R') => {
                copy.toggle_rectangle();
                self.note = if copy.rectangle {
                    "矩形选区".to_string()
                } else {
                    "线性选区".to_string()
                };
            }
            KeyCode::Char('o') => {
                if let Some(uri) = self
                    .panes
                    .get(&copy.pane)
                    .and_then(|pane| pane.view.link_at(copy.cursor.0, u32::from(copy.cursor.1)))
                {
                    let opener = if cfg!(target_os = "macos") {
                        "open"
                    } else {
                        "xdg-open"
                    };
                    let _ = std::process::Command::new(opener)
                        .arg(&uri)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                    self.note = format!("打开 {uri}");
                } else {
                    self.note = "光标处没有链接".to_string();
                }
            }
            KeyCode::Char('/') => {
                copy.typing = true;
                copy.search = Some(String::new());
                self.note = "搜索（Enter 确认，Esc 取消）".to_string();
            }
            KeyCode::Char('n') => {
                if !copy.find_next() {
                    self.note = "没有找到".to_string();
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                scroll = (copy.size.1 / 2).max(1) as isize;
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                scroll = -((copy.size.1 / 2).max(1) as isize);
            }
            KeyCode::PageUp => scroll = copy.size.1.max(1) as isize,
            KeyCode::PageDown => scroll = -(copy.size.1.max(1) as isize),
            KeyCode::Char('y') | KeyCode::Enter => {
                let (start, end) = copy.selection().unwrap_or((
                    (0, copy.cursor.1),
                    (copy.size.0.saturating_sub(1), copy.cursor.1),
                ));
                let text = self
                    .panes
                    .get(&copy.pane)
                    .map(|pane| pane.view.selection_text(start, end, false))
                    .unwrap_or_default();
                self.note = clipboard::copy(&text);
                done = true;
            }
            _ => {}
        }
        if done {
            self.dirty = true;
            return;
        }
        if scroll != 0 {
            if let Some(pane) = self.panes.get_mut(&copy.pane) {
                pane.view.scroll(scroll);
            }
        }
        if let Some((size, lines)) = self
            .panes
            .get(&copy.pane)
            .map(|pane| (pane.view.size(), pane.view.viewport_lines()))
        {
            copy.refresh(size, lines);
        }
        self.copy = Some(copy);
        self.dirty = true;
    }

    // ---------------------------------------------------------------- 动作

    fn ask(&mut self, kind: PromptKind, label: &str) {
        self.prompt = Some(Prompt {
            label: label.to_string(),
            buffer: String::new(),
            cursor: 0,
            history_pos: None,
            kind,
        });
        self.prefix = false;
        self.dirty = true;
    }

    fn commit_prompt(&mut self, prompt: Prompt) {
        let text = prompt.buffer.trim().to_string();
        if prompt.kind.needs_confirm() && !matches!(text.as_str(), "y" | "Y" | "yes") {
            self.note = "已取消".to_string();
            self.dirty = true;
            return;
        }
        match prompt.kind {
            PromptKind::NewTab => self.new_tab(Some(text)),
            PromptKind::NewWorkspace => self.new_workspace(Some(text)),
            PromptKind::RenameTab { workspace, tab } => {
                if let Some(tab) = self
                    .workspaces
                    .get_mut(workspace)
                    .and_then(|workspace| workspace.tabs.iter_mut().find(|item| item.id == tab))
                {
                    tab.label = text;
                }
                self.mark_layout();
            }
            PromptKind::RenameWorkspace(index) => {
                if let Some(workspace) = self.workspaces.get_mut(index) {
                    workspace.label = text;
                }
                self.mark_layout();
            }
            PromptKind::RenamePane(pane) => {
                let terminal = self
                    .panes
                    .get(&pane)
                    .and_then(|state| state.terminal.clone());
                if let Some(state) = self.panes.get_mut(&pane) {
                    state.name = (!text.is_empty()).then_some(text.clone());
                }
                if let Some(terminal) = terminal {
                    self.call_rpc(
                        "label",
                        "terminal.label",
                        json!({"terminal": terminal, "labels": {"hctui": text}}),
                    );
                }
                self.mark_layout();
            }
            PromptKind::SendText(pane) => {
                let terminal = self
                    .panes
                    .get(&pane)
                    .and_then(|state| state.terminal.clone());
                if let Some(terminal) = terminal {
                    self.call_rpc(
                        "send",
                        "terminal.send",
                        json!({"terminal": terminal, "text": text}),
                    );
                    self.note = format!("正在发给 {terminal}");
                } else {
                    self.note = "这个 pane 还没有终端".to_string();
                }
            }
            PromptKind::SendKeys(pane) => {
                let keys: Vec<String> = text
                    .split(|character: char| character == ',' || character.is_whitespace())
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
                    .collect();
                if let Some(terminal) = self
                    .panes
                    .get(&pane)
                    .and_then(|state| state.terminal.clone())
                {
                    self.call_rpc(
                        "keys",
                        "terminal.keys",
                        json!({"terminal": terminal, "keys": keys}),
                    );
                    self.note = format!("按下 {} 个键", keys.len());
                }
            }
            PromptKind::WaitOutput(pane) => {
                if let Some(terminal) = self
                    .panes
                    .get(&pane)
                    .and_then(|state| state.terminal.clone())
                {
                    self.call_rpc(
                        "wait",
                        "terminal.wait",
                        json!({"terminal": terminal, "output": text}),
                    );
                    self.note = format!("等待输出：{text}");
                }
            }
            PromptKind::PutFile(pane) => {
                let path = std::path::PathBuf::from(&text);
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".to_string());
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        if let Some(terminal) = self
                            .panes
                            .get(&pane)
                            .and_then(|state| state.terminal.clone())
                        {
                            self.call_rpc(
                                "put",
                                "terminal.put",
                                json!({
                                    "terminal": terminal,
                                    "name": name,
                                    "data": crate::clipboard::base64(&bytes),
                                }),
                            );
                            self.note = format!("发送文件 {name}");
                        }
                    }
                    Err(error) => self.note = format!("读取失败：{error}"),
                }
            }
            PromptKind::AddMachine => {
                self.call_rpc("machine.add", "machine.add", json!({"target": text}));
                self.note = format!("正在添加机器 {text}");
            }
            PromptKind::RenameMachine(machine) => {
                self.call_rpc(
                    "machine.rename",
                    "machine.rename",
                    json!({"machine": machine, "name": text}),
                );
            }
            PromptKind::CloseTab(workspace) => self.close_tab_in(workspace),
            PromptKind::CloseWorkspace(index) => self.close_workspace_in(index),
            PromptKind::KillPane(pane) => {
                let terminal = self
                    .panes
                    .get(&pane)
                    .and_then(|state| state.terminal.clone());
                if let Some(terminal) = terminal {
                    self.call_rpc("kill", "terminal.kill", json!({"terminal": terminal}));
                    self.note = format!("正在结束 {terminal}");
                } else {
                    self.note = "这个 pane 还没有终端".to_string();
                }
            }
        }
        self.dirty = true;
    }

    fn reload_config(&mut self) {
        let path = self.config.config_path.clone();
        let (session, attach) = (self.config.session.clone(), self.config.attach.clone());
        let mut config = Config::load(Some(path));
        config.session = session;
        config.attach = attach;
        self.sidebar_width = config
            .sidebar_width
            .clamp(config.sidebar_min_width, config.sidebar_max_width);
        self.note = if config.warnings.is_empty() {
            format!("配置已重新加载 · 主题 {}", config.theme.name)
        } else {
            config.warnings.join(" · ")
        };
        self.config = config;
        self.dirty = true;
    }

    fn new_tab(&mut self, label: Option<String>) {
        let pane = self.alloc_pane();
        let id = self.alloc_tab();
        self.spawn_pane(pane);
        let Some(workspace) = self.workspaces.get_mut(self.active_ws) else {
            return;
        };
        let mut tab = Tab::new(id, pane);
        tab.label = label.unwrap_or_default();
        workspace.tabs.push(tab);
        workspace.active = workspace.tabs.len() - 1;
        self.zoom = false;
        self.note = "new tab".to_string();
        self.mark_layout();
    }

    fn new_workspace(&mut self, label: Option<String>) {
        let pane = self.alloc_pane();
        let tab = self.alloc_tab();
        let id = self.alloc_workspace();
        self.spawn_pane(pane);
        let label = match label {
            Some(text) if !text.is_empty() => text,
            _ => format!("w{id}"),
        };
        self.workspaces.push(Workspace::new(id, label, pane, tab));
        self.active_ws = self.workspaces.len() - 1;
        self.zoom = false;
        self.note = "new workspace".to_string();
        self.mark_layout();
    }

    fn split_focused(&mut self, axis: Axis) {
        match self.split_new(axis) {
            Some(new) => {
                self.spawn_pane(new);
                self.note = "split".to_string();
            }
            None => {
                self.note =
                    format!("空间不足：每个 Pane 至少要 {MIN_PANE_WIDTH}x{MIN_PANE_HEIGHT}");
            }
        }
        self.dirty = true;
    }

    /// 在焦点 Pane 旁边开一格并返回新 Pane 的编号；视图由调用方创建。
    fn split_new(&mut self, axis: Axis) -> Option<u32> {
        let pane = self.focused();
        let new = self.alloc_pane();
        let content = self.content_rect();
        let candidates = {
            let Some(tab) = self.tab() else {
                self.next_pane -= 1;
                return None;
            };
            let mut tree = tab.tree.clone();
            if !tree.split_pane(PaneId(pane), axis, PaneId(new), 0.5) {
                self.next_pane -= 1;
                return None;
            }
            tree.layout(content)
        };
        for (_, rect) in &candidates {
            let inner = inner_rect(*rect, ratatui::widgets::Borders::ALL);
            if inner.width < MIN_PANE_WIDTH || inner.height < MIN_PANE_HEIGHT {
                self.next_pane -= 1;
                return None;
            }
        }
        let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) else {
            self.next_pane -= 1;
            return None;
        };
        if !tab.tree.split_pane(PaneId(pane), axis, PaneId(new), 0.5) {
            self.next_pane -= 1;
            return None;
        }
        tab.focus = new;
        self.send(Command::Focus {
            pane,
            focused: false,
        });
        self.zoom = false;
        self.last_pane = Some(pane);
        self.mark_layout();
        Some(new)
    }

    fn move_focus(&mut self, direction: Direction) {
        let rects = self
            .workspace()
            .and_then(Workspace::tab)
            .map(|tab| tab.tree.layout(self.content_rect()))
            .unwrap_or_default();
        let from = PaneId(self.focused());
        if let Some(next) = neighbor(&rects, from, direction) {
            self.set_focus(next.0);
        }
    }

    fn cycle_pane(&mut self, delta: isize) {
        let Some(tab) = self.tab() else {
            return;
        };
        let panes = tab.tree.panes();
        if panes.len() < 2 {
            return;
        }
        let current = panes
            .iter()
            .position(|pane| pane.0 == tab.focus)
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(panes.len() as isize) as usize;
        self.set_focus(panes[next].0);
    }

    /// 切换焦点终端的 keep 开关。
    fn toggle_keep(&mut self) {
        let pane = self.input_pane();
        let Some(terminal) = self
            .panes
            .get(&pane)
            .and_then(|state| state.terminal.clone())
        else {
            self.note = "这个 pane 还没有终端".to_string();
            self.dirty = true;
            return;
        };
        let keep = self
            .terminals
            .iter()
            .find(|summary| summary.address == terminal)
            .map(|summary| summary.keep)
            .unwrap_or(false);
        self.call_rpc(
            "keep",
            "terminal.keep",
            json!({"terminal": terminal, "keep": !keep}),
        );
        self.note = format!("{terminal} keep = {}", !keep);
        self.dirty = true;
    }

    /// 从列表里移除一个已结束的终端。
    fn remove_finished_terminal(&mut self) {
        let pane = self.input_pane();
        let Some(terminal) = self
            .panes
            .get(&pane)
            .and_then(|state| state.terminal.clone())
        else {
            self.note = "这个 pane 还没有终端".to_string();
            self.dirty = true;
            return;
        };
        let live = self
            .terminals
            .iter()
            .find(|summary| summary.address == terminal)
            .map(|summary| summary.live)
            .unwrap_or(true);
        if live {
            self.note = format!("{terminal} 还在运行，先 prefix shift+d 结束它");
            self.dirty = true;
            return;
        }
        self.call_rpc("rm", "terminal.rm", json!({"terminal": terminal}));
        self.remove_pane_view(pane);
        self.note = format!("已移除 {terminal}");
        self.dirty = true;
    }

    /// 打开光标位置的链接。
    fn open_link_at_cursor(&mut self) {
        let pane = self.input_pane();
        let Some(state) = self.panes.get(&pane) else {
            return;
        };
        let position = state.view.cursor().unwrap_or((0, 0));
        let Some(uri) = state.view.link_at(position.0, u32::from(position.1)) else {
            self.note = "光标处没有链接".to_string();
            self.dirty = true;
            return;
        };
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        match std::process::Command::new(opener)
            .arg(&uri)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => self.note = format!("打开 {uri}"),
            Err(error) => self.note = format!("打开链接失败：{error}"),
        }
        self.dirty = true;
    }

    /// 把当前视口的 ANSI 内容写到临时文件，在 popup 里用 `$EDITOR` 打开。
    fn edit_history_popup(&mut self) {
        let pane = self.input_pane();
        let Some(state) = self.panes.get(&pane) else {
            return;
        };
        let ansi = state.view.ansi_viewport();
        let path = std::env::temp_dir().join(format!("hc-tui-history-{pane}.ansi"));
        if let Err(error) = std::fs::write(&path, ansi) {
            self.note = format!("写临时文件失败：{error}");
            self.dirty = true;
            return;
        }
        let editor = std::env::var("EDITOR")
            .ok()
            .filter(|editor| !editor.trim().is_empty())
            .unwrap_or_else(|| "vi".to_string());
        self.open_popup(
            vec![editor, path.to_string_lossy().into_owned()],
            Some(std::env::temp_dir().to_string_lossy().into_owned()),
        );
    }

    /// 打开一个会话级浮层终端。
    fn open_popup(&mut self, argv: Vec<String>, cwd: Option<String>) {
        let pane = self.alloc_pane();
        self.spawn_pane(pane);
        if let Some(state) = self.panes.get_mut(&pane) {
            state.transient = true;
            state.launch = Some(Launch::local(argv, cwd));
        }
        self.popup = Some(PopupPane {
            pane,
            width: Span::Percent(80),
            height: Span::Percent(80),
        });
        self.note = "popup 打开（Esc 关闭）".to_string();
        self.dirty = true;
    }

    fn toggle_last_pane(&mut self) {
        let current = self.focused();
        if let Some(previous) = self.last_pane.replace(current) {
            if previous != current {
                self.focus_pane(previous);
            }
        }
    }

    fn set_focus(&mut self, pane: u32) {
        if self.focused() == pane {
            self.mark_seen(pane);
            return;
        }
        let previous = self.focused();
        self.send(Command::Focus {
            pane: previous,
            focused: false,
        });
        if let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) {
            tab.focus = pane;
        }
        self.send(Command::Focus {
            pane,
            focused: true,
        });
        self.last_pane = Some(previous);
        self.mark_seen(pane);
        self.mark_layout();
    }

    /// 跨 Tab、跨工作区把焦点落到某个 Pane 上。
    pub fn focus_pane(&mut self, pane: u32) {
        let Some((workspace, tab)) = self.locate(pane) else {
            return;
        };
        let previous = self.focused();
        if previous != pane {
            self.send(Command::Focus {
                pane: previous,
                focused: false,
            });
            self.last_pane = Some(previous);
        }
        self.zoom = false;
        if workspace != self.active_ws {
            self.active_ws = workspace;
        }
        if let Some(state) = self.workspaces.get_mut(workspace) {
            state.active = tab;
            if let Some(tab) = state.tab_mut() {
                tab.focus = pane;
            }
        }
        self.send(Command::Focus {
            pane,
            focused: true,
        });
        self.mark_seen(pane);
        self.dirty = true;
    }

    fn locate(&self, pane: u32) -> Option<(usize, usize)> {
        for (workspace_index, workspace) in self.workspaces.iter().enumerate() {
            if let Some(tab) = workspace.tab_of(pane) {
                return Some((workspace_index, tab));
            }
        }
        None
    }

    fn mark_seen(&mut self, pane: u32) {
        let Some(state) = self.panes.get_mut(&pane) else {
            return;
        };
        state.agent.mark_seen();
        if let Some(terminal) = state.terminal.clone() {
            let turn = state.agent.turn;
            self.seen.insert(terminal, turn);
            self.seen_dirty = true;
            self.seen_since = Some(Instant::now());
        }
        self.dirty = true;
    }

    fn focus_next_attention(&mut self) {
        let current = self.focused();
        let mut candidates: Vec<(Badge, u32)> = self
            .panes
            .iter()
            .filter(|(pane, state)| **pane != current && state.agent.wants_attention())
            .map(|(pane, state)| (state.agent.badge(), *pane))
            .collect();
        candidates.sort();
        match candidates.first().copied() {
            Some((badge, pane)) => {
                let label = self
                    .panes
                    .get(&pane)
                    .map(Pane::label)
                    .filter(|label| !label.is_empty())
                    .unwrap_or_else(|| format!("pane {pane}"));
                self.focus_pane(pane);
                self.note = format!("跳到 {} 的 {label}", badge.text());
            }
            None => self.note = "没有需要处理的 Agent".to_string(),
        }
        self.dirty = true;
    }

    fn close_focused_pane(&mut self) {
        let pane = self.focused();
        let tab_panes = {
            let Some(tab) = self.tab() else {
                return;
            };
            tab.tree.panes()
        };
        if tab_panes.len() <= 1 {
            if self.config.confirm_close {
                self.ask(
                    PromptKind::CloseTab(self.active_ws),
                    "这是 tab 里最后一个 pane，关闭整个 tab？(y/N)",
                );
            } else {
                let workspace = self.active_ws;
                self.close_tab_in(workspace);
            }
            return;
        }
        self.remove_pane_view(pane);
        let remaining = {
            let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) else {
                return;
            };
            let tree = tab.tree.clone();
            match tree.remove_pane(PaneId(pane)) {
                Some(tree) => {
                    tab.tree = tree;
                    let panes = tab.tree.panes();
                    tab.focus = panes.first().map(|pane| pane.0).unwrap_or_default();
                    Some(tab.focus)
                }
                None => None,
            }
        };
        if let Some(next) = remaining {
            self.send(Command::Focus {
                pane: next,
                focused: true,
            });
            self.zoom = false;
            self.note = "pane 视图已关闭，程序仍在 core 里运行".to_string();
        }
        self.mark_layout();
    }

    /// 只断视图，不杀进程。
    fn remove_pane_view(&mut self, pane: u32) {
        self.send(Command::Close { pane });
        self.panes.remove(&pane);
        if self.copy.as_ref().is_some_and(|copy| copy.pane == pane) {
            self.copy = None;
        }
    }

    fn close_tab_in(&mut self, workspace_index: usize) {
        let tab_count = self
            .workspaces
            .get(workspace_index)
            .map(|workspace| workspace.tabs.len())
            .unwrap_or(0);
        if tab_count <= 1 && self.workspaces.len() <= 1 {
            self.note = "最后一个 tab：用 prefix q 退出".to_string();
            self.dirty = true;
            return;
        }
        let removed = {
            let Some(workspace) = self.workspaces.get_mut(workspace_index) else {
                return;
            };
            let removed = workspace.tabs.remove(workspace.active);
            if workspace.active >= workspace.tabs.len() {
                workspace.active = workspace.tabs.len().saturating_sub(1);
            }
            removed
        };
        for pane in removed.panes() {
            self.remove_pane_view(pane.0);
        }
        let empty = self
            .workspaces
            .get(workspace_index)
            .is_some_and(|workspace| workspace.tabs.is_empty());
        if empty {
            self.close_workspace_in(workspace_index);
        }
        self.zoom = false;
        self.mark_layout();
    }

    fn close_workspace_in(&mut self, index: usize) {
        if index >= self.workspaces.len() {
            return;
        }
        if self.workspaces.len() <= 1 {
            self.note = "最后一个工作区：用 prefix q 退出".to_string();
            self.dirty = true;
            return;
        }
        let workspace = self.workspaces.remove(index);
        for tab in &workspace.tabs {
            for pane in tab.panes() {
                self.remove_pane_view(pane.0);
            }
        }
        if self.active_ws >= self.workspaces.len() {
            self.active_ws = self.workspaces.len() - 1;
        }
        self.zoom = false;
        self.mark_layout();
    }

    fn select_tab(&mut self, index: usize) {
        let Some(workspace) = self.workspaces.get_mut(self.active_ws) else {
            return;
        };
        if index >= workspace.tabs.len() || index == workspace.active {
            return;
        }
        let previous = workspace.focus().unwrap_or_default();
        workspace.active = index;
        let next = workspace.focus().unwrap_or_default();
        self.zoom = false;
        self.note = format!("tab {}", index + 1);
        if previous != next {
            self.send(Command::Focus {
                pane: previous,
                focused: false,
            });
            self.send(Command::Focus {
                pane: next,
                focused: true,
            });
        }
        self.mark_seen(next);
        self.mark_layout();
    }

    fn select_workspace(&mut self, index: usize) {
        if index >= self.workspaces.len() || index == self.active_ws {
            return;
        }
        let previous = self.focused();
        self.active_ws = index;
        self.zoom = false;
        let next = self.focused();
        self.note = format!("workspace {}", index + 1);
        if previous != next {
            self.send(Command::Focus {
                pane: previous,
                focused: false,
            });
            self.send(Command::Focus {
                pane: next,
                focused: true,
            });
        }
        self.mark_seen(next);
        self.mark_layout();
    }

    fn cycle_tab(&mut self, delta: isize) {
        let count = self.tabs_in_workspace();
        if count < 2 {
            return;
        }
        let current = self
            .workspace()
            .map(|workspace| workspace.active)
            .unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(count as isize) as usize;
        self.select_tab(next);
    }

    fn move_tab(&mut self, delta: isize) {
        let Some(workspace) = self.workspaces.get_mut(self.active_ws) else {
            return;
        };
        let count = workspace.tabs.len();
        if count < 2 {
            return;
        }
        let from = workspace.active;
        let to = (from as isize + delta).rem_euclid(count as isize) as usize;
        if from == to {
            return;
        }
        let tab = workspace.tabs.remove(from);
        workspace.tabs.insert(to, tab);
        workspace.active = to;
        self.note = format!("tab {} → {}", from + 1, to + 1);
        self.mark_layout();
    }

    fn tabs_in_workspace(&self) -> usize {
        self.workspace()
            .map(|workspace| workspace.tabs.len())
            .unwrap_or(0)
    }

    /// 把当前 Pane 移到同一工作区的另一个 Tab。
    fn move_pane_to_tab(&mut self, target_index: usize) {
        let pane = self.focused();
        let workspace_index = self.active_ws;
        let Some(workspace) = self.workspaces.get(workspace_index) else {
            return;
        };
        let Some(source_index) = workspace.tab_of(pane) else {
            return;
        };
        if source_index == target_index || target_index >= workspace.tabs.len() {
            self.note = "目标 tab 无效".to_string();
            self.dirty = true;
            return;
        }
        let target_id = workspace.tabs[target_index].id;
        let previous = workspace.tab().map(|tab| tab.focus).unwrap_or_default();
        let tree = std::mem::replace(
            &mut self.workspaces[workspace_index].tabs[source_index].tree,
            SplitNode::Leaf(PaneId(pane)),
        );
        let remaining = tree.remove_pane(PaneId(pane));
        match remaining {
            Some(tree) => {
                let tab = &mut self.workspaces[workspace_index].tabs[source_index];
                let panes = tree.panes();
                tab.tree = tree;
                tab.focus = panes.first().map(|pane| pane.0).unwrap_or_default();
            }
            None => {
                self.workspaces[workspace_index].tabs.remove(source_index);
            }
        }
        // 源 tab 可能已经消失，按 id 重新定位目标。
        let Some(target_index) = self
            .workspaces
            .get(workspace_index)
            .and_then(|workspace| workspace.tabs.iter().position(|tab| tab.id == target_id))
        else {
            return;
        };
        let axis = {
            let rect = self.content_rect();
            if rect.width >= rect.height {
                Axis::Horizontal
            } else {
                Axis::Vertical
            }
        };
        let workspace = &mut self.workspaces[workspace_index];
        let tab = &mut workspace.tabs[target_index];
        let focus = tab.focus;
        if !tab.tree.split_pane(PaneId(focus), axis, PaneId(pane), 0.5) {
            self.note = "移动失败：目标 tab 找不到插入点".to_string();
            self.dirty = true;
            return;
        }
        tab.focus = pane;
        workspace.active = target_index;
        self.send(Command::Focus {
            pane: previous,
            focused: false,
        });
        self.send(Command::Focus {
            pane,
            focused: true,
        });
        self.zoom = false;
        self.note = format!("pane 已移到 tab {}", target_index + 1);
        self.mark_layout();
    }

    /// 把当前 Pane 提成一个新 Tab。
    fn move_pane_to_new_tab(&mut self) {
        let pane = self.focused();
        let workspace_index = self.active_ws;
        let Some(workspace) = self.workspaces.get(workspace_index) else {
            return;
        };
        let Some(source_index) = workspace.tab_of(pane) else {
            return;
        };
        let tree = std::mem::replace(
            &mut self.workspaces[workspace_index].tabs[source_index].tree,
            SplitNode::Leaf(PaneId(pane)),
        );
        let remaining = tree.remove_pane(PaneId(pane));
        match remaining {
            Some(tree) => {
                let tab = &mut self.workspaces[workspace_index].tabs[source_index];
                let panes = tree.panes();
                tab.tree = tree;
                tab.focus = panes.first().map(|pane| pane.0).unwrap_or_default();
            }
            None => {
                self.workspaces[workspace_index].tabs.remove(source_index);
            }
        }
        let id = self.alloc_tab();
        let workspace = &mut self.workspaces[workspace_index];
        workspace.tabs.push(Tab::new(id, pane));
        workspace.active = workspace.tabs.len() - 1;
        self.zoom = false;
        self.note = "pane 已移成新 tab".to_string();
        self.mark_layout();
    }

    fn spawn_pane(&mut self, pane: u32) {
        let view = match TerminalView::new(80, 24, self.config.scrollback_bytes) {
            Ok(view) => view,
            Err(error) => {
                self.note = format!("cannot create local VT: {error}");
                return;
            }
        };
        self.panes.insert(pane, Pane::new(view));
    }

    fn alloc_pane(&mut self) -> u32 {
        self.next_pane += 1;
        self.next_pane
    }

    fn alloc_tab(&mut self) -> u32 {
        self.next_tab += 1;
        self.next_tab
    }

    fn alloc_workspace(&mut self) -> u32 {
        self.next_workspace += 1;
        self.next_workspace
    }

    fn content_rect(&self) -> Rect {
        self.content
    }

    fn workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.workspaces.get_mut(self.active_ws)
    }

    fn pane_by_channel(&self, channel: u32) -> Option<u32> {
        self.panes
            .iter()
            .find_map(|(pane, state)| (state.channel() == Some(channel)).then_some(*pane))
    }

    fn pane_by_terminal(&self, terminal: &str) -> Option<u32> {
        self.panes.iter().find_map(|(pane, state)| {
            (state.terminal.as_deref() == Some(terminal)).then_some(*pane)
        })
    }

    // ------------------------------------------------------------- 网络事件

    fn load_local_state(&mut self) {
        let Ok(text) = std::fs::read_to_string(&self.config.state_path) else {
            return;
        };
        let Ok(state) = serde_json::from_str::<LocalState>(&text) else {
            return;
        };
        if state.schema_version <= 1 {
            self.seen = state.seen;
        }
    }

    fn save_local_state(&mut self) {
        self.seen_dirty = false;
        self.seen_since = None;
        let state = LocalState {
            schema_version: 1,
            seen: self.seen.clone(),
        };
        let path = self.config.state_path.clone();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(&state) {
            let _ = std::fs::write(path, text);
        }
    }

    /// 记下请求参数，答复丢了还能重放。
    fn call_rpc(&mut self, tag: &str, method: &str, params: Value) {
        self.inflight
            .insert(tag.to_string(), (method.to_string(), params.clone()));
        self.send(Command::Call {
            tag: tag.to_string(),
            method: method.to_string(),
            params,
        });
    }

    fn retry_pending_calls(&mut self) {
        let pending = std::mem::take(&mut self.pending_calls);
        for call in pending {
            self.call_rpc(&call.tag, &call.method, call.params);
        }
    }

    /// 命中一条自定义命令。
    fn custom_command(&self, key: &KeyEvent, prefix: bool) -> Option<usize> {
        self.config.commands.iter().position(|command| {
            if prefix {
                command.key.matches_prefix(key)
            } else {
                command.key.matches_direct(key)
            }
        })
    }

    fn run_custom(&mut self, index: usize) {
        let Some(command) = self.config.commands.get(index).cloned() else {
            return;
        };
        let cwd = self.focused_pane().and_then(|pane| pane.cwd.clone());
        match command.kind {
            CommandKind::Shell => {
                let argv = shell_argv(&command.command);
                let spawned = std::process::Command::new(&argv[0])
                    .args(&argv[1..])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                self.note = match spawned {
                    Ok(_) => format!("后台运行：{}", command.command),
                    Err(error) => format!("启动失败：{error}"),
                };
            }
            CommandKind::Pane => {
                let axis = if self.content.width >= self.content.height {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                };
                match self.split_new(axis) {
                    Some(pane) => {
                        self.spawn_pane(pane);
                        let launch = Launch::local(shell_argv(&command.command), cwd);
                        if let Some(state) = self.panes.get_mut(&pane) {
                            state.transient = true;
                            state.launch = Some(launch);
                        }
                        self.note = format!("pane：{}", command.command);
                    }
                    None => {
                        self.note =
                            format!("空间不足：每个 Pane 至少要 {MIN_PANE_WIDTH}x{MIN_PANE_HEIGHT}")
                    }
                }
            }
            CommandKind::Popup => {
                self.open_popup(shell_argv(&command.command), cwd);
                if let Some(popup) = &mut self.popup {
                    popup.width = command.width.unwrap_or(Span::Percent(80));
                    popup.height = command.height.unwrap_or(Span::Percent(80));
                }
                self.note = format!("popup：{}（Esc 关闭）", command.command);
            }
        }
        self.dirty = true;
    }

    fn close_popup(&mut self) {
        if let Some(popup) = self.popup.take() {
            self.remove_pane_view(popup.pane);
            self.note = "popup 已关闭（程序仍在 core 里运行）".to_string();
            self.dirty = true;
        }
    }

    fn open_reader(&mut self) {
        let pane = self.input_pane();
        let Some(terminal) = self
            .panes
            .get(&pane)
            .and_then(|state| state.terminal.clone())
        else {
            self.note = "这个 pane 还没有终端".to_string();
            self.dirty = true;
            return;
        };
        self.call_rpc(
            "read",
            "terminal.read",
            json!({"terminal": terminal, "lines": 500}),
        );
        self.note = format!("读取 {terminal}…");
        self.dirty = true;
    }

    fn on_reader_key(&mut self, key: KeyEvent) {
        let height = self.content.height.max(1) as usize;
        let Some(reader) = &mut self.reader else {
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.reader = None;
                self.note = "reader 关闭".to_string();
            }
            KeyCode::Char('j') | KeyCode::Down => reader.scroll = reader.scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => reader.scroll = reader.scroll.saturating_sub(1),
            KeyCode::PageDown => reader.scroll = reader.scroll.saturating_add(height),
            KeyCode::PageUp => reader.scroll = reader.scroll.saturating_sub(height),
            KeyCode::Char('g') | KeyCode::Home => reader.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => {
                reader.scroll = reader.lines.len().saturating_sub(height)
            }
            _ => {}
        }
        let max = self
            .reader
            .as_ref()
            .map(|reader| reader.lines.len().saturating_sub(height))
            .unwrap_or(0);
        if let Some(reader) = &mut self.reader {
            reader.scroll = reader.scroll.min(max);
        }
        self.dirty = true;
    }

    fn open_chooser(&mut self, chooser: Chooser) {
        self.chooser = Some(chooser);
        self.dirty = true;
    }

    fn on_chooser_key(&mut self, key: KeyEvent) {
        let Some(chooser) = &mut self.chooser else {
            return;
        };
        let kind = chooser.kind;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.chooser = None;
                self.dirty = true;
                return;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !chooser.items.is_empty() {
                    chooser.selected = (chooser.selected + 1) % chooser.items.len();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if !chooser.items.is_empty() {
                    chooser.selected = chooser
                        .selected
                        .checked_sub(1)
                        .unwrap_or(chooser.items.len() - 1);
                }
            }
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index < chooser.items.len() {
                    chooser.selected = index;
                    self.activate_chooser();
                }
            }
            KeyCode::Enter => self.activate_chooser(),
            KeyCode::Char('a') if kind == ChooserKind::Machines => {
                self.chooser = None;
                self.ask(
                    PromptKind::AddMachine,
                    "ssh 目标（user@host 或 ~/.ssh/config 的 Host）",
                );
            }
            KeyCode::Char('d') if kind == ChooserKind::Machines => {
                if let Some(item) = chooser.items.get(chooser.selected) {
                    let machine = item.value.clone();
                    self.call_rpc("machine.rm", "machine.rm", json!({"machine": machine}));
                    self.chooser = None;
                    self.note = "正在移除机器".to_string();
                }
            }
            KeyCode::Char('c') if kind == ChooserKind::Machines => {
                if let Some(item) = chooser.items.get(chooser.selected) {
                    let machine = item.value.clone();
                    self.call_rpc(
                        "machine.connect",
                        "machine.connect",
                        json!({"machine": machine}),
                    );
                    self.note = format!("正在连接 {machine}");
                }
            }
            KeyCode::Char('r') if kind == ChooserKind::Machines => {
                if let Some(item) = chooser.items.get(chooser.selected) {
                    let machine = item.value.clone();
                    self.chooser = None;
                    self.ask(PromptKind::RenameMachine(machine), "机器新名字");
                }
            }
            KeyCode::Char('n') if kind == ChooserKind::Machines => {
                if let Some(item) = chooser.items.get(chooser.selected) {
                    let machine = item.value.clone();
                    self.chooser = None;
                    self.open_remote_pane(machine);
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn activate_chooser(&mut self) {
        let Some(chooser) = self.chooser.clone() else {
            return;
        };
        let Some(item) = chooser.items.get(chooser.selected).cloned() else {
            return;
        };
        self.chooser = None;
        match chooser.kind {
            ChooserKind::Templates => self.spawn_program_in_new_tab(vec![item.value], None),
            ChooserKind::Machines => self.open_remote_pane(item.value),
        }
        self.dirty = true;
    }

    /// 在当前 Tab 开一个新 Pane，并在另一台机器上起它的 shell。
    fn open_remote_pane(&mut self, machine: String) {
        let axis = if self.content.width >= self.content.height {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };
        match self.split_new(axis) {
            Some(pane) => {
                self.spawn_pane(pane);
                if let Some(state) = self.panes.get_mut(&pane) {
                    state.launch = Some(Launch::remote(Vec::new(), None, machine.clone()));
                }
                self.note = format!("在 {machine} 上开新 pane");
            }
            None => self.note = "空间不足".to_string(),
        }
        self.dirty = true;
    }

    /// 新 Tab 里跑一个程序（模板用）。
    fn spawn_program_in_new_tab(&mut self, argv: Vec<String>, cwd: Option<String>) {
        let pane = self.alloc_pane();
        let tab = self.alloc_tab();
        self.spawn_pane(pane);
        if let Some(state) = self.panes.get_mut(&pane) {
            state.launch = Some(Launch::local(argv, cwd));
        }
        if let Some(workspace) = self.workspaces.get_mut(self.active_ws) {
            workspace.tabs.push(Tab::new(tab, pane));
            workspace.active = workspace.tabs.len() - 1;
        }
        self.mark_layout();
        self.dirty = true;
    }

    /// 剪贴板里的图片：存成本地文件后交给 `terminal.put`，再把路径贴进程序。
    fn paste_image(&mut self) {
        let pane = self.input_pane();
        let Some(terminal) = self
            .panes
            .get(&pane)
            .and_then(|state| state.terminal.clone())
        else {
            self.note = "这个 pane 还没有终端".to_string();
            self.dirty = true;
            return;
        };
        let Some(bytes) = clipboard_image() else {
            self.note = "剪贴板里没有图片".to_string();
            self.dirty = true;
            return;
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        self.call_rpc(
            "put-image",
            "terminal.put",
            json!({
                "terminal": terminal,
                "name": format!("paste-{stamp}.png"),
                "data": crate::clipboard::base64(&bytes),
            }),
        );
        self.note = "正在发送剪贴板图片".to_string();
        self.dirty = true;
    }

    /// 命令退出后自动移除视图，并把树里的空位折叠掉。
    fn remove_transient(&mut self, pane: u32) {
        let Some((workspace, tab)) = self.locate(pane) else {
            self.remove_pane_view(pane);
            return;
        };
        let focused_before = self.workspaces[workspace].tabs[tab].focus;
        self.remove_pane_view(pane);
        let replacement = {
            let tab = &mut self.workspaces[workspace].tabs[tab];
            let tree = tab.tree.clone();
            match tree.remove_pane(PaneId(pane)) {
                Some(tree) => {
                    let panes = tree.panes();
                    tab.tree = tree;
                    tab.focus = panes.first().map(|pane| pane.0).unwrap_or_default();
                    Some(tab.focus)
                }
                None => None,
            }
        };
        match replacement {
            Some(next) => {
                if focused_before == pane {
                    self.send(Command::Focus {
                        pane: next,
                        focused: true,
                    });
                }
                self.note = "临时命令已结束".to_string();
            }
            None => self.close_tab_in(workspace),
        }
        self.mark_layout();
    }

    /// 输入该去的 Pane：popup 打开时独占输入。
    pub fn input_pane(&self) -> u32 {
        self.popup
            .map(|popup| popup.pane)
            .unwrap_or_else(|| self.focused())
    }

    fn on_net(&mut self, event: NetEvent) {
        match event {
            NetEvent::Hello { version, core } => {
                self.connection = Connection::Ready { version, core };
                self.retry_pending_calls();
                if self.restore_pending {
                    self.request_doc("doc.get");
                } else {
                    self.note = "subscribing".to_string();
                }
                self.dirty = true;
            }
            NetEvent::Reconnecting { reason } => {
                self.connection = Connection::Reconnecting { reason };
                self.dirty = true;
            }
            NetEvent::Failed { reason } => {
                self.connection = Connection::Failed { reason };
                self.dirty = true;
            }
            NetEvent::Snapshot { seq, terminals } => {
                self.terminals = terminals;
                self.events_seen = seq;
                self.sync_terminals();
                self.dirty = true;
            }
            NetEvent::Event { event } => self.apply_event(event),
            NetEvent::Reply { tag, result } => self.on_reply(&tag, result),
            NetEvent::Attached {
                pane,
                channel,
                terminal,
                label,
                cols,
                rows,
                resize_notifications,
            } => {
                if let Some(state) = self.panes.get_mut(&pane) {
                    let _ = state.view.resize(cols, rows, CELL_PX.0, CELL_PX.1);
                    state.attached(channel, cols, rows);
                    state.adopt(terminal.clone(), label);
                    if let Some(seen) = self.seen.get(&terminal) {
                        state.agent.seen_turn = state.agent.seen_turn.max(*seen);
                    }
                }
                if pane == self.focused() {
                    self.send(Command::Focus {
                        pane,
                        focused: true,
                    });
                }
                self.note = if resize_notifications {
                    format!("attached {terminal} (resize notifications on)")
                } else {
                    format!("attached {terminal}")
                };
                self.dirty = true;
            }
            NetEvent::Output { channel, bytes } => {
                if let Some(pane) = self.pane_by_channel(channel) {
                    if let Some(state) = self.panes.get_mut(&pane) {
                        state.view.write(&bytes);
                        self.dirty = true;
                    }
                }
            }
            NetEvent::AttachResized {
                channel,
                cols,
                rows,
            } => {
                if let Some(pane) = self.pane_by_channel(channel) {
                    if let Some(state) = self.panes.get_mut(&pane) {
                        let _ = state.view.resize(cols, rows, CELL_PX.0, CELL_PX.1);
                        state.mark_resized((cols, rows));
                    }
                    self.dirty = true;
                }
            }
            NetEvent::AttachClosed {
                pane,
                reason,
                program_exit,
                ..
            } => {
                if let Some(state) = self.panes.get_mut(&pane) {
                    state.state = PaneState::Ended {
                        reason: reason.clone(),
                    };
                }
                if program_exit && self.popup.is_some_and(|popup| popup.pane == pane) {
                    self.popup = None;
                    self.remove_pane_view(pane);
                    self.note = format!("popup 结束：{reason}");
                    self.dirty = true;
                    return;
                }
                if program_exit && self.panes.get(&pane).is_some_and(|state| state.transient) {
                    self.remove_transient(pane);
                    return;
                }
                self.note = if program_exit {
                    format!("程序结束：{reason}（prefix c 新 tab 继续）")
                } else {
                    format!("stream 断开：{reason}")
                };
                self.dirty = true;
            }
        }
    }

    fn on_reply(
        &mut self,
        tag: &str,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) {
        if let Some((method, params)) = self.inflight.remove(tag) {
            if let Err(error) = &result {
                // 连接层失败：连接恢复后重放同一条请求。
                if matches!(error.kind.as_str(), "offline" | "io" | "closed") {
                    self.pending_calls.push(PendingCall {
                        tag: tag.to_string(),
                        method,
                        params,
                    });
                    if self.pending_calls.len() > 32 {
                        self.pending_calls.remove(0);
                    }
                }
            }
        }
        match tag {
            "doc.get" => match result {
                Ok(value) => {
                    self.doc_version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
                    self.doc_loaded = true;
                    let restored = value
                        .get("value")
                        .filter(|value| !value.is_null())
                        .and_then(|value| match value {
                            Value::String(text) => SavedState::parse(text),
                            other => serde_json::from_value::<SavedState>(other.clone()).ok(),
                        })
                        .filter(SavedState::is_usable);
                    if let Some(state) = restored {
                        self.restore(state);
                        self.note = "已从 core 恢复布局".to_string();
                    }
                    self.restore_pending = false;
                    self.ensure_layout();
                    self.dirty = true;
                }
                Err(error) => {
                    self.doc_loaded = false;
                    self.restore_pending = false;
                    self.ensure_layout();
                    self.note = format!("无法读取布局：{}", error.message);
                    self.dirty = true;
                }
            },
            "doc.sync" => match result {
                Ok(value) => {
                    self.doc_version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
                    self.doc_loaded = true;
                    self.layout_dirty = true;
                    self.layout_since = Some(Instant::now());
                }
                Err(error) => self.note = format!("布局同步失败：{}", error.message),
            },
            "doc.remote" => match result {
                Ok(value) => {
                    if let Some(version) = value.get("version").and_then(Value::as_u64) {
                        if version != self.doc_version {
                            self.doc_version = version;
                            self.note =
                                "布局被另一个客户端修改，下一次操作将以本地为准".to_string();
                            self.dirty = true;
                        }
                    }
                }
                Err(error) => self.note = format!("读取布局失败：{}", error.message),
            },
            "doc.set" => match result {
                Ok(value) => {
                    if let Some(version) = value.get("version").and_then(Value::as_u64) {
                        self.doc_version = version;
                    }
                }
                Err(error) if error.kind == "conflict" => {
                    // 另一个客户端先写了：拿最新版本后再提交本地的意图。
                    self.request_doc("doc.sync");
                }
                Err(error) => self.note = format!("布局未保存：{}", error.message),
            },
            "read" => match result {
                Ok(value) => {
                    let text = value
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let how = value
                        .get("how")
                        .and_then(Value::as_str)
                        .unwrap_or("screen")
                        .to_string();
                    let complete = value
                        .get("complete")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    let lines: Vec<String> = text.lines().map(str::to_string).collect();
                    let scroll = lines.len().saturating_sub(10);
                    self.reader = Some(Reader {
                        title: format!("read · {how}"),
                        lines,
                        scroll,
                        complete,
                        how,
                    });
                    self.dirty = true;
                }
                Err(error) => self.note = format!("读取失败：{}", error.message),
            },
            "keys" => match result {
                Ok(value) => {
                    let turn = value.get("turn").and_then(Value::as_u64).unwrap_or(0);
                    let status = value
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    self.note = format!("按键已送达 · {status} · turn {turn}");
                }
                Err(error) => self.note = format!("按键失败：{}", error.message),
            },
            "wait" => match result {
                Ok(value) => {
                    let reason = value
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("status");
                    let ended = value.get("ended").and_then(Value::as_bool).unwrap_or(false);
                    self.note = if ended {
                        format!("等待结束：{reason}")
                    } else {
                        format!("等待超时（{reason}）")
                    };
                }
                Err(error) => self.note = format!("等待失败：{}", error.message),
            },
            "put" => match result {
                Ok(value) => {
                    let path = value
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.note = format!("文件已放到 {path}");
                }
                Err(error) => self.note = format!("发送文件失败：{}", error.message),
            },
            "put-image" => match result {
                Ok(value) => {
                    let path = value
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let pane = self.input_pane();
                    let bytes = self
                        .panes
                        .get(&pane)
                        .map(|state| state.view.encode_paste(&path));
                    if let Some(bytes) = bytes {
                        self.send(Command::Input { pane, bytes });
                    }
                    self.note = format!("图片已发送，路径 {path}");
                }
                Err(error) => self.note = format!("发送图片失败：{}", error.message),
            },
            "templates" => match result {
                Ok(value) => {
                    let items = value
                        .get("templates")
                        .and_then(Value::as_array)
                        .map(|templates| {
                            templates
                                .iter()
                                .filter_map(|template| {
                                    let name =
                                        template.get("name").and_then(Value::as_str)?.to_string();
                                    let description = template
                                        .get("description")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default();
                                    Some(ChooserItem {
                                        label: if description.is_empty() {
                                            name.clone()
                                        } else {
                                            format!("{name} — {description}")
                                        },
                                        value: name,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    self.open_chooser(Chooser {
                        title: "templates（Enter 在新 tab 里运行）".to_string(),
                        items,
                        selected: 0,
                        kind: ChooserKind::Templates,
                    });
                }
                Err(error) => self.note = format!("模板列表失败：{}", error.message),
            },
            "machines" => match result {
                Ok(value) => {
                    self.machines = parse_machines(&value);
                    let items = self
                        .machines
                        .iter()
                        .map(|machine| ChooserItem {
                            label: format!(
                                "{} [{}] {}",
                                machine.name, machine.state, machine.target
                            ),
                            value: machine.name.clone(),
                        })
                        .collect();
                    self.open_chooser(Chooser {
                        title: "machines · Enter/n 新 pane · a 添加 · c 连接 · r 改名 · d 移除"
                            .to_string(),
                        items,
                        selected: 0,
                        kind: ChooserKind::Machines,
                    });
                }
                Err(error) => self.note = format!("机器列表失败：{}", error.message),
            },
            "machine.add" => match result {
                Ok(value) => {
                    let name = value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.note = format!("已添加 {name}");
                    self.call_rpc("machines", "machine.list", json!({}));
                }
                Err(error) => self.note = format!("添加失败：{}", error.message),
            },
            "machine.rm" => match result {
                Ok(_) => {
                    self.note = "机器已移除".to_string();
                    self.call_rpc("machines", "machine.list", json!({}));
                }
                Err(error) => self.note = format!("移除失败：{}", error.message),
            },
            "machine.connect" => match result {
                Ok(value) => {
                    let state = value
                        .get("state")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    self.note = format!("机器状态 {state}");
                    self.call_rpc("machines", "machine.list", json!({}));
                }
                Err(error) => self.note = format!("连接失败：{}", error.message),
            },
            "machine.rename" => match result {
                Ok(_) => {
                    self.note = "机器已改名".to_string();
                    self.call_rpc("machines", "machine.list", json!({}));
                }
                Err(error) => self.note = format!("改名失败：{}", error.message),
            },
            "send" => match result {
                Ok(_) => self.note = "已发送（等 Agent 开始这一轮）".to_string(),
                Err(error) => self.note = format!("发送失败：{}", error.message),
            },
            "interrupt" => match result {
                Ok(value) => {
                    let state = value
                        .get("status")
                        .and_then(|status| status.get("state"))
                        .and_then(Value::as_str)
                        .unwrap_or("done");
                    self.note = format!("已中断，当前状态 {state}");
                }
                Err(error) => self.note = format!("中断失败：{}", error.message),
            },
            "keep" => match result {
                Ok(value) => {
                    let keep = value.get("keep").and_then(Value::as_bool).unwrap_or(false);
                    self.note = format!("keep = {keep}");
                }
                Err(error) => self.note = format!("keep 失败：{}", error.message),
            },
            "rm" => match result {
                Ok(_) => self.note = "终端记录已移除".to_string(),
                Err(error) => self.note = format!("移除失败：{}", error.message),
            },
            "label" => match result {
                Ok(_) => {}
                Err(error) => {
                    self.note = format!("命名失败：{}", error.message);
                    self.dirty = true;
                }
            },
            "kill" => match result {
                Ok(value) => {
                    let _ = value;
                    self.note = "程序已结束（prefix c 新 tab 继续）".to_string();
                }
                Err(error) => self.note = format!("结束失败：{}", error.message),
            },
            _ => {}
        }
    }

    fn request_doc(&mut self, tag: &str) {
        self.call_rpc(tag, "doc.get", json!({ "name": DOC_NAME }));
    }

    fn push_layout(&mut self) {
        let state = self.snapshot();
        let Ok(value) = serde_json::to_value(&state) else {
            return;
        };
        self.layout_dirty = false;
        self.layout_since = None;
        self.call_rpc(
            "doc.set",
            "doc.set",
            json!({ "name": DOC_NAME, "value": value, "version": self.doc_version }),
        );
    }

    fn mark_layout(&mut self) {
        self.layout_dirty = true;
        self.layout_since = Some(Instant::now());
        self.dirty = true;
    }

    pub fn snapshot(&self) -> SavedState {
        let mut panes = BTreeMap::new();
        for (id, pane) in &self.panes {
            panes.insert(
                id.to_string(),
                SavedPane {
                    terminal: pane.terminal.clone(),
                    name: pane.name.clone(),
                },
            );
        }
        SavedState {
            schema_version: SavedState::SCHEMA,
            active_workspace: self.active_ws,
            workspaces: self
                .workspaces
                .iter()
                .map(|workspace| SavedWorkspace {
                    id: workspace.id,
                    label: workspace.label.clone(),
                    active_tab: workspace.active,
                    tabs: workspace
                        .tabs
                        .iter()
                        .map(|tab| SavedTab {
                            id: tab.id,
                            label: tab.label.clone(),
                            focus: tab.focus,
                            tree: SavedNode::from(&tab.tree),
                        })
                        .collect(),
                })
                .collect(),
            panes,
        }
    }

    /// 恢复布局：Pane 重新编号，终端地址照旧，界面尺寸交给 reconcile。
    fn restore(&mut self, state: SavedState) {
        for pane in self.panes.keys().copied().collect::<Vec<_>>() {
            self.send(Command::Close { pane });
        }
        self.panes.clear();
        self.workspaces.clear();
        self.zoom = false;
        self.copy = None;

        let mut mapping: BTreeMap<u32, u32> = BTreeMap::new();
        let mut counter = 0u32;
        for saved in &state.workspaces {
            let workspace_id = self.alloc_workspace();
            let mut workspace = Workspace {
                id: workspace_id,
                label: saved.label.clone(),
                tabs: Vec::new(),
                active: saved.active_tab,
            };
            for saved_tab in &saved.tabs {
                let mut node = saved_tab.tree.clone();
                renumber_node(&mut node, &mut mapping, &mut counter);
                if node.panes().is_empty() {
                    continue;
                }
                let tree: SplitNode = (&node).into();
                let tab_id = self.alloc_tab();
                let focus = mapping
                    .get(&saved_tab.focus)
                    .copied()
                    .or_else(|| tree.panes().first().map(|pane| pane.0))
                    .unwrap_or_default();
                workspace.tabs.push(Tab {
                    id: tab_id,
                    label: saved_tab.label.clone(),
                    tree,
                    focus,
                });
                for original in saved_tab.tree.panes() {
                    let Some(fresh) = mapping.get(&original).copied() else {
                        continue;
                    };
                    let saved_pane = state.panes.get(&original.to_string());
                    self.spawn_pane(fresh);
                    if let Some(pane) = self.panes.get_mut(&fresh) {
                        pane.terminal = saved_pane.and_then(|pane| pane.terminal.clone());
                        pane.name = saved_pane.and_then(|pane| pane.name.clone());
                    }
                }
            }
            if workspace.tabs.is_empty() {
                continue;
            }
            if workspace.active >= workspace.tabs.len() {
                workspace.active = workspace.tabs.len() - 1;
            }
            self.workspaces.push(workspace);
        }
        if self.workspaces.is_empty() {
            return;
        }
        self.next_pane = self.next_pane.max(counter);
        self.active_ws = state.active_workspace.min(self.workspaces.len() - 1);
        self.layout_dirty = false;
        self.layout_since = None;
    }

    /// 用终端列表补齐 Pane 上还没从事件里拿到的名字。
    fn sync_terminals(&mut self) {
        for (id, pane) in &mut self.panes {
            let _ = id;
            let Some(address) = pane.terminal.clone() else {
                continue;
            };
            let Some(terminal) = self
                .terminals
                .iter()
                .find(|terminal| terminal.address == address)
            else {
                continue;
            };
            if pane.title.is_empty() && !terminal.title.is_empty() {
                pane.title = terminal.title.clone();
            }
            if pane.name.is_none() {
                pane.name = terminal.name.clone();
            }
            if pane.agent.status.is_none() {
                pane.agent.set_status(terminal.status.clone());
            }
        }
    }

    fn apply_event(&mut self, event: TerminalEvent) {
        if event.seq > self.events_seen {
            self.events_seen = event.seq;
        }
        let target = event.terminal.clone();
        match event.kind.as_str() {
            "size" => {
                let cols = event.data.get("cols").and_then(Value::as_u64);
                let rows = event.data.get("rows").and_then(Value::as_u64);
                if let (Some(cols), Some(rows)) = (cols, rows) {
                    self.update_terminal(target.as_deref(), |terminal| {
                        terminal.cols = cols as u16;
                        terminal.rows = rows as u16;
                    });
                    if let Some(pane) = target
                        .as_deref()
                        .and_then(|terminal| self.pane_by_terminal(terminal))
                    {
                        if let Some(state) = self.panes.get_mut(&pane) {
                            let _ =
                                state
                                    .view
                                    .resize(cols as u16, rows as u16, CELL_PX.0, CELL_PX.1);
                            state.mark_resized((cols as u16, rows as u16));
                        }
                    }
                    self.dirty = true;
                }
            }
            "title" => {
                let title = event
                    .data
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.update_terminal(target.as_deref(), |terminal| terminal.title = title.clone());
                if let Some(pane) = target
                    .as_deref()
                    .and_then(|terminal| self.pane_by_terminal(terminal))
                {
                    if let Some(state) = self.panes.get_mut(&pane) {
                        state.title = title;
                    }
                }
                self.dirty = true;
            }
            "labels" => {
                if let Some(pane) = target
                    .as_deref()
                    .and_then(|terminal| self.pane_by_terminal(terminal))
                {
                    let name = event
                        .data
                        .get("labels")
                        .and_then(|labels| labels.get("hctui"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .filter(|name| !name.is_empty());
                    if let Some(state) = self.panes.get_mut(&pane) {
                        state.name = name;
                    }
                    self.dirty = true;
                }
            }
            "foreground" => {
                if let Some(pane) = target
                    .as_deref()
                    .and_then(|terminal| self.pane_by_terminal(terminal))
                {
                    let program = event
                        .data
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let cwd = event
                        .data
                        .get("cwd")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if let Some(state) = self.panes.get_mut(&pane) {
                        state.foreground = program;
                        if cwd.is_some() {
                            state.cwd = cwd;
                        }
                    }
                    self.dirty = true;
                }
            }
            "status" => {
                // data.status 是 `{state, kind?, pct?, msg?}`；旧版直接给字符串。
                let status = event
                    .data
                    .get("status")
                    .and_then(|status| status.get("state").and_then(Value::as_str))
                    .or_else(|| event.data.get("status").and_then(Value::as_str))
                    .map(str::to_string);
                self.update_terminal(target.as_deref(), |terminal| {
                    terminal.status = status.clone()
                });
                if let Some(pane) = target
                    .as_deref()
                    .and_then(|terminal| self.pane_by_terminal(terminal))
                {
                    let before = self
                        .panes
                        .get(&pane)
                        .map(|state| state.agent.badge())
                        .unwrap_or(Badge::None);
                    if let Some(state) = self.panes.get_mut(&pane) {
                        state.agent.set_status(status);
                    }
                    self.notify_attention(pane, before);
                }
                self.dirty = true;
            }
            "turn" => {
                let turn = event.data.get("turn").and_then(Value::as_u64).unwrap_or(0);
                if let Some(pane) = target
                    .as_deref()
                    .and_then(|terminal| self.pane_by_terminal(terminal))
                {
                    let before = self
                        .panes
                        .get(&pane)
                        .map(|state| state.agent.badge())
                        .unwrap_or(Badge::None);
                    if let Some(state) = self.panes.get_mut(&pane) {
                        state.agent.observe_turn(turn);
                    }
                    self.notify_attention(pane, before);
                }
                self.dirty = true;
            }
            "created" | "restarted" => {
                if let Some(terminal) = TerminalSummary::parse(&event.data) {
                    match self
                        .terminals
                        .iter_mut()
                        .find(|existing| existing.address == terminal.address)
                    {
                        Some(existing) => *existing = terminal,
                        None => self.terminals.push(terminal),
                    }
                    self.dirty = true;
                }
            }
            "finished" | "removed" | "lost" => {
                if let Some(target) = target.as_deref() {
                    self.update_terminal(Some(target), |terminal| terminal.live = false);
                }
                self.dirty = true;
            }
            "doc" => {
                // 只关心自己的布局文档；别的客户端改了就先同步版本号。
                let name = event
                    .data
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if name == DOC_NAME && self.doc_loaded {
                    self.request_doc("doc.remote");
                }
            }
            "missed" => {
                self.note = "resynced after missed events".to_string();
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// Agent 进入 blocked / done 且当前不可见时提示一次。
    fn notify_attention(&mut self, pane: u32, before: Badge) {
        let after = self
            .panes
            .get(&pane)
            .map(|state| state.agent.badge())
            .unwrap_or(Badge::None);
        if after == before || !matches!(after, Badge::Blocked | Badge::Done) {
            return;
        }
        if self.is_visible(pane) {
            self.mark_seen(pane);
            return;
        }
        let label = self
            .panes
            .get(&pane)
            .map(Pane::label)
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| format!("pane {pane}"));
        let text = format!("{label} · {}", after.text());
        match self.config.notifications {
            Delivery::Off => {}
            Delivery::Toast => {
                self.toast = Some(Toast {
                    text: text.clone(),
                    until: Instant::now() + TOAST_LIFETIME,
                });
            }
            Delivery::System => {
                if std::env::var_os("DISPLAY").is_some()
                    || std::env::var_os("WAYLAND_DISPLAY").is_some()
                {
                    let _ = std::process::Command::new("notify-send")
                        .arg("hc-tui")
                        .arg(&text)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
                self.toast = Some(Toast {
                    text,
                    until: Instant::now() + TOAST_LIFETIME,
                });
            }
        }
        self.note = format!("{label} 需要你看一眼（prefix a 跳过去）");
        self.play_sound(after);
        self.dirty = true;
    }

    fn update_terminal(
        &mut self,
        address: Option<&str>,
        change: impl FnOnce(&mut TerminalSummary),
    ) {
        let Some(address) = address else {
            return;
        };
        if let Some(terminal) = self
            .terminals
            .iter_mut()
            .find(|terminal| terminal.address == address)
        {
            change(terminal);
        }
    }

    /// 侧栏用的 Agent 视图：按配置排序，只保留有明确状态的 Pane。
    pub fn agent_rows(&self) -> Vec<(u32, Badge, String)> {
        let mut rows: Vec<(u32, Badge, String)> = self
            .panes
            .iter()
            .filter_map(|(id, pane)| {
                let badge = pane.agent.badge();
                (badge != Badge::None).then(|| (*id, badge, pane.label()))
            })
            .collect();
        match self.config.agent_sort {
            AgentSort::Priority => rows.sort_by_key(|(id, badge, _)| (*badge, *id)),
            AgentSort::Spaces => rows.sort_by_key(|(id, _, _)| {
                self.locate(*id)
                    .map(|(workspace, tab)| (workspace, tab, *id))
                    .unwrap_or((usize::MAX, usize::MAX, *id))
            }),
        }
        rows
    }

    /// 一个工作区里有多少待处理 Agent。
    pub fn attention_count(&self, workspace: usize) -> usize {
        self.workspaces
            .get(workspace)
            .map(|workspace| {
                workspace
                    .panes()
                    .iter()
                    .filter(|pane| {
                        self.panes
                            .get(&pane.0)
                            .is_some_and(|state| state.agent.wants_attention())
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn shutdown(&mut self) {
        if let Some(net) = self.net.take() {
            drop(net);
        }
    }

    fn send(&self, command: Command) {
        if let Some(net) = self.net.as_ref() {
            net.send(command);
        }
    }
}

fn parse_machines(value: &Value) -> Vec<MachineInfo> {
    value
        .get("machines")
        .and_then(Value::as_array)
        .map(|machines| {
            machines
                .iter()
                .filter_map(|machine| {
                    let name = machine.get("name").and_then(Value::as_str)?.to_string();
                    Some(MachineInfo {
                        name,
                        target: machine
                            .get("target")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        state: machine
                            .get("state")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                        host: machine
                            .get("host")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 读剪贴板里的 PNG。`None` 表示环境里没有可用的剪贴板工具或内容不是图片。
fn clipboard_image() -> Option<Vec<u8>> {
    let tools: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pngpaste", &["-"])]
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        &[("wl-paste", &["--no-newline", "--type", "image/png"])]
    } else {
        &[
            (
                "xclip",
                &["-selection", "clipboard", "-t", "image/png", "-o"],
            ),
            ("wl-paste", &["--no-newline", "--type", "image/png"]),
        ]
    };
    for (program, args) in tools {
        let Ok(output) = std::process::Command::new(program).args(*args).output() else {
            continue;
        };
        if output.status.success() && output.stdout.starts_with(b"\x89PNG") {
            return Some(output.stdout);
        }
    }
    None
}

fn prev_char_boundary(text: &str, index: usize) -> usize {
    text[..index]
        .char_indices()
        .next_back()
        .map(|(position, _)| position)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, index: usize) -> usize {
    text[index..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| index + offset)
        .unwrap_or(text.len())
}

/// 自定义命令交给平台 shell 执行。
fn shell_argv(command: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            "cmd".to_string(),
            "/d".to_string(),
            "/c".to_string(),
            command.to_string(),
        ]
    } else {
        vec!["sh".to_string(), "-c".to_string(), command.to_string()]
    }
}

fn order(a: (u16, u16), b: (u16, u16)) -> ((u16, u16), (u16, u16)) {
    if (a.1, a.0) <= (b.1, b.0) {
        (a, b)
    } else {
        (b, a)
    }
}

fn to_inner(inner: Rect, mouse: MouseEvent) -> (u16, u16) {
    (
        mouse
            .column
            .saturating_sub(inner.x)
            .min(inner.width.saturating_sub(1)),
        mouse
            .row
            .saturating_sub(inner.y)
            .min(inner.height.saturating_sub(1)),
    )
}

fn borders_for(
    outer: Rect,
    leaves: &[(PaneId, Rect)],
    mode: PaneBorders,
    gaps: bool,
) -> ratatui::widgets::Borders {
    use ratatui::widgets::Borders;
    let mut borders = match mode {
        PaneBorders::Off => Borders::NONE,
        PaneBorders::Always => Borders::ALL,
        PaneBorders::Auto => {
            if leaves.len() > 1 {
                Borders::ALL
            } else {
                Borders::NONE
            }
        }
    };
    if gaps || borders.is_empty() {
        return borders;
    }
    for (_, other) in leaves {
        if other == &outer {
            continue;
        }
        let vertical_overlap = outer.y < other.y + other.height && other.y < outer.y + outer.height;
        let horizontal_overlap = outer.x < other.x + other.width && other.x < outer.x + outer.width;
        if vertical_overlap {
            if other.x + other.width == outer.x {
                borders.remove(Borders::LEFT);
            }
            if other.x == outer.x + outer.width {
                borders.remove(Borders::RIGHT);
            }
        }
        if horizontal_overlap {
            if other.y + other.height == outer.y {
                borders.remove(Borders::TOP);
            }
            if other.y == outer.y + outer.height {
                borders.remove(Borders::BOTTOM);
            }
        }
    }
    borders
}

fn inner_rect(outer: Rect, borders: ratatui::widgets::Borders) -> Rect {
    use ratatui::widgets::Borders;
    let left = u16::from(borders.contains(Borders::LEFT));
    let right = u16::from(borders.contains(Borders::RIGHT));
    let top = u16::from(borders.contains(Borders::TOP));
    let bottom = u16::from(borders.contains(Borders::BOTTOM));
    if outer.width <= left + right || outer.height <= top + bottom {
        return Rect::new(outer.x + left, outer.y + top, 0, 0);
    }
    Rect::new(
        outer.x + left,
        outer.y + top,
        outer.width - left - right,
        outer.height - top - bottom,
    )
}

fn contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

pub fn run(config: Config) -> Result<()> {
    let mouse = config.mouse_capture;
    let frame = Duration::from_millis(1000 / u64::from(config.render_fps.clamp(1, 240)));
    let mut terminal = tui::init_with(mouse)?;
    let _guard = tui::Guard;
    let mut app = App::new(config)?;
    let mut next_draw = Instant::now();

    loop {
        app.pump();
        app.tick();

        let mut wait = next_draw.saturating_duration_since(Instant::now());
        if let Some(deadline) = app.wake_at() {
            wait = wait.min(deadline.saturating_duration_since(Instant::now()));
        }
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(key) => app.on_key(key),
                Event::Paste(text) => app.on_paste(text),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                Event::Resize(_, _) => app.mark_dirty(),
                _ => {}
            }
        }
        app.pump();
        app.tick();

        if app.take_dirty() || Instant::now() >= next_draw {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            next_draw = Instant::now() + frame;
        }
        app.reconcile();

        if app.should_quit() {
            break;
        }
    }
    app.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_rect_follows_the_drawn_borders() {
        use ratatui::widgets::Borders;
        let outer = Rect::new(2, 3, 20, 10);
        assert_eq!(inner_rect(outer, Borders::ALL), Rect::new(3, 4, 18, 8));
        assert_eq!(inner_rect(outer, Borders::NONE), outer);
        assert_eq!(
            inner_rect(outer, Borders::LEFT | Borders::RIGHT),
            Rect::new(3, 3, 18, 10)
        );
    }

    #[test]
    fn shared_edges_disappear_without_gaps() {
        use ratatui::widgets::Borders;
        let leaves = vec![
            (PaneId(1), Rect::new(0, 0, 10, 10)),
            (PaneId(2), Rect::new(10, 0, 10, 10)),
        ];
        let left = borders_for(leaves[0].1, &leaves, PaneBorders::Auto, false);
        let right = borders_for(leaves[1].1, &leaves, PaneBorders::Auto, false);
        assert!(!left.contains(Borders::RIGHT));
        assert!(!right.contains(Borders::LEFT));
        assert!(left.contains(Borders::LEFT));

        let gapped = borders_for(leaves[0].1, &leaves, PaneBorders::Auto, true);
        assert!(gapped.contains(Borders::RIGHT));
    }

    #[test]
    fn a_lone_pane_has_no_border_in_auto_mode() {
        use ratatui::widgets::Borders;
        let leaves = vec![(PaneId(1), Rect::new(0, 0, 10, 10))];
        assert_eq!(
            borders_for(leaves[0].1, &leaves, PaneBorders::Auto, true),
            Borders::NONE
        );
        assert_eq!(
            borders_for(leaves[0].1, &leaves, PaneBorders::Always, true),
            Borders::ALL
        );
    }

    #[test]
    fn char_boundaries_walk_utf8_safely() {
        let text = "a宽b";
        assert_eq!(prev_char_boundary(text, 0), 0);
        assert_eq!(prev_char_boundary(text, 1), 0);
        assert_eq!(prev_char_boundary(text, 4), 1);
        assert_eq!(next_char_boundary(text, 0), 1);
        assert_eq!(next_char_boundary(text, 1), 4);
        assert_eq!(next_char_boundary(text, text.len()), text.len());
    }

    #[test]
    fn drag_selection_orders_its_corners() {
        assert_eq!(order((5, 2), (1, 1)), ((1, 1), (5, 2)));
        assert_eq!(order((1, 1), (1, 1)), ((1, 1), (1, 1)));
    }
}
