//! 默认外框绑定：侧栏、Tab 栏、模式行按清单声明交给模块渲染，内置实现回退。
//!
//! 每一帧先用内置树算一遍区域，这样侧栏收起成轨道、单 Tab 隐藏、模式行按需
//! 出现这些运行时规则照旧生效；只把这一帧确实可见的区域换成模块块，尺寸取
//! 内置区域刚算出的矩形，所以第二遍布局与第一遍逐字段相同。模块没装、停用或
//! 渲染失败时该区域保持内置块，回退到 Rust 实现。
//!
//! 区域本身来自 [`crate::mods::wiring::REGIONS`]；谁接管某个区域来自注册表的
//! `regions` 映射（清单 `[module] region` 解析结果）。
//!
//! 侧栏 Agents 段的分组与徽标由宿主决议（`navigation_rows`），作为 props 交给
//! 模块；props 变化时用既有的 `props.changed` 事件推新的一份，模块监听后重绘，
//! 不需要重载表面。

use std::collections::BTreeMap;

use ratatui_core::buffer::Buffer;
use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;

use crate::app::App;
use crate::mods::chrome::{ChromeContent, ChromeLayout, ChromeNode, LayoutParams};
use crate::mods::wiring::{self, RegionLayer};
use crate::theme::Theme;

/// 一帧的绑定结果。
pub struct Plan {
    /// 求解出的区域。
    pub layout: ChromeLayout,
    /// 由模块渲染、但输入仍归 Rust 的区域矩形：命中域要照旧登记。
    pub hits: BTreeMap<&'static str, Rect>,
}

impl Plan {
    /// 该区域这一帧是否由模块绘制。
    fn hits(&self, region: &str) -> Option<Rect> {
        self.hits.get(region).copied()
    }
}

/// 一帧的布局：先把可见区域换成模块块，再按外框树求解。
pub fn layout(app: &mut App, area: Rect) -> Plan {
    let builtin = app.chrome.layout(params(app, area));
    let fallback = Plan {
        layout: builtin.clone(),
        hits: BTreeMap::new(),
    };
    if app.config.no_mods {
        return fallback;
    }
    let owners = app.plugins.regions.clone();
    if owners.is_empty() {
        return fallback;
    }
    let mut tree = app.chrome.clone();
    let mut plan = Plan {
        layout: builtin.clone(),
        hits: BTreeMap::new(),
    };
    for region in &wiring::REGIONS {
        let Some(module) = owners.get(region.name) else {
            continue;
        };
        let Some(rect) = (region.rect)(&builtin) else {
            continue;
        };
        let props = (region.props)(app);
        if !app.ensure_region_module(module, props.clone()) {
            continue;
        }
        refresh_props(app, module, &props);
        if !bind_block(&mut tree.root, region, module, rect, props) {
            continue;
        }
        plan.hits.insert(region.name, rect);
    }
    if plan.hits.is_empty() {
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

/// 按 [`RegionLayer`] 绘制区域：可见时画内置实现，模块接管时只登记命中域。
///
/// 内置绘制在模块接管时仍要跑一遍（画到临时 Buffer），因为命中域、Tab 角标
/// 这些输入用的矩形由内置实现登记；像素最终被模块表面覆盖。返回本层里
/// 「模块已接管但还没按首份 props 落笔」的内置帧，由 [`cover`] 在模块表面
/// 之后盖回去。
pub fn paint(
    frame: &mut Frame,
    app: &mut App,
    plan: &Plan,
    layer: RegionLayer,
    theme: &Theme,
    mode_line: &Option<(String, String)>,
) -> Vec<(Rect, Buffer)> {
    let mut backdrops = Vec::new();
    for region in &wiring::REGIONS {
        if region.layer != layer {
            continue;
        }
        let pending = needs_backdrop(app, plan, region.name);
        match region.name {
            "spaces" => match (plan.layout.sidebar, plan.layout.rail) {
                (Some(rail), _) => crate::ui::render_sidebar(frame.buffer_mut(), app, rail, theme),
                (None, Some(rail)) => crate::ui::render_rail(frame, app, rail, theme),
                (None, None) => {
                    app.workspace_hits.clear();
                    app.agent_hits.clear();
                    if let Some(rect) = plan.hits(region.name) {
                        let mut scratch = Buffer::empty(frame.area());
                        crate::ui::render_sidebar(&mut scratch, app, rect, theme);
                        if pending {
                            backdrops.push((rect, scratch));
                        }
                    }
                }
            },
            "tabs" => {
                if let Some(rect) = plan.layout.tab_bar {
                    crate::ui::render_tab_bar(frame.buffer_mut(), app, rect, theme);
                } else if let Some(rect) = plan.hits(region.name) {
                    let mut scratch = Buffer::empty(frame.area());
                    crate::ui::render_tab_bar(&mut scratch, app, rect, theme);
                    if pending {
                        backdrops.push((rect, scratch));
                    }
                } else {
                    let corner = Rect::new(plan.layout.column.x, plan.layout.column.y, 0, 0);
                    app.tab_bar = corner;
                    app.tab_hits.clear();
                    app.tab_add = corner;
                }
            }
            "status" => {
                if let (Some(status), Some(line)) = (plan.layout.status, mode_line.as_ref()) {
                    crate::ui::render_mode_line(frame.buffer_mut(), app, status, line, theme);
                } else if let (Some(rect), Some(line)) =
                    (plan.hits(region.name), mode_line.as_ref())
                {
                    let mut scratch = Buffer::empty(frame.area());
                    crate::ui::render_mode_line(&mut scratch, app, rect, line, theme);
                    if pending {
                        backdrops.push((rect, scratch));
                    }
                }
            }
            _ => {}
        }
    }
    backdrops
}

/// 该区域这一帧是不是「模块已接管，但还没按首份 props 落笔」。
fn needs_backdrop(app: &App, plan: &Plan, region: &str) -> bool {
    let Some(module) = app.plugins.regions.get(region).cloned() else {
        return false;
    };
    plan.hits(region).is_some() && !app.region_ready(&module)
}

/// 把内置帧盖回模块帧上：模块落笔之前，该区域的像素归内置实现。
pub fn cover(frame: &mut Frame, backdrops: Vec<(Rect, Buffer)>) {
    for (rect, builtin) in backdrops {
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                frame.buffer_mut()[(x, y)] = builtin[(x, y)].clone();
            }
        }
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
        // 新一份 props 落笔前区域照旧归内置实现。
        for surface in app
            .mods
            .surfaces
            .iter_mut()
            .filter(|surface| surface.mod_id == module)
        {
            surface.props_revision += 1;
        }
    }
    app.region_mods_props.insert(module.to_string(), digest);
}

/// 把该区域的块换成模块块，尺寸取该区域这一帧的矩形。
fn bind_block(
    node: &mut ChromeNode,
    region: &wiring::Region,
    module: &str,
    rect: Rect,
    props: serde_json::Value,
) -> bool {
    match node {
        ChromeNode::Row { children } | ChromeNode::Column { children } => {
            let mut found = false;
            for child in children.iter_mut() {
                found |= bind_block(child, region, module, rect, props.clone());
            }
            found
        }
        ChromeNode::Block { content, size, .. } => {
            let matches = matches!(
                content,
                ChromeContent::Builtin { builtin } if builtin == region.name
            );
            if !matches {
                return false;
            }
            *content = ChromeContent::Mod {
                r#mod: module.to_string(),
                props,
            };
            *size = Some((region.size)(rect));
            true
        }
    }
}
