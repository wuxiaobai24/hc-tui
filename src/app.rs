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
use ratatui_core::layout::Rect;
use serde_json::{json, Value};

pub use self::form::{ActionForm, FormRow, RowKind};
use crate::chat::commands::{Kind, Local};
use crate::chat::{hook, Chat, ChatAction};
use crate::clipboard;
use crate::config::{AgentSort, CommandKind, Config, Delivery, PaneBorders, Span};
use crate::copy::CopyMode;
use crate::hc::net::{Command, NetConfig, NetEvent, NetHandle};
use crate::hc::proto::{Event as TerminalEvent, TerminalSummary};
use crate::i18n::{t, t1, t2, tv};
use crate::layout::split::{neighbor, neighbor_by, Axis, Direction, PaneId, Placement, SplitNode};
use crate::model::{
    renumber_node, Badge, FloatingPane, Launch, Pane, PaneState, SavedNode, SavedPane, SavedState,
    SavedTab, SavedWorkspace, Slot, Tab, Workspace,
};
use crate::term::view::TerminalView;
use crate::tui;
use crate::ui;

mod actions;
mod agent_tree;
mod bindings;
mod branches;
mod form;
pub(crate) mod interaction;
mod keyboard;
mod layout_sync;
mod layouts;
mod mods;
mod mouse_resize;
#[cfg(test)]
mod package_tests;
mod panels;
mod plugin;
mod renaming;
pub mod services;
mod shared_sync;
pub mod winless;

/// 分组、已有视图、终端地址、状态和显示名。
#[derive(Clone, Debug)]
pub struct NavigationRow {
    pub group: String,
    pub group_id: String,
    pub pane: Option<u32>,
    pub terminal: Option<String>,
    pub badge: Badge,
    pub label: String,
}

pub fn command_catalog() -> serde_json::Value {
    json!(actions::COMMANDS
        .iter()
        .map(|(name, about)| json!({"name":name,"about":about}))
        .collect::<Vec<_>>())
}

/// 内置动作加插件动作；插件动作带来源、上下文与参数 schema。
pub fn command_catalog_for(config: &crate::config::Config) -> serde_json::Value {
    let registry = crate::plugin::registry::Registry::load(config);
    let mut entries: Vec<serde_json::Value> = actions::COMMANDS
        .iter()
        .map(|(name, about)| json!({"name":name,"about":about}))
        .collect();
    for action in registry.actions() {
        entries.push(json!({
            "name": action.name,
            "about": action.title,
            "source": format!("plugin:{}", action.package),
            "contexts": action.contexts,
            "args": action.args,
        }));
    }
    json!(entries)
}

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
/// 事件循环检查一次退出信号的间隔；SIGTERM/SIGHUP 只置标志。
const TERMINATION_POLL: Duration = Duration::from_millis(50);
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
            Self::Connecting => t("connecting").to_string(),
            Self::Ready { version, core } => format!("hc {version} · {core}"),
            Self::Reconnecting { .. } => t("reconnecting").to_string(),
            Self::Failed { .. } => t("disconnected").to_string(),
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
    RenameTab { workspace: u32, tab: u32 },
    RenameWorkspace(usize),
    RenamePane(u32),
    RenameAgent(String),
    SendText(u32),
    SendKeys(u32),
    WaitOutput(u32),
    PutFile(u32),
    AddMachine,
    RenameMachine(String),
    CloseTab(usize),
    CloseWorkspace(usize),
    KillPane(u32),
    Command,
}

impl PromptKind {
    /// 浮层顶部的标题，跟 herdr 的面板标题一致。
    pub fn title(&self) -> &'static str {
        t(match self {
            Self::Command => "command",
            Self::NewTab => "new tab",
            Self::NewWorkspace => "new workspace",
            Self::RenameTab { .. } => "rename tab",
            Self::RenameWorkspace(_) => "rename workspace",
            Self::RenamePane(_) => "rename pane",
            Self::RenameAgent(_) => "rename agent",
            Self::SendText(_) => "send text",
            Self::SendKeys(_) => "send keys",
            Self::WaitOutput(_) => "wait for output",
            Self::PutFile(_) => "send file",
            Self::AddMachine => "add machine",
            Self::RenameMachine(_) => "rename machine",
            Self::CloseTab(_) => "close tab",
            Self::CloseWorkspace(_) => "close workspace",
            Self::KillPane(_) => "kill terminal",
        })
    }

    /// 只有这些动作需要输入 `y` 再回车。
    pub fn needs_confirm(&self) -> bool {
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
    /// 通知级别：`info` / `warn` / `error`，只影响绘制颜色。
    pub level: String,
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

/// Keybinds 浮层：`/` 过滤、方向键或 j/k 滚动。
#[derive(Debug, Clone)]
pub struct HelpOverlay {
    pub query: String,
    pub selected: usize,
    pub scroll: usize,
}

/// 帮助面板里的一行；`section` 为真的行只是标题。
#[derive(Debug, Clone)]
pub struct HelpRow {
    pub section: bool,
    pub key: String,
    pub text: String,
}

/// 设置浮层：主题、状态图标、通知、声音。
#[derive(Debug, Clone)]
pub struct SettingsOverlay {
    pub section: usize,
    pub selected: usize,
}

/// Go to 浮层：搜索并跳转到任意终端。
#[derive(Debug, Clone)]
pub struct GotoOverlay {
    pub query: String,
    pub selected: usize,
}

/// Go to 里的一行：工作区标题、已有视图的 Pane，或其它客户端创建的终端。
#[derive(Debug, Clone)]
pub struct GotoRow {
    pub pane: Option<u32>,
    pub terminal: Option<String>,
    pub text: String,
    pub right: String,
    pub section: bool,
    /// 选中后显示在底部的面包屑与目录。
    pub context: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChooserKind {
    Templates,
    Machines,
    Commands,
    Actions,
    Grants,
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
/// schema 4 起带外框树与窗口覆盖层位置；缺失字段按空处理。
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct LocalState {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    seen: BTreeMap<String, u64>,
    #[serde(default)]
    folded_agent_groups: std::collections::HashSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mods_chrome: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    mods_windows: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mods_grants: Option<serde_json::Value>,
    /// 已经按包记录自动停靠过的模块；用户主动撤下的包留在这里，不再自动停靠。
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    mods_docked: std::collections::BTreeSet<String>,
}

/// 普通模式下鼠标拖选。
#[derive(Debug, Clone, Copy)]
pub struct Drag {
    pub pane: u32,
    pub start: (u16, u16),
    pub current: (u16, u16),
}

/// 拖浮动 Pane：抓标题行移动，抓右下角改尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatingDragMode {
    Move,
    Resize,
}

#[derive(Debug, Clone, Copy)]
pub struct FloatingDrag {
    pub pane: u32,
    pub mode: FloatingDragMode,
    /// 按下时鼠标在 Pane 矩形里的偏移（列, 行）。
    pub grab: (u16, u16),
}

/// 手机切换面板里可点的条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileTarget {
    Workspace(usize),
    Tab(usize),
    Agent(u32),
    Terminal(usize),
    Action(MobileAction),
}

/// 面板 `menu` 段里的动作：手机上没有前缀键，这些动作只能靠点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileAction {
    NewTab,
    SplitRight,
    SplitDown,
    ClosePane,
    CopyMode,
    NextAgent,
    Actions,
    Chat,
    Detach,
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
    /// 外框树：本机区域布局，模块区域与内置界面的排列。
    pub chrome: crate::mods::chrome::Chrome,
    /// `window` 覆盖层位置与 `key`；schema 4 起随本地状态保存。
    pub mods_windows: std::collections::BTreeMap<String, serde_json::Value>,
    /// 模块运行时：表面、代际、授予、请求。
    pub mods: crate::mods::ModRuntime,
    /// 已自动停靠过的模块 ID；停用后移除，用户 `mods.undock` 不再自动恢复。
    pub mods_docked: std::collections::BTreeSet<String>,
    /// 模块可见的数据版本；变化时递增并发 `state.changed`。
    pub mod_revision: u64,
    /// 引擎线程置位的唤醒标记；只置位，不触碰 App 状态。
    pub mod_wake: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 外框与表面已同步的树快照，避免每轮重建。
    pub mod_chrome_sync: Option<String>,
    /// 本轮 `state.changed` 涉及的域；`mod_all_domains` 为真时给全集。
    pub mod_changed_domains: std::collections::BTreeSet<&'static str>,
    /// 下一轮 `state.changed` 报全部域（重连、结构整体变化时使用）。
    pub mod_all_domains: bool,
    /// 待答的 `ask` 调用：宿主已把 props 交付目标表面。
    pub mod_asks: Vec<crate::mods::ModAsk>,
    /// 已登记的通知队列，供 `notifications` 域与 `dismiss` 使用。
    pub mod_notifications: Vec<crate::mods::ModNotification>,
    /// 内建 toast 当前显示的通知 ID；`dismiss` 按 id 只撤对应条目。
    pub toast_notification: Option<String>,
    /// 队列有变化，下一轮把 toast props 推给提示表面。
    pub mod_toast_dirty: bool,
    /// 最近一次 `templates.list` 的结果缓存。
    pub mod_templates: Vec<serde_json::Value>,
    /// `run` 打开确认或表单时记下的 (request, 模块)。
    pub mod_confirm: Option<(String, String)>,
    /// `run` 发起后仍在飞的 core RPC：tag → (request, 模块)。
    pub mod_action_rpc: std::collections::BTreeMap<String, (String, String)>,
    /// 插件注册表：配置目录与安装存储的当前代际。
    pub plugins: crate::plugin::registry::Registry,
    /// 服务作业台账：写进布局文档 `services` 段，旧读者忽略。
    pub services: Vec<crate::model::SavedService>,
    /// 本次对账的时机，决定 `on = "install"` 的服务是否拉起。
    service_trigger: services::Trigger,
    /// 等 `terminal.run` 回复的服务规划：`<package>.<service>` → (规划, 代际)。
    service_pending: HashMap<String, (crate::plugin::service::Plan, u64)>,
    /// 物化失败的服务：`missing_program` 等，`packages list` 与状态行共用。
    pub service_issues: Vec<String>,
    /// 注册表加载时的磁盘指纹；变化时无需重启即可重新加载。
    pub plugin_revision: u64,
    /// 上一次主动扫描磁盘的时刻。
    pub plugin_scan_at: Option<std::time::Instant>,
    /// 插件动作运行器。
    pub plugin_runner: crate::plugin::runner::Runner,
    /// 最近一次插件调用的请求 ID，控制通道回包时带上。
    pub plugin_request: Option<String>,
    /// 事件流发布端：控制 socket 的订阅连接从这里取数据。
    pub event_publisher: crate::control::Publisher,
    /// 落在 Pane 里的插件终端：Pane → 入口与代际引用。
    pub plugin_terminals: HashMap<u32, plugin::PluginTerminal>,
    /// 终端入口请求的进展，`plugins pane status` 查询。
    pub plugin_panes: HashMap<String, plugin::PaneRequest>,
    /// 请求 ID 的插入顺序，超过 128 条时丢弃最早的。
    pub plugin_pane_order: Vec<String>,
    /// 等目标机器程序探测返回的入口：请求 ID → 规划。
    pub plugin_pending: HashMap<String, crate::plugin::terminal::PreparedTerminal>,
    /// 终端入口请求编号。
    pub next_plugin_pane: u64,
    /// 上一个进程留下的终端 pin（core 地址）；第一次终端快照时对账。
    pub plugin_startup_pins: Vec<String>,
    /// 有待发的 `state.changed`。
    pub mods_dirty: bool,
    /// 模块通过 `setAgentOrder` 设定的 agent 顺序（按终端地址）。
    pub mod_agent_order: Option<Vec<String>>,
    /// 等待 core RPC 应答的宿主调用：tag → (方法, 应答通道)。
    pub pending_mod_rpc: std::collections::HashMap<String, (String, Option<nsh_engine::HostReply>)>,
    /// 当前持有键盘焦点的模块区域 ID；`None` 时焦点在 Pane。
    pub focused_mod: Option<String>,
    /// 本轮绘制记录的模块区域；图形遮挡与焦点循环使用。
    pub mod_regions: Vec<crate::mods::chrome::ModRegion>,
    /// 本轮布局算出的提示层覆盖区；外框没有 `toasts` 节点时为 `None`。
    pub mod_toasts: Option<Rect>,
    pub tab_bar: Rect,
    /// Tab 栏右侧 `+` 按钮的区域。
    pub tab_add: Rect,
    /// 侧栏 `new` / `menu` 按钮的区域。
    pub sidebar_new: Rect,
    pub sidebar_menu: Rect,
    /// Tab 栏上每一段的点击区域，值是当前工作区里的 Tab 下标。
    pub tab_hits: Vec<(usize, Rect)>,
    /// 侧栏里 Agent 行的点击区域。
    pub agent_hits: Vec<(u32, Rect)>,
    pub agent_group_hits: Vec<(String, Rect)>,
    pub folded_agent_groups: std::collections::HashSet<String>,
    pub agent_sort_hit: Rect,
    /// 侧栏里工作区行的点击区域。
    pub workspace_hits: Vec<(usize, Rect)>,
    /// 侧栏里其它客户端终端的点击区域。
    pub terminal_hits: Vec<(String, Rect)>,
    pub zoom: bool,
    /// 浮动层是否收起；pin 的浮动 Pane 不受影响。
    pub floating_hidden: bool,
    /// 正在拖动的浮动 Pane。
    pub floating_drag: Option<FloatingDrag>,
    layout_drag: Option<mouse_resize::LayoutDrag>,
    pub resize_mode: bool,
    pub move_mode: bool,
    pub workspace_mode: bool,
    pub prompt: Option<Prompt>,
    /// 按动作的 args schema 生成的参数表单；打开期间不吃其它分发。
    pub form: Option<ActionForm>,
    pub copy: Option<CopyMode>,
    /// 打开的 Chat 视图；覆盖内容区，但 Pane 照常 attach。
    pub chat: Option<Chat>,
    pub drag: Option<Drag>,
    pub toast: Option<Toast>,
    pub popup: Option<PopupPane>,
    pub reader: Option<Reader>,
    pub help: Option<HelpOverlay>,
    pub settings: Option<SettingsOverlay>,
    pub goto: Option<GotoOverlay>,
    pub palette: bool,
    shown_original: HashMap<u32, Option<String>>,
    held_done: Option<(String, u64)>,
    initial_tasks: HashMap<u32, String>,
    program_right_click: std::collections::HashSet<String>,
    terminal_display: Option<crate::hc::proto::Display>,
    pub chooser: Option<Chooser>,
    pub machines: Vec<MachineInfo>,
    machine_refresh: Option<Instant>,
    pub machine_hits: Vec<(String, Rect)>,
    /// `tab_bar_right` 里 `cmd:` 条目的最近一次输出。
    pub command_output: HashMap<String, String>,
    /// 侧栏滚动偏移（行）。
    pub sidebar_scroll: usize,
    pub space_scroll: usize,
    pub spaces_area: Rect,
    /// 侧栏区域，鼠标滚轮用。
    pub sidebar_area: Rect,
    /// 上次绘制进入了手机单列布局。
    pub mobile: bool,
    pub keyboard_mode: interaction::KeyboardMode,
    pub pointer_targets: Vec<(interaction::PointerTarget, Rect)>,
    pub menu_hit: Rect,
    /// 内置 toast 的绘制矩形，鼠标点击命中时执行它携带的动作。
    pub toast_hit: Rect,
    pub pane_drag_hits: Vec<(u32, Rect)>,
    pub pane_menu_hits: Vec<(u32, Rect)>,
    pointer_drag: Option<interaction::PointerDrag>,
    /// 手机顶栏上 `switch` / `close` 按钮的区域。
    pub mobile_switch: Rect,
    /// 切换面板的 `close` 按钮、行点击区域与滚动偏移。
    pub mobile_close: Rect,
    pub mobile_targets: Vec<(MobileTarget, Rect)>,
    pub mobile_scroll: usize,
    pub mobile_max_scroll: usize,
    pub switcher: bool,
    pub events_seen: u64,
    /// core 布局文档的副本：`base`/`version` 是最近一次读到的远端值。
    layout: shared_sync::Replica,
    pub doc_loaded: bool,
    shared_docs: HashMap<String, shared_sync::Replica>,
    shared_dirty: bool,
    panel_refresh: HashMap<u32, Instant>,
    shared_labels: HashMap<String, (String, String, String)>,
    pub last_pane: Option<u32>,
    /// 最近一次后台完成或需要人工介入的 Pane（`prefix+o` 跳过去）。
    pub last_attention: Option<u32>,
    /// 分支名缓存：cwd → (branch, 取到的时刻)。
    pub branches: HashMap<String, (String, Instant)>,
    branch_probes: branches::BranchProbes,
    note_until: Option<Instant>,
    note_seen: String,
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
    control: Option<Receiver<crate::control::Request>>,
}

impl App {
    pub fn new(config: Config) -> Result<Self> {
        let (sender, events) = std::sync::mpsc::channel();
        let net = NetHandle::spawn(
            NetConfig {
                session: config.session.clone(),
            },
            sender,
        );
        Self::with_network(config, Some(net), events)
    }

    /// 测试用离屏构造：不启动 `hc connect`，不碰任何 session。
    #[cfg(test)]
    pub(crate) fn offline(config: Config) -> Result<Self> {
        let (_, events) = std::sync::mpsc::channel();
        Self::with_network(config, None, events)
    }

    fn with_network(
        config: Config,
        net: Option<NetHandle>,
        events: Receiver<NetEvent>,
    ) -> Result<Self> {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let note = if config.warnings.is_empty() {
            t("starting hc connect").to_string()
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
            chrome: crate::mods::chrome::Chrome::default_tree(&config),
            mod_regions: Vec::new(),
            mod_toasts: None,
            mods_windows: std::collections::BTreeMap::new(),
            mods: crate::mods::ModRuntime::default(),
            mods_docked: std::collections::BTreeSet::new(),
            plugins: crate::plugin::registry::Registry::default(),
            services: Vec::new(),
            service_trigger: services::Trigger::default(),
            service_pending: HashMap::new(),
            service_issues: Vec::new(),
            plugin_revision: 0,
            plugin_scan_at: None,
            plugin_runner: crate::plugin::runner::Runner::new(""),
            plugin_request: None,
            event_publisher: crate::control::Publisher::default(),
            plugin_terminals: HashMap::new(),
            plugin_panes: HashMap::new(),
            plugin_pane_order: Vec::new(),
            plugin_pending: HashMap::new(),
            next_plugin_pane: 0,
            plugin_startup_pins: Vec::new(),
            mod_revision: 0,
            mod_wake: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            mod_chrome_sync: None,
            mod_changed_domains: std::collections::BTreeSet::new(),
            mod_all_domains: false,
            mod_asks: Vec::new(),
            mod_notifications: Vec::new(),
            toast_notification: None,
            mod_toast_dirty: false,
            mod_templates: Vec::new(),
            mod_confirm: None,
            mod_action_rpc: std::collections::BTreeMap::new(),
            mods_dirty: false,
            mod_agent_order: None,
            pending_mod_rpc: std::collections::HashMap::new(),
            focused_mod: None,
            tab_bar: Rect::new(0, 0, 0, 0),
            tab_add: Rect::new(0, 0, 0, 0),
            sidebar_new: Rect::new(0, 0, 0, 0),
            sidebar_menu: Rect::new(0, 0, 0, 0),
            tab_hits: Vec::new(),
            agent_hits: Vec::new(),
            agent_group_hits: Vec::new(),
            folded_agent_groups: std::collections::HashSet::new(),
            agent_sort_hit: Rect::default(),
            workspace_hits: Vec::new(),
            terminal_hits: Vec::new(),
            zoom: false,
            floating_hidden: false,
            floating_drag: None,
            layout_drag: None,
            resize_mode: false,
            move_mode: false,
            workspace_mode: false,
            prompt: None,
            form: None,
            copy: None,
            chat: None,
            drag: None,
            toast: None,
            popup: None,
            reader: None,
            help: None,
            settings: None,
            goto: None,
            palette: false,
            shown_original: HashMap::new(),
            held_done: None,
            initial_tasks: HashMap::new(),
            program_right_click: std::collections::HashSet::new(),
            terminal_display: None,
            chooser: None,
            machines: Vec::new(),
            machine_refresh: None,
            machine_hits: Vec::new(),
            command_output: HashMap::new(),
            sidebar_scroll: 0,
            space_scroll: 0,
            spaces_area: Rect::default(),
            sidebar_area: Rect::new(0, 0, 0, 0),
            mobile: false,
            keyboard_mode: interaction::KeyboardMode::Normal,
            pointer_targets: Vec::new(),
            menu_hit: Rect::default(),
            toast_hit: Rect::default(),
            pane_drag_hits: Vec::new(),
            pane_menu_hits: Vec::new(),
            pointer_drag: None,
            mobile_switch: Rect::new(0, 0, 0, 0),
            mobile_close: Rect::new(0, 0, 0, 0),
            mobile_targets: Vec::new(),
            mobile_scroll: 0,
            mobile_max_scroll: 0,
            switcher: false,
            events_seen: 0,
            layout: shared_sync::Replica {
                base: Value::Null,
                version: 0,
                pending: None,
                blocked: false,
                needs_refresh: false,
            },
            doc_loaded: false,
            shared_docs: HashMap::new(),
            shared_dirty: false,
            panel_refresh: HashMap::new(),
            shared_labels: HashMap::new(),
            last_pane: None,
            last_attention: None,
            branches: HashMap::new(),
            branch_probes: branches::BranchProbes::default(),
            note_until: None,
            note_seen: String::new(),
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
            net,
            events,
            control: None,
            config,
        };
        app.load_local_state();
        app.plugins = crate::plugin::registry::Registry::load(&app.config);
        app.plugin_revision = crate::plugin::registry::revision(&app.config);
        app.plugin_runner = crate::plugin::runner::Runner::new(app.config.plugins_state_dir());
        app.plugin_startup_pins = app
            .plugin_terminals_on_disk()
            .into_iter()
            .filter(|address| !address.starts_with("pending/"))
            .collect();
        crate::mods::store::apply_enabled(&app.config, &mut app.mods);
        for state in [app.config.plugins_state_dir(), app.config.mods_state_dir()] {
            for record in crate::store::Store::open(state).list() {
                let grants = crate::mods::store::load_grants(&app.config, &record.id);
                if !grants.is_empty() {
                    app.mods.approve_grants(&record.id, &grants);
                }
            }
        }
        if !app.restore_pending {
            app.ensure_layout();
        }
        Ok(app)
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// 收到 SIGTERM / SIGHUP：走与 `app.quit` 相同的收尾路径。
    pub fn request_quit(&mut self) {
        self.quit = true;
    }

    pub fn prefix_active(&self) -> bool {
        self.prefix
    }

    /// 底部模式行：左边标签加提示；`None` 表示不占行。
    pub fn mode_line(&self) -> Option<(String, String)> {
        if self.copy.is_some() {
            return Some((
                t("COPY").to_string(),
                t("h/j/k/l w/b/e { } move  / ? search  n/N repeat  v/space select  y/enter copy  q/esc exit")
                    .to_string(),
            ));
        }
        if self.resize_mode {
            return Some((
                t("RESIZE").to_string(),
                t("h/l width  j/k height  esc done").to_string(),
            ));
        }
        if self.move_mode {
            return Some((
                t("MOVE").to_string(),
                t("1-9 target tab  t new tab  esc back").to_string(),
            ));
        }
        if self.workspace_mode {
            return Some((
                t("NAVIGATE").to_string(),
                t("\u{2191}/\u{2193} workspace  1-9 switch  esc back").to_string(),
            ));
        }
        if self.prefix_active() {
            return Some((
                t("PREFIX").to_string(),
                t("? keybinds  s settings  c new tab  v/- split  hjkl focus  z zoom  x close  q detach")
                    .to_string(),
            ));
        }
        if let Some(line) = self.zellij_mode_line() {
            return Some(line);
        }
        if !self.note.is_empty() {
            return Some((String::new(), self.note.clone()));
        }
        if !matches!(self.connection, Connection::Ready { .. }) {
            return Some((String::new(), self.connection.label()));
        }
        None
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn take_dirty(&mut self) -> bool {
        let awake = self
            .mod_wake
            .swap(false, std::sync::atomic::Ordering::Relaxed);
        std::mem::take(&mut self.dirty) || awake
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
            let label = String::new();
            self.workspaces
                .push(Workspace::new(workspace, label, pane, tab));
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

    /// 控制 socket 的一问一答；界面与测试共用同一条分发路径。
    pub(crate) fn control_reply(&mut self, body: &Value) -> Value {
        // 启用状态可能在另一个进程里刚被改过：处理请求前先对齐磁盘。
        self.refresh_plugins_if_changed();
        match body["method"].as_str() {
            Some("snapshot") => {
                json!({"state":self.snapshot(),"connection":self.connection.label(),"terminals":self.terminals.iter().map(|t|json!({"address":t.address,"title":t.title,"status":t.status,"parent":t.parent,"pinned":t.pinned,"seen_turn":t.seen_turn,"session":t.session,"plugin":t.plugin,"plugin_terminal":t.plugin_terminal,"cwd":t.cwd})).collect::<Vec<_>>()})
            }
            Some("notify") => {
                let text = body.get("text").and_then(Value::as_str).unwrap_or("");
                if text.trim().is_empty() {
                    return json!({"error":"notify requires a text"});
                }
                let level = body.get("level").and_then(Value::as_str).unwrap_or("info");
                if !matches!(level, "info" | "warn" | "error") {
                    return json!({"error":"level must be info, warn or error"});
                }
                let action = body.get("action").cloned().filter(|value| !value.is_null());
                let target = body.get("target").and_then(Value::as_str);
                let (id, surface) = self.host_notify(text, level, action, target);
                json!({"id":id,"surface":surface,"fallback":if surface.is_none(){"builtin"}else{"toast"}})
            }
            Some("do") if self.restore_pending => json!({"error":"layout is still loading"}),
            Some("do") => {
                let command = body["command"].as_str().unwrap_or("");
                let args = body.get("args").cloned().unwrap_or(json!({}));
                // 表单开着时同名动作只在界面里完成，控制通道不重复派发。
                if self
                    .form
                    .as_ref()
                    .is_some_and(|form| form.action == command)
                {
                    return json!({"accepted":true,"confirmation_pending":true});
                }
                self.plugin_request = None;
                if let Some(package) =
                    crate::plugin::registry::disabled_package(&self.config, command)
                {
                    return json!({"error":t1("plugin {} is disabled", &package),"error_kind":"disabled"});
                }
                match self.execute_action(command, args) {
                    Ok(()) => {
                        let mut response =
                            json!({"accepted":true,"confirmation_pending":self.prompt.is_some()});
                        if let Some(request) = self.plugin_request.take() {
                            response["request"] = json!(request);
                        }
                        response
                    }
                    Err(error) => json!({"error":error.to_string()}),
                }
            }
            Some("plugins.status") => self.plugin_status(body),
            Some("plugins.cancel") => self.plugin_cancel(body),
            Some("plugins.log") => self.plugin_log(body),
            Some("plugins.pane.open") => self.plugin_pane_open(body),
            Some("plugins.pane.status") => self.plugin_pane_status(body),
            Some("plugins.pane.focus") => self.plugin_pane_focus(body),
            Some("plugins.pane.close") => self.plugin_pane_close(body),
            _ => json!({"error":"unknown request method"}),
        }
    }

    pub fn pump(&mut self) {
        self.poll_plugin_registry();
        let requests: Vec<_> = self
            .control
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for request in requests {
            let response = self.control_reply(&request.body);
            let _ = request.reply.send(response);
        }
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
        crate::theme::source::poll(self);
        self.sync_package_modules();
        self.sync_mod_surfaces();
        self.pump_mods();
        self.expire_mod_notifications(now);
        if self.restore_pending && now.duration_since(self.restore_since) >= RESTORE_TIMEOUT {
            self.restore_pending = false;
            self.ensure_layout();
            self.note = t("core returned no layout; using the default").to_string();
            self.dirty = true;
        }
        if self.toast.as_ref().is_some_and(|toast| now >= toast.until) {
            self.toast = None;
            self.toast_notification = None;
            self.dirty = true;
        }
        // note 变了就重新开始计时；过期后清掉，不给屏幕留固定状态栏。
        if self.note != self.note_seen {
            self.note_seen = self.note.clone();
            self.note_until = Some(now + TOAST_LIFETIME);
        }
        if !self.note.is_empty() && self.note_until.is_some_and(|until| now >= until) {
            self.note.clear();
            self.note_seen.clear();
            self.note_until = None;
            self.dirty = true;
        }
        if self.chat.is_some() {
            let label = self
                .chat
                .as_ref()
                .and_then(|chat| self.panes.get(&chat.pane))
                .and_then(|pane| pane.agent_session.clone());
            if let Some(chat) = self.chat.as_mut() {
                if chat.machine.is_some() {
                    chat.set_remote_session(label.as_deref());
                } else if let Some(label) = label {
                    if !chat.exact
                        || chat.path.as_ref().and_then(|path| path.to_str()) != Some(label.as_str())
                    {
                        chat.adopt(label.into());
                        self.dirty = true;
                    }
                }
                if chat.poll() {
                    self.dirty = true;
                }
            }
            if let Some((tag, params)) = self.chat.as_mut().and_then(Chat::remote_request) {
                if !self.inflight.contains_key(&tag) {
                    self.call_rpc(&tag, "file.read", params);
                }
            }
            let stale = self
                .chat
                .as_ref()
                .is_some_and(|chat| !self.panes.contains_key(&chat.pane));
            if stale {
                self.chat = None;
                self.dirty = true;
            }
        }
        self.dirty |= self.branch_probes.drain(&mut self.branches);
        self.refresh_tab_commands(now);
        self.refresh_panels(now);
        self.refresh_shared_labels();
        if self.config.restore
            && matches!(self.connection, Connection::Ready { .. })
            && self
                .machine_refresh
                .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(5))
            && !self.inflight.contains_key("machines.sync")
        {
            self.machine_refresh = Some(now);
            self.call_rpc("machines.sync", "machine.list", json!({}));
        }
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
            self.flush_layout();
            self.flush_shared();
        }
        if self.shared_dirty && !self.layout_dirty {
            self.flush_shared();
        }
        self.poll_plugins();
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
        if let Some(until) = self.note_until {
            if !self.note.is_empty() {
                deadlines.push(until);
            }
        }
        if self.seen_dirty {
            if let Some(since) = self.seen_since {
                deadlines.push(since + Duration::from_millis(500));
            }
        }
        for surface in &self.mods.surfaces {
            if let Some(deadline) = surface.view.next_frame_deadline() {
                deadlines.push(deadline);
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
        let focus_floating = self
            .workspace()
            .and_then(Workspace::tab)
            .is_some_and(|tab| tab.floating_of(focus).is_some());
        // 手机单列布局只摆当前 Pane，其余 Pane 靠 `switch` 面板跳过去。
        let leaves = if (self.zoom || self.mobile) && !focus_floating {
            vec![Placement {
                pane: PaneId(focus),
                rect: content,
                collapsed: false,
            }]
        } else {
            // Stack 的展开项始终跟着焦点走，所以这里先把它对齐再算矩形。
            if let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) {
                tab.tree.activate_in_stack(PaneId(focus));
            }
            self.workspace()
                .and_then(Workspace::tab)
                .map(|tab| tab.tree.layout(content))
                .unwrap_or_default()
        };
        let rects: Vec<(PaneId, Rect)> = leaves.iter().map(|leaf| (leaf.pane, leaf.rect)).collect();
        let mut slots = Vec::with_capacity(leaves.len() + 1);
        for leaf in &leaves {
            // 折叠成标题行的成员只留顶线，不参与相邻边共享。
            let borders = if leaf.collapsed {
                ratatui_widgets::borders::Borders::TOP
            } else {
                borders_for(
                    leaf.rect,
                    &rects,
                    self.config.pane_borders,
                    self.config.pane_gaps,
                    self.config.pane_outer_borders,
                )
            };
            slots.push(Slot {
                pane: leaf.pane.0,
                outer: leaf.rect,
                inner: inner_rect(leaf.rect, borders),
                borders,
                overlay: false,
                collapsed: leaf.collapsed,
            });
        }
        // 浮动层叠在分屏之上：pin 的排最后画，其余按 z 序。
        let mut floating = self
            .workspace()
            .and_then(Workspace::tab)
            .map(|tab| tab.floating.clone())
            .unwrap_or_default();
        floating.retain(|_| !self.floating_hidden);
        floating.sort_by_key(|entry| (entry.pinned, entry.z));
        for entry in floating {
            let outer = entry.rect(content);
            slots.push(Slot {
                pane: entry.pane,
                outer,
                inner: inner_rect(outer, ratatui_widgets::borders::Borders::ALL),
                borders: ratatui_widgets::borders::Borders::ALL,
                overlay: true,
                collapsed: false,
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
                inner: inner_rect(outer, ratatui_widgets::borders::Borders::ALL),
                borders: ratatui_widgets::borders::Borders::ALL,
                overlay: true,
                collapsed: false,
            });
        }
        for slot in &slots {
            // 折叠的 Pane 沿用上次展开时的终端尺寸，不跟着标题行缩。
            if slot.collapsed {
                continue;
            }
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
            if slot.collapsed {
                continue;
            }
            if slot.inner.width < 2 || slot.inner.height < 2 {
                continue;
            }
            let Some(pane) = self.panes.get_mut(&slot.pane) else {
                continue;
            };
            if pane.module.is_some() {
                continue;
            }
            let size = (slot.inner.width, slot.inner.height);
            if pane.needs_spawn() {
                pane.mark_spawned(size);
                let (argv, cwd, on, env) = pane
                    .launch
                    .as_ref()
                    .map(|launch| {
                        (
                            launch.argv.clone(),
                            launch.cwd.clone(),
                            launch.on.clone(),
                            launch.env.clone(),
                        )
                    })
                    .unwrap_or_default();
                spawns.push((
                    slot.pane,
                    pane.terminal.clone(),
                    argv,
                    cwd,
                    on,
                    env,
                    size,
                    !pane.transient,
                ));
            } else if pane.mark_resized(size) {
                resizes.push((slot.pane, size));
            }
        }
        for (pane, target, argv, cwd, on, env, (cols, rows), keep) in spawns {
            self.send(Command::Spawn {
                keep,
                pane,
                target,
                argv,
                cwd,
                on,
                env,
                cols,
                rows,
            });
        }
        for (pane, (cols, rows)) in resizes {
            self.send(Command::Resize { pane, cols, rows });
        }
    }

    // ---------------------------------------------------------------- 输入

    pub fn sync_terminal_theme(&mut self) {
        for pane in self.panes.values_mut() {
            if let Err(error) = pane.view.apply_theme(&self.config.theme) {
                self.note = t1("Terminal theme: {}", error);
            }
        }
        let display = crate::hc::proto::Display::from_theme(&self.config.theme);
        if self.terminal_display.as_ref() != Some(&display) {
            self.send(Command::Display(display.clone()));
            self.terminal_display = Some(display);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            let local = self.prefix
                || self.prompt.is_some()
                || self.form.is_some()
                || self.help.is_some()
                || self.settings.is_some()
                || self.goto.is_some()
                || self.reader.is_some()
                || self.chooser.is_some()
                || self.copy.is_some()
                || self.chat.is_some()
                || self.switcher
                || self.resize_mode
                || self.move_mode
                || self.workspace_mode
                || self.keyboard_mode != interaction::KeyboardMode::Normal;
            if !local {
                if self.focused_mod.is_some() {
                    self.route_key_to_mod(key);
                } else {
                    self.forward_key(key);
                }
            }
            return;
        }
        if self.form.is_some() {
            self.on_form_key(key);
            return;
        }
        if self.prompt.is_some() {
            self.on_prompt_key(key);
            return;
        }
        if self.help.is_some() {
            self.on_help_key(key);
            return;
        }
        if self.settings.is_some() {
            self.on_settings_key(key);
            return;
        }
        if self.goto.is_some() {
            self.on_goto_key(key);
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
        if self.on_zellij_key(key) {
            return;
        }
        if self.copy.is_some() {
            self.on_copy_key(key);
            return;
        }
        if self.chat.is_some() {
            match chat_image_key(key, &self.config.keys, &mut self.prefix) {
                ChatImageKey::Paste => self.paste_image(),
                ChatImageKey::Input => self.on_chat_key(key),
                ChatImageKey::Consumed => self.dirty = true,
            }
            return;
        }
        if self.switcher && key.kind == KeyEventKind::Press && key.code == KeyCode::Esc {
            self.close_switcher();
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
            // 增强键盘会报告松键；前缀只由下一次按下消费。
            if key.kind != KeyEventKind::Press {
                return;
            }
            self.prefix = false;
            self.dirty = true;
            if key.kind == KeyEventKind::Press {
                self.on_prefix_key(key);
            }
            return;
        }
        // `Esc` 关闭顶层覆盖层，覆盖层内的其它 `Esc` 交给模块。
        if key.kind == KeyEventKind::Press
            && key.code == KeyCode::Esc
            && self.focused_mod.is_some()
            && self.close_top_mod_overlay()
        {
            return;
        }
        let keys = &self.config.keys;
        if key.kind == KeyEventKind::Press && keys.prefix.matches_direct(&key) {
            self.prefix = true;
            self.dirty = true;
            return;
        }
        if key.kind == KeyEventKind::Press {
            if self
                .config
                .keys
                .entries()
                .iter()
                .any(|(name, binding)| *name != "prefix" && binding.matches_direct(&key))
            {
                self.dispatch_binding(key, false);
                return;
            }
            if let Some(index) = self.custom_command(&key, false) {
                self.run_custom(index);
                return;
            }
        }
        if self.focused_mod.is_some() {
            if key.kind == KeyEventKind::Press
                && matches!(key.code, KeyCode::Char('v' | 'V'))
                && key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
            {
                if let Ok(bytes) = clipboard::image() {
                    self.upload_image(bytes);
                    return;
                }
            }
            self.route_key_to_mod(key);
            return;
        }
        // Host text paste remains Event::Paste. Ctrl+V can request image data
        // directly when the host cannot represent it as a paste event.
        if key.kind == KeyEventKind::Press
            && matches!(key.code, KeyCode::Char('v' | 'V'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            if let Ok(bytes) = clipboard::image() {
                self.upload_image(bytes);
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
            if key.kind != KeyEventKind::Release {
                pane.view.scroll_bottom();
                self.dirty = true;
            }
            self.send(Command::Input {
                pane: pane_id,
                bytes,
            });
        }
    }

    pub fn on_paste(&mut self, text: String) {
        if let Some(form) = &mut self.form {
            if let Some(row) = form.focused_row_mut() {
                let text = text.replace(['\r', '\n'], " ");
                row.text.insert_str(row.cursor, &text);
                row.cursor += text.len();
                self.dirty = true;
            }
            return;
        }
        if let Some(prompt) = &mut self.prompt {
            prompt.buffer.insert_str(prompt.cursor, &text);
            prompt.cursor += text.len();
            self.dirty = true;
            return;
        }
        if let Some(copy) = self.copy.as_mut() {
            if copy.typing {
                copy.search
                    .get_or_insert_with(String::new)
                    .push_str(&text.replace(['\r', '\n'], " "));
                self.dirty = true;
            }
            return;
        }
        if let Some(help) = &mut self.help {
            help.query.push_str(&text.replace(['\r', '\n'], " "));
            help.selected = 0;
            self.dirty = true;
            return;
        }
        if let Some(goto) = &mut self.goto {
            goto.query.push_str(&text.replace(['\r', '\n'], " "));
            goto.selected = if self.palette { 0 } else { 1 };
            self.dirty = true;
            return;
        }
        if self.chooser.is_some()
            || self.settings.is_some()
            || self.reader.is_some()
            || self.switcher
            || (self.config.zellij_keys
                && !matches!(
                    self.keyboard_mode,
                    interaction::KeyboardMode::Normal | interaction::KeyboardMode::Locked
                ))
        {
            return;
        }
        if let Some(image) = clipboard::image_file_paste(&text) {
            match image {
                Ok(image) => self.upload_image_file(image.bytes, image.extension),
                Err(error) => {
                    self.note = error;
                    self.dirty = true;
                }
            }
            return;
        }
        if let Some(chat) = self.chat.as_mut() {
            chat.insert(&text);
            self.dirty = true;
            return;
        }
        if self.focused_mod.is_some() {
            self.route_paste_to_mod(text);
            return;
        }
        let pane_id = self.input_pane();
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };
        if !pane.is_live() {
            return;
        }
        pane.view.scroll_bottom();
        let bytes = pane.view.encode_paste(&text);
        self.dirty = true;
        self.send(Command::Input {
            pane: pane_id,
            bytes,
        });
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        if self.on_pointer_mouse(mouse) {
            return;
        }
        if self.mobile
            || self.chat.is_some()
            || self.prompt.is_some()
            || self.form.is_some()
            || self.help.is_some()
            || self.settings.is_some()
            || self.goto.is_some()
            || self.chooser.is_some()
            || self.reader.is_some()
            || self.popup.is_some()
        {
            self.layout_drag = None;
        }
        if self.chat.is_some() && self.on_chat_mouse(mouse) {
            return;
        }
        if self.mobile && self.on_mobile_mouse(mouse) {
            return;
        }
        if self.prompt.is_some()
            || self.form.is_some()
            || self.help.is_some()
            || self.settings.is_some()
            || self.goto.is_some()
            || self.chooser.is_some()
        {
            return;
        }
        if let Some(reader) = self.reader.as_mut() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    reader.scroll = reader.scroll.saturating_sub(self.config.mouse_scroll_lines)
                }
                MouseEventKind::ScrollDown => {
                    reader.scroll = (reader.scroll + self.config.mouse_scroll_lines)
                        .min(reader.lines.len().saturating_sub(1))
                }
                _ => {}
            }
            self.dirty = true;
            return;
        }
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
                pane.view.scroll(-(self.config.mouse_scroll_lines as isize));
                self.dirty = true;
            } else if mouse.kind == MouseEventKind::ScrollDown {
                pane.view.scroll(self.config.mouse_scroll_lines as isize);
                self.dirty = true;
            }
            return;
        }

        if self.on_layout_mouse(mouse) {
            return;
        }

        if self.route_mouse_to_mod(mouse) {
            return;
        }

        if contains(self.sidebar_area, mouse.column, mouse.row) {
            let step = self.config.mouse_scroll_lines;
            let scroll = if contains(self.spaces_area, mouse.column, mouse.row) {
                &mut self.space_scroll
            } else {
                &mut self.sidebar_scroll
            };
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    *scroll = scroll.saturating_sub(step);
                    self.dirty = true;
                }
                MouseEventKind::ScrollDown => {
                    *scroll = scroll.saturating_add(step);
                    self.dirty = true;
                }
                _ => {}
            }
            if !matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
            ) {
                return;
            }
        }

        if mouse.kind == MouseEventKind::Down(MouseButton::Right) {
            let address = self
                .agent_hits
                .iter()
                .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                .and_then(|(pane, _)| self.panes.get(pane))
                .and_then(|pane| pane.terminal.clone())
                .or_else(|| {
                    self.terminal_hits
                        .iter()
                        .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                        .map(|(address, _)| address.clone())
                });
            if let Some(address) = address {
                self.open_terminal_menu(&address);
                return;
            }
        }
        let pressed = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if pressed {
            if contains(self.agent_sort_hit, mouse.column, mouse.row) {
                self.config.agent_sort = if self.config.agent_sort == AgentSort::Spaces {
                    AgentSort::Priority
                } else {
                    AgentSort::Spaces
                };
                let sort = if self.config.agent_sort == AgentSort::Spaces {
                    "spaces"
                } else {
                    "priority"
                };
                if let Err(error) = crate::config::write_setting(
                    &self.config.config_path,
                    "ui.agent_panel_sort",
                    &format!("\"{sort}\""),
                ) {
                    self.note = t1("sort applied; {}", error);
                }
                self.sidebar_scroll = 0;
                self.dirty = true;
                return;
            }
            if let Some((group, _)) = self
                .agent_group_hits
                .iter()
                .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                .cloned()
            {
                if !self.folded_agent_groups.remove(&group) {
                    self.folded_agent_groups.insert(group);
                }
                self.seen_dirty = true;
                self.seen_since = Some(Instant::now());
                self.dirty = true;
                return;
            }

            if contains(self.tab_add, mouse.column, mouse.row) {
                if self.config.prompt_new_tab {
                    let suggested = format!("{}", self.tabs_in_workspace() + 1);
                    self.ask_with(PromptKind::NewTab, t("tab name"), suggested);
                } else {
                    self.new_tab(None);
                }
                return;
            }
            if contains(self.sidebar_new, mouse.column, mouse.row) {
                if self.config.prompt_new_workspace {
                    self.ask(PromptKind::NewWorkspace, t("workspace name"));
                } else {
                    self.new_workspace(None);
                }
                return;
            }
            if contains(self.sidebar_menu, mouse.column, mouse.row) {
                self.open_actions_menu();
                return;
            }
            if let Some((index, _)) = self
                .tab_hits
                .iter()
                .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
            {
                self.select_tab(*index);
                return;
            }
            if let Some((machine, _)) = self
                .machine_hits
                .iter()
                .find(|(_, area)| contains(*area, mouse.column, mouse.row))
                .cloned()
            {
                let connected = self
                    .machines
                    .iter()
                    .any(|m| m.name == machine && m.state == "connected");
                if connected {
                    if let Err(error) = self.execute_action("space.new", json!({"machine":machine}))
                    {
                        self.note = error.to_string();
                    }
                } else {
                    self.call_rpc(
                        "machine.connect",
                        "machine.connect",
                        json!({"machine":machine}),
                    );
                }
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
            if let Some((terminal, _)) = self
                .terminal_hits
                .iter()
                .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                .cloned()
            {
                self.open_terminal(&terminal);
                return;
            }
        }

        let Some(slot) = self
            .slots
            .iter()
            .rev()
            .find(|slot| contains(slot.outer, mouse.column, mouse.row))
            .copied()
        else {
            return;
        };
        if mouse.kind == MouseEventKind::Down(MouseButton::Right) {
            if let Some(pane) = self.panes.get(&slot.pane) {
                if let Some(address) = pane.terminal.clone() {
                    let passthrough = self.program_right_click.contains(&address)
                        && pane.view.mouse_tracking()
                        && !mouse.modifiers.contains(KeyModifiers::SHIFT);
                    if !passthrough {
                        self.set_focus(slot.pane);
                        self.open_terminal_menu(&address);
                        return;
                    }
                }
            }
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.set_focus(slot.pane);
                if let Some(mode) = self.floating_grab(&slot, mouse) {
                    self.floating_drag = Some(FloatingDrag {
                        pane: slot.pane,
                        mode,
                        grab: (
                            mouse.column.saturating_sub(slot.outer.x),
                            mouse.row.saturating_sub(slot.outer.y),
                        ),
                    });
                    self.dirty = true;
                    return;
                }
                if !slot.collapsed
                    && !self
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
                if self.floating_drag.is_some() {
                    self.drag_floating(mouse);
                    return;
                }
                if let Some(drag) = &mut self.drag {
                    let next = to_inner(slot.inner, mouse);
                    if self.mobile {
                        // 触屏拖动是翻看 scrollback：手指下移就往前翻，不选文本。
                        let delta = i32::from(next.1) - i32::from(drag.current.1);
                        drag.current = next;
                        drag.start = next;
                        if delta != 0 {
                            if let Some(pane) = self.panes.get_mut(&slot.pane) {
                                pane.view.scroll(-(delta as isize));
                            }
                            self.dirty = true;
                        }
                        return;
                    }
                    drag.current = next;
                    self.dirty = true;
                    return;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.floating_drag.take().is_some() {
                    self.mark_layout();
                    return;
                }
                if let Some(drag) = self.drag.take() {
                    if !self.mobile && self.config.copy_on_select && drag.start != drag.current {
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
                pane.view.scroll(-lines);
                self.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                pane.view.scroll(lines);
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// 手机布局下的鼠标事件：`switch` 打开面板，面板里 tap 跳转、滚动翻列表。
    /// 返回 `true` 表示已消费。
    fn on_mobile_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self.switcher {
            let overlay = self.prompt.is_some()
                || self.reader.is_some()
                || self.chooser.is_some()
                || self.popup.is_some()
                || self.copy.is_some();
            if overlay {
                return true;
            }
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.mobile_scroll = self.mobile_scroll.saturating_sub(2);
                    self.dirty = true;
                }
                MouseEventKind::ScrollDown => {
                    self.mobile_scroll = self
                        .mobile_scroll
                        .saturating_add(2)
                        .min(self.mobile_max_scroll);
                    self.dirty = true;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if contains(self.mobile_close, mouse.column, mouse.row) {
                        self.close_switcher();
                        return true;
                    }
                    let target = self
                        .mobile_targets
                        .iter()
                        .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                        .map(|(target, _)| *target);
                    if let Some(target) = target {
                        self.run_mobile_target(target);
                    }
                }
                _ => {}
            }
            return true;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && self.prompt.is_none()
            && contains(self.mobile_switch, mouse.column, mouse.row)
        {
            self.open_switcher();
            return true;
        }
        false
    }

    pub fn open_switcher(&mut self) {
        self.switcher = true;
        self.mobile_scroll = 0;
        self.dirty = true;
    }

    fn close_switcher(&mut self) {
        self.switcher = false;
        self.mobile_scroll = 0;
        self.dirty = true;
    }

    fn run_mobile_target(&mut self, target: MobileTarget) {
        self.close_switcher();
        match target {
            MobileTarget::Workspace(index) => self.select_workspace(index),
            MobileTarget::Tab(index) => self.select_tab(index),
            MobileTarget::Agent(pane) => self.focus_pane(pane),
            MobileTarget::Terminal(index) => {
                if let Some(terminal) = self.terminals.get(index) {
                    let address = terminal.address.clone();
                    self.open_terminal(&address);
                }
            }
            MobileTarget::Action(action) => self.run_mobile_action(action),
        }
    }

    fn run_mobile_action(&mut self, action: MobileAction) {
        match action {
            MobileAction::Actions => self.open_actions_menu(),
            MobileAction::NewTab => self.new_tab(None),
            MobileAction::SplitRight => self.split_focused(Axis::Horizontal),
            MobileAction::SplitDown => self.split_focused(Axis::Vertical),
            MobileAction::ClosePane => self.close_focused_pane(),
            MobileAction::CopyMode => self.enter_copy_mode(),
            MobileAction::NextAgent => self.focus_next_attention(),
            MobileAction::Chat => self.toggle_chat(),
            MobileAction::Detach => self.quit = true,
        }
    }

    /// 打开/关闭 Chat 视图。打开时跟随当前输入 Pane 的 cwd 找 pi 会话。
    fn toggle_chat(&mut self) {
        if self.chat.take().is_some() {
            self.dirty = true;
            return;
        }
        let pane = self.input_pane();
        let Some(state) = self.panes.get(&pane) else {
            self.note = "chat: 没有可用的 pane".to_string();
            return;
        };
        if !state.is_live() {
            self.note = "chat: pane 还没连上".to_string();
            return;
        }
        let cwd = state
            .cwd
            .clone()
            .or_else(|| self.workspace_cwd(self.active_ws));
        let session = state.agent_session.clone();
        let machine = state
            .terminal
            .as_deref()
            .and_then(|address| address.split_once('/'))
            .map(|(machine, _)| machine);
        let chat = match machine {
            Some(machine) => Chat::open_remote(pane, machine, cwd.as_deref(), session.as_deref()),
            None => Chat::open(pane, cwd.as_deref(), session.as_deref()),
        };
        self.note = chat.note.clone();
        self.chat = Some(chat);
        self.dirty = true;
    }

    /// 把一段文本当作输入发给 Chat 的 Pane（末尾补回车）。
    fn send_chat_text(&mut self, text: &str) {
        let Some(chat) = self.chat.as_ref() else {
            return;
        };
        let pane = chat.pane;
        if !self.panes.get(&pane).is_some_and(Pane::is_live) {
            self.note = "chat: pane 已结束".to_string();
            return;
        }
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(b'\r');
        self.send(Command::Input { pane, bytes });
        self.dirty = true;
    }

    /// 发送裸字节（中断、按键）。
    fn send_chat_keys(&mut self, bytes: Vec<u8>) {
        let Some(chat) = self.chat.as_ref() else {
            return;
        };
        let pane = chat.pane;
        if !self.panes.get(&pane).is_some_and(Pane::is_live) {
            return;
        }
        self.send(Command::Input { pane, bytes });
        self.dirty = true;
    }

    /// 发送输入行内容。
    fn send_chat_input(&mut self) {
        let Some(text) = self.chat.as_mut().and_then(Chat::take_input) else {
            self.dirty = true;
            return;
        };
        self.send_chat_text(&text);
    }

    /// 采纳命令面板当前项：本地动作自己执行，普通命令发送，需要浮层的切回终端。
    fn accept_chat_command(&mut self) {
        let command = self.chat.as_ref().and_then(Chat::palette_selected);
        let Some(command) = command else {
            return;
        };
        if let Some(chat) = self.chat.as_mut() {
            chat.clear_input();
        }
        match command.kind {
            Kind::Local(local) => self.run_local_command(local),
            Kind::Terminal => {
                self.send_chat_text(&command.name);
                self.chat = None;
            }
            Kind::Prompt => self.send_chat_text(&command.name),
        }
    }

    /// hc-tui 自己的命令面板动作。
    fn run_local_command(&mut self, local: Local) {
        if let Some(machine) = self.chat.as_ref().and_then(|chat| chat.machine.as_deref()) {
            self.note = format!("hook 操作需在 {machine} 上运行 hc-tui session-hook install/remove，然后在 Pi /reload");
            if let Some(chat) = self.chat.as_mut() {
                chat.close_palette();
            }
            self.dirty = true;
            return;
        }
        match local {
            Local::InstallHook => match hook::install() {
                Ok(path) => {
                    self.note = format!("已写入 {}，发送 /reload 生效", path.display());
                    self.send_chat_text("/reload");
                }
                Err(error) => self.note = format!("安装 hook 失败：{error}"),
            },
            Local::RemoveHook => match hook::remove() {
                Ok(true) => {
                    self.note = "已删除会话映射 hook".to_string();
                    self.send_chat_text("/reload");
                }
                Ok(false) => self.note = "没有可删除的 hook".to_string(),
                Err(error) => self.note = format!("删除 hook 失败：{error}"),
            },
        }
        if let Some(chat) = self.chat.as_mut() {
            chat.close_palette();
        }
        self.dirty = true;
    }

    fn run_chat_action(&mut self, action: ChatAction) {
        match action {
            ChatAction::Send => self.send_chat_input(),
            ChatAction::Interrupt => {
                self.send_chat_keys(vec![3]);
                self.note = "chat: 已发送中断".to_string();
            }
            ChatAction::Palette => {
                if let Some(chat) = self.chat.as_mut() {
                    // 已有文本当作过滤词，不丢内容。
                    if !chat.input.starts_with('/') {
                        chat.cursor = 0;
                        chat.insert("/");
                    }
                    chat.cursor = chat.input.len();
                    chat.open_palette();
                }
                self.dirty = true;
            }
            ChatAction::Terminal => {
                self.chat = None;
                self.dirty = true;
            }
        }
    }

    fn on_chat_key(&mut self, key: KeyEvent) {
        let Some(chat) = self.chat.as_mut() else {
            return;
        };
        if key.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if chat.palette.is_some() {
                    chat.close_palette();
                } else {
                    self.chat = None;
                }
            }
            KeyCode::Enter => {
                if chat.palette.is_some() {
                    self.accept_chat_command();
                } else if chat.input.is_empty() {
                    if let Some(index) = chat.selected_tool() {
                        chat.toggle_expanded(index);
                    } else {
                        self.send_chat_input();
                    }
                } else {
                    self.send_chat_input();
                }
            }
            KeyCode::Char('c') if ctrl => {
                self.send_chat_keys(vec![3]);
                self.note = "chat: 已发送中断".to_string();
            }
            KeyCode::Char('n') if ctrl => self.send_chat_text("/new"),
            KeyCode::Char('p') if ctrl => self.run_chat_action(ChatAction::Palette),
            KeyCode::Char('o') if ctrl => self.run_chat_action(ChatAction::Terminal),
            KeyCode::Char('y') if ctrl => {
                let reply = chat.last_reply().map(str::to_string);
                match reply {
                    Some(text) => {
                        let note = clipboard::copy(&text);
                        self.note = format!("chat: {note}");
                    }
                    None => self.note = "chat: 还没有回复".to_string(),
                }
            }
            KeyCode::Char('u') if ctrl => chat.clear_input(),
            KeyCode::Char('e') if ctrl => chat.scroll = 0,
            KeyCode::Char(ch) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                chat.insert(&ch.to_string());
            }
            KeyCode::Backspace => chat.backspace(),
            KeyCode::Delete => chat.delete(),
            KeyCode::Left => chat.move_cursor(-1),
            KeyCode::Right => chat.move_cursor(1),
            KeyCode::Home => chat.cursor = 0,
            KeyCode::End => chat.cursor = chat.input.len(),
            KeyCode::Up => {
                if chat.palette.is_some() {
                    chat.palette_move(-1);
                } else if chat.input.is_empty() {
                    chat.select(-1);
                    chat.reveal_selected();
                } else {
                    chat.recall(-1);
                }
            }
            KeyCode::Down => {
                if chat.palette.is_some() {
                    chat.palette_move(1);
                } else if chat.input.is_empty() {
                    chat.select(1);
                    chat.reveal_selected();
                } else {
                    chat.recall(1);
                }
            }
            KeyCode::PageUp => {
                chat.scroll = chat.scroll.saturating_add(chat.view_height.max(1) / 2)
            }
            KeyCode::PageDown => {
                chat.scroll = chat.scroll.saturating_sub(chat.view_height.max(1) / 2)
            }
            KeyCode::Tab => {
                if let Some(command) = chat.palette_selected() {
                    chat.clear_input();
                    chat.insert(&command.name);
                    chat.close_palette();
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    /// Chat 打开时的鼠标：滚轮翻消息、点工具块展开、点动作条。
    fn on_chat_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self.mobile
            && (contains(self.mobile_switch, mouse.column, mouse.row)
                || contains(self.mobile_close, mouse.column, mouse.row))
        {
            return false;
        }
        // 点内容区之外（侧栏、Tab 栏）先退回终端视图，再按常规处理这次点击。
        if !self.mobile
            && mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && !contains(self.content, mouse.column, mouse.row)
        {
            self.chat = None;
            self.dirty = true;
            return false;
        }
        let Some(chat) = self.chat.as_mut() else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::ScrollUp => chat.scroll = chat.scroll.saturating_add(1),
            MouseEventKind::ScrollDown => chat.scroll = chat.scroll.saturating_sub(1),
            MouseEventKind::Down(MouseButton::Left) => {
                let action = chat
                    .actions
                    .iter()
                    .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                    .map(|(action, _)| *action);
                match action {
                    Some(action) => self.run_chat_action(action),
                    None => {
                        let block = chat
                            .hits
                            .iter()
                            .find(|(_, rect)| contains(*rect, mouse.column, mouse.row))
                            .map(|(block, _)| *block);
                        if let Some(block) = block {
                            chat.toggle_expanded(block);
                            chat.selected = Some(block);
                        }
                    }
                }
            }
            _ => {}
        }
        self.dirty = true;
        true
    }

    fn on_prefix_key(&mut self, key: KeyEvent) {
        self.dispatch_binding(key, true);
    }
    fn dispatch_binding(&mut self, key: KeyEvent, prefix: bool) {
        let keys = self.config.keys.clone();

        // 前缀键再按一次：把这次按键原样交给当前输入目标（字面 ctrl+b）。
        if prefix && keys.prefix.matches_direct(&key) {
            if self.focused_mod.is_some() {
                self.route_key_to_mod(key);
            } else {
                self.forward_key(key);
            }
            return;
        }
        if let Some(index) = self.custom_command(&key, prefix) {
            self.run_custom(index);
            return;
        }
        if keys.help.matches_mode(&key, prefix) {
            self.run_named_binding("help");
            return;
        }
        if keys.settings.matches_mode(&key, prefix) {
            self.run_named_binding("settings");
            return;
        }
        if key.code == KeyCode::Char(':') {
            self.open_palette();
            return;
        }
        if keys.goto.matches_mode(&key, prefix) {
            self.run_named_binding("goto");
            return;
        }
        if keys.open_notification_target.matches_mode(&key, prefix) {
            self.run_named_binding("open_notification_target");
            return;
        }
        if keys.detach.matches_mode(&key, prefix) {
            self.run_named_binding("detach");
            return;
        }
        if keys.reload_config.matches_mode(&key, prefix) {
            self.run_named_binding("reload_config");
            return;
        }
        if keys.new_tab.matches_mode(&key, prefix) {
            self.run_named_binding("new_tab");
            return;
        }
        if keys.rename_tab.matches_mode(&key, prefix) {
            self.run_named_binding("rename_tab");
            return;
        }
        if keys.next_tab.matches_mode(&key, prefix) {
            self.run_named_binding("next_tab");
            return;
        }
        if keys.previous_tab.matches_mode(&key, prefix) {
            self.run_named_binding("previous_tab");
            return;
        }
        if keys.move_tab_previous.matches_mode(&key, prefix) {
            self.run_named_binding("move_tab_previous");
            return;
        }
        if keys.move_tab_next.matches_mode(&key, prefix) {
            self.run_named_binding("move_tab_next");
            return;
        }
        if keys.close_tab.matches_mode(&key, prefix) {
            self.run_named_binding("close_tab");
            return;
        }
        if keys.new_workspace.matches_mode(&key, prefix) {
            self.run_named_binding("new_workspace");
            return;
        }
        if keys.rename_workspace.matches_mode(&key, prefix) {
            self.run_named_binding("rename_workspace");
            return;
        }
        if keys.close_workspace.matches_mode(&key, prefix) {
            self.run_named_binding("close_workspace");
            return;
        }
        if keys.workspace_picker.matches_mode(&key, prefix) {
            self.run_named_binding("workspace_picker");
            return;
        }
        if keys.next_workspace.matches_mode(&key, prefix) {
            self.run_named_binding("next_workspace");
            return;
        }
        if keys.previous_workspace.matches_mode(&key, prefix) {
            self.run_named_binding("previous_workspace");
            return;
        }
        if keys.split_vertical.matches_mode(&key, prefix) {
            self.run_named_binding("split_vertical");
            return;
        }
        if keys.split_horizontal.matches_mode(&key, prefix) {
            self.run_named_binding("split_horizontal");
            return;
        }
        if keys.focus_pane_left.matches_mode(&key, prefix) {
            self.run_named_binding("focus_pane_left");
            return;
        }
        if keys.focus_pane_down.matches_mode(&key, prefix) {
            self.run_named_binding("focus_pane_down");
            return;
        }
        if keys.focus_pane_up.matches_mode(&key, prefix) {
            self.run_named_binding("focus_pane_up");
            return;
        }
        if keys.focus_pane_right.matches_mode(&key, prefix) {
            self.run_named_binding("focus_pane_right");
            return;
        }
        if keys.cycle_pane_next.matches_mode(&key, prefix) {
            self.run_named_binding("cycle_pane_next");
            return;
        }
        if keys.cycle_pane_previous.matches_mode(&key, prefix) {
            self.run_named_binding("cycle_pane_previous");
            return;
        }
        if keys.last_pane.matches_mode(&key, prefix) {
            self.run_named_binding("last_pane");
            return;
        }
        if keys.zoom.matches_mode(&key, prefix) {
            self.run_named_binding("zoom");
            return;
        }
        if keys.layout_next.matches_mode(&key, prefix) {
            self.run_named_binding("layout_next");
            return;
        }
        if keys.layout_previous.matches_mode(&key, prefix) {
            self.run_named_binding("layout_previous");
            return;
        }
        if keys.stack_panes.matches_mode(&key, prefix) {
            self.run_named_binding("stack_panes");
            return;
        }
        if keys.float_pane.matches_mode(&key, prefix) {
            self.run_named_binding("float_pane");
            return;
        }
        if keys.float_pin.matches_mode(&key, prefix) {
            self.run_named_binding("float_pin");
            return;
        }
        if keys.float_hide.matches_mode(&key, prefix) {
            self.run_named_binding("float_hide");
            return;
        }
        if keys.close_pane.matches_mode(&key, prefix) {
            self.run_named_binding("close_pane");
            return;
        }
        if keys.rename_pane.matches_mode(&key, prefix) {
            self.run_named_binding("rename_pane");
            return;
        }
        if keys.kill_pane.matches_mode(&key, prefix) {
            self.run_named_binding("kill_pane");
            return;
        }
        if keys.move_pane.matches_mode(&key, prefix) {
            self.run_named_binding("move_pane");
            return;
        }
        if keys.resize_mode.matches_mode(&key, prefix) {
            self.run_named_binding("resize_mode");
            return;
        }
        if keys.toggle_sidebar.matches_mode(&key, prefix) {
            self.run_named_binding("toggle_sidebar");
            return;
        }
        if keys.scroll_bottom.matches_mode(&key, prefix) {
            self.run_named_binding("scroll_bottom");
            return;
        }
        if keys.copy_mode.matches_mode(&key, prefix) {
            self.run_named_binding("copy_mode");
            return;
        }
        if keys.next_agent.matches_mode(&key, prefix) {
            self.run_named_binding("next_agent");
            return;
        }
        if keys.chat.matches_mode(&key, prefix) {
            self.run_named_binding("chat");
            return;
        }
        if keys.send_text.matches_mode(&key, prefix) {
            self.run_named_binding("send_text");
            return;
        }
        if keys.interrupt.matches_mode(&key, prefix) {
            self.run_named_binding("interrupt");
            return;
        }
        if keys.toggle_keep.matches_mode(&key, prefix) {
            self.run_named_binding("toggle_keep");
            return;
        }
        if keys.remove_terminal.matches_mode(&key, prefix) {
            self.run_named_binding("remove_terminal");
            return;
        }
        if keys.open_link.matches_mode(&key, prefix) {
            self.run_named_binding("open_link");
            return;
        }
        if keys.edit_history.matches_mode(&key, prefix) {
            self.run_named_binding("edit_history");
            return;
        }
        if keys.read_pane.matches_mode(&key, prefix) {
            self.run_named_binding("read_pane");
            return;
        }
        if keys.send_keys.matches_mode(&key, prefix) {
            self.run_named_binding("send_keys");
            return;
        }
        if keys.wait_output.matches_mode(&key, prefix) {
            self.run_named_binding("wait_output");
            return;
        }
        if keys.put_file.matches_mode(&key, prefix) {
            self.run_named_binding("put_file");
            return;
        }
        if keys.templates.matches_mode(&key, prefix) {
            self.run_named_binding("templates");
            return;
        }
        if keys.machines.matches_mode(&key, prefix) {
            self.run_named_binding("machines");
            return;
        }
        if keys.paste_image.matches_mode(&key, prefix) {
            self.run_named_binding("paste_image");
            return;
        }
        if let KeyCode::Char(digit @ '1'..='9') = key.code {
            let index = digit as usize - '1' as usize;
            if index < self.tabs_in_workspace() {
                self.select_tab(index);
            }
            return;
        }
        self.note = String::new();
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
                    self.note = String::new();
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
                self.note = String::new();
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
                self.note = t("cancelled").to_string();
                self.settle_mod_confirm("cancelled");
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.buffer.clear();
                prompt.cursor = 0;
            }
            KeyCode::Enter => {
                let prompt = self.prompt.take().expect("prompt exists");
                let confirmed = !prompt.kind.needs_confirm()
                    || matches!(prompt.buffer.trim(), "y" | "Y" | "yes");
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
                self.settle_mod_confirm(if confirmed { "done" } else { "cancelled" });
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
        self.note = String::new();
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
                        format!("/{}", copy.search.clone().unwrap_or_default())
                    } else {
                        t("not found").to_string()
                    };
                }
                KeyCode::Esc => {
                    copy.typing = false;
                    copy.search = None;
                    self.note = t("copy mode").to_string();
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
                self.note = t("copy mode closed").to_string();
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
                    t("block selection").to_string()
                } else {
                    t("line selection").to_string()
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
                    self.note = t1("opening {}", uri);
                } else {
                    self.note = t("no link under cursor").to_string();
                }
            }
            KeyCode::Char('/') => {
                copy.typing = true;
                copy.search = Some(String::new());
                self.note = t("search").to_string();
            }
            KeyCode::Char('n') => {
                if !copy.find_next() {
                    self.note = t("not found").to_string();
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

    /// 带预填内容的提示行（重命名与新建 Tab 用）。
    fn ask_with(&mut self, kind: PromptKind, label: &str, initial: String) {
        self.ask(kind, label);
        if let Some(prompt) = &mut self.prompt {
            prompt.cursor = initial.len();
            prompt.buffer = initial;
        }
    }

    // ------------------------------------------------------------- 浮层面板

    /// 键位分组：组标题与绑定名顺序；帮助面板与 `keybindings` 域共用。
    pub const HELP_GROUPS: [(&'static str, &'static [&'static str]); 5] = [
        (
            "global",
            &[
                "prefix",
                "help",
                "settings",
                "detach",
                "reload_config",
                "open_notification_target",
            ],
        ),
        ("navigation", &["goto", "workspace_picker"]),
        (
            "panes",
            &[
                "split_vertical",
                "split_horizontal",
                "focus_pane_left",
                "focus_pane_down",
                "focus_pane_up",
                "focus_pane_right",
                "cycle_pane_next",
                "cycle_pane_previous",
                "last_pane",
                "zoom",
                "stack_panes",
                "layout_next",
                "layout_previous",
                "float_pane",
                "float_pin",
                "float_hide",
                "resize_mode",
                "close_pane",
                "rename_pane",
                "kill_pane",
                "move_pane",
                "copy_mode",
                "edit_history",
                "toggle_sidebar",
            ],
        ),
        (
            "tabs and workspaces",
            &[
                "new_tab",
                "rename_tab",
                "next_tab",
                "previous_tab",
                "move_tab_previous",
                "move_tab_next",
                "close_tab",
                "new_workspace",
                "rename_workspace",
                "close_workspace",
                "next_workspace",
                "previous_workspace",
            ],
        ),
        (
            "agents and terminals",
            &[
                "next_agent",
                "chat",
                "send_text",
                "send_keys",
                "wait_output",
                "put_file",
                "paste_image",
                "interrupt",
                "read_pane",
                "open_link",
                "templates",
                "machines",
                "toggle_keep",
                "remove_terminal",
            ],
        ),
    ];

    /// Keybinds 面板的全部行，按 herdr 的分组分好。
    pub fn help_rows(&self) -> Vec<HelpRow> {
        let bindings = self.config.keys.entries();
        let text = Self::binding_title;
        let groups = Self::HELP_GROUPS;
        let by_name: std::collections::HashMap<&str, crate::config::Binding> =
            bindings.into_iter().collect();
        let mut rows = Vec::new();
        for (title, names) in groups {
            rows.push(HelpRow {
                section: true,
                key: String::new(),
                text: t(title).to_string(),
            });
            for name in names {
                let Some(binding) = by_name.get(name) else {
                    continue;
                };
                if binding.is_off() {
                    continue;
                }
                rows.push(HelpRow {
                    section: false,
                    key: binding.to_string().trim_matches('"').to_string(),
                    text: text(name).to_string(),
                });
            }
        }
        rows.push(HelpRow {
            section: true,
            key: String::new(),
            text: t("copy mode").to_string(),
        });
        for (key, text) in [
            ("h j k l", "move"),
            ("w b e", "word"),
            ("{ }", "paragraph"),
            ("/ ?", "search"),
            ("n N", "repeat search"),
            ("v space", "select"),
            ("y enter", "copy"),
            ("q esc", "exit"),
        ] {
            rows.push(HelpRow {
                section: false,
                key: key.to_string(),
                text: t(text).to_string(),
            });
        }
        rows
    }

    /// 过滤后的 Keybinds 行；查询为空时原样返回。
    pub fn help_view(&self) -> Vec<HelpRow> {
        let query = self
            .help
            .as_ref()
            .map(|help| help.query.clone())
            .unwrap_or_default();
        self.help_view_for(&query)
    }

    fn help_view_for(&self, query: &str) -> Vec<HelpRow> {
        let rows = self.help_rows();
        let query = query.to_ascii_lowercase();
        if query.is_empty() {
            return rows;
        }
        let matched: Vec<HelpRow> = rows
            .iter()
            .filter(|row| {
                !row.section
                    && (row.key.to_ascii_lowercase().contains(&query)
                        || row.text.to_ascii_lowercase().contains(&query))
            })
            .cloned()
            .collect();
        let mut out = Vec::new();
        if !matched.is_empty() {
            out.push(HelpRow {
                section: true,
                key: String::new(),
                text: t("matches").to_string(),
            });
            out.extend(matched);
        }
        out
    }

    fn on_help_key(&mut self, key: KeyEvent) {
        let Some(mut help) = self.help.take() else {
            return;
        };
        let rows = self.help_view_for(&help.query);
        let max = rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.dirty = true;
                return;
            }
            KeyCode::Char('q') if help.query.is_empty() => {
                self.dirty = true;
                return;
            }
            KeyCode::Char('k') | KeyCode::Up => help.selected = help.selected.saturating_sub(1),
            KeyCode::Char('j') | KeyCode::Down => help.selected = (help.selected + 1).min(max),
            KeyCode::PageUp => help.selected = help.selected.saturating_sub(10),
            KeyCode::PageDown => help.selected = (help.selected + 10).min(max),
            KeyCode::Backspace => {
                help.query.pop();
                help.selected = 0;
                help.scroll = 0;
            }
            KeyCode::Char('/') => {}
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                help.query.push(character);
                help.selected = 0;
                help.scroll = 0;
            }
            _ => {}
        }
        self.help = Some(help);
        self.dirty = true;
    }

    pub const SETTINGS_SECTIONS: [&'static str; 4] = ["theme", "indicators", "toasts", "sound"];

    /// 设置面板当前分段的条目数。
    pub fn settings_len(&self, section: usize) -> usize {
        match section {
            0 => crate::theme::names().len(),
            1 => 2,
            2 => 3,
            _ => 2,
        }
    }

    /// 打开设置时让选中项落在当前值上。
    fn settings_selected(&self, section: usize) -> usize {
        match section {
            0 => crate::theme::names()
                .iter()
                .position(|name| *name == self.config.theme.name)
                .unwrap_or(0),
            1 => usize::from(self.config.indicators == crate::config::Indicators::Symbols),
            2 => match self.config.notifications {
                crate::config::Delivery::Off => 0,
                crate::config::Delivery::Toast => 1,
                crate::config::Delivery::System => 2,
            },
            _ => usize::from(self.config.sound),
        }
    }

    fn on_settings_key(&mut self, key: KeyEvent) {
        let Some(mut settings) = self.settings.take() else {
            return;
        };
        let section = settings.section;
        let len = self.settings_len(section);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.dirty = true;
                return;
            }
            KeyCode::Tab | KeyCode::Right => {
                settings.section = (section + 1) % Self::SETTINGS_SECTIONS.len();
                settings.selected = self.settings_selected(settings.section);
            }
            KeyCode::BackTab | KeyCode::Left => {
                let next =
                    (section + Self::SETTINGS_SECTIONS.len() - 1) % Self::SETTINGS_SECTIONS.len();
                settings.section = next;
                settings.selected = self.settings_selected(next);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                settings.selected = settings.selected.saturating_sub(1)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                settings.selected = (settings.selected + 1).min(len.saturating_sub(1))
            }
            KeyCode::Enter => {
                self.apply_setting(settings.section, settings.selected);
                settings.selected = self.settings_selected(settings.section);
            }
            _ => {}
        }
        self.settings = Some(settings);
        self.dirty = true;
    }

    /// 应用设置项并写回配置文件，让下次启动保持同一选择。
    fn apply_setting(&mut self, section: usize, selected: usize) {
        let (key, value) = match section {
            0 => {
                let name = crate::theme::names()[selected].to_string();
                self.config.theme = crate::theme::builtin(&name).expect("built-in theme resolves");
                ("theme.name", format!("\"{name}\""))
            }
            1 => {
                self.config.indicators = if selected == 1 {
                    crate::config::Indicators::Symbols
                } else {
                    crate::config::Indicators::Dots
                };
                let text = if selected == 1 { "symbols" } else { "dots" };
                ("ui.status_indicators", format!("\"{text}\""))
            }
            2 => {
                self.config.notifications = match selected {
                    0 => crate::config::Delivery::Off,
                    1 => crate::config::Delivery::Toast,
                    _ => crate::config::Delivery::System,
                };
                let text = ["off", "toast", "system"][selected.min(2)];
                ("ui.toast.delivery", format!("\"{text}\""))
            }
            _ => {
                self.config.sound = selected == 1;
                ("ui.sound.enabled", format!("{}", selected == 1))
            }
        };
        let path = self.config.config_path.clone();
        let outcome = crate::config::write_setting(&path, key, &value);
        self.note = match outcome {
            Ok(()) => format!("{key} = {value}"),
            Err(error) => t1("applied for this session; {}", error),
        };
        self.dirty = true;
    }

    /// Go to 面板的行：工作区标题 + 每个终端；没有本地视图的终端也能跳过去。
    pub fn goto_rows(&self) -> Vec<GotoRow> {
        let query = self
            .goto
            .as_ref()
            .map(|goto| goto.query.clone())
            .unwrap_or_default();
        self.goto_rows_for(&query)
    }

    fn goto_rows_for(&self, query: &str) -> Vec<GotoRow> {
        if self.palette {
            let query = query.to_lowercase();
            let mut rows: Vec<GotoRow> = actions::COMMANDS
                .iter()
                .filter(|(name, description)| {
                    format!("{name} {description}")
                        .to_lowercase()
                        .contains(&query)
                })
                .map(|(name, description)| GotoRow {
                    pane: None,
                    terminal: None,
                    text: name.to_string(),
                    right: String::new(),
                    section: false,
                    context: description.to_string(),
                    cwd: String::new(),
                })
                .collect();
            for action in self.plugins.actions() {
                if !action.contexts.iter().any(|context| context == "global") {
                    continue;
                }
                if format!("{} {} {}", action.name, action.title, action.description)
                    .to_lowercase()
                    .contains(&query)
                {
                    rows.push(GotoRow {
                        pane: None,
                        terminal: None,
                        text: action.name,
                        right: String::new(),
                        section: false,
                        context: action.title,
                        cwd: String::new(),
                    });
                }
            }
            return rows;
        }
        let query = query.to_ascii_lowercase();
        let matches = |text: &str, extra: &str| -> bool {
            query.is_empty()
                || text.to_ascii_lowercase().contains(&query)
                || extra.to_ascii_lowercase().contains(&query)
        };
        let mut rows = Vec::new();
        for (index, workspace) in self.workspaces.iter().enumerate() {
            let label = self.workspace_label(index);
            let mut panes: Vec<(u32, String, String)> = Vec::new();
            for tab in &workspace.tabs {
                for pane in tab.panes() {
                    let Some(state) = self.panes.get(&pane.0) else {
                        continue;
                    };
                    let name = state.label();
                    let address = state.terminal.clone().unwrap_or_default();
                    panes.push((pane.0, name, address));
                }
            }
            if !query.is_empty()
                && !panes
                    .iter()
                    .any(|(_, name, address)| matches(name, address))
                && !matches(&label, "")
            {
                continue;
            }
            rows.push(GotoRow {
                pane: None,
                terminal: None,
                text: label,
                right: self
                    .workspace_cwd(index)
                    .and_then(|cwd| self.branch(&cwd))
                    .unwrap_or_default(),
                section: true,
                context: String::new(),
                cwd: self.workspace_cwd(index).unwrap_or_default(),
            });
            for (pane, name, address) in panes {
                if !matches(&name, &address) {
                    continue;
                }
                let located = self.locate_public(pane);
                let position = located
                    .map(|(workspace, tab)| {
                        format!(
                            "{}/tab{}/{}",
                            self.workspaces[workspace].id,
                            self.workspaces[workspace].tabs[tab].id,
                            pane
                        )
                    })
                    .unwrap_or_default();
                let context = located
                    .map(|(workspace, tab)| {
                        format!(
                            "{} / {} / w{}:p{}",
                            self.workspace_label(workspace),
                            self.workspaces[workspace].tabs[tab].display(&self.panes),
                            self.workspaces[workspace].id,
                            pane
                        )
                    })
                    .unwrap_or_default();
                let cwd = self
                    .panes
                    .get(&pane)
                    .and_then(|state| state.cwd.clone())
                    .unwrap_or_default();
                rows.push(GotoRow {
                    pane: Some(pane),
                    terminal: Some(address.clone()),
                    text: format!("{} · {}", short(&address), short(&name)),
                    right: position,
                    section: false,
                    context,
                    cwd,
                });
            }
        }
        let unowned: Vec<GotoRow> = self
            .terminals_without_pane()
            .into_iter()
            .filter(|terminal| {
                matches(
                    &format!("{} · {}", terminal.address, terminal.title),
                    &terminal.address,
                )
            })
            .map(|terminal| {
                let label = format!("{} · {}", short(&terminal.address), short(terminal.label()));
                GotoRow {
                    pane: None,
                    terminal: Some(terminal.address.clone()),
                    text: label,
                    right: if terminal.live {
                        "terminal"
                    } else {
                        "finished"
                    }
                    .to_string(),
                    section: false,
                    context: String::new(),
                    cwd: String::new(),
                }
            })
            .collect();
        if !unowned.is_empty() {
            rows.push(GotoRow {
                pane: None,
                terminal: None,
                text: t("other clients").to_string(),
                right: String::new(),
                section: true,
                context: String::new(),
                cwd: String::new(),
            });
            rows.extend(unowned);
        }
        rows
    }

    fn on_goto_key(&mut self, key: KeyEvent) {
        let Some(mut goto) = self.goto.take() else {
            return;
        };
        let rows = self.goto_rows_for(&goto.query);
        let max = rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => {
                self.dirty = true;
                return;
            }
            KeyCode::Enter => {
                if let Some(row) = rows.get(goto.selected) {
                    if self.palette {
                        self.palette = false;
                        if self.open_plugin_form(&row.text, json!({})) {
                            self.dirty = true;
                            return;
                        }
                        self.ask_with(
                            PromptKind::Command,
                            t("command + JSON arguments"),
                            format!("{} {{}}", row.text),
                        );
                        self.dirty = true;
                        return;
                    }
                    if let Some(pane) = row.pane {
                        self.focus_pane(pane);
                    } else if let Some(terminal) = &row.terminal {
                        self.open_terminal(terminal);
                    }
                }
                self.dirty = true;
                return;
            }
            KeyCode::Up => goto.selected = goto.selected.saturating_sub(1),
            KeyCode::Down => goto.selected = (goto.selected + 1).min(max),
            KeyCode::PageUp => goto.selected = goto.selected.saturating_sub(10),
            KeyCode::PageDown => goto.selected = (goto.selected + 10).min(max),
            KeyCode::Backspace => {
                goto.query.pop();
            }
            KeyCode::Char('/') => {}
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                goto.query.push(character);
            }
            _ => {}
        }
        let rows = self.goto_rows_for(&goto.query);
        let max = rows.len().saturating_sub(1);
        // 过滤后选不中标题行时，落到第一条真正的终端上。
        if rows.get(goto.selected).is_some_and(|row| row.section) {
            if let Some(index) = rows
                .iter()
                .enumerate()
                .skip(goto.selected)
                .find(|(_, row)| !row.section)
                .map(|(index, _)| index)
            {
                goto.selected = index;
            }
        }
        goto.selected = goto.selected.min(max);
        self.goto = Some(goto);
        self.dirty = true;
    }

    /// 焦点落到最近的注意力目标；没有就跳下一个待处理的 Pane。
    fn open_notification_target(&mut self) {
        if let Some(pane) = self
            .last_attention
            .filter(|pane| self.panes.contains_key(pane))
        {
            self.focus_pane(pane);
            return;
        }
        self.focus_next_attention();
    }

    /// 工作区里任一 Pane 的前台目录，用来查分支；都还没上报时退回本进程目录。
    pub fn workspace_cwd(&self, workspace: usize) -> Option<String> {
        let state = self.workspaces.get(workspace)?;
        state
            .panes()
            .into_iter()
            .filter_map(|pane| self.panes.get(&pane.0))
            .find_map(|pane| {
                pane.cwd
                    .clone()
                    .or_else(|| pane.launch.as_ref().and_then(|launch| launch.cwd.clone()))
            })
            .or_else(|| state.folder.clone())
            .or_else(|| {
                if self.workspace_machine(workspace) == "local" {
                    std::env::current_dir()
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
    }

    /// 工作区在侧栏与 Go to 里的名字：创建时定下来，之后只随重命名变化。
    pub fn workspace_label(&self, workspace: usize) -> String {
        let Some(state) = self.workspaces.get(workspace) else {
            return String::new();
        };
        if !state.label.is_empty() {
            return state.label.clone();
        }
        dir_label(self.workspace_cwd(workspace), format!("w{}", state.id))
    }

    pub fn workspace_machine(&self, workspace: usize) -> String {
        if let Some(machine) = self
            .workspaces
            .get(workspace)
            .and_then(|w| w.machine.clone())
        {
            return machine;
        }
        self.workspaces
            .get(workspace)
            .into_iter()
            .flat_map(|w| w.panes())
            .filter_map(|p| self.panes.get(&p.0))
            .find_map(|p| {
                p.terminal
                    .as_deref()
                    .map(|address| {
                        address
                            .split_once('/')
                            .map(|(machine, _)| machine)
                            .unwrap_or("local")
                            .to_string()
                    })
                    .or_else(|| p.launch.as_ref().and_then(|l| l.on.clone()))
            })
            .unwrap_or_else(|| "local".to_string())
    }

    /// 写给宿主终端的窗口标题。
    pub fn window_title(&self) -> Option<String> {
        if self.config.window_title.is_empty() {
            return None;
        }
        let workspace = self.workspace_label(self.active_ws);
        let tab = self
            .tab()
            .map(|tab| tab.display(&self.panes))
            .unwrap_or_default();
        let pane = self
            .focused_pane()
            .map(|pane| pane.label())
            .unwrap_or_default();
        let terminal_title = self
            .focused_pane()
            .map(|pane| pane.title.clone())
            .unwrap_or_default();
        let mut out = self.config.window_title.clone();
        for (token, value) in [
            ("{hostname}", hostname_text()),
            ("{workspace}", workspace),
            ("{tab}", tab),
            ("{pane}", pane),
            ("{terminal_title}", terminal_title),
        ] {
            out = out.replace(token, &value);
        }
        out = out.replace("{{", "\u{0}").replace("}}", "\u{1}");
        out = out.replace('\u{0}', "{").replace('\u{1}', "}");
        Some(out)
    }

    /// 仓库分支名，5 秒缓存，避免每帧跑 git。
    pub fn branch(&self, cwd: &str) -> Option<String> {
        self.branches.get(cwd).map(|(branch, _)| branch.clone())
    }

    /// 刷新分支缓存；切工作区或前台目录变化时调用。
    pub fn refresh_branch(&mut self, cwd: &str) {
        if self
            .branches
            .get(cwd)
            .is_some_and(|(_, at)| at.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        self.branch_probes.refresh(cwd);
    }

    /// core 里存在、但没有任何本地视图的终端（其它客户端创建的）。
    pub fn terminals_without_pane(&self) -> Vec<&crate::hc::proto::TerminalSummary> {
        self.terminals
            .iter()
            .filter(|terminal| {
                !self.panes.values().any(|pane| {
                    pane.terminal.as_deref() == Some(terminal.address.as_str()) && !pane.transient
                })
            })
            .collect()
    }

    /// 已有视图切焦点；外部终端在独立 Tab 打开，保留当前布局。
    pub fn open_terminal(&mut self, address: &str) {
        if let Some(pane) = self.pane_by_terminal(address) {
            self.focus_pane(pane);
            return;
        }
        if let Some(terminal) = self
            .terminals
            .iter()
            .find(|t| t.address == address)
            .cloned()
        {
            let space = terminal
                .space
                .clone()
                .unwrap_or_else(|| dir_label(terminal.cwd.clone(), "Terminals".to_string()));
            let machine = terminal.machine().to_string();
            let source = terminal.space_id.as_ref().map(|id| {
                if machine == "local" {
                    id.clone()
                } else {
                    format!("{machine}/{id}")
                }
            });
            if let Some(index) = (0..self.workspaces.len()).find(|index| {
                if let Some(source) = &source {
                    self.workspaces[*index].origin.as_ref() == Some(source)
                } else {
                    self.workspace_label(*index) == space
                        && self.workspace_machine(*index) == machine
                }
            }) {
                self.active_ws = index;
            } else {
                let id = self.alloc_workspace();
                self.workspaces.push(Workspace {
                    origin: source,
                    machine: (machine != "local").then_some(machine.clone()),
                    folder: terminal.cwd.clone(),
                    id,
                    label: space,
                    tabs: Vec::new(),
                    active: 0,
                });
                self.active_ws = self.workspaces.len() - 1;
            }
        }
        let pane = self.alloc_pane();
        let tab = self.alloc_tab();
        self.spawn_pane(pane);
        if !self.panes.contains_key(&pane) {
            return;
        }
        if let Some(state) = self.panes.get_mut(&pane) {
            state.terminal = Some(address.to_string());
        }
        if let Some(workspace) = self.workspaces.get_mut(self.active_ws) {
            workspace.tabs.push(Tab::new(tab, pane));
            workspace.active = workspace.tabs.len() - 1;
        }
        self.sync_terminals();
        self.zoom = false;
        self.mark_layout();
        self.focus_pane(pane);
        self.dirty = true;
    }

    fn commit_prompt(&mut self, prompt: Prompt) {
        let text = prompt.buffer.trim().to_string();
        if prompt.kind.needs_confirm() && !matches!(text.as_str(), "y" | "Y" | "yes") {
            self.note = "cancelled".to_string();
            self.dirty = true;
            return;
        }
        match prompt.kind {
            PromptKind::Command => {
                let (name, arguments) = text
                    .split_once(char::is_whitespace)
                    .unwrap_or((&text, "{}"));
                let result = serde_json::from_str(arguments.trim())
                    .map_err(anyhow::Error::from)
                    .and_then(|args| self.execute_action(name, args));
                if let Err(error) = result {
                    self.note = error.to_string();
                }
            }
            PromptKind::NewTab => self.new_tab(Some(text)),
            PromptKind::NewWorkspace => self.new_workspace(Some(text)),
            PromptKind::RenameTab { workspace, tab } => {
                if let Some(tab) = self
                    .workspaces
                    .iter_mut()
                    .find(|item| item.id == workspace)
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
                if let Err(error) = self.rename_pane(pane, &text) {
                    self.note = error.to_string();
                }
            }
            PromptKind::RenameAgent(terminal) => {
                if let Err(error) = self.rename_terminal(&terminal, &text) {
                    self.note = error.to_string();
                }
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
                    self.note = t1("sending to {}", terminal);
                } else {
                    self.note = t("this pane has no terminal yet").to_string();
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
                    self.note = t1("sent {} keys", keys.len());
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
                    self.note = t1("waiting for: {}", text);
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
                            self.note = t1("sending {}", name);
                        }
                    }
                    Err(error) => self.note = t1("read failed: {}", error),
                }
            }
            PromptKind::AddMachine => {
                self.call_rpc("machine.add", "machine.add", json!({"target": text}));
                self.note = t1("adding machine {}", text);
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
                    self.note = t1("killing {}", terminal);
                } else {
                    self.note = t("this pane has no terminal yet").to_string();
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
        if self.workspaces.is_empty() {
            self.new_workspace(None);
            if let Some(tab) = self.workspaces[self.active_ws].tab_mut() {
                tab.label = label.unwrap_or_default();
            }
            return;
        }
        let pane = self.alloc_pane();
        let id = self.alloc_tab();
        self.spawn_pane(pane);
        let launch = self.new_pane_launch();
        if let Some(state) = self.panes.get_mut(&pane) {
            state.launch = launch;
        }
        let Some(workspace) = self.workspaces.get_mut(self.active_ws) else {
            return;
        };
        let mut tab = Tab::new(id, pane);
        tab.label = label.unwrap_or_default();
        workspace.tabs.push(tab);
        workspace.active = workspace.tabs.len() - 1;
        self.zoom = false;
        self.note = t("new tab").to_string();
        self.mark_layout();
    }

    fn new_workspace(&mut self, label: Option<String>) {
        let pane = self.alloc_pane();
        let tab = self.alloc_tab();
        let id = self.alloc_workspace();
        self.spawn_pane(pane);
        let launch = self.new_pane_launch();
        if let Some(state) = self.panes.get_mut(&pane) {
            state.launch = launch;
        }
        let label = label.unwrap_or_default();
        let mut workspace = Workspace::new(id, label, pane, tab);
        if let Some(launch) = self.panes.get(&pane).and_then(|p| p.launch.as_ref()) {
            workspace.machine = launch.on.clone();
            workspace.folder = launch.cwd.clone();
        }
        self.workspaces.push(workspace);
        self.active_ws = self.workspaces.len() - 1;
        self.zoom = false;
        self.note = t("new workspace").to_string();
        self.mark_layout();
    }

    fn split_focused(&mut self, axis: Axis) {
        let launch = self.new_pane_launch();
        let floating = self
            .workspace()
            .and_then(Workspace::tab)
            .is_some_and(|tab| tab.floating_of(self.focused()).is_some());
        if floating {
            // 浮动的 Pane 分屏：新 Pane 也浮起来，错开压在旁边。
            let pane = self.focused();
            let new = self.alloc_pane();
            self.spawn_pane(new);
            if let Some(state) = self.panes.get_mut(&new) {
                state.launch = launch;
            }
            if let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) {
                tab.float_beside(new, pane);
                tab.focus = new;
            }
            self.note = t("floating split").to_string();
            self.mark_layout();
            self.dirty = true;
            return;
        }
        match self.split_new(axis) {
            Some(new) => {
                self.spawn_pane(new);
                if let Some(state) = self.panes.get_mut(&new) {
                    state.launch = launch;
                }
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
        for placement in candidates.iter().filter(|placement| !placement.collapsed) {
            let inner = inner_rect(placement.rect, ratatui_widgets::borders::Borders::ALL);
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
        #[derive(Clone, Copy, PartialEq)]
        enum Target {
            Pane(PaneId),
            Mod(usize),
        }
        let mut rects: Vec<(Target, Rect)> = self
            .workspace()
            .and_then(Workspace::tab)
            .map(|tab| tab.tree.layout(self.content_rect()))
            .unwrap_or_default()
            .into_iter()
            .map(|placement| (Target::Pane(placement.pane), placement.rect))
            .collect();
        // `focusable` 的模块区域按外框顺序并入循环；没有表面的区域不参与。
        for (index, region) in self.mod_focusables().iter().enumerate() {
            rects.push((Target::Mod(index), region.1));
        }
        let in_mod = self
            .focused_mod
            .as_ref()
            .and_then(|id| self.mod_focusables().iter().position(|(mid, _)| mid == id));
        let from = match in_mod {
            Some(index) => Target::Mod(index),
            None => Target::Pane(PaneId(self.focused())),
        };
        if let Some(next) = neighbor_by(&rects, from, direction) {
            match next {
                Target::Pane(pane) => {
                    self.focus_mod_surface(None);
                    self.set_focus(pane.0);
                }
                Target::Mod(index) => {
                    if let Some((id, _)) = self.mod_focusables().get(index) {
                        self.focus_mod_surface(Some(id.clone()));
                    }
                }
            }
        }
    }

    /// 参与 `pane.go` 焦点循环的模块区域：未折叠且有运行中的表面。
    fn mod_focusables(&self) -> Vec<(String, Rect)> {
        if self.config.no_mods {
            return Vec::new();
        }
        self.mod_regions
            .iter()
            .filter(|region| !region.folded)
            .filter_map(|region| {
                let surface = self.mods.surfaces.iter().find(|surface| {
                    surface.mod_id == region.id
                        && matches!(
                            surface.place,
                            crate::mods::SurfacePlace::Region | crate::mods::SurfacePlace::Pane
                        )
                })?;
                matches!(
                    surface.state,
                    crate::mods::SurfaceState::Running | crate::mods::SurfaceState::Loading
                )
                .then(|| (surface.id.clone(), region.rect))
            })
            .collect()
    }

    /// 鼠标按在浮动 Pane 上时判断这一下是移动还是改尺寸。
    fn floating_grab(&self, slot: &Slot, mouse: MouseEvent) -> Option<FloatingDragMode> {
        self.workspace()
            .and_then(Workspace::tab)
            .and_then(|tab| tab.floating_of(slot.pane))?;
        grab_zone(slot.outer, mouse.column, mouse.row)
    }

    fn drag_floating(&mut self, mouse: MouseEvent) {
        let Some(drag) = self.floating_drag else {
            return;
        };
        let content = self.content_rect();
        let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) else {
            return;
        };
        let Some(entry) = tab
            .floating
            .iter_mut()
            .find(|entry| entry.pane == drag.pane)
        else {
            self.floating_drag = None;
            return;
        };
        let current = entry.rect(content);
        let rect = match drag.mode {
            FloatingDragMode::Move => {
                let x = mouse.column.saturating_sub(drag.grab.0).max(content.x);
                let y = mouse.row.saturating_sub(drag.grab.1).max(content.y);
                Rect::new(
                    x.min(content.x + content.width.saturating_sub(current.width)),
                    y.min(content.y + content.height.saturating_sub(current.height)),
                    current.width,
                    current.height,
                )
            }
            FloatingDragMode::Resize => {
                let max_width = content
                    .x
                    .saturating_add(content.width)
                    .saturating_sub(current.x);
                let max_height = content
                    .y
                    .saturating_add(content.height)
                    .saturating_sub(current.y);
                Rect::new(
                    current.x,
                    current.y,
                    clamp_extent(
                        mouse.column.saturating_sub(current.x) + 1,
                        FloatingPane::MIN_WIDTH,
                        max_width,
                    ),
                    clamp_extent(
                        mouse.row.saturating_sub(current.y) + 1,
                        FloatingPane::MIN_HEIGHT,
                        max_height,
                    ),
                )
            }
        };
        entry.place(rect, content);
        self.mark_layout();
        self.dirty = true;
    }

    /// 按预设重排当前 Tab 的分屏，`delta` 为 +1/-1 时向后/向前换一个。
    fn cycle_layout(&mut self, delta: isize) {
        let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) else {
            return;
        };
        let panes = tab.tree.panes();
        if panes.len() < 2 {
            self.note = "布局预设需要至少两个分屏 Pane".to_string();
            self.dirty = true;
            return;
        }
        let count = crate::layout::preset::NAMES.len() as isize;
        let index = match tab.layout {
            Some(index) => (index as isize + delta).rem_euclid(count) as usize,
            None if delta >= 0 => 0,
            None => (count - 1) as usize,
        };
        tab.tree = crate::layout::preset::build(index, &panes);
        tab.layout = Some(index);
        self.mark_layout();
        self.note = t1("layout · {}", crate::layout::preset::NAMES[index]);
        self.dirty = true;
    }

    /// 焦点 Pane 在分屏与浮动层之间切换。
    fn toggle_float(&mut self) {
        let pane = self.focused();
        let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) else {
            return;
        };
        if tab.floating_of(pane).is_some() {
            tab.unfloat(pane);
            self.note = "浮动 Pane 回到分屏".to_string();
        } else if tab.float(pane) {
            self.note = "Pane 浮起：拖标题行移动，拖右下角改尺寸".to_string();
        } else {
            self.note = "分屏里只剩一个 Pane，没有可让出的空间".to_string();
            self.dirty = true;
            return;
        }
        self.floating_hidden = false;
        self.mark_layout();
        self.dirty = true;
    }

    fn toggle_float_pin(&mut self) {
        let pane = self.focused();
        let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) else {
            return;
        };
        let Some(entry) = tab.floating.iter_mut().find(|entry| entry.pane == pane) else {
            self.note = "这个 Pane 不在浮动层里".to_string();
            self.dirty = true;
            return;
        };
        entry.pinned = !entry.pinned;
        self.note = if entry.pinned {
            "浮动 Pane 已 pin".to_string()
        } else {
            "浮动 Pane 取消 pin".to_string()
        };
        self.mark_layout();
        self.dirty = true;
    }

    fn toggle_floating_hidden(&mut self) {
        let has_floating = self
            .workspace()
            .and_then(Workspace::tab)
            .is_some_and(|tab| !tab.floating.is_empty());
        if !has_floating {
            self.note = "没有浮动 Pane".to_string();
            self.dirty = true;
            return;
        }
        self.floating_hidden = !self.floating_hidden;
        self.note = if self.floating_hidden {
            "浮动层收起".to_string()
        } else {
            "浮动层展开".to_string()
        };
        self.dirty = true;
    }

    /// 把当前 Tab 的所有 Pane 收成一个 Stack，焦点那个展开。
    fn stack_tab_panes(&mut self) {
        let focus = self.focused();
        let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) else {
            return;
        };
        let panes = tab.tree.panes();
        if panes.len() < 2 {
            self.note = "stack 需要至少两个 pane".to_string();
            self.dirty = true;
            return;
        }
        let active = panes.iter().position(|pane| pane.0 == focus).unwrap_or(0);
        tab.tree = SplitNode::stack(panes.clone(), active);
        self.zoom = false;
        self.mark_layout();
        self.note = t1("stacked {} panes · prefix+方向切换", panes.len());
        self.dirty = true;
    }

    fn cycle_pane(&mut self, delta: isize) {
        let Some(tab) = self.tab() else {
            return;
        };
        let panes = tab.panes();
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
            self.note = t("this pane has no terminal yet").to_string();
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
        self.note = t2("{} keep = {}", terminal, !keep);
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
            self.note = t("this pane has no terminal yet").to_string();
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
            self.note = t1(
                "{} is still running; kill it first with prefix shift+k",
                terminal,
            );
            self.dirty = true;
            return;
        }
        self.call_rpc("rm", "terminal.rm", json!({"terminal": terminal}));
        self.remove_pane_view(pane);
        self.note = t1("removed {}", terminal);
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
            self.note = t("no link under cursor").to_string();
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
            Ok(_) => self.note = t1("opening {}", uri),
            Err(error) => self.note = t1("cannot open link: {}", error),
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
            self.note = t1("cannot write temp file: {}", error);
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
        self.note = t("popup open").to_string();
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
        if self.focused_mod.is_some() {
            self.focus_mod_surface(None);
        }
        let previous = self.focused();
        self.send(Command::Focus {
            pane: previous,
            focused: false,
        });
        if let Some(tab) = self.workspace_mut().and_then(Workspace::tab_mut) {
            tab.focus = pane;
            tab.raise_floating(pane);
        }
        self.reveal_floating(pane);
        self.send(Command::Focus {
            pane,
            focused: true,
        });
        self.last_pane = Some(previous);
        self.mark_seen(pane);
        self.mark_layout();
        self.bump_mod_revision_domains(&["focused"]);
        self.send_focus_changed();
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
                tab.settled = false;
                tab.raise_floating(pane);
            }
        }
        self.reveal_floating(pane);
        self.send(Command::Focus {
            pane,
            focused: true,
        });
        self.mark_seen(pane);
        self.dirty = true;
    }

    /// 焦点落到一个收起的浮动 Pane 上时，把浮动层重新展开。
    fn reveal_floating(&mut self, pane: u32) {
        if !self.floating_hidden {
            return;
        }
        let floating = self
            .workspace()
            .and_then(Workspace::tab)
            .is_some_and(|tab| tab.floating_of(pane).is_some());
        if floating {
            self.floating_hidden = false;
        }
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
        let changed = state.agent.turn > state.agent.seen_turn;
        let address = state.terminal.clone();
        if state.agent.badge() == Badge::Done {
            if let Some(address) = address.clone() {
                self.held_done = Some((address, state.agent.turn));
            }
        } else if self.held_done.as_ref().is_some_and(|(held, turn)| {
            Some(held) != address.as_ref()
                || *turn != state.agent.turn
                || !matches!(state.agent.status.as_deref(), Some("idle" | "done"))
        }) {
            self.held_done = None;
        }
        state.agent.mark_seen();
        let acknowledged = state
            .terminal
            .clone()
            .map(|terminal| (terminal, state.agent.turn));
        if let Some(terminal) = state.terminal.clone() {
            let turn = state.agent.turn;
            self.seen.insert(terminal, turn);
            self.seen_dirty = true;
            self.seen_since = Some(Instant::now());
        }
        if changed {
            if let Some((terminal, turn)) = acknowledged {
                self.update_terminal(Some(&terminal), |summary| summary.seen_turn = turn);
                self.call_rpc(
                    "seen",
                    "terminal.label",
                    json!({"terminal": terminal, "labels": {"herdr.seen": turn.to_string()}}),
                );
            }
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
                    .unwrap_or_else(|| t1("pane {}", pane));
                self.focus_pane(pane);
                self.note = format!("{} · {}", t(badge.text()), label);
            }
            None => {
                let external = self
                    .navigation_rows()
                    .into_iter()
                    .filter(|row| {
                        row.pane.is_none()
                            && matches!(row.badge, Badge::Blocked | Badge::Error | Badge::Done)
                    })
                    .min_by_key(|row| row.badge)
                    .and_then(|row| row.terminal);
                if let Some(address) = external {
                    self.open_terminal(&address);
                } else {
                    self.note = t("no agent needs attention").to_string();
                }
            }
        }
        self.dirty = true;
    }

    fn close_focused_pane(&mut self) {
        let pane = self.focused();
        if self.tab().is_some_and(|t| t.panes().len() == 1) && self.config.confirm_close {
            self.ask(
                PromptKind::CloseTab(self.active_ws),
                t("End the last terminal and close this tab? (y/N)"),
            );
            return;
        }
        if let Some(address) = self.panes.get(&pane).and_then(|p| p.terminal.clone()) {
            self.kill_terminal_tree(&address);
        }
        self.detach_view_from_layout(pane);
    }

    /// 只断视图，不杀进程。
    fn remove_pane_view(&mut self, pane: u32) {
        self.release_plugin_terminal(pane);
        self.send(Command::Close { pane });
        self.panes.remove(&pane);
        if self.copy.as_ref().is_some_and(|copy| copy.pane == pane) {
            self.copy = None;
        }
    }

    fn close_tab_in(&mut self, workspace_index: usize) {
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
            if let Some(address) = self.panes.get(&pane.0).and_then(|p| p.terminal.clone()) {
                self.kill_terminal_tree(&address);
            }
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
        let workspace = self.workspaces.remove(index);
        for tab in &workspace.tabs {
            for pane in tab.panes() {
                if let Some(address) = self.panes.get(&pane.0).and_then(|p| p.terminal.clone()) {
                    self.kill_terminal_tree(&address);
                }
                self.remove_pane_view(pane.0);
            }
        }
        if self.active_ws >= self.workspaces.len() {
            self.active_ws = self.workspaces.len().saturating_sub(1);
        } else if index < self.active_ws {
            self.active_ws -= 1;
        }
        self.zoom = false;
        self.mark_layout();
    }

    fn select_tab(&mut self, index: usize) {
        let Some(workspace) = self.workspaces.get_mut(self.active_ws) else {
            return;
        };
        if index >= workspace.tabs.len() {
            return;
        }
        workspace.tabs[index].settled = false;
        if index == workspace.active {
            self.dirty = true;
            return;
        }
        let previous = workspace.focus().unwrap_or_default();
        workspace.active = index;
        workspace.tabs[index].settled = false;
        let next = workspace.focus().unwrap_or_default();
        self.zoom = false;
        self.note = t1("tab {}", index + 1);
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
        self.note = t1("workspace {}", index + 1);
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
        let Some(ws) = self.workspace() else {
            return;
        };
        let visible: Vec<_> = ws
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.settled)
            .map(|(i, _)| i)
            .collect();
        if visible.is_empty() {
            return;
        }
        let current = visible.iter().position(|i| *i == ws.active).unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(visible.len() as isize) as usize;
        self.select_tab(visible[next]);
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
        self.note = t2("tab {} → {}", from + 1, to + 1);
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
            self.note = t("invalid target tab").to_string();
            self.dirty = true;
            return;
        }
        let target_id = workspace.tabs[target_index].id;
        let previous = workspace.tab().map(|tab| tab.focus).unwrap_or_default();
        let floating = self.workspaces[workspace_index].tabs[source_index]
            .floating
            .iter()
            .position(|entry| entry.pane == pane);
        if let Some(index) = floating {
            let tab = &mut self.workspaces[workspace_index].tabs[source_index];
            tab.floating.remove(index);
            let panes = tab.tree.panes();
            tab.focus = panes.first().map(|pane| pane.0).unwrap_or_default();
        } else {
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
                    let tab = &mut self.workspaces[workspace_index].tabs[source_index];
                    if !tab.promote_floating() {
                        self.workspaces[workspace_index].tabs.remove(source_index);
                    }
                }
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
        let focus = tab
            .tree
            .panes()
            .iter()
            .find(|id| id.0 == tab.focus)
            .copied()
            .or_else(|| tab.tree.panes().last().copied())
            .map(|id| id.0);
        let Some(focus) = focus else {
            self.note = t("no room in the target tab").to_string();
            self.dirty = true;
            return;
        };
        if !tab.tree.split_pane(PaneId(focus), axis, PaneId(pane), 0.5) {
            self.note = t("no room in the target tab").to_string();
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
        self.note = t1("pane moved to tab {}", target_index + 1);
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
        let floating = self.workspaces[workspace_index].tabs[source_index]
            .floating
            .iter()
            .position(|entry| entry.pane == pane);
        if let Some(index) = floating {
            self.workspaces[workspace_index].tabs[source_index]
                .floating
                .remove(index);
        } else {
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
                    let tab = &mut self.workspaces[workspace_index].tabs[source_index];
                    if !tab.promote_floating() {
                        self.workspaces[workspace_index].tabs.remove(source_index);
                    }
                }
            }
        }
        let id = self.alloc_tab();
        let workspace = &mut self.workspaces[workspace_index];
        workspace.tabs.push(Tab::new(id, pane));
        workspace.active = workspace.tabs.len() - 1;
        self.zoom = false;
        self.note = t("pane moved into a new tab").to_string();
        self.mark_layout();
    }

    fn spawn_pane(&mut self, pane: u32) {
        let view = match TerminalView::new(80, 24, self.config.scrollback_bytes) {
            Ok(view) => view,
            Err(error) => {
                self.note = t1("cannot create local VT: {}", error);
                return;
            }
        };
        self.panes.insert(pane, Pane::new(view));
    }

    /// 新建 Pane 时的 shell 启动参数：按 `terminal.new_cwd` 决定目录。
    fn new_pane_launch(&self) -> Option<Launch> {
        let machine = self.workspace_machine(self.active_ws);
        let on = (machine != "local").then_some(machine);
        let cwd = match &self.config.new_cwd {
            crate::config::NewCwd::Follow => self
                .focused_pane()
                .and_then(|pane| pane.cwd.clone())
                .or_else(|| {
                    if on.is_some() {
                        return None;
                    }
                    std::env::current_dir()
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned())
                }),
            crate::config::NewCwd::Home if on.is_some() => None,
            crate::config::NewCwd::Home => std::env::var("HOME").ok(),
            crate::config::NewCwd::Current => std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned()),
            crate::config::NewCwd::Path(path) => Some(expand_home(path)),
        };
        let argv = if self.config.default_shell.is_empty() {
            Vec::new()
        } else {
            vec![self.config.default_shell.clone()]
        };
        if argv.is_empty() && cwd.is_none() && on.is_none() {
            None
        } else {
            Some(Launch {
                argv,
                cwd,
                on,
                env: Default::default(),
            })
        }
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
        self.panes
            .iter()
            .filter(|(_, state)| state.terminal.as_deref() == Some(terminal))
            .min_by_key(|(id, _)| {
                (
                    **id != self.focused(),
                    !self.is_visible(**id),
                    self.locate(**id).unwrap_or((usize::MAX, usize::MAX)),
                    **id,
                )
            })
            .map(|(id, _)| *id)
    }

    // ------------------------------------------------------------- 网络事件

    fn load_local_state(&mut self) {
        let Ok(text) = std::fs::read_to_string(&self.config.state_path) else {
            return;
        };
        let Ok(state) = serde_json::from_str::<LocalState>(&text) else {
            return;
        };
        if state.schema_version <= 4 {
            self.seen = state.seen;
            self.folded_agent_groups = state.folded_agent_groups;
            // 外框树缺失、非法或 --no-mods 时保持默认树。
            let state_chrome = if self.config.no_mods {
                None
            } else {
                crate::mods::chrome::Chrome::from_state(state.mods_chrome.as_ref())
            };
            if let Some(chrome) = state_chrome {
                self.chrome = chrome;
            }
            self.mods_windows = state.mods_windows;
            self.mods_docked = state.mods_docked;
            self.mods.grants_from_state(state.mods_grants.as_ref());
            self.load_mod_windows();
        }
    }

    fn save_local_state(&mut self) {
        self.seen_dirty = false;
        self.seen_since = None;
        let state = LocalState {
            schema_version: 4,
            seen: self.seen.clone(),
            folded_agent_groups: self.folded_agent_groups.clone(),
            mods_chrome: Some(self.chrome.to_state()),
            mods_windows: self.mods_windows.clone(),
            mods_grants: Some(self.mods.grants_to_state()),
            mods_docked: self.mods_docked.clone(),
        };
        let path = self.config.state_path.clone();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(&state) {
            let _ = std::fs::write(path, text);
        }
    }

    /// 覆盖层与 Pane 内表面的遮挡矩形；region 已由 `mod_regions` 覆盖。
    pub fn mods_windows_blockers(&self) -> Vec<ratatui_core::layout::Rect> {
        self.mods
            .surfaces
            .iter()
            .filter(|surface| {
                !matches!(surface.place, crate::mods::SurfacePlace::Region)
                    && surface.area.width > 0
                    && surface.area.height > 0
            })
            .map(|surface| surface.area)
            .collect()
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
        let machine = self.workspace_machine(self.active_ws);
        let on = (machine != "local").then_some(machine);
        match command.kind {
            CommandKind::Action => {
                let (name, arguments) = command
                    .command
                    .split_once(char::is_whitespace)
                    .unwrap_or((&command.command, "{}"));
                let result = serde_json::from_str(arguments.trim())
                    .map_err(anyhow::Error::from)
                    .and_then(|args| self.execute_action(name, args));
                if let Err(error) = result {
                    self.note = error.to_string();
                }
            }
            CommandKind::Shell => {
                self.call_rpc(
                    "custom.exec",
                    "machine.exec",
                    json!({"line":command.command,"cwd":cwd,"on":on}),
                );
                self.note = t1("running in background: {}", command.command);
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
                        let launch = Launch {
                            argv: shell_argv(&command.command),
                            cwd,
                            on: on.clone(),
                            env: Default::default(),
                        };
                        if let Some(state) = self.panes.get_mut(&pane) {
                            state.transient = true;
                            state.launch = Some(launch);
                        }
                        self.note = t1("pane: {}", command.command);
                    }
                    None => {
                        self.note = t2(
                            "no room: panes need at least {}x{}",
                            MIN_PANE_WIDTH,
                            MIN_PANE_HEIGHT,
                        )
                    }
                }
            }
            CommandKind::Popup => {
                self.open_popup(shell_argv(&command.command), cwd);
                if let Some(pane) = self.popup.and_then(|p| self.panes.get_mut(&p.pane)) {
                    if let Some(launch) = pane.launch.as_mut() {
                        launch.on = on;
                    }
                }
                if let Some(popup) = &mut self.popup {
                    popup.width = command.width.unwrap_or(Span::Percent(80));
                    popup.height = command.height.unwrap_or(Span::Percent(80));
                }
                self.note = t1("popup: {}", command.command);
            }
        }
        self.dirty = true;
    }

    fn close_popup(&mut self) {
        if let Some(popup) = self.popup.take() {
            self.remove_pane_view(popup.pane);
            self.note = t("popup closed; the program keeps running in core").to_string();
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
            self.note = t("this pane has no terminal yet").to_string();
            self.dirty = true;
            return;
        };
        self.call_rpc(
            "read",
            "terminal.read",
            json!({"terminal": terminal, "lines": 500}),
        );
        self.note = t1("reading {}…", terminal);
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
                self.note = t("reader closed").to_string();
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
                    self.note = t("removing machine").to_string();
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
                    self.note = t1("connecting {}", machine);
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
            ChooserKind::Actions => self.activate_pointer_action(&item.value),
            ChooserKind::Grants => self.approve_mod_grant(&item.value),
            ChooserKind::Templates => self.spawn_program_in_new_tab(vec![item.value], None),
            ChooserKind::Machines => self.open_remote_pane(item.value),
            ChooserKind::Commands => {
                let (name, args) = item.value.split_once(' ').unwrap_or((&item.value, "{}"));
                let parsed = serde_json::from_str::<Value>(args);
                // 带参数 schema 的插件动作先开表单；没字段、停用或解析失败走原路。
                if let Ok(arguments) = &parsed {
                    if self.open_plugin_form(name, arguments.clone()) {
                        self.dirty = true;
                        return;
                    }
                }
                let result = parsed
                    .map_err(anyhow::Error::from)
                    .and_then(|args| self.execute_action(name, args));
                if let Err(error) = result {
                    self.note = error.to_string();
                }
            }
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
                self.note = t1("opening a pane on {}", machine);
            }
            None => self.note = t("no room").to_string(),
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

    /// 上传本机剪贴板图片，再将目标机器上的路径贴给原程序。
    fn paste_image(&mut self) {
        match clipboard::image() {
            Ok(bytes) => self.upload_image(bytes),
            Err(error) => {
                self.note = error;
                self.dirty = true;
            }
        }
    }

    fn upload_image(&mut self, bytes: Vec<u8>) {
        self.upload_image_file(bytes, "png");
    }

    fn upload_image_file(&mut self, bytes: Vec<u8>, extension: &str) {
        if bytes.len() > clipboard::MAX_IMAGE_BYTES {
            self.note = t("image was not uploaded: core accepts at most 8 MiB").into();
            self.dirty = true;
            return;
        }
        let pane = self
            .chat
            .as_ref()
            .map(|chat| chat.pane)
            .unwrap_or_else(|| self.input_pane());
        let Some(terminal) = self
            .panes
            .get(&pane)
            .filter(|state| state.is_live())
            .and_then(|state| state.terminal.clone())
        else {
            self.note = t("image was not uploaded: target terminal is not connected").to_string();
            self.dirty = true;
            return;
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let mut tag = format!("put-image:{pane}:{stamp}");
        if let Some(chat) = self.chat.as_ref() {
            tag.push_str(&format!(":chat:{}", chat.request_tag));
        }
        self.call_rpc(
            &tag,
            "terminal.put",
            json!({ "terminal": terminal, "name": format!("paste-{stamp}.{extension}"), "data": crate::clipboard::base64(&bytes) }),
        );
        self.note = t("pasting clipboard image").to_string();
        self.dirty = true;
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
                self.bump_mod_revision();
                self.notify_mods_ready();
                if self.restore_pending {
                    self.request_layout("doc.get");
                } else {
                    self.note = "subscribing".to_string();
                    if self.config.restore {
                        let machines: Vec<_> = self.shared_docs.keys().cloned().collect();
                        for machine in machines {
                            self.request_shared(&machine);
                        }
                        self.call_rpc("machines.sync", "machine.list", json!({}));
                    }
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
            NetEvent::InputRejected { pane, bytes } => {
                self.note = t2(
                    "Pane {}: input buffer full; {} new bytes were not sent",
                    pane,
                    bytes,
                );
                self.dirty = true;
            }
            NetEvent::Snapshot { seq, terminals } => {
                let live: std::collections::BTreeSet<String> = terminals
                    .iter()
                    .filter(|terminal| terminal.live)
                    .map(|terminal| self.terminal_address(&terminal.address).key())
                    .collect();
                self.reconcile_plugin_pins(&live);
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
                self.sync_terminals();
                self.label_plugin_terminal(pane, &terminal);
                self.publish_event("terminal.opened", Some(&terminal), json!({}));
                if let Some(task) = self.initial_tasks.remove(&pane) {
                    self.call_rpc(
                        &format!("send-task:{pane}"),
                        "terminal.send",
                        json!({"terminal":terminal,"text":task}),
                    );
                }
                if pane == self.focused() {
                    self.send(Command::Focus {
                        pane,
                        focused: true,
                    });
                }
                self.note = if resize_notifications {
                    t1("attached {} (resize notifications on)", terminal)
                } else {
                    t1("attached {}", terminal)
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
                let address = self
                    .panes
                    .get(&pane)
                    .and_then(|state| state.terminal.clone());
                if let Some(state) = self.panes.get_mut(&pane) {
                    state.state = PaneState::Ended {
                        reason: reason.clone(),
                    };
                }
                if program_exit {
                    self.publish_event(
                        "terminal.finished",
                        address.as_deref(),
                        json!({"reason": reason}),
                    );
                    // 程序结束是 core 侧事实：终结证据。拿不到地址时至少释放占位 pin。
                    match address.as_deref() {
                        Some(address) => self.plugin_terminal_ended(address),
                        None => self.release_pending_plugin_pin(pane),
                    }
                }
                // 插件终端还没拿到地址就断开：请求直接失败，不让状态停在 opening。
                if let Some(terminal) = self.plugin_terminals.get(&pane) {
                    if let Some(record) = self.plugin_panes.get_mut(&terminal.request) {
                        if record.terminal.is_none() {
                            record.status = plugin::PaneStatus::Failed {
                                kind: crate::plugin::terminal::UNAVAILABLE.to_string(),
                                message: reason.clone(),
                            };
                        }
                    }
                }
                if program_exit && self.popup.is_some_and(|popup| popup.pane == pane) {
                    self.popup = None;
                    self.remove_pane_view(pane);
                    self.note = t1("popup ended: {}", reason);
                    self.dirty = true;
                    return;
                }
                if program_exit {
                    self.detach_view_from_layout(pane);
                    self.note = format!("程序结束：{reason}");
                } else {
                    self.note = format!("stream 断开：{reason}");
                }
                self.dirty = true;
            }
        }
    }

    fn on_reply(
        &mut self,
        tag: &str,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) {
        // `run` 发起的异步动作：先把终态写回请求记录并投递 `action.finished`。
        let outcome = result
            .as_ref()
            .cloned()
            .map_err(|error| error.message.clone());
        self.settle_mod_action_rpc(tag, &outcome);
        self.on_reply_inner(tag, result);
    }

    fn on_reply_inner(
        &mut self,
        tag: &str,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) {
        let request = self.inflight.remove(tag);
        let chat_request = tag
            .starts_with("chat:")
            .then(|| request.as_ref().map(|(_, params)| params.clone()))
            .flatten();
        let image_terminal = request
            .as_ref()
            .and_then(|(_, params)| params.get("terminal"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some((method, params)) = request {
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
        if tag.starts_with("chat:") {
            if let (Some(chat), Some(params)) = (self.chat.as_mut(), chat_request) {
                self.dirty |=
                    chat.remote_reply(tag, &params, result.map_err(|error| error.message));
            }
            return;
        }
        if tag.starts_with("mod-rpc-") {
            let outcome = result.map_err(|error| error.message);
            self.settle_mod_rpc(tag, &outcome);
            return;
        }
        if let Some(request) = tag.strip_prefix("plugin-probe:") {
            self.plugin_probe_reply(request, result);
            return;
        }
        if let Some(id) = tag
            .strip_prefix("panel:")
            .and_then(|s| s.parse::<u32>().ok())
        {
            self.panel_reply(id, result);
            return;
        }
        if tag.starts_with("kill:") {
            if let Err(error) = result {
                self.note = t1("Kill failed: {}", error.message);
            }
            return;
        }
        if tag.starts_with("svc:") {
            self.service_reply(tag, result);
            self.dirty = true;
            return;
        }
        if tag.starts_with("shared.") {
            self.shared_reply(tag, result);
            return;
        }
        if let Some(pane) = tag
            .strip_prefix("put-image:")
            .and_then(|rest| rest.split(':').next())
            .and_then(|id| id.parse::<u32>().ok())
        {
            match result {
                Ok(value) => {
                    let path = value
                        .get("path")
                        .and_then(Value::as_str)
                        .filter(|path| !path.is_empty());
                    if let Some(path) = path {
                        if let Some(state) = self
                            .panes
                            .get_mut(&pane)
                            .filter(|state| state.terminal == image_terminal && state.is_live())
                        {
                            state.view.scroll_bottom();
                            if let Some((_, chat_id)) = tag.split_once(":chat:") {
                                if let Some(chat) = self
                                    .chat
                                    .as_mut()
                                    .filter(|chat| chat.pane == pane && chat.request_tag == chat_id)
                                {
                                    chat.insert(&format!("{path} "));
                                    self.note = t1("image attached · {}", path);
                                } else {
                                    self.note = t1("image uploaded · chat closed · {}", path);
                                }
                            } else {
                                let bytes = state.view.encode_paste(path);
                                self.send(Command::Input { pane, bytes });
                                self.note = t1("image sent · {}", path);
                            }
                        } else {
                            self.note = t1("image uploaded · target pane closed · {}", path);
                        }
                    } else {
                        self.note = t("send image failed: core returned no file path").to_string();
                    }
                }
                Err(error) => self.note = t1("send image failed: {}", error.message),
            }
            self.dirty = true;
            return;
        }
        match tag {
            "doc.get" | "doc.sync" | "doc.set" => {
                self.layout_reply(tag, result);
            }
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
                        title: t1("read · {}", tv(&how)),
                        lines,
                        scroll,
                        complete,
                        how,
                    });
                    self.dirty = true;
                }
                Err(error) => self.note = t1("read failed: {}", error.message),
            },
            "keys" => match result {
                Ok(value) => {
                    let turn = value.get("turn").and_then(Value::as_u64).unwrap_or(0);
                    let status = value
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    self.note = t2("keys delivered · {} · turn {}", status, turn);
                }
                Err(error) => self.note = t1("keys failed: {}", error.message),
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
                Err(error) => self.note = t1("wait failed: {}", error.message),
            },
            "put" => match result {
                Ok(value) => {
                    let path = value
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.note = t1("file delivered to {}", path);
                }
                Err(error) => self.note = t1("send file failed: {}", error.message),
            },
            "templates" => match result {
                Ok(value) => {
                    // 模块快照的 `templates` 域与选择器读同一份缓存。
                    self.mod_templates = value
                        .get("templates")
                        .and_then(Value::as_array)
                        .map(|templates| {
                            templates
                                .iter()
                                .filter_map(|template| {
                                    let name =
                                        template.get("name").and_then(Value::as_str)?.to_string();
                                    Some(json!({
                                        "name": name,
                                        "title": template
                                            .get("title")
                                            .and_then(Value::as_str)
                                            .unwrap_or(&name),
                                        "description": template
                                            .get("description")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default(),
                                    }))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    self.bump_mod_revision_domains(&["templates"]);
                    let items = self
                        .mod_templates
                        .iter()
                        .filter_map(|template| {
                            let name = template.get("name")?.as_str()?.to_string();
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
                        .collect();
                    self.open_chooser(Chooser {
                        title: "templates（Enter 在新 tab 里运行）".to_string(),
                        items,
                        selected: 0,
                        kind: ChooserKind::Templates,
                    });
                }
                Err(error) => self.note = t1("templates failed: {}", error.message),
            },
            "machines.sync" => {
                if let Ok(value) = result {
                    self.machines = parse_machines(&value);
                    let machines: Vec<_> = self
                        .machines
                        .iter()
                        .filter(|m| m.state == "connected")
                        .map(|m| m.name.clone())
                        .collect();
                    for machine in machines {
                        self.request_shared(&machine);
                    }
                    self.dirty = true;
                }
            }
            "custom.exec" => match result {
                Ok(value) => {
                    self.note = if value["timed_out"] == true {
                        t("Command timed out").into()
                    } else {
                        t1("Command finished · exit {}", &value["code"])
                    };
                    self.dirty = true;
                }
                Err(error) => self.note = error.message,
            },
            "panel.exec" | "panel.file" => match result {
                Ok(value) => {
                    use base64::Engine;
                    let text = if tag == "panel.file" {
                        value["text"].as_str().unwrap_or("").to_string()
                    } else {
                        let decode = |key: &str| {
                            base64::engine::general_purpose::STANDARD
                                .decode(value[key].as_str().unwrap_or(""))
                                .map(|b| String::from_utf8_lossy(&b).into_owned())
                                .unwrap_or_default()
                        };
                        format!("{}{}", decode("stdout"), decode("stderr"))
                    };
                    self.reader = Some(Reader {
                        title: t("Workspace").into(),
                        lines: text.lines().map(str::to_string).collect(),
                        scroll: 0,
                        complete: !value["truncated"].as_bool().unwrap_or(false),
                        how: "workspace".into(),
                    });
                    self.dirty = true;
                }
                Err(error) => self.note = error.message,
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
                        title: t("machines · Enter/n 新 pane · a 添加 · c 连接 · r 改名 · d 移除")
                            .to_string(),
                        items,
                        selected: 0,
                        kind: ChooserKind::Machines,
                    });
                }
                Err(error) => self.note = t1("machine list failed: {}", error.message),
            },
            "machine.add" => match result {
                Ok(value) => {
                    let name = value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.note = t1("added {}", name);
                    self.call_rpc("machines", "machine.list", json!({}));
                }
                Err(error) => self.note = t1("add failed: {}", error.message),
            },
            "machine.rm" => match result {
                Ok(_) => {
                    self.note = t("machine removed").to_string();
                    self.call_rpc("machines", "machine.list", json!({}));
                }
                Err(error) => self.note = t1("remove failed: {}", error.message),
            },
            "machine.connect" => match result {
                Ok(value) => {
                    let state = value
                        .get("state")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    self.note = t1("machine state {}", state);
                    self.call_rpc("machines", "machine.list", json!({}));
                }
                Err(error) => self.note = t1("connect failed: {}", error.message),
            },
            "machine.rename" => match result {
                Ok(_) => {
                    self.note = t("machine renamed").to_string();
                    self.call_rpc("machines", "machine.list", json!({}));
                }
                Err(error) => self.note = t1("rename failed: {}", error.message),
            },
            "send" => match result {
                Ok(_) => self.note = t("sent; waiting for the turn to start").to_string(),
                Err(error) => self.note = t1("send failed: {}", error.message),
            },
            "interrupt" => match result {
                Ok(value) => {
                    let state = value
                        .get("status")
                        .and_then(|status| status.get("state"))
                        .and_then(Value::as_str)
                        .unwrap_or("done");
                    self.note = t1("interrupted · state {}", state);
                }
                Err(error) => self.note = t1("interrupt failed: {}", error.message),
            },
            "keep" => match result {
                Ok(value) => {
                    let keep = value.get("keep").and_then(Value::as_bool).unwrap_or(false);
                    self.note = t1("keep = {}", keep);
                }
                Err(error) => self.note = t1("keep failed: {}", error.message),
            },
            "rm" => match result {
                Ok(_) => self.note = t("terminal record removed").to_string(),
                Err(error) => self.note = t1("remove failed: {}", error.message),
            },
            "label" => match result {
                Ok(_) => {}
                Err(error) => {
                    self.note = t1("rename failed: {}", error.message);
                    self.dirty = true;
                }
            },
            "kill" => match result {
                Ok(value) => {
                    let _ = value;
                    self.note = t("program finished; prefix c starts a new tab").to_string();
                }
                Err(error) => self.note = t1("kill failed: {}", error.message),
            },
            _ => {
                if let Err(error) = result {
                    self.note = format!("{tag}: {}", error.message);
                    self.dirty = true;
                }
            }
        }
    }

    fn mark_layout(&mut self) {
        self.assign_origins();
        self.shared_dirty = true;
        self.layout_dirty = true;
        self.layout_since = Some(Instant::now());
        self.dirty = true;
        self.bump_mod_revision_domains(&["spaces", "tabs", "panes"]);
    }

    pub fn snapshot(&self) -> SavedState {
        let mut panes = BTreeMap::new();
        for (id, pane) in &self.panes {
            panes.insert(
                id.to_string(),
                SavedPane {
                    origin: pane.origin.clone(),
                    module: pane.module.clone(),
                    terminal: self
                        .shown_original
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| pane.terminal.clone()),
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
                    origin: workspace.origin.clone(),
                    machine: workspace.machine.clone(),
                    folder: workspace.folder.clone(),
                    id: workspace.id,
                    label: workspace.label.clone(),
                    active_tab: workspace.active,
                    tabs: workspace
                        .tabs
                        .iter()
                        .map(|tab| SavedTab {
                            origin: tab.origin.clone(),
                            settled: tab.settled,
                            id: tab.id,
                            label: tab.label.clone(),
                            focus: tab.focus,
                            tree: SavedNode::from(&tab.tree),
                            floating: tab.floating.clone(),
                            layout: tab.layout,
                        })
                        .collect(),
                })
                .collect(),
            panes,
            services: self.services.clone(),
        }
    }

    /// Reconcile saved structure while retaining unchanged VT views and attach streams.
    fn restore(&mut self, state: SavedState) {
        self.services = state.services.clone();
        let mut previous = std::mem::take(&mut self.panes);
        let old_workspaces = std::mem::take(&mut self.workspaces);
        let mut mapping = BTreeMap::new();
        let mut ids = std::collections::HashSet::new();
        for workspace in &state.workspaces {
            for tab in &workspace.tabs {
                ids.extend(tab.tree.panes());
                ids.extend(tab.floating.iter().map(|f| f.pane));
            }
        }
        for original in ids {
            let saved = state.panes.get(&original.to_string());
            let matched = saved.and_then(|saved| {
                previous
                    .iter()
                    .find(|(_, pane)| saved.origin.is_some() && pane.origin == saved.origin)
                    .or_else(|| {
                        previous.iter().find(|(id, pane)| {
                            let owned = self
                                .shown_original
                                .get(id)
                                .cloned()
                                .unwrap_or_else(|| pane.terminal.clone());
                            saved.terminal.is_some() && owned == saved.terminal
                        })
                    })
                    .map(|(id, _)| *id)
            });
            let id = matched.unwrap_or_else(|| self.alloc_pane());
            mapping.insert(original, id);
            let old = matched.and_then(|id| previous.remove(&id));
            let compatible = old.as_ref().is_some_and(|pane| {
                saved.is_some_and(|saved| {
                    let owned = self
                        .shown_original
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| pane.terminal.clone());
                    owned == saved.terminal && pane.module == saved.module
                })
            });
            if compatible {
                self.panes.insert(id, old.unwrap());
            } else {
                if old.is_some() {
                    self.send(Command::Close { pane: id });
                    self.shown_original.remove(&id);
                    self.panel_refresh.remove(&id);
                }
                self.spawn_pane(id);
                if let Some(pane) = self.panes.get_mut(&id) {
                    pane.terminal = saved.and_then(|p| p.terminal.clone());
                    pane.module = saved.and_then(|p| p.module.clone());
                }
            }
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.origin = saved.and_then(|p| p.origin.clone());
                pane.name = saved.and_then(|p| p.name.clone());
            }
        }
        fn remap(node: &mut SavedNode, mapping: &BTreeMap<u32, u32>) {
            match node {
                SavedNode::Leaf { pane } => *pane = mapping[pane],
                SavedNode::Split { first, second, .. } => {
                    remap(first, mapping);
                    remap(second, mapping);
                }
                SavedNode::Stack { panes, .. } => {
                    for pane in panes {
                        *pane = mapping[pane];
                    }
                }
            }
        }
        for saved in &state.workspaces {
            let id = old_workspaces
                .iter()
                .find(|w| w.origin.is_some() && w.origin == saved.origin)
                .map(|w| w.id)
                .unwrap_or_else(|| self.alloc_workspace());
            let mut workspace = Workspace {
                origin: saved.origin.clone(),
                machine: saved.machine.clone(),
                folder: saved.folder.clone(),
                id,
                label: saved.label.clone(),
                tabs: Vec::new(),
                active: saved.active_tab,
            };
            for saved_tab in &saved.tabs {
                let mut node = saved_tab.tree.clone();
                remap(&mut node, &mapping);
                if node.panes().is_empty() {
                    continue;
                }
                let focus = mapping
                    .get(&saved_tab.focus)
                    .copied()
                    .unwrap_or_else(|| node.panes()[0]);
                let mut tree: SplitNode = (&node).into();
                tree.activate_in_stack(PaneId(focus));
                let id = old_workspaces
                    .iter()
                    .flat_map(|w| &w.tabs)
                    .find(|t| t.origin.is_some() && t.origin == saved_tab.origin)
                    .map(|t| t.id)
                    .unwrap_or_else(|| self.alloc_tab());
                let floating = saved_tab
                    .floating
                    .iter()
                    .map(|f| FloatingPane {
                        pane: mapping[&f.pane],
                        ..f.clone()
                    })
                    .collect();
                workspace.tabs.push(Tab {
                    origin: saved_tab.origin.clone(),
                    settled: saved_tab.settled,
                    id,
                    label: saved_tab.label.clone(),
                    tree,
                    focus,
                    floating,
                    layout: saved_tab.layout,
                });
            }
            if !workspace.tabs.is_empty() {
                workspace.active = workspace.active.min(workspace.tabs.len() - 1);
                self.workspaces.push(workspace);
            }
        }
        if let Some(popup) = self.popup {
            if let Some(pane) = previous.remove(&popup.pane) {
                self.panes.insert(popup.pane, pane);
            }
        }
        for id in previous.keys() {
            self.send(Command::Close { pane: *id });
        }
        self.shown_original
            .retain(|id, _| self.panes.contains_key(id));
        self.panel_refresh
            .retain(|id, _| self.panes.contains_key(id));
        self.initial_tasks
            .retain(|id, _| self.panes.contains_key(id));
        if self
            .copy
            .as_ref()
            .is_some_and(|copy| !self.panes.contains_key(&copy.pane))
        {
            self.copy = None;
        }
        if self
            .popup
            .is_some_and(|popup| !self.panes.contains_key(&popup.pane))
        {
            self.popup = None;
        }
        self.active_ws = state
            .active_workspace
            .min(self.workspaces.len().saturating_sub(1));
        self.sync_terminals();
        self.layout_dirty = false;
        self.layout_since = None;
        self.dirty = true;
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
            if (pane.title.is_empty() || pane.title == terminal.address)
                && !terminal.title.is_empty()
            {
                pane.title = terminal.title.clone();
            }
            if pane.title == terminal.address {
                pane.title.clear();
            }
            if pane.name.is_none() {
                pane.name = terminal.name.clone();
            }
            pane.cwd = terminal.cwd.clone();
            pane.foreground = terminal.program.clone();
            pane.agent_session = terminal.session.clone();
            pane.agent.set_status(terminal.status.clone());
            pane.agent.observe_turn(terminal.turn);
            pane.agent.seen_turn = pane
                .agent
                .seen_turn
                .max(terminal.seen_turn)
                .max(self.seen.get(&terminal.address).copied().unwrap_or(0));
        }
    }

    fn apply_event(&mut self, event: TerminalEvent) {
        if event.kind.starts_with("machine")
            && self.config.restore
            && !self.inflight.contains_key("machines.sync")
        {
            self.call_rpc("machines.sync", "machine.list", json!({}));
        }
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
                for pane in target
                    .as_deref()
                    .map(|terminal| {
                        self.panes
                            .iter()
                            .filter(|(_, p)| p.terminal.as_deref() == Some(terminal))
                            .map(|(id, _)| *id)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
                {
                    if let Some(state) = self.panes.get_mut(&pane) {
                        state.title = title.clone();
                    }
                }
                self.dirty = true;
            }
            "labels" => {
                if let Some(labels) = event.data.get("labels") {
                    self.update_terminal(target.as_deref(), |terminal| {
                        terminal.space_id = labels
                            .get("herdr.space")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        terminal.tab_id = labels
                            .get("herdr.tab")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        terminal.name = labels
                            .get("hctui")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string);
                        terminal.space = labels.get("where").and_then(Value::as_str).map(|s| {
                            s.rsplit_once(" / ")
                                .map(|(space, _)| space)
                                .unwrap_or(s)
                                .to_string()
                        });
                        terminal.seen_turn = labels
                            .get("herdr.seen")
                            .and_then(Value::as_str)
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0);
                        terminal.pinned = labels
                            .get("herdr.pinned")
                            .and_then(Value::as_str)
                            .is_some_and(|s| !s.is_empty() && s != "false" && s != "0");
                        terminal.session = labels
                            .get("session")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                    });
                }
                self.dirty = true;
                for pane in target
                    .as_deref()
                    .map(|terminal| {
                        self.panes
                            .iter()
                            .filter(|(_, p)| p.terminal.as_deref() == Some(terminal))
                            .map(|(id, _)| *id)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
                {
                    let label = |key: &str| {
                        event
                            .data
                            .get("labels")
                            .and_then(|labels| labels.get(key))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .filter(|value| !value.is_empty())
                    };
                    let name = label("hctui");
                    let session = label("session");
                    if let Some(state) = self.panes.get_mut(&pane) {
                        state.name = name;
                        state.agent_session = session;
                    }
                    self.sync_terminals();
                    self.dirty = true;
                }
            }
            "foreground" => {
                self.update_terminal(target.as_deref(), |terminal| {
                    terminal.program = event
                        .data
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if let Some(cwd) = event.data.get("cwd").and_then(Value::as_str) {
                        terminal.cwd = Some(cwd.to_string());
                    }
                });
                self.dirty = true;
                for pane in target
                    .as_deref()
                    .map(|terminal| {
                        self.panes
                            .iter()
                            .filter(|(_, p)| p.terminal.as_deref() == Some(terminal))
                            .map(|(id, _)| *id)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
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
                for pane in target
                    .as_deref()
                    .map(|terminal| {
                        self.panes
                            .iter()
                            .filter(|(_, p)| p.terminal.as_deref() == Some(terminal))
                            .map(|(id, _)| *id)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
                {
                    let before = self
                        .panes
                        .get(&pane)
                        .map(|state| state.agent.badge())
                        .unwrap_or(Badge::None);
                    if let Some(state) = self.panes.get_mut(&pane) {
                        state.agent.set_status(status.clone());
                    }
                    self.notify_attention(pane, before);
                }
                self.publish_event(
                    "terminal.status",
                    target.as_deref(),
                    json!({"status": status}),
                );
                self.dirty = true;
            }
            "turn" => {
                let turn = event.data.get("turn").and_then(Value::as_u64).unwrap_or(0);
                self.update_terminal(target.as_deref(), |terminal| terminal.turn = turn);
                for pane in target
                    .as_deref()
                    .map(|terminal| {
                        self.panes
                            .iter()
                            .filter(|(_, p)| p.terminal.as_deref() == Some(terminal))
                            .map(|(id, _)| *id)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
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
                self.publish_event("terminal.turn", target.as_deref(), json!({"turn": turn}));
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
                    // core 说这条终端结束了：这是释放它代际 pin 的唯一自动证据。
                    self.plugin_terminal_ended(target);
                }
                self.publish_event(
                    "terminal.finished",
                    target.as_deref(),
                    json!({"reason": event.kind}),
                );
                self.dirty = true;
            }
            "doc" => {
                // 只关心自己的布局文档；别的客户端改了就先同步版本号。
                let name = event
                    .data
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if name == crate::shared::DOCUMENT && self.config.restore {
                    let machine = event
                        .data
                        .get("on")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    self.request_shared(&machine);
                    let machines: Vec<_> = self
                        .shared_docs
                        .keys()
                        .filter(|m| *m != &machine)
                        .cloned()
                        .collect();
                    for machine in machines {
                        self.request_shared(&machine);
                    }
                }
                if name == DOC_NAME && self.doc_loaded {
                    self.request_layout("doc.sync");
                }
            }
            "missed" => {
                self.note = t("resynced after missed events").to_string();
                self.dirty = true;
            }
            _ => {}
        }
        self.on_mod_terminal_event(&event.kind, target.as_deref(), &event.data);
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
        if let Some((wi, ti)) = self.locate(pane) {
            self.workspaces[wi].tabs[ti].settled = false;
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
            .unwrap_or_else(|| t1("pane {}", pane));
        let text = format!("{label} · {}", t(after.text()));
        self.last_attention = Some(pane);
        let level = if after == Badge::Blocked {
            "warn"
        } else {
            "info"
        };
        let timeout_ms = TOAST_LIFETIME.as_millis() as u64;
        match self.config.notifications {
            Delivery::Off => {}
            Delivery::Toast => {
                self.register_notification(text, level, timeout_ms, None, None, None, false);
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
                self.register_notification(text, level, timeout_ms, None, None, None, false);
            }
        }
        self.note = t1("{} needs attention (prefix o)", label);
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

    /// 事件流：把一次状态变化推给控制 socket 的订阅者。
    ///
    /// 负载带 `kind`、`machine`、`terminal`、`turn`、`at_ms` 与 `data`；属于插件
    /// 终端时另带 `plugin`。
    pub(crate) fn publish_event(&self, kind: &str, terminal: Option<&str>, data: Value) {
        let plugin = terminal.and_then(|address| self.terminal_plugin(address));
        let machine = terminal
            .and_then(|address| address.split_once('/'))
            .map(|(machine, _)| machine.to_string())
            .unwrap_or_else(|| "local".to_string());
        let turn = terminal
            .and_then(|address| {
                self.terminals
                    .iter()
                    .find(|summary| summary.address == address)
            })
            .map(|summary| summary.turn)
            .unwrap_or(0);
        let at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let mut event = json!({
            "kind": kind,
            "machine": machine,
            "turn": turn,
            "at_ms": at_ms,
            "data": data,
        });
        if let Some(terminal) = terminal {
            event["terminal"] = json!(terminal);
        }
        if let Some(plugin) = plugin {
            event["plugin"] = json!(plugin);
        }
        self.event_publisher.publish(&event);
    }

    /// 终端地址对应的插件包；本地与带机器前缀的地址按最后一段比对。
    fn terminal_plugin(&self, address: &str) -> Option<String> {
        let short = |value: &str| value.rsplit('/').next().unwrap_or(value).to_string();
        let wanted = short(address);
        if let Some(terminal) = self
            .terminals
            .iter()
            .find(|summary| short(&summary.address) == wanted)
        {
            if let Some(package) = &terminal.plugin {
                return Some(package.clone());
            }
        }
        self.plugin_terminals.iter().find_map(|(pane, terminal)| {
            let candidate = self
                .panes
                .get(pane)
                .and_then(|state| state.terminal.as_deref())?;
            (short(candidate) == wanted).then(|| terminal.package.clone())
        })
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

    /// 同一份分组列表供侧栏和窄屏使用，外部终端无需 attach 就能展示。
    pub fn navigation_rows(&self) -> Vec<NavigationRow> {
        let mut entries = Vec::new();
        for (id, pane) in &self.panes {
            if pane.transient || pane.module.is_some() || pane.agent.status.is_none() {
                continue;
            }
            let terminal = self
                .terminals
                .iter()
                .find(|t| Some(t.address.as_str()) == pane.terminal.as_deref());
            let machine = terminal.map(|t| t.machine()).unwrap_or("local");
            let mut group = terminal
                .filter(|t| t.space.is_some())
                .map(|t| t.group())
                .or_else(|| {
                    self.locate(*id)
                        .map(|(ws, _)| format!("{machine} / {}", self.workspace_label(ws)))
                })
                .unwrap_or_else(|| t1("{} / Other terminals", machine));
            let detail = pane
                .name
                .as_ref()
                .filter(|name| !name.is_empty())
                .cloned()
                .or_else(|| terminal.and_then(|terminal| terminal.name.clone()))
                .unwrap_or_else(|| {
                    self.locate(*id)
                        .and_then(|(ws, tab)| self.workspaces.get(ws)?.tabs.get(tab))
                        .filter(|tab| {
                            !tab.label.is_empty() && !tab.label.chars().all(|c| c.is_ascii_digit())
                        })
                        .map(|tab| tab.label.clone())
                        .unwrap_or_else(|| pane.label())
                });
            let badge = if self.focused() == *id
                && self.held_done.as_ref().is_some_and(|(address, turn)| {
                    pane.terminal.as_ref() == Some(address)
                        && pane.agent.turn == *turn
                        && matches!(pane.agent.status.as_deref(), Some("idle" | "done"))
                }) {
                Badge::Done
            } else {
                pane.agent.badge()
            };
            let pinned = terminal.is_some_and(|t| t.pinned);
            let label = if pinned || self.config.agent_sort == AgentSort::Priority {
                format!("{group} · {detail}")
            } else {
                detail
            };
            if pinned {
                group = "Pinned".to_string();
            } else if self.config.agent_sort == AgentSort::Priority {
                group = t(attention_section(badge)).to_string();
            }
            let group_id = if group == "Pinned" {
                "pinned".to_string()
            } else if self.config.agent_sort == AgentSort::Priority {
                format!("priority:{group}")
            } else {
                self.locate(*id)
                    .map(|(ws, _)| {
                        self.workspaces[ws]
                            .origin
                            .clone()
                            .unwrap_or_else(|| format!("space:{}", self.workspaces[ws].id))
                    })
                    .unwrap_or_else(|| group.clone())
            };
            entries.push(NavigationRow {
                group,
                group_id,
                pane: Some(*id),
                terminal: pane.terminal.clone(),
                badge,
                label,
            });
        }
        for terminal in self
            .terminals_without_pane()
            .into_iter()
            .filter(|t| t.live && t.status.is_some())
        {
            let mut agent = crate::model::Agent::default();
            agent.set_status(terminal.status.clone());
            agent.observe_turn(terminal.turn);
            agent.seen_turn = terminal
                .seen_turn
                .max(self.seen.get(&terminal.address).copied().unwrap_or(0));
            let badge = agent.badge();
            let full = terminal.pinned || self.config.agent_sort == AgentSort::Priority;
            let label = if full {
                format!("{} · {}", terminal.group(), terminal.label())
            } else {
                terminal.label().to_string()
            };
            let group = if terminal.pinned {
                "Pinned".to_string()
            } else if self.config.agent_sort == AgentSort::Priority {
                t(attention_section(badge)).to_string()
            } else {
                terminal.group()
            };
            let group_id = if terminal.pinned {
                "pinned".to_string()
            } else if self.config.agent_sort == AgentSort::Priority {
                format!("priority:{group}")
            } else {
                terminal
                    .space_id
                    .as_ref()
                    .map(|id| {
                        if terminal.machine() == "local" {
                            id.clone()
                        } else {
                            format!("{}/{id}", terminal.machine())
                        }
                    })
                    .unwrap_or_else(|| group.clone())
            };
            entries.push(NavigationRow {
                group,
                group_id,
                pane: None,
                terminal: Some(terminal.address.clone()),
                badge,
                label,
            });
        }
        entries.sort_by(|a, b| {
            let pin = (a.group != "Pinned").cmp(&(b.group != "Pinned"));
            if pin != std::cmp::Ordering::Equal {
                return pin;
            }
            if self.config.agent_sort == AgentSort::Priority {
                a.badge
                    .cmp(&b.badge)
                    .then(a.group.cmp(&b.group))
                    .then(a.group_id.cmp(&b.group_id))
                    .then(a.terminal.cmp(&b.terminal))
                    .then(a.pane.cmp(&b.pane))
            } else {
                a.group
                    .cmp(&b.group)
                    .then(a.group_id.cmp(&b.group_id))
                    .then(a.badge.cmp(&b.badge))
                    .then(a.terminal.cmp(&b.terminal))
                    .then(a.pane.cmp(&b.pane))
            }
        });
        agent_tree::arrange(
            entries,
            &self.terminals,
            &self.folded_agent_groups,
            self.config.agent_sort == AgentSort::Priority,
        )
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
        // 宿主退出走与取消相同的终止路径：取消排队项、终止运行中调用的进程组，
        // 等收尾完成；3 秒后补 SIGKILL。
        if let Err(error) = self
            .plugin_runner
            .shutdown(Instant::now() + Duration::from_secs(3))
        {
            tracing::warn!(%error, "plugin runner did not finish shutting down");
        }
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

fn attention_section(badge: Badge) -> &'static str {
    match badge {
        Badge::Blocked | Badge::Error => "Needs you",
        Badge::Done => "Done",
        Badge::Working => "Working",
        _ => "Idle",
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

/// Chat owns its input keys; only the image binding shares the terminal prefix.
#[derive(Debug, PartialEq, Eq)]
enum ChatImageKey {
    Input,
    Consumed,
    Paste,
}

fn chat_image_key(key: KeyEvent, keys: &crate::config::Keys, prefix: &mut bool) -> ChatImageKey {
    if key.kind != KeyEventKind::Press {
        return if *prefix {
            ChatImageKey::Consumed
        } else {
            ChatImageKey::Input
        };
    }
    if *prefix {
        *prefix = false;
        if keys.paste_image.matches_prefix(&key) {
            return ChatImageKey::Paste;
        }
        // Unknown prefix keys are consumed, just like terminal mode.
        return ChatImageKey::Consumed;
    }
    if keys.prefix.matches_direct(&key) {
        *prefix = true;
        return ChatImageKey::Consumed;
    }
    let ctrl_v = matches!(key.code, KeyCode::Char('v' | 'V'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT);
    if keys.paste_image.matches_direct(&key) || ctrl_v {
        ChatImageKey::Paste
    } else {
        ChatImageKey::Input
    }
}

/// 目录路径的最后一段；空路径时用 fallback。
fn dir_label(path: Option<String>, fallback: String) -> String {
    path.and_then(|path| {
        std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    })
    .filter(|name| !name.is_empty())
    .unwrap_or(fallback)
}

/// 展开配置里以 `~/` 开头的路径。
fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

fn hostname_text() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default()
}

fn short(text: &str) -> String {
    if text.chars().count() <= 28 {
        return text.to_string();
    }
    let mut out: String = text.chars().take(27).collect();
    out.push('…');
    out
}

pub(crate) fn prev_char_boundary(text: &str, index: usize) -> usize {
    text[..index]
        .char_indices()
        .next_back()
        .map(|(position, _)| position)
        .unwrap_or(0)
}

pub(crate) fn next_char_boundary(text: &str, index: usize) -> usize {
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
    outer_edges: bool,
) -> ratatui_widgets::borders::Borders {
    use ratatui_widgets::borders::Borders;
    let mut borders = match mode {
        PaneBorders::Off => Borders::NONE,
        PaneBorders::Always => Borders::ALL,
        PaneBorders::Titles => Borders::TOP,
        PaneBorders::Auto => {
            if leaves.len() > 1 {
                Borders::ALL
            } else {
                Borders::NONE
            }
        }
    };
    if !outer_edges && !borders.is_empty() {
        let left = leaves
            .iter()
            .map(|(_, rect)| rect.x)
            .min()
            .unwrap_or(outer.x);
        let top = leaves
            .iter()
            .map(|(_, rect)| rect.y)
            .min()
            .unwrap_or(outer.y);
        let right = leaves
            .iter()
            .map(|(_, rect)| rect.x + rect.width)
            .max()
            .unwrap_or(outer.x + outer.width);
        let bottom = leaves
            .iter()
            .map(|(_, rect)| rect.y + rect.height)
            .max()
            .unwrap_or(outer.y + outer.height);
        if outer.x == left {
            borders.remove(Borders::LEFT);
        }
        if outer.y == top && mode != PaneBorders::Titles {
            borders.remove(Borders::TOP);
        }
        if outer.x + outer.width == right {
            borders.remove(Borders::RIGHT);
        }
        if outer.y + outer.height == bottom {
            borders.remove(Borders::BOTTOM);
        }
    }
    if gaps || borders.is_empty() {
        return borders;
    }
    for (_, other) in leaves {
        if other == &outer {
            continue;
        }
        let vertical_overlap = outer.y < other.y + other.height && other.y < outer.y + outer.height;
        let horizontal_overlap = outer.x < other.x + other.width && other.x < outer.x + outer.width;
        if vertical_overlap && other.x + other.width == outer.x {
            borders.remove(Borders::LEFT);
        }
        if horizontal_overlap && other.y + other.height == outer.y && mode != PaneBorders::Titles {
            borders.remove(Borders::TOP);
        }
    }
    borders
}

/// 浮动 Pane 上按下鼠标时的操作：右下角改尺寸，标题行移动，其余交给拖选。
fn grab_zone(outer: Rect, column: u16, row: u16) -> Option<FloatingDragMode> {
    let right = outer.x + outer.width.saturating_sub(1);
    let bottom = outer.y + outer.height.saturating_sub(1);
    if column == right && row == bottom {
        return Some(FloatingDragMode::Resize);
    }
    (row == outer.y).then_some(FloatingDragMode::Move)
}

/// 把拖拽得到的长宽夹进 `[min, max]`；`max` 比 `min` 还小时以 `max` 为准。
fn clamp_extent(value: u16, min: u16, max: u16) -> u16 {
    if max <= min {
        return max;
    }
    value.clamp(min, max)
}

fn inner_rect(outer: Rect, borders: ratatui_widgets::borders::Borders) -> Rect {
    use ratatui_widgets::borders::Borders;
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

/// SIGTERM / SIGHUP 只置标志；收尾（flush 布局与共享文档、回收动作进程组）
/// 在事件循环里走与 `app.quit` 相同的那条路。
static TERMINATION_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_termination(_signal: libc::c_int) {
    // 处理器里只写一个原子量：async-signal-safe。
    TERMINATION_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(unix)]
fn install_termination_handlers() {
    // SAFETY: 处理器只写原子量；SA_RESTART 让阻塞的终端读取不被信号打断。
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_termination as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGHUP, &action, std::ptr::null_mut());
    }
}

#[cfg(not(unix))]
fn install_termination_handlers() {}

fn termination_requested() -> bool {
    TERMINATION_REQUESTED.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn run(config: Config) -> Result<()> {
    crate::i18n::set_zh(config.zh());
    let mouse = config.mouse_capture;
    let frame = Duration::from_millis(1000 / u64::from(config.render_fps.clamp(1, 240)));
    let mut terminal = tui::init_with(mouse)?;
    let _guard = tui::Guard;
    let mut app = App::new(config)?;
    install_termination_handlers();
    let _control_server = match crate::control::Server::start(&app.config) {
        Ok((server, receiver, publisher)) => {
            app.control = Some(receiver);
            app.event_publisher = publisher;
            Some(server)
        }
        Err(error) => {
            app.note = t1("Command socket: {}", error);
            None
        }
    };
    let mut graphics = crate::term::graphics::Graphics::default();
    let mut next_draw = Instant::now();
    let mut title = String::new();

    loop {
        app.pump();
        app.tick();

        let mut wait = next_draw.saturating_duration_since(Instant::now());
        if let Some(deadline) = app.wake_at() {
            wait = wait.min(deadline.saturating_duration_since(Instant::now()));
        }
        wait = wait.min(TERMINATION_POLL);
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(key) => app.on_key(key),
                Event::Paste(text) => app.on_paste(text),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                Event::FocusGained => app.route_focus_to_mods(true),
                Event::FocusLost => app.route_focus_to_mods(false),
                Event::Resize(_, _) => app.mark_dirty(),
            }
        }
        if termination_requested() && !app.should_quit() {
            app.request_quit();
        }
        app.pump();
        app.tick();

        if app.take_dirty() || Instant::now() >= next_draw {
            use crossterm::SynchronizedUpdate;
            std::io::stdout().sync_update(|output| -> Result<()> {
                terminal.draw(|frame| ui::draw(frame, &mut app))?;
                graphics.draw(&mut app, output)?;
                Ok(())
            })??;
            next_draw = Instant::now() + frame;
            if let Some(next) = app.window_title() {
                if next != title {
                    title = next.clone();
                    let mut stdout = std::io::stdout();
                    let _ =
                        crossterm::execute!(stdout, crossterm::terminal::SetTitle(title.clone()));
                }
            }
        }
        app.reconcile();

        if app.should_quit() {
            app.flush_layout();
            app.flush_shared();
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline && app.writes_pending() {
                app.pump();
                app.flush_layout();
                app.flush_shared();
                std::thread::sleep(Duration::from_millis(10));
            }
            if app.seen_dirty {
                app.save_local_state();
            }
            break;
        }
    }
    app.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offline_app() -> App {
        let (_, events) = std::sync::mpsc::channel();
        App::with_network(
            Config {
                restore: false,
                state_path: std::path::PathBuf::from("/nonexistent/hctui-test-state"),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap()
    }

    #[test]
    fn chat_image_shortcuts_do_not_type_v_or_consume_text_paste() {
        let keys = crate::config::Keys::default();
        let mut prefix = false;
        let press = |code, modifiers| KeyEvent::new(code, modifiers);
        let ctrl_b = press(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(
            chat_image_key(ctrl_b, &keys, &mut prefix),
            ChatImageKey::Consumed
        );
        assert!(prefix);
        for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
            let mut key = ctrl_b;
            key.kind = kind;
            assert_eq!(
                chat_image_key(key, &keys, &mut prefix),
                ChatImageKey::Consumed
            );
            assert!(prefix);
        }
        assert_eq!(
            chat_image_key(
                press(KeyCode::Char('V'), KeyModifiers::SHIFT),
                &keys,
                &mut prefix
            ),
            ChatImageKey::Paste
        );
        assert!(!prefix);
        assert_eq!(
            chat_image_key(
                press(KeyCode::Char('V'), KeyModifiers::SHIFT),
                &keys,
                &mut prefix
            ),
            ChatImageKey::Input
        );
        assert_eq!(
            chat_image_key(
                press(KeyCode::Char('v'), KeyModifiers::CONTROL),
                &keys,
                &mut prefix
            ),
            ChatImageKey::Paste
        );
        assert_eq!(
            chat_image_key(
                press(
                    KeyCode::Char('v'),
                    KeyModifiers::ALT | KeyModifiers::CONTROL
                ),
                &keys,
                &mut prefix
            ),
            ChatImageKey::Input
        );
        prefix = true;
        assert_eq!(
            chat_image_key(press(KeyCode::Esc, KeyModifiers::NONE), &keys, &mut prefix),
            ChatImageKey::Consumed
        );
        assert!(!prefix);
        let custom = crate::config::Keys {
            paste_image: crate::config::Binding::parse("alt+i").unwrap(),
            ..keys
        };
        assert_eq!(
            chat_image_key(
                press(KeyCode::Char('i'), KeyModifiers::ALT),
                &custom,
                &mut prefix
            ),
            ChatImageKey::Paste
        );
    }

    #[test]
    fn sidebar_machine_folding_and_scrolling_keep_agents_reachable() {
        use ratatui_core::{backend::TestBackend, terminal::Terminal};
        let mut app = offline_app();
        app.config.theme = crate::theme::builtin("catppuccin").unwrap();
        app.workspaces[0].label = "hc-tui".into();
        app.workspaces[0].machine = Some("arch".into());
        for index in 0..28 {
            app.new_workspace(Some(format!("space-{index}")));
            app.workspaces.last_mut().unwrap().machine = Some("arch".into());
        }
        app.active_ws = 0;
        let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let (_, hit) = app
            .agent_group_hits
            .iter()
            .find(|(id, _)| id == "machine:arch")
            .unwrap()
            .clone();
        assert!(app.agent_sort_hit.height > 0, "Agents must remain visible");
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x + 1,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(click);
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        assert!(app.folded_agent_groups.contains("machine:arch"));
        assert!(app.workspace_hits.is_empty());
        app.on_mouse(click);
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        assert!(!app.workspace_hits.is_empty());
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: app.spaces_area.x + 2,
            row: app.spaces_area.y + 2,
            modifiers: KeyModifiers::NONE,
        });
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        assert!(app.space_scroll > 0);
        assert_eq!(app.sidebar_scroll, 0);
        assert!(!app.workspace_hits.iter().any(|(index, _)| *index == 0));
    }

    #[test]
    fn pasted_image_files_upload_to_the_remote_terminal_before_inserting_any_path() {
        let mut app = offline_app();
        let pane = app.focused();
        app.panes.get_mut(&pane).unwrap().terminal = Some("dev/t7".into());
        app.panes.get_mut(&pane).unwrap().state = PaneState::Live { channel: 7 };
        let path =
            std::env::temp_dir().join(format!("{}.png", crate::shared::identity("paste-file-")));
        let bytes = b"\x89PNG\r\n\x1a\noriginal image";
        std::fs::write(&path, bytes).unwrap();
        app.on_paste(path.to_string_lossy().into_owned());
        let (tag, (method, params)) = app
            .inflight
            .iter()
            .find(|(tag, _)| tag.starts_with("put-image:"))
            .unwrap();
        assert_eq!(method, "terminal.put");
        assert_eq!(params["terminal"], "dev/t7");
        assert_eq!(params["data"], crate::clipboard::base64(bytes));
        let tag = tag.clone();
        app.on_reply(&tag, Ok(json!({"path":"/remote/received.png"})));
        assert_eq!(app.note, "image sent · /remote/received.png");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn image_path_paste_in_chat_waits_for_upload_and_does_not_keep_a_mac_path() {
        let mut app = offline_app();
        let pane = app.focused();
        app.panes.get_mut(&pane).unwrap().terminal = Some("dev/t7".into());
        app.panes.get_mut(&pane).unwrap().state = PaneState::Live { channel: 7 };
        app.chat = Some(Chat::from_lines(pane, vec![]));
        app.on_paste("/Users/missing/mac-only-image.png".into());
        assert!(app.note.contains("image was not uploaded"));
        assert!(app.chat.as_ref().unwrap().input.is_empty());
        assert!(!app.inflight.keys().any(|tag| tag.starts_with("put-image:")));
        app.upload_image(b"png".to_vec());
        assert!(app.chat.as_ref().unwrap().input.is_empty());
        let tag = app
            .inflight
            .keys()
            .find(|tag| tag.starts_with("put-image:"))
            .unwrap()
            .clone();
        app.on_reply(&tag, Ok(json!({"path":"/remote/image.png"})));
        assert_eq!(app.chat.as_ref().unwrap().input, "/remote/image.png ");
        let before = app.chat.as_ref().unwrap().input.clone();
        app.upload_image(b"png".to_vec());
        let tag = app
            .inflight
            .keys()
            .find(|tag| tag.starts_with("put-image:"))
            .unwrap()
            .clone();
        app.on_reply(
            &tag,
            Err(crate::hc::net::RpcFailure {
                kind: "unavailable".into(),
                message: "remote unavailable".into(),
            }),
        );
        assert_eq!(app.chat.as_ref().unwrap().input, before);
        assert!(app.note.contains("send image failed"));
        app.upload_image(b"png".to_vec());
        let tag = app
            .inflight
            .keys()
            .find(|tag| tag.starts_with("put-image:"))
            .unwrap()
            .clone();
        app.chat = None;
        app.on_reply(&tag, Ok(json!({"path":"/remote/late.png"})));
        assert!(app.note.contains("chat closed"));
    }

    #[test]
    fn image_upload_reply_uses_original_pane_after_focus_changes() {
        let mut app = offline_app();
        let original = app.input_pane();
        app.panes.get_mut(&original).unwrap().terminal = Some("arch/t1".into());
        app.panes.get_mut(&original).unwrap().state = PaneState::Live { channel: 1 };
        app.upload_image(b"png".to_vec());
        let tag = app
            .inflight
            .keys()
            .find(|tag| tag.starts_with("put-image:"))
            .unwrap()
            .clone();
        app.new_workspace(Some("other".into()));
        assert_ne!(app.input_pane(), original);
        app.on_reply(&tag, Ok(json!({"path":"/tmp/image.png"})));
        assert_eq!(app.note, "image sent · /tmp/image.png");
        // Replacing the original terminal must not insert the path into its replacement.
        app.active_ws = 0;
        app.upload_image(b"png".to_vec());
        let tag = app
            .inflight
            .keys()
            .find(|tag| tag.starts_with("put-image:"))
            .unwrap()
            .clone();
        app.panes.get_mut(&original).unwrap().terminal = Some("arch/t2".into());
        app.on_reply(&tag, Ok(json!({"path":"/tmp/second.png"})));
        assert!(app.note.contains("target pane closed"));
    }

    #[test]
    fn right_click_on_sidebar_agent_opens_menu_with_rename_and_shared_pin() {
        let mut app = offline_app();
        app.terminals =
            vec![TerminalSummary::parse(&json!({"address": "t9", "title": "Review"})).unwrap()];
        app.sidebar_area = Rect::new(0, 0, 25, 30);
        app.terminal_hits
            .push(("t9".to_string(), Rect::new(0, 8, 24, 1)));
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 5,
            row: 8,
            modifiers: KeyModifiers::NONE,
        };
        for expected in [true, false] {
            app.on_mouse(mouse);
            let chooser = app.chooser.as_mut().expect("Agent context menu");
            assert!(chooser
                .items
                .iter()
                .any(|item| item.value.starts_with("agent.rename ")));
            chooser.selected = chooser
                .items
                .iter()
                .position(|item| item.value.starts_with("terminal.pin "))
                .unwrap();
            app.activate_chooser();
            assert_eq!(app.terminals[0].pinned, expected);
        }
    }

    #[test]
    fn goto_search_keeps_j_and_k_and_selects_fresh_results() {
        let mut app = offline_app();
        app.terminals =
            vec![TerminalSummary::parse(&json!({"address": "t9", "title": "jack"})).unwrap()];
        app.goto = Some(GotoOverlay {
            query: String::new(),
            selected: 0,
        });
        for c in "jack".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(app.goto.as_ref().unwrap().query, "jack");
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.focused_pane().unwrap().terminal.as_deref(), Some("t9"));
    }

    #[test]
    fn typing_control_key_returns_scrolled_view_to_bottom() {
        let mut app = offline_app();
        let id = app.focused();
        let pane = app.panes.get_mut(&id).unwrap();
        pane.attached(1, 80, 24);
        pane.view.write(&b"line\r\n".repeat(100));
        pane.view.scroll(-20);
        let before = pane.view.scrollbar().unwrap();
        assert!(before.1 + before.2 < before.0);
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let after = app.panes[&id].view.scrollbar().unwrap();
        assert_eq!(after.1 + after.2, after.0);
    }

    #[test]
    fn unnamed_workspace_follows_first_pane_directory() {
        let mut app = offline_app();
        let id = app.focused();
        app.panes.get_mut(&id).unwrap().cwd = Some("/repo/backend".to_string());
        assert_eq!(app.workspace_label(0), "backend");
        app.workspaces[0].label = "Production".to_string();
        app.panes.get_mut(&id).unwrap().cwd = Some("/repo/frontend".to_string());
        assert_eq!(app.workspace_label(0), "Production");
    }

    fn mouse_at(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_resizes_nested_splits_without_changing_focus_or_target() {
        let mut app = offline_app();
        app.split_focused(Axis::Horizontal);
        app.split_focused(Axis::Vertical);
        app.reflow(Rect::new(0, 0, 100, 30));
        let focused = app.focused();
        let before = app.slots.clone();
        let bottom = before.iter().max_by_key(|s| s.outer.y).unwrap();
        let x = bottom.outer.x + 5;
        let y = bottom.outer.y;
        app.on_mouse(mouse_at(MouseEventKind::Down(MouseButton::Left), x, y));
        assert!(app.layout_drag.is_some());
        app.on_mouse(mouse_at(MouseEventKind::Drag(MouseButton::Left), x, y + 5));
        // Release outside the original divider still completes the same drag.
        app.on_mouse(mouse_at(MouseEventKind::Up(MouseButton::Left), 99, y + 5));
        app.reflow(Rect::new(0, 0, 100, 30));
        let after = app.slots.iter().find(|s| s.pane == bottom.pane).unwrap();
        assert!(after.outer.y > bottom.outer.y);
        assert_eq!(app.slots[0].outer, before[0].outer);
        assert_eq!(app.focused(), focused);
        assert!(app.layout_drag.is_none());
        assert!(app.layout_dirty);
        assert!(app.drag.is_none());
    }

    #[test]
    fn mouse_resizes_shared_vertical_border_even_with_terminal_mouse_tracking() {
        let mut app = offline_app();
        app.config.pane_gaps = false;
        app.split_focused(Axis::Horizontal);
        app.reflow(Rect::new(0, 0, 100, 30));
        for pane in app.panes.values_mut() {
            pane.view.write(b"\x1b[?1000h");
        }
        let x = app.slots[1].outer.x - 1;
        let y = app.slots[1].outer.y + 5;
        app.on_mouse(mouse_at(MouseEventKind::Down(MouseButton::Left), x, y));
        app.on_mouse(mouse_at(MouseEventKind::Drag(MouseButton::Left), x + 10, y));
        app.on_mouse(mouse_at(MouseEventKind::Up(MouseButton::Left), x + 10, y));
        app.reflow(Rect::new(0, 0, 100, 30));
        assert!(app.slots[1].outer.x > x + 1);
        assert!(app.layout_drag.is_none());
    }

    #[test]
    fn sidebar_drag_is_clamped_and_modal_clicks_do_not_start_resize() {
        let mut app = offline_app();
        let root = std::env::temp_dir().join(format!("hctui-resize-{}", std::process::id()));
        app.config.config_path = root.join("config.toml");
        app.sidebar_area = Rect::new(0, 0, 26, 30);
        app.content = Rect::new(26, 1, 74, 29);
        let press = mouse_at(MouseEventKind::Down(MouseButton::Left), 25, 10);
        app.on_mouse(press);
        app.on_mouse(mouse_at(MouseEventKind::Drag(MouseButton::Left), 99, 10));
        assert_eq!(app.sidebar_width, app.config.sidebar_max_width);
        app.on_mouse(mouse_at(MouseEventKind::Up(MouseButton::Left), 0, 10));
        assert_eq!(app.sidebar_width, app.config.sidebar_min_width);
        assert!(app.layout_drag.is_none());
        assert!(std::fs::read_to_string(&app.config.config_path)
            .unwrap()
            .contains(&format!("sidebar_width = {}", app.config.sidebar_min_width)));
        app.ask(PromptKind::NewWorkspace, "name");
        app.on_mouse(press);
        assert!(app.layout_drag.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn background_tab_exit_keeps_the_active_tab_and_focus() {
        let mut app = offline_app();
        let background = app.focused();
        app.new_tab(None);
        let focused = app.focused();
        let active = app.tab().unwrap().id;
        app.new_tab(None);
        app.select_tab(1);
        app.on_net(NetEvent::AttachClosed {
            pane: background,
            channel: 1,
            reason: "exit 0".into(),
            program_exit: true,
        });
        assert_eq!(app.tab().unwrap().id, active);
        assert_eq!(app.focused(), focused);
    }

    #[test]
    fn program_exit_removes_pane_but_stream_loss_keeps_it() {
        let mut app = offline_app();
        let original = app.focused();
        app.split_focused(Axis::Horizontal);
        let exited = app.focused();
        app.on_net(NetEvent::AttachClosed {
            pane: exited,
            channel: 2,
            reason: "exit 0".into(),
            program_exit: true,
        });
        assert!(!app.panes.contains_key(&exited));
        assert_eq!(app.tab().unwrap().panes(), vec![PaneId(original)]);
        assert_eq!(app.focused(), original);
        assert!(!app
            .inflight
            .values()
            .any(|(method, _)| method == "terminal.kill"));
        app.on_net(NetEvent::AttachClosed {
            pane: original,
            channel: 1,
            reason: "disconnected".into(),
            program_exit: false,
        });
        assert!(app.panes.contains_key(&original));
        app.on_net(NetEvent::AttachClosed {
            pane: original,
            channel: 1,
            reason: "exit 0".into(),
            program_exit: true,
        });
        assert!(app.panes.is_empty());
        assert!(app.workspaces.is_empty());
    }

    #[test]
    fn agent_navigation_excludes_shells_and_tracks_status_reports() {
        let mut app = offline_app();
        let pane = app.focused();
        app.panes.get_mut(&pane).unwrap().terminal = Some("t1".into());
        app.on_net(NetEvent::Snapshot {
            seq: 1,
            terminals: [
                json!({"address":"t1", "foreground":{"name":"bash"}}),
                json!({"address":"t2", "title":"Codex", "labels":{"herdr.pinned":"1"}}),
                json!({"address":"t3", "status":{"state":"working"}}),
            ]
            .iter()
            .map(|v| TerminalSummary::parse(v).unwrap())
            .collect(),
        });
        let addresses = |app: &App| {
            app.navigation_rows()
                .into_iter()
                .filter_map(|row| row.terminal)
                .collect::<Vec<_>>()
        };
        assert_eq!(addresses(&app), vec!["t3"]);
        app.apply_event(TerminalEvent {
            kind: "status".into(),
            terminal: Some("t1".into()),
            seq: 2,
            data: json!({"status":{"state":"idle"}, "reported":true}),
        });
        assert!(addresses(&app).contains(&"t1".to_string()));
        app.apply_event(TerminalEvent {
            kind: "status".into(),
            terminal: Some("t1".into()),
            seq: 3,
            data: json!({"status":null, "reported":false}),
        });
        assert_eq!(addresses(&app), vec!["t3"]);
        assert_eq!(app.panes[&pane].terminal.as_deref(), Some("t1"));
    }

    #[test]
    fn external_agent_opens_in_its_space_without_mutating_current_tab() {
        let mut app = offline_app();
        let original_ws = app.active_ws;
        let original_tab = app.tab().unwrap().id;
        let original_panes = app.tab().unwrap().panes();
        app.on_net(NetEvent::Snapshot {
            seq: 1,
            terminals: vec![TerminalSummary::parse(&json!({
                "address": "vps/t3", "title": "Review auth", "cwd": "/repo",
                "labels": {"where": "backend / 1"}, "status": {"state": "working"}
            }))
            .unwrap()],
        });
        assert!(app
            .navigation_rows()
            .iter()
            .any(|r| r.group == "vps / backend" && r.label == "Review auth"));
        app.open_terminal("vps/t3");
        let target = app.focused();
        assert_ne!(app.active_ws, original_ws);
        assert_eq!(app.workspace_label(app.active_ws), "backend");
        assert_eq!(app.new_pane_launch().unwrap().on.as_deref(), Some("vps"));
        assert_eq!(app.workspaces[original_ws].tabs[0].id, original_tab);
        assert_eq!(app.workspaces[original_ws].tabs[0].panes(), original_panes);
        let tabs = app.workspace().unwrap().tabs.len();
        app.open_terminal("vps/t3");
        assert_eq!(app.focused(), target);
        assert_eq!(app.workspace().unwrap().tabs.len(), tabs);
    }

    #[test]
    fn shared_seen_and_pinned_labels_apply_before_opening() {
        let mut app = offline_app();
        app.terminals = vec![TerminalSummary::parse(&json!({
            "address": "t3", "title": "Review", "turn": 4, "status": "idle",
            "labels": {"herdr.seen": "3", "herdr.pinned": "1"}
        }))
        .unwrap()];
        let row = app
            .navigation_rows()
            .into_iter()
            .find(|r| r.terminal.as_deref() == Some("t3"))
            .unwrap();
        assert_eq!(row.group, "Pinned");
        assert_eq!(row.badge, Badge::Done);
        app.apply_event(TerminalEvent {
            kind: "labels".to_string(),
            terminal: Some("t3".to_string()),
            seq: 2,
            data: json!({"labels": {"herdr.seen": "4"}}),
        });
        let row = app
            .navigation_rows()
            .into_iter()
            .find(|r| r.terminal.as_deref() == Some("t3"))
            .unwrap();
        assert_eq!(row.badge, Badge::Idle);
        assert_ne!(row.group, "Pinned");
    }

    #[test]
    fn prefix_release_does_not_consume_next_command() {
        let mut app = offline_app();
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        let mut release = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        release.kind = KeyEventKind::Release;
        app.on_key(release);
        assert!(app.prefix_active());
        app.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.help.is_some());
        assert!(!app.prefix_active());
    }

    #[test]
    fn inner_rect_follows_the_drawn_borders() {
        use ratatui_widgets::borders::Borders;
        let outer = Rect::new(2, 3, 20, 10);
        assert_eq!(inner_rect(outer, Borders::ALL), Rect::new(3, 4, 18, 8));
        assert_eq!(inner_rect(outer, Borders::NONE), outer);
        assert_eq!(
            inner_rect(outer, Borders::LEFT | Borders::RIGHT),
            Rect::new(3, 3, 18, 10)
        );
    }

    #[test]
    fn adjacent_panes_keep_one_shared_border_without_gaps() {
        use ratatui_widgets::borders::Borders;
        let leaves = vec![
            (PaneId(1), Rect::new(0, 0, 10, 10)),
            (PaneId(2), Rect::new(10, 0, 10, 10)),
        ];
        let left = borders_for(leaves[0].1, &leaves, PaneBorders::Auto, false, true);
        let right = borders_for(leaves[1].1, &leaves, PaneBorders::Auto, false, true);
        assert!(left.contains(Borders::RIGHT));
        assert!(!right.contains(Borders::LEFT));
        assert!(left.contains(Borders::LEFT));

        let gapped = borders_for(leaves[0].1, &leaves, PaneBorders::Auto, true, true);
        assert!(gapped.contains(Borders::RIGHT));
    }

    #[test]
    fn a_lone_pane_has_no_border_in_auto_mode() {
        use ratatui_widgets::borders::Borders;
        let leaves = vec![(PaneId(1), Rect::new(0, 0, 10, 10))];
        assert_eq!(
            borders_for(leaves[0].1, &leaves, PaneBorders::Auto, true, true),
            Borders::NONE
        );
        assert_eq!(
            borders_for(leaves[0].1, &leaves, PaneBorders::Always, true, true),
            Borders::ALL
        );
    }

    #[test]
    fn titles_mode_keeps_one_top_line_per_pane() {
        use ratatui_widgets::borders::Borders;
        let leaves = vec![
            (PaneId(1), Rect::new(0, 0, 10, 10)),
            (PaneId(2), Rect::new(0, 10, 10, 10)),
        ];
        // 无 gap 时下方面具靠上的边会被去掉，标题行不能跟着丢。
        for (_, rect) in &leaves {
            assert_eq!(
                borders_for(*rect, &leaves, PaneBorders::Titles, false, true),
                Borders::TOP
            );
        }
        // 顶部 Pane 在 `pane_outer_borders = false` 下仍然保留标题。
        assert_eq!(
            borders_for(leaves[0].1, &leaves, PaneBorders::Titles, true, false),
            Borders::TOP
        );
    }

    #[test]
    fn floating_grab_covers_title_row_and_corner() {
        let outer = Rect::new(20, 5, 40, 12);
        assert_eq!(grab_zone(outer, 30, 5), Some(FloatingDragMode::Move));
        assert_eq!(grab_zone(outer, 59, 16), Some(FloatingDragMode::Resize));
        assert_eq!(grab_zone(outer, 30, 10), None);
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

    #[test]
    fn mobile_switcher_lists_panes_without_agent_status() {
        use ratatui_core::{backend::TestBackend, terminal::Terminal};
        let mut app = offline_app();
        app.config.theme = crate::theme::builtin("catppuccin").unwrap();
        let agent_pane = app.focused();
        app.split_focused(Axis::Horizontal);
        let shell_pane = app.focused();
        assert_ne!(agent_pane, shell_pane);
        app.panes.get_mut(&shell_pane).unwrap().name = Some("plain-shell".into());
        assert!(app.panes[&shell_pane].agent.status.is_none());
        app.focus_pane(agent_pane);

        let mut terminal = Terminal::new(TestBackend::new(40, 40)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        assert!(app.mobile, "窄屏应进入手机布局");
        app.open_switcher();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();
        let text: String = (0..40)
            .flat_map(|y| (0..40).map(move |x| (x, y)))
            .map(|(x, y)| buffer[(x, y)].symbol())
            .collect();
        assert!(text.contains("plain-shell"), "{text}");
        assert!(
            app.mobile_targets
                .iter()
                .any(|(target, _)| *target == MobileTarget::Agent(shell_pane)),
            "Pane 段要给出没有 agent 的 Pane 的点击区域"
        );

        app.run_mobile_target(MobileTarget::Agent(shell_pane));
        assert_eq!(app.focused(), shell_pane);
        assert!(!app.switcher);
    }

    fn count_mod_blocks(chrome: &crate::mods::chrome::Chrome) -> usize {
        fn walk(node: &crate::mods::chrome::ChromeNode) -> usize {
            use crate::mods::chrome::ChromeNode;
            match node {
                ChromeNode::Row { children } | ChromeNode::Column { children } => {
                    children.iter().map(walk).sum()
                }
                ChromeNode::Block { .. } => usize::from(node.mod_id().is_some()),
            }
        }
        walk(&chrome.root)
    }

    #[test]
    fn mods_dock_and_undock_persist_the_chrome_tree() {
        use ratatui_core::{backend::TestBackend, terminal::Terminal};
        let mut app = offline_app();
        app.config.state_path =
            std::env::temp_dir().join(format!("hctui-m3-dock-{}.json", std::process::id()));
        app.execute_action(
            "mods.dock",
            json!({"name": "agents", "place": "right", "size": {"cols": 20}, "props": {"sort": "priority"}}),
        )
        .unwrap();
        assert_eq!(count_mod_blocks(&app.chrome), 1);
        // 保存的状态带外框树，重载后结构保持。
        app.save_local_state();
        let text = std::fs::read_to_string(&app.config.state_path).unwrap();
        let state: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(state["schema_version"], 4);
        let restored = crate::mods::chrome::Chrome::from_state(Some(&state["mods_chrome"]))
            .expect("mods_chrome must roundtrip");
        assert_eq!(count_mod_blocks(&restored), 1);

        // 渲染后模块区域有矩形；没有表面的区域不参与焦点循环。
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        assert_eq!(app.mod_regions.len(), 1);
        assert_eq!(app.mod_regions[0].id, "agents");
        assert!(!app.mod_regions[0].folded);
        assert!(app.mod_focusables().is_empty(), "no surface, not focusable");

        app.execute_action("mods.undock", json!({"name": "agents"}))
            .unwrap();
        assert_eq!(count_mod_blocks(&app.chrome), 0);
        let text = std::fs::read_to_string(&app.config.state_path).unwrap();
        let state: serde_json::Value = serde_json::from_str(&text).unwrap();
        let restored =
            crate::mods::chrome::Chrome::from_state(Some(&state["mods_chrome"])).unwrap();
        assert_eq!(count_mod_blocks(&restored), 0);
    }

    #[test]
    fn mods_chrome_action_replaces_the_tree_and_rejects_broken_trees() {
        use ratatui_core::{backend::TestBackend, terminal::Terminal};
        let mut app = offline_app();
        let tree = json!({
            "schema_version": 1,
            "root": {"type": "row", "children": [
                {"type": "column", "children": [
                    {"type": "block", "content": {"content": "panes"}},
                    {"type": "block", "size": {"rows": 1}, "content": {"builtin": "status"}}
                ]},
                {"type": "block", "id": "left", "size": {"cols": 26}, "content": {"builtin": "spaces"}}
            ]}
        });
        app.execute_action("mods.chrome", json!({"tree": tree}))
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        // 侧栏在右：body 从最左开始。
        assert_eq!(app.content.x, 0);
        assert_eq!(app.content.width, 74);
        assert!(app.spaces_area.x >= 74);

        let broken = json!({
            "schema_version": 1,
            "root": {"type": "row", "children": [
                {"type": "block", "content": {"content": "panes"}},
                {"type": "block", "content": {"content": "panes"}}
            ]}
        });
        assert!(
            app.execute_action("mods.chrome", json!({"tree": broken}))
                .is_err(),
            "two panes blocks must be rejected"
        );
    }

    #[test]
    fn no_mods_rejects_dock_actions_and_ignores_saved_chrome() {
        let mut app = offline_app();
        app.config.no_mods = true;
        assert!(app
            .execute_action("mods.dock", json!({"name": "agents", "place": "right"}))
            .is_err());
        assert_eq!(count_mod_blocks(&app.chrome), 0);
        assert!(app.mod_focusables().is_empty());
    }

    #[test]
    fn mod_regions_block_kitty_images_until_undocked() {
        use base64::Engine;
        let mut app = offline_app();
        let pane = app.input_pane();
        app.panes.get_mut(&pane).unwrap().attached(1, 80, 24);
        // 虚拟放置：宽 2 列，锚定在 1;1H 的占位字符上（U=1 的锚定规则）。
        let pixels = [255, 0, 0, 255].repeat(8);
        let payload = base64::engine::general_purpose::STANDARD.encode(pixels);
        app.panes
            .get_mut(&pane)
            .unwrap()
            .view
            .write(format!("\x1b_Ga=T,f=32,s=4,v=2,U=1,i=7,q=2;{payload}\x1b\\").as_bytes());
        app.panes
            .get_mut(&pane)
            .unwrap()
            .view
            .write("\x1b[H\x1b[38;2;0;0;7m\u{10eeee}\u{0305}\x1b[0m".as_bytes());
        app.reflow(Rect::new(0, 0, 80, 24));
        // 遮挡矩形覆盖图片所在的整个内容区：图片完全被盖住，不输出。
        app.mod_regions.push(crate::mods::chrome::ModRegion {
            id: "agents".into(),
            rect: Rect::new(0, 0, 40, 24),
            folded: false,
        });

        let mut graphics = crate::term::graphics::Graphics::default();
        let mut output = Vec::new();
        graphics.draw(&mut app, &mut output).unwrap();
        assert!(
            !output.windows(3).any(|w| w == b"\x1b_G"),
            "a fully covered image must not upload"
        );

        // 模块区域消失（undock）后，下一帧重新上传。
        app.mod_regions.clear();
        let mut output = Vec::new();
        graphics.draw(&mut app, &mut output).unwrap();
        assert!(
            output.windows(3).any(|w| w == b"\x1b_G"),
            "image must re-upload after the region closes"
        );
        let mut host = crate::term::view::TerminalView::new(80, 24, 1 << 20).unwrap();
        host.write(&output);
        assert!(
            !host.placement_images().is_empty(),
            "host VT sees the restored placement"
        );
    }

    #[test]
    fn pane_go_cycle_skips_mod_regions_without_surfaces() {
        let mut app = offline_app();
        app.reflow(Rect::new(0, 0, 80, 24));
        let focused = app.focused();
        // 存在一个模块区域，但没有任何表面：焦点循环只在 Pane 之间。
        app.mod_regions.push(crate::mods::chrome::ModRegion {
            id: "agents".into(),
            rect: Rect::new(60, 0, 20, 24),
            folded: false,
        });
        app.move_focus(crate::layout::split::Direction::Right);
        assert_eq!(app.focused(), focused);
        assert!(app.focused_mod.is_none());
    }
}

#[cfg(test)]
mod space_origin_tests {
    use super::*;

    fn offline_app() -> App {
        let (_, events) = std::sync::mpsc::channel();
        App::with_network(
            Config {
                restore: false,
                state_path: std::path::PathBuf::from("/nonexistent/hctui-test-state"),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap()
    }

    #[test]
    fn a_local_space_created_from_a_remote_space_gets_local_origins() {
        let mut app = offline_app();
        app.execute_action("space.new", json!({"name":"remote","machine":"peer"}))
            .unwrap();
        assert!(app
            .workspace()
            .unwrap()
            .origin
            .as_deref()
            .is_some_and(|origin| origin.starts_with("peer/")));
        app.execute_action("space.new", json!({"name":"local","machine":""}))
            .unwrap();
        let workspace = app.workspace().unwrap();
        let origin = workspace.origin.clone().unwrap();
        assert!(!origin.contains('/'), "local space origin: {origin}");
        for tab in &workspace.tabs {
            let origin = tab.origin.clone().unwrap();
            assert!(!origin.contains('/'), "local tab origin: {origin}");
            for pane in tab.panes() {
                let origin = app.panes.get(&pane.0).and_then(|p| p.origin.clone());
                assert!(
                    origin.as_deref().is_none_or(|origin| !origin.contains('/')),
                    "local pane origin: {origin:?}"
                );
            }
        }
    }
}
