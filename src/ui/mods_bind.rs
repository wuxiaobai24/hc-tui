//! 默认外框绑定：侧栏、Tab 栏、模式行默认交给同名模块渲染，内置实现回退。
//!
//! 每一帧先用内置树算一遍区域，这样侧栏收起成轨道、单 Tab 隐藏、模式行按需
//! 出现这些运行时规则照旧生效；只把这一帧确实可见的区域换成模块块，尺寸取
//! 内置区域刚算出的矩形，所以第二遍布局与第一遍逐字段相同。模块没装、停用或
//! 渲染失败时该区域保持内置块，回退到 Rust 实现。

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

/// 区域模块随包打开的启动参数；配置项不在 `state` 里，只能随包会话固定。
fn region_props(app: &App, builtin: &str) -> serde_json::Value {
    match builtin {
        "spaces" => serde_json::json!({
            "sort": match app.config.agent_sort {
                crate::config::AgentSort::Priority => "priority",
                crate::config::AgentSort::Spaces => "grouped",
            },
        }),
        _ => serde_json::json!({}),
    }
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
