//! 默认外框绑定：侧栏、Tab 栏、模式行默认交给同名模块渲染，内置实现回退。
//!
//! 每一帧先用内置树算一遍区域，这样侧栏收起成轨道、单 Tab 隐藏、模式行按需
//! 出现这些运行时规则照旧生效；只把这一帧确实可见的区域换成模块块，尺寸取
//! 内置区域刚算出的矩形，所以第二遍布局与第一遍逐字段相同。模块没装、停用或
//! 渲染失败时该区域保持内置块，回退到 Rust 实现。
//!
//! 侧栏 Agents 段的分组与徽标由宿主决议（`navigation_rows`），作为 props 交给
//! 模块；props 变化时用既有的 `props.changed` 事件推新的一份，模块监听后重绘，
//! 不需要重载表面。

use ratatui_core::layout::Rect;

use crate::app::App;
use crate::mods::chrome::{ChromeContent, ChromeLayout, ChromeNode, ChromeSize, LayoutParams};

/// 内置区域名 → 默认提供它的模块 ID。
pub const REGION_MODULES: [(&str, &str); 3] = [
    ("spaces", "sidebar"),
    ("tabs", "tabs"),
    ("status", "status"),
];

/// 一帧的绑定结果。
pub struct Plan {
    /// 求解出的区域。
    pub layout: ChromeLayout,
    /// 由模块渲染、但输入仍归 Rust 的侧栏矩形：命中域要照旧登记。
    pub sidebar_hits: Option<Rect>,
    /// 同上，Tab 栏。
    pub tab_bar_hits: Option<Rect>,
}

/// 一帧的布局：先把可见区域换成模块块，再按外框树求解。
pub fn layout(app: &mut App, area: Rect) -> Plan {
    let builtin = app.chrome.layout(params(app, area));
    let fallback = Plan {
        layout: builtin.clone(),
        sidebar_hits: None,
        tab_bar_hits: None,
    };
    if app.config.no_mods {
        return fallback;
    }
    let mut tree = app.chrome.clone();
    let mut plan = Plan {
        layout: builtin.clone(),
        sidebar_hits: None,
        tab_bar_hits: None,
    };
    for (builtin_name, module) in REGION_MODULES {
        let Some(rect) = builtin_rect(&builtin, builtin_name) else {
            continue;
        };
        let props = region_props(app, builtin_name);
        if !app.ensure_region_module(module, props.clone()) {
            continue;
        }
        refresh_props(app, module, &props);
        if !bind_block(&mut tree.root, builtin_name, module, rect, props) {
            continue;
        }
        match builtin_name {
            "spaces" => plan.sidebar_hits = Some(rect),
            "tabs" => plan.tab_bar_hits = Some(rect),
            _ => {}
        }
    }
    if plan.sidebar_hits.is_none() && plan.tab_bar_hits.is_none() {
        return fallback;
    }
    plan.layout = tree.layout(params(app, area));
    plan
}

/// 本帧的布局输入。
fn params(app: &App, area: Rect) -> LayoutParams<'_> {
    let tabs = app
        .workspace()
        .map(|workspace| workspace.tabs.len())
        .unwrap_or(0);
    let mode_line = app.mode_line();
    LayoutParams {
        config: &app.config,
        sidebar_visible: app.sidebar,
        tab_count: tabs,
        mode_line: mode_line.is_some(),
        area,
    }
}

/// 该模块是否接管某个内置区域（它的像素归模块，输入仍归内置实现）。
pub fn is_region_module(module: &str) -> bool {
    REGION_MODULES.iter().any(|(_, id)| *id == module)
}

/// 内置区域这一帧的矩形。
fn builtin_rect(layout: &ChromeLayout, name: &str) -> Option<Rect> {
    match name {
        "spaces" => layout.sidebar,
        "tabs" => layout.tab_bar,
        "status" => layout.status,
        _ => None,
    }
}

/// 区域模块的启动参数。侧栏的 Agents 段需要分组、徽标与折叠状态，这些由宿主
/// 决议（`navigation_rows`），模块只按快照与最新 props 渲染。
fn region_props(app: &App, builtin: &str) -> serde_json::Value {
    match builtin {
        "spaces" => {
            let rows: Vec<serde_json::Value> = app
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
                "agents": {
                    "rows": rows,
                    "folded": app.folded_agent_groups.iter().cloned().collect::<Vec<_>>(),
                },
            })
        }
        _ => serde_json::json!({}),
    }
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

/// props 变了就把最新一份推给模块：`props.changed` 是既有的宿主事件，模块
/// 监听后更新信号，不需要重载表面（重载会清空模块内存里的快照）。
fn refresh_props(app: &mut App, module: &str, props: &serde_json::Value) {
    let digest = props.to_string();
    if app.region_mods_props.get(module) == Some(&digest) {
        return;
    }
    if app.region_mods_props.contains_key(module) {
        app.send_mod_event(
            module,
            "props.changed",
            &serde_json::json!({ "props": props }),
        );
    }
    app.region_mods_props.insert(module.to_string(), digest);
}

/// 把 `builtin` 区域的块换成模块块，尺寸取该区域这一帧的矩形。
fn bind_block(
    node: &mut ChromeNode,
    name: &str,
    module: &str,
    rect: Rect,
    props: serde_json::Value,
) -> bool {
    match node {
        ChromeNode::Row { children } | ChromeNode::Column { children } => {
            let mut found = false;
            for child in children.iter_mut() {
                found |= bind_block(child, name, module, rect, props.clone());
            }
            found
        }
        ChromeNode::Block { content, size, .. } => {
            let matches = matches!(
                content,
                ChromeContent::Builtin { builtin } if builtin == name
            );
            if !matches {
                return false;
            }
            *content = ChromeContent::Mod {
                r#mod: module.to_string(),
                props,
            };
            // 主轴方向决定声明哪个尺寸：侧栏在水平分组里，Tab 栏与模式行在垂直分组里。
            *size = Some(if name == "spaces" {
                ChromeSize {
                    cols: Some(rect.width),
                    rows: None,
                    ratio: None,
                }
            } else {
                ChromeSize {
                    cols: None,
                    rows: Some(rect.height),
                    ratio: None,
                }
            });
            true
        }
    }
}
