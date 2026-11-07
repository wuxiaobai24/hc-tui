//! 外框树：hc-tui 界面的本机区域布局，与共享 Pane 树分开。
//!
//! 节点分三类：`row` / `column` 是分组，`block` 是区域。区域内容为内置界面
//! （`{"builtin":"spaces"|"tabs"|"status"}`）、Pane 内容区（`{"content":"panes"}`，
//! 恰好一个）或模块表面（`{"mod":"id","props":{}}`）。树保存在本地 state 的
//! `mods_chrome`，不写共享文档。序列化形态见 docs/mods.md §4。
//!
//! 内置区域保留运行时动态行为：侧栏折叠成紧凑轨道、单 Tab 隐藏 Tab 栏、
//! 模式行按需出现；树只决定排列与显式绑定的模块区域。默认树由配置推导，
//! 侧栏在左、Tab 栏位置跟随 `tab_bar_position`。空间不足时区域按现有规则
//! 折叠：内置区域消失，声明尺寸的模块区域放不下时不渲染。

use serde::Deserialize;
use serde::Serialize;
use serde_json::{json, Value};

use crate::config::{CollapsedSidebar, Config, TabBarPosition};
use ratatui_core::layout::{Constraint, Direction, Layout, Rect};

/// 侧栏（Spaces 与 Agents）区域 ID。
pub const SPACES_ID: &str = "left";
/// 顶部 Tab 栏区域 ID。
pub const TABS_TOP_ID: &str = "top";
/// 底部 Tab 栏区域 ID。
pub const TABS_BOTTOM_ID: &str = "bottom";
/// 模式行区域 ID。
pub const STATUS_ID: &str = "bottom-status";
/// Pane 内容区 ID。
pub const PANES_ID: &str = "content";

/// 区域尺寸：字符格或比例，如 `{"cols": 26}`、`{"rows": 1}`、`{"ratio": 0.3}`。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ChromeSize {
    /// 固定列数（水平分组下生效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    /// 固定行数（垂直分组下生效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    /// 剩余空间的比例（0.0–1.0）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f32>,
}

impl ChromeSize {
    /// 固定列数。
    pub fn cols(cols: u16) -> Option<ChromeSize> {
        Some(ChromeSize {
            cols: Some(cols),
            rows: None,
            ratio: None,
        })
    }

    /// 固定行数。
    pub fn rows(rows: u16) -> Option<ChromeSize> {
        Some(ChromeSize {
            cols: None,
            rows: Some(rows),
            ratio: None,
        })
    }

    /// 该尺寸的主轴长度；`None` 表示比例或未声明。
    fn extent(&self, horizontal: bool) -> Option<u16> {
        match (self.cols, self.rows, self.ratio) {
            (Some(cols), _, _) if horizontal => Some(cols),
            (_, Some(rows), _) if !horizontal => Some(rows),
            (Some(cols), None, None) => Some(cols),
            (None, Some(rows), None) => Some(rows),
            _ => None,
        }
    }
}

/// 区域内容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChromeContent {
    /// 内置界面：`{"builtin":"spaces"|"tabs"|"status"}`。
    Builtin {
        /// 内置区域名。
        builtin: String,
    },
    /// Pane 内容区：`{"content":"panes"}`。
    Panes {
        /// 内容区名，取值 `panes`。
        content: String,
    },
    /// 模块表面：`{"mod":"id","props":{}}`。
    Mod {
        /// 模块 ID。
        r#mod: String,
        /// 模块启动参数，原样往返保留。
        #[serde(default)]
        props: Value,
    },
}

/// 外框树节点。未知字段在 [`Chrome::from_state`] 的前置校验里拒绝。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ChromeNode {
    /// 水平分组。
    Row {
        /// 子节点。
        #[serde(default)]
        children: Vec<ChromeNode>,
    },
    /// 垂直分组。
    Column {
        /// 子节点。
        #[serde(default)]
        children: Vec<ChromeNode>,
    },
    /// 区域。
    Block {
        /// 稳定 ID。
        #[serde(default)]
        id: String,
        /// 显式尺寸。
        #[serde(default)]
        size: Option<ChromeSize>,
        /// 区域内容。
        content: ChromeContent,
    },
}

impl ChromeNode {
    /// 模块区域内容名，非模块区域返回 `None`。
    pub fn mod_id(&self) -> Option<&str> {
        match self {
            ChromeNode::Block {
                content: ChromeContent::Mod { r#mod, .. },
                ..
            } => Some(r#mod),
            _ => None,
        }
    }
}

/// 外框树与 schema 版本。
#[derive(Debug, Clone, PartialEq)]
pub struct Chrome {
    /// 根节点；恰好一个 `panes` 叶子。
    pub root: ChromeNode,
}

/// 布局输入：内置区域的运行时动态状态。
#[derive(Debug, Clone, Copy)]
pub struct LayoutParams<'a> {
    /// hc-tui 配置。
    pub config: &'a Config,
    /// 侧栏当前是否展开（`layout.show` / `layout.hide` 状态）。
    pub sidebar_visible: bool,
    /// 当前 Tab 数（决定单 Tab 隐藏规则）。
    pub tab_count: usize,
    /// 模式行当前是否有内容。
    pub mode_line: bool,
    /// 终端区域。
    pub area: Rect,
}

/// 模块区域矩形与折叠状态。
#[derive(Debug, Clone, PartialEq)]
pub struct ModRegion {
    /// 模块 ID。
    pub id: String,
    /// 分配到的矩形；折叠时为空矩形。
    pub rect: Rect,
    /// 空间不足被折叠时不渲染。
    pub folded: bool,
}

/// 一轮布局算出的区域矩形。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChromeLayout {
    /// 侧栏完整轨道；`None` 表示不渲染。
    pub sidebar: Option<Rect>,
    /// 侧栏紧凑轨道；`None` 表示不渲染。
    pub rail: Option<Rect>,
    /// Tab 栏；`None` 表示隐藏。
    pub tab_bar: Option<Rect>,
    /// Pane 内容区。
    pub body: Rect,
    /// 包含 `panes` 的列矩形；侧栏右侧的整块主区。
    pub column: Rect,
    /// 模式行；`None` 表示不渲染。
    pub status: Option<Rect>,
    /// 模块区域，按外框顺序。
    pub mods: Vec<ModRegion>,
}

/// 构造一个 `block` 节点。
fn block(id: &str, size: Option<ChromeSize>, content: ChromeContent) -> ChromeNode {
    ChromeNode::Block {
        id: id.to_string(),
        size,
        content,
    }
}

/// 内置界面区域。
pub fn builtin(name: &str) -> ChromeContent {
    ChromeContent::Builtin {
        builtin: name.to_string(),
    }
}

/// Pane 内容区。
pub fn panes() -> ChromeContent {
    ChromeContent::Panes {
        content: "panes".to_string(),
    }
}

impl Chrome {
    /// 默认外框：侧栏在左，Tab 栏位置跟随配置，模式行在底。
    pub fn default_tree(config: &Config) -> Chrome {
        let bottom_tabs = config.tab_bar_position == TabBarPosition::Bottom;
        let mut column = Vec::new();
        if !bottom_tabs {
            column.push(block(TABS_TOP_ID, ChromeSize::rows(1), builtin("tabs")));
        }
        column.push(block(PANES_ID, None, panes()));
        if bottom_tabs {
            column.push(block(TABS_BOTTOM_ID, ChromeSize::rows(1), builtin("tabs")));
        }
        column.push(block(STATUS_ID, ChromeSize::rows(1), builtin("status")));
        Chrome {
            root: ChromeNode::Row {
                children: vec![
                    block(
                        SPACES_ID,
                        ChromeSize::cols(config.sidebar_width),
                        builtin("spaces"),
                    ),
                    ChromeNode::Column { children: column },
                ],
            },
        }
    }

    /// 从 state.json 的 `mods_chrome` 值解析；缺失、非法或结构不符时返回 `None`。
    pub fn from_state(value: Option<&Value>) -> Option<Chrome> {
        let value = value?;
        if value.get("schema_version")?.as_u64() != Some(1) {
            return None;
        }
        let root_value = value.get("root")?;
        validate_shape(root_value)?;
        let root = serde_json::from_value::<ChromeNode>(root_value.clone()).ok()?;
        let chrome = Chrome { root };
        chrome.validate().ok()?;
        Some(chrome)
    }

    /// 序列化为 state.json 的 `mods_chrome` 值。
    pub fn to_state(&self) -> Value {
        json!({
            "schema_version": 1,
            "root": serde_json::to_value(&self.root).unwrap_or(Value::Null),
        })
    }

    /// 校验结构：恰好一个 `panes` 叶子。
    pub fn validate(&self) -> Result<(), String> {
        let panes = count_panes(&self.root);
        if panes == 1 {
            Ok(())
        } else {
            Err(format!(
                "chrome must contain exactly one panes block, found {panes}"
            ))
        }
    }

    /// 按外框树与运行时状态计算区域矩形。
    pub fn layout(&self, params: LayoutParams<'_>) -> ChromeLayout {
        let mut out = ChromeLayout::default();
        let panes = self
            .split(&self.root, params.area, params, &mut out, true)
            .unwrap_or(params.area);
        out.body = panes;
        if out.column == Rect::ZERO {
            out.column = panes;
        }
        out
    }

    /// `mods.dock`：把模块绑定到指定位置。同 ID 区域先移除再插入。
    pub fn dock(
        &mut self,
        id: &str,
        place: DockPlace,
        size: Option<ChromeSize>,
        index: Option<usize>,
        props: Value,
    ) {
        self.undock(id);
        let new_block = block(
            id,
            size,
            ChromeContent::Mod {
                r#mod: id.to_string(),
                props,
            },
        );
        dock_insert(&mut self.root, new_block, place, index);
    }

    /// `mods.undock`：移除该 ID 的模块区域；返回是否移除了一个。
    pub fn undock(&mut self, id: &str) -> bool {
        undock_node(&mut self.root, id)
    }

    /// 递归分配矩形；返回该子树里 `panes` 叶子的矩形。`horizontal` 是父分组的方向。
    fn split(
        &self,
        node: &ChromeNode,
        rect: Rect,
        params: LayoutParams<'_>,
        out: &mut ChromeLayout,
        horizontal: bool,
    ) -> Option<Rect> {
        match node {
            ChromeNode::Row { children } => self.split_children(children, rect, params, out, true),
            ChromeNode::Column { children } => {
                self.split_children(children, rect, params, out, false)
            }
            ChromeNode::Block { content, size, .. } => match content {
                ChromeContent::Panes { .. } => Some(rect),
                ChromeContent::Builtin { builtin: name } => match name.as_str() {
                    "spaces" => {
                        if sidebar_expanded(params) {
                            out.sidebar = non_empty(rect);
                        } else if rail_width(params) > 0 {
                            out.rail = non_empty(rect);
                        }
                        None
                    }
                    "tabs" => {
                        if tabs_visible(params) {
                            out.tab_bar = non_empty(rect);
                        }
                        None
                    }
                    "status" => {
                        if params.mode_line {
                            out.status = non_empty(rect);
                        }
                        None
                    }
                    _ => None,
                },
                ChromeContent::Mod { r#mod, .. } => {
                    let folded = extent_folded(rect, *size, horizontal);
                    out.mods.push(ModRegion {
                        id: r#mod.clone(),
                        rect: if folded { Rect::ZERO } else { rect },
                        folded,
                    });
                    None
                }
            },
        }
    }

    fn split_children(
        &self,
        children: &[ChromeNode],
        rect: Rect,
        params: LayoutParams<'_>,
        out: &mut ChromeLayout,
        horizontal: bool,
    ) -> Option<Rect> {
        if children.is_empty() {
            return None;
        }
        let constraints: Vec<Constraint> = children
            .iter()
            .map(|child| child_constraint(child, params))
            .collect();
        let cells = Layout::default()
            .direction(if horizontal {
                Direction::Horizontal
            } else {
                Direction::Vertical
            })
            .constraints(constraints)
            .split(rect);
        let mut panes = None;
        for (child, cell) in children.iter().zip(cells.iter()) {
            if let Some(found) = self.split(child, *cell, params, out, horizontal) {
                panes = Some(found);
                if matches!(child, ChromeNode::Column { .. }) {
                    out.column = *cell;
                }
            }
        }
        panes
    }
}

/// `mods.dock` 的插入位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockPlace {
    /// 根行最左。
    Left,
    /// 侧栏之右。
    Right,
    /// 内容列最上。
    Top,
    /// 模式行之上的底部。
    Bottom,
}

/// 前置形状校验：拒绝未知字段与未知取值（内部 tag 的 serde 反序列化做不到）。
fn validate_shape(value: &Value) -> Option<()> {
    let object = value.as_object()?;
    let kind = object.get("type")?.as_str()?;
    match kind {
        "row" | "column" => {
            let allowed_keys = object.keys().all(|key| key == "type" || key == "children");
            if !allowed_keys {
                return None;
            }
            match object.get("children") {
                Some(children) => children.as_array()?.iter().try_for_each(validate_shape),
                None => Some(()),
            }
        }
        "block" => {
            let allowed = ["type", "id", "size", "content"];
            if object.keys().any(|key| !allowed.contains(&key.as_str())) {
                return None;
            }
            if let Some(size) = object.get("size") {
                if size.is_null() {
                    // 序列化后的缺省尺寸。
                } else {
                    let size = size.as_object()?;
                    let size_allowed = ["cols", "rows", "ratio"];
                    if size.keys().any(|key| !size_allowed.contains(&key.as_str())) {
                        return None;
                    }
                }
            }
            let content = object.get("content")?;
            let content_object = content.as_object()?;
            let content_keys: Vec<&str> = content_object.keys().map(String::as_str).collect();
            match content_keys.as_slice() {
                ["builtin"] => {
                    let name = content_object["builtin"].as_str()?;
                    matches!(name, "spaces" | "tabs" | "status").then_some(())
                }
                ["content"] => (content_object["content"].as_str()? == "panes").then_some(()),
                ["mod"] | ["mod", "props"] => content_object["mod"].as_str().map(|_| ()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn sidebar_expanded(params: LayoutParams<'_>) -> bool {
    params.sidebar_visible && params.area.width > params.config.sidebar_width + 40
}

fn rail_width(params: LayoutParams<'_>) -> u16 {
    if params.sidebar_visible {
        0
    } else if params.config.collapsed_sidebar == CollapsedSidebar::Compact && params.area.width > 40
    {
        4
    } else {
        0
    }
}

fn tabs_visible(params: LayoutParams<'_>) -> bool {
    !(params.config.hide_tab_bar_when_single_tab && params.tab_count <= 1)
}

fn non_empty(rect: Rect) -> Option<Rect> {
    (rect.width > 0 && rect.height > 0).then_some(rect)
}

/// 折叠规则：声明固定尺寸而分配到的主轴长度不足时视为放不下，不渲染。
/// 比例尺寸随空间伸缩，不折叠。
fn extent_folded(rect: Rect, size: Option<ChromeSize>, horizontal: bool) -> bool {
    let Some(size) = size else {
        return rect.width == 0 || rect.height == 0;
    };
    match size.extent(horizontal) {
        Some(declared) => {
            let allocated = if horizontal { rect.width } else { rect.height };
            allocated < declared
        }
        None => rect.width == 0 || rect.height == 0,
    }
}

fn child_constraint(child: &ChromeNode, params: LayoutParams<'_>) -> Constraint {
    let ChromeNode::Block { content, size, .. } = child else {
        return Constraint::Min(1);
    };
    match content {
        ChromeContent::Builtin { builtin: name } => match name.as_str() {
            "spaces" => {
                if sidebar_expanded(params) {
                    Constraint::Length(params.config.sidebar_width)
                } else {
                    Constraint::Length(rail_width(params))
                }
            }
            "tabs" => Constraint::Length(u16::from(tabs_visible(params))),
            "status" => Constraint::Length(u16::from(params.mode_line)),
            _ => Constraint::Length(0),
        },
        ChromeContent::Panes { .. } => Constraint::Min(3),
        ChromeContent::Mod { .. } => match size {
            Some(ChromeSize {
                cols: Some(cols), ..
            }) => Constraint::Length(*cols),
            Some(ChromeSize {
                rows: Some(rows), ..
            }) => Constraint::Length(*rows),
            Some(ChromeSize {
                ratio: Some(ratio), ..
            }) => Constraint::Percentage((ratio.clamp(0.0, 1.0) * 100.0).round() as u16),
            _ => Constraint::Length(0),
        },
    }
}

fn count_panes(node: &ChromeNode) -> usize {
    match node {
        ChromeNode::Row { children } | ChromeNode::Column { children } => {
            children.iter().map(count_panes).sum()
        }
        ChromeNode::Block { content, .. } => {
            usize::from(matches!(content, ChromeContent::Panes { .. }))
        }
    }
}

fn undock_node(node: &mut ChromeNode, id: &str) -> bool {
    match node {
        ChromeNode::Row { children } | ChromeNode::Column { children } => {
            let before = children.len();
            children.retain(|child| child.mod_id() != Some(id));
            if children.len() != before {
                return true;
            }
            children.iter_mut().any(|child| undock_node(child, id))
        }
        ChromeNode::Block { .. } => false,
    }
}

fn dock_insert(
    node: &mut ChromeNode,
    new_block: ChromeNode,
    place: DockPlace,
    index: Option<usize>,
) {
    let ChromeNode::Row { children } = node else {
        return;
    };
    match place {
        DockPlace::Left | DockPlace::Right => {
            let at = match (place, index) {
                (DockPlace::Left, Some(index)) => index.min(children.len()),
                (DockPlace::Left, None) => 0,
                (DockPlace::Right, Some(index)) => index.min(children.len()),
                (DockPlace::Right, None) => children
                    .iter()
                    .position(
                        |child| matches!(child, ChromeNode::Block { id, .. } if id == SPACES_ID),
                    )
                    .map_or(children.len(), |at| at + 1),
                (DockPlace::Top | DockPlace::Bottom, _) => 0,
            };
            children.insert(at, new_block);
        }
        DockPlace::Top | DockPlace::Bottom => {
            let column = children
                .iter_mut()
                .find(|child| matches!(child, ChromeNode::Column { .. }));
            let Some(ChromeNode::Column { children: column }) = column else {
                return;
            };
            let at = match (place, index) {
                (_, Some(index)) => index.min(column.len()),
                (DockPlace::Top, None) => 0,
                (DockPlace::Bottom, None) => column
                    .iter()
                    .rposition(
                        |child| matches!(child, ChromeNode::Block { id, .. } if id == STATUS_ID),
                    )
                    .unwrap_or(column.len()),
                (DockPlace::Left | DockPlace::Right, None) => 0,
            };
            column.insert(at, new_block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::default()
    }

    fn params<'a>(config: &'a Config, area: Rect) -> LayoutParams<'a> {
        LayoutParams {
            config,
            sidebar_visible: true,
            tab_count: 2,
            mode_line: true,
            area,
        }
    }

    #[test]
    fn default_tree_roundtrips_through_state_json() {
        let config = config();
        let chrome = Chrome::default_tree(&config);
        chrome.validate().unwrap();
        let state = chrome.to_state();
        let restored = Chrome::from_state(Some(&state)).unwrap();
        assert_eq!(restored, chrome);
        assert!(Chrome::from_state(None).is_none());
    }

    #[test]
    fn default_layout_matches_existing_regions() {
        let config = config();
        let chrome = Chrome::default_tree(&config);
        let area = Rect::new(0, 0, 100, 30);
        let layout = chrome.layout(params(&config, area));
        let sidebar = layout.sidebar.unwrap();
        assert_eq!((sidebar.width, sidebar.height), (config.sidebar_width, 30));
        assert_eq!(layout.rail, None);
        let tab_bar = layout.tab_bar.unwrap();
        assert_eq!(tab_bar.height, 1);
        assert_eq!(layout.body.height, 28);
        assert_eq!(layout.status.unwrap().height, 1);
        assert!(layout.mods.is_empty());
        // Tab 栏贴着侧栏，模式行贴底。
        assert_eq!(tab_bar.x, sidebar.right());
        assert_eq!(layout.status.unwrap().bottom(), area.bottom());
    }

    #[test]
    fn sidebar_rules_follow_config_and_width() {
        let mut config = config();
        let chrome = Chrome::default_tree(&config);
        // 收起侧栏：紧凑轨道占 4 列。
        let compact = params(&config, Rect::new(0, 0, 100, 30));
        let compact = LayoutParams {
            sidebar_visible: false,
            ..compact
        };
        let layout = chrome.layout(compact);
        assert_eq!(layout.sidebar, None);
        assert_eq!(layout.rail.unwrap().width, 4);
        // 窄终端：轨道也消失。
        let narrow = params(&config, Rect::new(0, 0, 30, 24));
        let layout = chrome.layout(narrow);
        assert_eq!(layout.sidebar, None);
        assert_eq!(layout.rail, None);
        assert_eq!(layout.body.width, 30);
        // 完全隐藏形态：没有紧凑轨道。
        config.collapsed_sidebar = CollapsedSidebar::Hidden;
        let hidden = LayoutParams {
            sidebar_visible: false,
            ..params(&config, Rect::new(0, 0, 100, 30))
        };
        let layout = chrome.layout(hidden);
        assert_eq!(layout.rail, None);
    }

    #[test]
    fn single_tab_hides_bar_when_configured() {
        let mut config = config();
        config.hide_tab_bar_when_single_tab = true;
        let chrome = Chrome::default_tree(&config);
        let mut p = params(&config, Rect::new(0, 0, 100, 30));
        p.tab_count = 1;
        let layout = chrome.layout(p);
        assert_eq!(layout.tab_bar, None);
        assert_eq!(layout.body.height, 29);
        p.tab_count = 2;
        assert!(chrome.layout(p).tab_bar.is_some());
    }

    #[test]
    fn tab_bar_position_bottom_moves_the_row() {
        let mut config = config();
        config.tab_bar_position = TabBarPosition::Bottom;
        let chrome = Chrome::default_tree(&config);
        let layout = chrome.layout(params(&config, Rect::new(0, 0, 100, 30)));
        let tab_bar = layout.tab_bar.unwrap();
        assert_eq!(tab_bar.y, layout.body.bottom());
        assert_eq!(layout.status.unwrap().y, tab_bar.bottom());
    }

    #[test]
    fn mode_line_absence_frees_the_row() {
        let config = config();
        let chrome = Chrome::default_tree(&config);
        let p = params(&config, Rect::new(0, 0, 100, 30));
        let p = LayoutParams {
            mode_line: false,
            ..p
        };
        let layout = chrome.layout(p);
        assert_eq!(layout.status, None);
        assert_eq!(layout.body.height, 29);
    }

    #[test]
    fn dock_places_mod_regions_and_persists() {
        let config = config();
        let mut chrome = Chrome::default_tree(&config);
        chrome.dock(
            "agents",
            DockPlace::Right,
            ChromeSize::cols(20),
            None,
            json!({"sort": "priority"}),
        );
        chrome.validate().unwrap();
        let layout = chrome.layout(params(&config, Rect::new(0, 0, 120, 30)));
        let region = &layout.mods[0];
        assert_eq!(region.id, "agents");
        assert!(!region.folded);
        assert_eq!(region.rect.width, 20);
        // props 原样保留。
        let state = chrome.to_state();
        let props = state.pointer("/root/children/1/content/props").unwrap();
        assert_eq!(props, &json!({"sort": "priority"}));
        // 再 dock 同 ID：先移除再插入，不出现两个区域。
        chrome.dock(
            "agents",
            DockPlace::Left,
            ChromeSize::cols(12),
            None,
            json!({}),
        );
        assert_eq!(count_mods(&chrome.root), 1);
        let layout = chrome.layout(params(&config, Rect::new(0, 0, 120, 30)));
        assert_eq!(layout.mods[0].rect.width, 12);
        // undock 恢复默认结构。
        assert!(chrome.undock("agents"));
        assert_eq!(count_mods(&chrome.root), 0);
        assert!(!chrome.undock("agents"));
        chrome.validate().unwrap();
    }

    #[test]
    fn dock_top_and_bottom_insert_into_the_column() {
        let config = config();
        let mut chrome = Chrome::default_tree(&config);
        chrome.dock(
            "top-mod",
            DockPlace::Top,
            ChromeSize::rows(1),
            None,
            json!({}),
        );
        chrome.dock(
            "bottom-mod",
            DockPlace::Bottom,
            ChromeSize::rows(2),
            None,
            json!({}),
        );
        let layout = chrome.layout(params(&config, Rect::new(0, 0, 100, 30)));
        let top = &layout.mods.iter().find(|m| m.id == "top-mod").unwrap();
        let bottom = &layout.mods.iter().find(|m| m.id == "bottom-mod").unwrap();
        // Top dock 在列最上，压着 Tab 栏之上。
        assert_eq!(top.rect.y, 0);
        assert_eq!(layout.tab_bar.unwrap().y, top.rect.bottom());
        assert_eq!(bottom.rect.height, 2);
        assert_eq!(bottom.rect.bottom(), layout.status.unwrap().y);
        assert_eq!(layout.body.height, 25);
    }

    #[test]
    fn narrow_terminal_folds_mod_regions() {
        let config = config();
        let mut chrome = Chrome::default_tree(&config);
        chrome.dock(
            "agents",
            DockPlace::Right,
            ChromeSize::cols(20),
            None,
            json!({}),
        );
        // 宽度不够声明的 20 列：求解器压缩模块区域，折叠不渲染。
        let layout = chrome.layout(params(&config, Rect::new(0, 0, 20, 24)));
        assert!(layout.mods[0].folded);
        assert_eq!(layout.mods[0].rect, Rect::ZERO);
        // 宽终端正常。
        let layout = chrome.layout(params(&config, Rect::new(0, 0, 120, 30)));
        assert!(!layout.mods[0].folded);
    }

    #[test]
    fn from_state_rejects_broken_trees() {
        // 两个 panes 叶子。
        let tree = json!({
            "schema_version": 1,
            "root": {"type": "row", "children": [
                {"type": "block", "content": {"content": "panes"}},
                {"type": "block", "content": {"content": "panes"}},
            ]}
        });
        assert!(Chrome::from_state(Some(&tree)).is_none());
        // 未知字段拒绝。
        let tree = json!({
            "schema_version": 1,
            "root": {"type": "block", "content": {"content": "panes"}, "extra": 1}
        });
        assert!(Chrome::from_state(Some(&tree)).is_none());
        // 未知内置区域名。
        let tree = json!({
            "schema_version": 1,
            "root": {"type": "block", "content": {"builtin": "toasts"}}
        });
        assert!(Chrome::from_state(Some(&tree)).is_none());
        // 未知尺寸字段。
        let tree = json!({
            "schema_version": 1,
            "root": {"type": "block", "size": {"width": 5}, "content": {"content": "panes"}}
        });
        assert!(Chrome::from_state(Some(&tree)).is_none());
        // 版本不符。
        let tree = json!({
            "schema_version": 99,
            "root": {"type": "block", "content": {"content": "panes"}}
        });
        assert!(Chrome::from_state(Some(&tree)).is_none());
    }

    #[test]
    fn chrome_tree_moves_the_sidebar_to_the_right() {
        let config = config();
        // mods.chrome 动作传入的树：内容列在左，侧栏块在右。
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
        let chrome = Chrome::from_state(Some(&tree)).unwrap();
        let area = Rect::new(0, 0, 100, 30);
        let layout = chrome.layout(params(&config, area));
        let sidebar = layout.sidebar.unwrap();
        assert_eq!(sidebar.x, 74);
        assert_eq!(sidebar.width, 26);
        assert_eq!(layout.body.x, 0);
        assert_eq!(layout.body.width, 74);
        assert_eq!(layout.status.unwrap().bottom(), area.bottom());
        // 行为与默认一致：Tab 计数与模式行规则照常生效。
        let p = LayoutParams {
            tab_count: 1,
            mode_line: false,
            ..params(&config, area)
        };
        let mut hidden = config.clone();
        hidden.hide_tab_bar_when_single_tab = true;
        let p = LayoutParams {
            config: &hidden,
            ..p
        };
        let layout = chrome.layout(p);
        assert_eq!(layout.tab_bar, None);
    }

    fn count_mods(node: &ChromeNode) -> usize {
        match node {
            ChromeNode::Row { children } | ChromeNode::Column { children } => {
                children.iter().map(count_mods).sum()
            }
            ChromeNode::Block { content, .. } => {
                usize::from(matches!(content, ChromeContent::Mod { .. }))
            }
        }
    }
}
