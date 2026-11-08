//! 区域接线表：内置区域由哪个模块接管，以及表面形态的行为。
//!
//! [`REGIONS`] 是「模块挂到界面哪里」的唯一清单：每项给出内置区域名、绘制次序、
//! 本帧矩形取法与启动 props 的组装函数。谁接管某个区域由清单里的
//! `[module] region` 声明，加载注册表时解析成 [`crate::plugin::registry::Registry::regions`]
//! 里的 `区域 → 模块 ID` 映射；这里只描述区域本身，不记录模块。
//!
//! [`behavior`] 是表面形态的行为表：清屏、焦点、`Esc`、鼠标、z 序与停靠语义都
//! 按 `place` 查表，宿主不再逐处写 `match place`。

use std::collections::BTreeMap;

use ratatui_core::layout::Rect;
use serde_json::Value;

use crate::app::App;
use crate::mods::chrome::{ChromeLayout, ChromeSize};
use crate::mods::{ClearArea, SurfacePlace};

/// 区域在一帧里的绘制次序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionLayer {
    /// 内容区之前绘制：侧栏、Tab 栏。
    Chrome,
    /// 覆盖层之后绘制：模式行压在被覆盖的内容区之上。
    Overlay,
}

/// 一个内置区域的接线。
pub struct Region {
    /// 内置区域名；清单 `region` 的合法取值。
    pub name: &'static str,
    /// 绘制次序。
    pub layer: RegionLayer,
    /// 本帧矩形；内置树判定不渲染时为 `None`。
    pub rect: fn(&ChromeLayout) -> Option<Rect>,
    /// 换成模块块时声明的尺寸。
    pub size: fn(Rect) -> ChromeSize,
    /// 模块的启动 props。
    pub props: fn(&App) -> Value,
}

/// 内置区域表；顺序即同一绘制次序内的绘制顺序。
pub const REGIONS: [Region; 3] = [
    Region {
        name: "spaces",
        layer: RegionLayer::Chrome,
        rect: |layout| layout.sidebar,
        // 侧栏在水平分组里，主轴上占宽度。
        size: |rect| ChromeSize {
            cols: Some(rect.width),
            rows: None,
            ratio: None,
        },
        props: spaces_props,
    },
    Region {
        name: "tabs",
        layer: RegionLayer::Chrome,
        rect: |layout| layout.tab_bar,
        // Tab 栏与模式行在垂直分组里，主轴上占高度。
        size: |rect| ChromeSize {
            cols: None,
            rows: Some(rect.height),
            ratio: None,
        },
        props: empty_props,
    },
    Region {
        name: "status",
        layer: RegionLayer::Overlay,
        rect: |layout| layout.status,
        size: |rect| ChromeSize {
            cols: None,
            rows: Some(rect.height),
            ratio: None,
        },
        props: empty_props,
    },
];

/// 按名字找区域；非法名（清单校验已拒绝）返回 `None`。
pub fn region(name: &str) -> Option<&'static Region> {
    REGIONS.iter().find(|region| region.name == name)
}

/// 区域名是否合法。
pub fn is_region(name: &str) -> bool {
    region(name).is_some()
}

/// 该模块是否接管了某个内置区域。
pub fn is_region_module(owners: &BTreeMap<&'static str, String>, module: &str) -> bool {
    owners.values().any(|owner| owner == module)
}

/// 区域启动 props 的组装函数；未注册的区域返回 `None`。
pub fn props_for(name: &str) -> Option<fn(&App) -> Value> {
    region(name).map(|region| region.props)
}

/// 侧栏的 Agents 段：分组、徽标与折叠状态由宿主决议（`navigation_rows`），
/// 模块只按快照与最新 props 渲染。
fn spaces_props(app: &App) -> Value {
    let rows: Vec<Value> = app
        .navigation_rows()
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "group": row.group,
                "group_id": row.group_id,
                "label": row.label,
                "badge": badge_name(row.badge),
                "focused": row.pane.is_some_and(|pane| pane == app.focused()),
            })
        })
        .collect();
    serde_json::json!({
        "sort": match app.config.agent_sort {
            crate::config::AgentSort::Priority => "priority",
            crate::config::AgentSort::Spaces => "grouped",
        },
        "indicators": match app.config.indicators {
            crate::config::Indicators::Symbols => "symbols",
            crate::config::Indicators::Dots => "dots",
        },
        "spaces": {
            // 机器组的折叠状态与内置实现同源（`folded_agent_groups` 里的
            // `machine:<name>`）；模块据此换箭头并收起该组的 Space 行。
            "folded": app
                .folded_agent_groups
                .iter()
                .filter(|group| group.starts_with("machine:"))
                .cloned()
                .collect::<Vec<_>>(),
        },
        "agents": {
            "rows": rows,
            "folded": app.folded_agent_groups.iter().cloned().collect::<Vec<_>>(),
        },
    })
}

/// Tab 栏与模式行不需要启动 props。
fn empty_props(_app: &App) -> Value {
    serde_json::json!({})
}

/// 徽标在 props 里的名字。
fn badge_name(badge: crate::model::Badge) -> &'static str {
    use crate::model::Badge;
    match badge {
        Badge::Blocked => "blocked",
        Badge::Error => "error",
        Badge::Done => "done",
        Badge::Working => "working",
        Badge::Idle => "idle",
        Badge::Unknown => "unknown",
        Badge::None => "none",
    }
}

/// 表面形态的行为：某条表面在宿主的清屏、焦点、`Esc`、鼠标与 z 序里的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceBehavior {
    /// 绘制前清哪一块；`None` 表示不清屏（对话框默认清整个内容区）。
    pub clear: Option<ClearArea>,
    /// 打开时抢焦点，鼠标命中时换焦点。
    pub focus: bool,
    /// 宿主在 `Esc` 时关闭这条表面（只有覆盖层参与）。
    pub esc: bool,
    /// 属于覆盖层：参与覆盖层 z 序与 `Esc` 关闭。
    pub overlay: bool,
    /// 默认模态：命中后不再继续传播。
    pub modal: bool,
    /// 默认穿透：不接收鼠标。
    pub click_through: bool,
    /// 固定停靠区域：外框 `mod` 块或 Pane 槽位。
    pub docked: bool,
    /// 提示层：需要外框 `toasts` 节点，同层内最后绘制。
    pub toast: bool,
    /// 表面互相遮挡时的层级。
    pub z: u8,
}

/// 停靠形态的公共一半：region 与 pane 只在矩形来源上不同。
const DOCKED: SurfaceBehavior = SurfaceBehavior {
    clear: None,
    focus: true,
    esc: false,
    overlay: false,
    modal: true,
    click_through: false,
    docked: true,
    toast: false,
    z: 0,
};

/// 覆盖层形态的公共一半：dialog / popup / window 只在清屏与 z 序上不同。
const OVERLAY: SurfaceBehavior = SurfaceBehavior {
    clear: None,
    focus: true,
    esc: true,
    overlay: true,
    modal: true,
    click_through: false,
    docked: false,
    toast: false,
    z: 0,
};

/// 形态 → 行为；语义与原先逐处 `match place` 的实现逐项一致。
pub const BEHAVIORS: [(SurfacePlace, SurfaceBehavior); 6] = [
    (SurfacePlace::Region, DOCKED),
    (SurfacePlace::Pane, DOCKED),
    (
        SurfacePlace::Dialog,
        SurfaceBehavior {
            clear: Some(ClearArea::Content),
            z: 1,
            ..OVERLAY
        },
    ),
    (SurfacePlace::Popup, SurfaceBehavior { z: 2, ..OVERLAY }),
    (SurfacePlace::Window, SurfaceBehavior { z: 3, ..OVERLAY }),
    (
        SurfacePlace::Toast,
        SurfaceBehavior {
            clear: None,
            focus: false,
            esc: false,
            overlay: false,
            modal: false,
            click_through: true,
            docked: false,
            toast: true,
            z: 0,
        },
    ),
];

/// 表面形态的行为：查 [`BEHAVIORS`] 表。
pub fn behavior(place: SurfacePlace) -> SurfaceBehavior {
    BEHAVIORS
        .iter()
        .find(|(name, _)| *name == place)
        .map(|(_, behavior)| *behavior)
        .unwrap_or(DOCKED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_cover_the_three_builtin_areas_in_paint_order() {
        let names: Vec<&str> = REGIONS.iter().map(|region| region.name).collect();
        assert_eq!(names, vec!["spaces", "tabs", "status"]);
        assert_eq!(
            REGIONS
                .iter()
                .filter(|region| region.layer == RegionLayer::Chrome)
                .map(|region| region.name)
                .collect::<Vec<_>>(),
            vec!["spaces", "tabs"]
        );
        assert_eq!(
            REGIONS
                .iter()
                .filter(|region| region.layer == RegionLayer::Overlay)
                .map(|region| region.name)
                .collect::<Vec<_>>(),
            vec!["status"]
        );
        assert!(is_region("status"));
        assert!(!is_region("sidebar"));
        assert!(props_for("spaces").is_some());
    }

    #[test]
    fn toast_is_the_only_behavior_without_focus_or_esc() {
        let toast = behavior(SurfacePlace::Toast);
        assert!(!toast.focus);
        assert!(!toast.esc);
        assert!(toast.click_through);
        assert!(toast.toast);
        for place in [
            SurfacePlace::Region,
            SurfacePlace::Pane,
            SurfacePlace::Dialog,
            SurfacePlace::Popup,
            SurfacePlace::Window,
        ] {
            let shape = behavior(place);
            assert!(shape.focus, "{place:?}");
            assert!(!shape.toast, "{place:?}");
            assert!(!shape.click_through, "{place:?}");
        }
        assert!(behavior(SurfacePlace::Window).z > behavior(SurfacePlace::Popup).z);
        assert!(behavior(SurfacePlace::Popup).z > behavior(SurfacePlace::Dialog).z);
        assert_eq!(
            behavior(SurfacePlace::Dialog).clear,
            Some(ClearArea::Content)
        );
        assert_eq!(behavior(SurfacePlace::Popup).clear, None);
    }
}
