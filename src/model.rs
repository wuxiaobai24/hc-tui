//! 客户端的对象模型：Workspace → Tab → Pane。
//!
//! Pane 是渲染与 attach 的单元，ID 在进程内唯一且跨 Tab 移动不变；终端地址
//! 才是 core 里的身份。布局可以整体序列化进 core 的 doc，见 [`SavedState`]。

use std::collections::BTreeMap;

use ratatui_core::layout::Rect;
use serde::{Deserialize, Serialize};

use crate::layout::split::{Axis, PaneId, SplitNode};
use crate::term::view::TerminalView;

/// 一个 Pane 的 attach 状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneState {
    /// 还没拿到 attach 流。
    Waiting,
    Live {
        channel: u32,
    },
    /// 流已关闭：程序结束、本地关闭视图，或 attach 失败。
    Ended {
        reason: String,
    },
}

/// Agent 状态。语义与 herdr 一致：`idle` 表示可输入，`done` 表示有一轮完成
/// 但本客户端还没看过。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Agent {
    /// core 上报的原始状态词。
    pub status: Option<String>,
    /// core 给的轮次序号。
    pub turn: u64,
    /// 本客户端已经看过的轮次。
    pub seen_turn: u64,
}

impl Agent {
    pub fn set_status(&mut self, status: Option<String>) {
        self.status = status.filter(|status| !status.is_empty());
    }

    pub fn observe_turn(&mut self, turn: u64) {
        if turn > self.turn {
            self.turn = turn;
        }
    }

    /// 焦点落到这个 Pane 上时调用：从这里开始的完成都算已读。
    pub fn mark_seen(&mut self) {
        self.seen_turn = self.turn;
    }

    pub fn badge(&self) -> Badge {
        match self.status.as_deref() {
            Some("blocked") => Badge::Blocked,
            Some("error") => Badge::Error,
            Some("working") => Badge::Working,
            Some("idle" | "done") if self.turn > self.seen_turn => Badge::Done,
            Some("idle" | "done") => Badge::Idle,
            Some(_) => Badge::Unknown,
            None => Badge::None,
        }
    }

    /// 有未读完成或正在等人处理。
    pub fn wants_attention(&self) -> bool {
        matches!(self.badge(), Badge::Blocked | Badge::Error | Badge::Done)
    }
}

/// 侧栏与 Pane 边框用的状态徽标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Badge {
    Blocked = 0,
    Error = 1,
    Done = 2,
    Working = 3,
    Idle = 4,
    Unknown = 5,
    None = 6,
}

impl Badge {
    pub fn glyph(self, symbols: bool) -> &'static str {
        if symbols {
            match self {
                Self::Blocked => "◆",
                Self::Error => "✖",
                Self::Done => "✔",
                Self::Working => "◐",
                Self::Idle => "○",
                Self::Unknown => "◌",
                Self::None => "·",
            }
        } else {
            match self {
                Self::Blocked => "!",
                Self::Error => "x",
                Self::Done => "✓",
                Self::Working => "●",
                Self::Idle => "○",
                Self::Unknown => "?",
                Self::None => "·",
            }
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Blocked => "agent_blocked",
            Self::Error => "agent_error",
            Self::Done => "agent_done",
            Self::Working => "agent_working",
            Self::Idle => "agent_idle",
            Self::Unknown | Self::None => "agent_unknown",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Error => "error",
            Self::Done => "done",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Unknown => "unknown",
            Self::None => "",
        }
    }
}

/// 首次 attach 时要执行的程序（自定义命令用）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Launch {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    /// 在另一台机器上运行（`terminal.run` 的 `on`）。
    pub on: Option<String>,
}

impl Launch {
    pub fn local(argv: Vec<String>, cwd: Option<String>) -> Self {
        Self {
            argv,
            cwd,
            on: None,
        }
    }

    pub fn remote(argv: Vec<String>, cwd: Option<String>, on: String) -> Self {
        Self {
            argv,
            cwd,
            on: Some(on),
        }
    }
}

pub struct Pane {
    pub view: TerminalView,
    /// core 里的终端地址，attach 成功后才有。
    pub terminal: Option<String>,
    /// 手动命名（`terminal.label` 的 `hctui` 键）。
    pub name: Option<String>,
    /// 程序标题，来自 `title` 事件。
    pub title: String,
    /// 前台程序名，来自 `foreground` 事件。
    pub foreground: Option<String>,
    /// 前台程序的工作目录，自定义命令沿用。
    pub cwd: Option<String>,
    /// agent 自报的会话文件路径（`labels.session`），Chat 视图首选它。
    pub agent_session: Option<String>,
    pub origin: Option<String>,
    pub module: Option<serde_json::Value>,
    pub agent: Agent,
    /// 命令退出后自动移除视图（`type = "pane"` 与 popup）。
    pub transient: bool,
    pub launch: Option<Launch>,
    pub state: PaneState,
    requested: bool,
    sent_size: (u16, u16),
}

impl Pane {
    pub fn new(view: TerminalView) -> Self {
        Self {
            view,
            terminal: None,
            name: None,
            title: String::new(),
            foreground: None,
            cwd: None,
            agent_session: None,
            origin: None,
            module: None,
            agent: Agent::default(),
            transient: false,
            launch: None,
            state: PaneState::Waiting,
            requested: false,
            sent_size: (0, 0),
        }
    }

    /// 给人看的名字：手动命名 > 程序标题 > 终端地址。
    pub fn label(&self) -> String {
        self.name
            .as_deref()
            .filter(|name| !name.is_empty())
            .or_else(|| (!self.title.is_empty()).then_some(self.title.as_str()))
            .or(self.foreground.as_deref())
            .or(self.terminal.as_deref())
            .unwrap_or("")
            .to_string()
    }

    /// 边框上显示的程序名：没有手动名字与标题时用前台程序。
    pub fn program(&self) -> Option<&str> {
        if self.name.is_some() || !self.title.is_empty() {
            return None;
        }
        self.foreground.as_deref()
    }

    pub fn is_live(&self) -> bool {
        matches!(self.state, PaneState::Live { .. })
    }

    pub fn channel(&self) -> Option<u32> {
        match self.state {
            PaneState::Live { channel } => Some(channel),
            _ => None,
        }
    }

    pub fn size(&self) -> (u16, u16) {
        self.sent_size
    }

    /// 还需要首次 attach（尺寸对齐前决定 Spawn）。
    pub fn needs_spawn(&self) -> bool {
        !self.requested
    }

    pub fn mark_spawned(&mut self, size: (u16, u16)) {
        self.requested = true;
        self.sent_size = size;
    }

    /// 尺寸变了就记下来并返回 `true`。
    pub fn mark_resized(&mut self, size: (u16, u16)) -> bool {
        if self.sent_size == size {
            return false;
        }
        self.sent_size = size;
        true
    }

    pub fn adopt(&mut self, terminal: String, title: String) {
        self.terminal = Some(terminal);
        if !title.is_empty() && self.title != title {
            self.title = title;
        }
    }

    pub fn attached(&mut self, channel: u32, cols: u16, rows: u16) {
        self.state = PaneState::Live { channel };
        self.sent_size = (cols, rows);
    }
}

pub struct Tab {
    pub origin: Option<String>,
    pub settled: bool,
    pub id: u32,
    pub label: String,
    pub tree: SplitNode,
    pub focus: u32,
    /// 浮动层。`tree` 里没有它们；按 `z` 叠放，`pinned` 的一直在最上面。
    pub floating: Vec<FloatingPane>,
    /// 当前用的是哪个布局预设（[`crate::layout::preset::NAMES`] 的下标）；
    /// `None` 表示手写布局。
    pub layout: Option<usize>,
}

/// 浮动层里的一个 Pane。位置与大小按内容区的百分比保存，
/// 窗口尺寸变化时保持相对位置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloatingPane {
    pub pane: u32,
    /// 左边缘与上边缘，按内容区减去自身尺寸之后的余量取百分比。
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// 叠放顺序，越大越靠上。
    pub z: u32,
    /// 固定在浮动层最上面，隐藏浮动层时也不收。
    #[serde(default)]
    pub pinned: bool,
}

impl FloatingPane {
    pub const MIN_WIDTH: u16 = 8;
    pub const MIN_HEIGHT: u16 = 4;

    pub fn new(pane: u32, z: u32) -> Self {
        Self {
            pane,
            x: 20,
            y: 10,
            width: 60,
            height: 60,
            z,
            pinned: false,
        }
    }

    /// 它在内容区里的实际矩形。
    pub fn rect(&self, content: Rect) -> Rect {
        let width = snap(self.width, content.width)
            .clamp(Self::MIN_WIDTH.min(content.width), content.width.max(1));
        let height = snap(self.height, content.height)
            .clamp(Self::MIN_HEIGHT.min(content.height), content.height.max(1));
        let x = content.x + snap(self.x, content.width.saturating_sub(width));
        let y = content.y + snap(self.y, content.height.saturating_sub(height));
        Rect::new(x, y, width, height)
    }

    /// 把屏幕上的矩形反推回百分比，拖拽后写回。
    pub fn place(&mut self, rect: Rect, content: Rect) {
        let (width, height) = (content.width.max(1), content.height.max(1));
        self.width = percent(rect.width, width);
        self.height = percent(rect.height, height);
        self.x = percent(
            rect.x.saturating_sub(content.x),
            width.saturating_sub(rect.width),
        );
        self.y = percent(
            rect.y.saturating_sub(content.y),
            height.saturating_sub(rect.height),
        );
    }
}

fn snap(percent: u16, total: u16) -> u16 {
    ((u32::from(percent) * u32::from(total) + 50) / 100) as u16
}

fn percent(value: u16, total: u16) -> u16 {
    let total = u32::from(total.max(1));
    let percent = (u32::from(value) * 100 + total / 2) / total;
    percent.min(100) as u16
}

impl Tab {
    pub fn new(id: u32, pane: u32) -> Self {
        Self {
            origin: None,
            settled: false,
            id,
            label: String::new(),
            tree: SplitNode::leaf(pane),
            focus: pane,
            floating: Vec::new(),
            layout: None,
        }
    }

    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = self.tree.panes();
        out.extend(self.floating.iter().map(|entry| PaneId(entry.pane)));
        out
    }

    pub fn is_empty(&self) -> bool {
        self.tree.panes().is_empty() && self.floating.is_empty()
    }

    pub fn floating_of(&self, pane: u32) -> Option<&FloatingPane> {
        self.floating.iter().find(|entry| entry.pane == pane)
    }

    /// 把一个分屏里的 Pane 抬到浮动层。分屏里至少还要剩一个 Pane。
    pub fn float(&mut self, pane: u32) -> bool {
        let tiled = self.tree.panes();
        if tiled.len() < 2 || !tiled.iter().any(|id| id.0 == pane) {
            return false;
        }
        let Some(tree) = self.tree.clone().remove_pane(PaneId(pane)) else {
            return false;
        };
        self.tree = tree;
        let z = self
            .floating
            .iter()
            .map(|entry| entry.z + 1)
            .max()
            .unwrap_or(1);
        self.floating.push(FloatingPane::new(pane, z));
        true
    }

    /// 把浮动的 Pane 放回分屏，接在最后一个分屏 Pane 的右边。
    pub fn unfloat(&mut self, pane: u32) -> bool {
        let Some(index) = self.floating.iter().position(|entry| entry.pane == pane) else {
            return false;
        };
        self.floating.remove(index);
        match self.tree.panes().last().map(|id| id.0) {
            Some(anchor) => {
                self.tree
                    .split_pane(PaneId(anchor), Axis::Horizontal, PaneId(pane), 0.5);
            }
            None => self.tree = SplitNode::leaf(pane),
        }
        true
    }

    /// 在某个浮动 Pane 旁边再浮一个：新 Pane 错开几格，压在原来那个上面。
    pub fn float_beside(&mut self, pane: u32, source: u32) {
        let z = self
            .floating
            .iter()
            .map(|entry| entry.z + 1)
            .max()
            .unwrap_or(1);
        let mut next = self
            .floating
            .iter()
            .find(|entry| entry.pane == source)
            .cloned()
            .unwrap_or_else(|| FloatingPane::new(pane, z));
        next.pane = pane;
        next.z = z;
        next.x = next.x.saturating_add(5).min(60);
        next.y = next.y.saturating_add(3).min(60);
        self.floating.push(next);
    }

    /// 分屏里已经空了时，把一个浮动 Pane 提上来接住分屏；没有浮动 Pane 返回 `false`。
    pub fn promote_floating(&mut self) -> bool {
        let Some(promoted) = self.floating.first().map(|entry| entry.pane) else {
            return false;
        };
        self.floating.retain(|entry| entry.pane != promoted);
        self.tree = SplitNode::leaf(promoted);
        self.focus = promoted;
        true
    }

    /// 把某个浮动 Pane 提到浮动层最上面。
    pub fn raise_floating(&mut self, pane: u32) {
        let z = self
            .floating
            .iter()
            .map(|entry| entry.z + 1)
            .max()
            .unwrap_or(1);
        if let Some(entry) = self.floating.iter_mut().find(|entry| entry.pane == pane) {
            entry.z = z;
        }
    }

    /// Tab 的显示名：手动名字，其次是焦点 Pane 的名字。
    pub fn display(&self, panes: &std::collections::HashMap<u32, Pane>) -> String {
        if !self.label.is_empty() {
            return self.label.clone();
        }
        panes
            .get(&self.focus)
            .map(Pane::label)
            .filter(|label| !label.is_empty())
            .unwrap_or_default()
    }
}

pub struct Workspace {
    pub origin: Option<String>,
    pub machine: Option<String>,
    pub folder: Option<String>,
    pub id: u32,
    pub label: String,
    pub tabs: Vec<Tab>,
    pub active: usize,
}

impl Workspace {
    pub fn new(id: u32, label: String, pane: u32, tab: u32) -> Self {
        Self {
            origin: None,
            machine: None,
            folder: None,
            id,
            label,
            tabs: vec![Tab::new(tab, pane)],
            active: 0,
        }
    }

    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    pub fn focus(&self) -> Option<u32> {
        self.tab().map(|tab| tab.focus)
    }

    pub fn panes(&self) -> Vec<PaneId> {
        self.tabs.iter().flat_map(Tab::panes).collect()
    }

    /// 某个 Pane 在这个工作区的哪个 Tab 里。
    pub fn tab_of(&self, pane: u32) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.panes().iter().any(|id| id.0 == pane))
    }
}

/// 一个 Pane 在屏幕上的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub pane: u32,
    /// 含边框的矩形。
    pub outer: Rect,
    /// 边框以内的内容区，attach 与 VT 用这个尺寸。
    pub inner: Rect,
    /// 实际要画的边框；相邻 Pane 共用的边会被去掉。
    pub borders: ratatui_widgets::borders::Borders,
    /// 浮层 Pane（popup），画在所有分屏之上。
    pub overlay: bool,
    /// 折叠成一行标题的 Stack 成员：不重排终端，也不重绘内容。
    pub collapsed: bool,
}

/// 持久化进 `doc` 的布局。不含屏幕内容，也不含任何进程状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedState {
    pub schema_version: u32,
    #[serde(default)]
    pub active_workspace: usize,
    #[serde(default)]
    pub workspaces: Vec<SavedWorkspace>,
    #[serde(default)]
    pub panes: BTreeMap<String, SavedPane>,
}

impl SavedState {
    pub const SCHEMA: u32 = 3;

    pub fn empty() -> Self {
        Self {
            schema_version: Self::SCHEMA,
            active_workspace: 0,
            workspaces: Vec::new(),
            panes: BTreeMap::new(),
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let state: Self = serde_json::from_str(text).ok()?;
        // 未来版本的结构可能不兼容：宁可丢弃也不要错误恢复。
        (state.schema_version <= Self::SCHEMA).then_some(state)
    }

    pub fn is_usable(&self) -> bool {
        self.workspaces.iter().any(|workspace| {
            workspace
                .tabs
                .iter()
                .any(|tab| !tab.tree.panes().is_empty())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedWorkspace {
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub machine: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    pub id: u32,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub active_tab: usize,
    #[serde(default)]
    pub tabs: Vec<SavedTab>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub settled: bool,
    pub id: u32,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub focus: u32,
    pub tree: SavedNode,
    #[serde(default)]
    pub floating: Vec<FloatingPane>,
    #[serde(default)]
    pub layout: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPane {
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub module: Option<serde_json::Value>,
    #[serde(default)]
    pub terminal: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedNode {
    Leaf {
        pane: u32,
    },
    Split {
        axis: SavedAxis,
        ratio: f32,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
    Stack {
        #[serde(default)]
        panes: Vec<u32>,
        #[serde(default)]
        active: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedAxis {
    Horizontal,
    Vertical,
}

impl From<&SplitNode> for SavedNode {
    fn from(node: &SplitNode) -> Self {
        match node {
            SplitNode::Leaf(id) => Self::Leaf { pane: id.0 },
            SplitNode::Split {
                axis,
                ratio,
                first,
                second,
            } => Self::Split {
                axis: match axis {
                    Axis::Horizontal => SavedAxis::Horizontal,
                    Axis::Vertical => SavedAxis::Vertical,
                },
                ratio: *ratio,
                first: Box::new(first.as_ref().into()),
                second: Box::new(second.as_ref().into()),
            },
            SplitNode::Stack { panes, active } => Self::Stack {
                panes: panes.iter().map(|pane| pane.0).collect(),
                active: *active,
            },
        }
    }
}

impl From<&SavedNode> for SplitNode {
    fn from(node: &SavedNode) -> Self {
        match node {
            SavedNode::Leaf { pane } => SplitNode::leaf(*pane),
            SavedNode::Split {
                axis,
                ratio,
                first,
                second,
            } => SplitNode::split(
                match axis {
                    SavedAxis::Horizontal => Axis::Horizontal,
                    SavedAxis::Vertical => Axis::Vertical,
                },
                *ratio,
                first.as_ref().into(),
                second.as_ref().into(),
            ),
            SavedNode::Stack { panes, active } => {
                SplitNode::stack(panes.iter().map(|pane| PaneId(*pane)).collect(), *active)
            }
        }
    }
}

impl SavedNode {
    pub fn panes(&self) -> Vec<u32> {
        match self {
            Self::Leaf { pane } => vec![*pane],
            Self::Split { first, second, .. } => {
                let mut out = first.panes();
                out.extend(second.panes());
                out
            }
            Self::Stack { panes, .. } => panes.clone(),
        }
    }
}

/// 把整棵树里的 Pane 重新编号，用于恢复时避开与当前进程已用的 ID 冲突。
pub fn renumber_node(node: &mut SavedNode, map: &mut BTreeMap<u32, u32>, next: &mut u32) {
    match node {
        SavedNode::Leaf { pane } => {
            let fresh = *next;
            *next += 1;
            map.insert(*pane, fresh);
            *pane = fresh;
        }
        SavedNode::Split { first, second, .. } => {
            renumber_node(first, map, next);
            renumber_node(second, map, next);
        }
        SavedNode::Stack { panes, .. } => {
            for pane in panes.iter_mut() {
                let fresh = *next;
                *next += 1;
                map.insert(*pane, fresh);
                *pane = fresh;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> SavedState {
        SavedState {
            schema_version: SavedState::SCHEMA,
            active_workspace: 0,
            workspaces: vec![SavedWorkspace {
                origin: None,
                machine: None,
                folder: None,
                id: 1,
                label: "main".to_string(),
                active_tab: 0,
                tabs: vec![SavedTab {
                    origin: None,
                    settled: false,
                    id: 1,
                    label: String::new(),
                    focus: 2,
                    layout: None,
                    floating: Vec::new(),
                    tree: SavedNode::Split {
                        axis: SavedAxis::Horizontal,
                        ratio: 0.5,
                        first: Box::new(SavedNode::Leaf { pane: 2 }),
                        second: Box::new(SavedNode::Leaf { pane: 3 }),
                    },
                }],
            }],
            panes: BTreeMap::new(),
        }
    }

    #[test]
    fn saved_layout_round_trips() {
        let state = saved();
        let text = serde_json::to_string(&state).unwrap();
        let parsed = SavedState::parse(&text).unwrap();
        assert_eq!(parsed, state);
        assert!(parsed.is_usable());
        let tree: SplitNode = (&state.workspaces[0].tabs[0].tree).into();
        assert_eq!(tree.panes(), vec![PaneId(2), PaneId(3)]);
    }

    #[test]
    fn a_floating_pane_maps_between_cells_and_percent() {
        let content = Rect::new(10, 4, 100, 40);
        let mut entry = FloatingPane::new(3, 1);
        let rect = entry.rect(content);
        assert!(rect.width >= FloatingPane::MIN_WIDTH);
        assert!(content.intersection(rect) == rect);
        entry.place(Rect::new(60, 20, 40, 20), content);
        assert_eq!(entry.rect(content), Rect::new(60, 20, 40, 20));
    }

    #[test]
    fn floating_and_tiling_move_panes_between_layers() {
        let mut tab = Tab::new(1, 1);
        tab.tree
            .split_pane(PaneId(1), Axis::Horizontal, PaneId(2), 0.5);
        assert!(tab.float(2));
        assert_eq!(tab.tree.panes(), vec![PaneId(1)]);
        assert_eq!(tab.panes(), vec![PaneId(1), PaneId(2)]);
        assert!(tab.floating_of(2).is_some());
        assert!(!tab.float(1), "the last tiled pane stays tiled");
        tab.raise_floating(2);
        assert!(tab.unfloat(2));
        assert_eq!(tab.tree.panes(), vec![PaneId(1), PaneId(2)]);
        assert!(tab.floating.is_empty());
    }

    #[test]
    fn an_empty_tiled_layer_promotes_a_floating_pane() {
        let mut tab = Tab::new(1, 1);
        tab.tree
            .split_pane(PaneId(1), Axis::Horizontal, PaneId(2), 0.5);
        assert!(tab.float(2));
        let tree = tab.tree.clone();
        assert!(tree.remove_pane(PaneId(1)).is_none());
        assert!(tab.promote_floating());
        assert_eq!(tab.tree.panes(), vec![PaneId(2)]);
        assert!(tab.floating.is_empty());
        assert_eq!(tab.focus, 2);
    }

    #[test]
    fn a_stack_survives_the_doc_round_trip() {
        let node = SplitNode::stack(vec![PaneId(2), PaneId(3)], 1);
        let saved: SavedNode = (&node).into();
        assert_eq!(saved.panes(), vec![2, 3]);
        let tree: SplitNode = (&saved).into();
        assert_eq!(tree, node);
    }

    #[test]
    fn a_newer_schema_is_rejected() {
        let mut state = saved();
        state.schema_version = SavedState::SCHEMA + 1;
        let text = serde_json::to_string(&state).unwrap();
        assert!(SavedState::parse(&text).is_none());
    }

    #[test]
    fn badges_order_by_attention() {
        let mut agent = Agent::default();
        agent.set_status(Some("idle".to_string()));
        assert_eq!(agent.badge(), Badge::Idle);
        agent.observe_turn(4);
        assert_eq!(agent.badge(), Badge::Done);
        assert!(agent.wants_attention());
        agent.mark_seen();
        assert_eq!(agent.badge(), Badge::Idle);
        agent.set_status(Some("blocked".to_string()));
        assert!(agent.badge() < Badge::Done);
        assert_eq!(Badge::Blocked.glyph(false), "!");
        assert_eq!(Badge::Blocked.token(), "agent_blocked");
    }
}
