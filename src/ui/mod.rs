//! Ratatui 渲染层：Tab 栏、侧栏、BSP 分屏里的 Pane、状态栏。
//!
//! 只画 Pane 的内容区；边框、Tab 栏、侧栏、状态栏都不交给终端内容。宽度与中文
//! 宽字符的对齐由 `term::view` 保证。所有颜色都来自 [`crate::theme`] 的 token。

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::{
    App, Chooser, Connection, GotoOverlay, HelpOverlay, MobileAction, MobileTarget, Prompt, Reader,
    SettingsOverlay, Toast,
};
use crate::config::{CollapsedSidebar, HostCursor, TabBarPosition};
use crate::model::{Badge, PaneState, Slot};
use crate::term::view::PlacementImage;
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let theme = app.config.theme.clone();
    let mobile = app.config.mouse_capture
        && app.config.mobile_width_threshold > 0
        && area.width <= app.config.mobile_width_threshold;
    app.mobile = mobile;
    if mobile {
        draw_mobile(frame, app, area, &theme);
        return;
    }
    app.switcher = false;
    app.mobile_scroll = 0;

    frame.render_widget(
        Block::default().style(Style::default().bg(theme.color("bg"))),
        area,
    );

    // 侧栏占满整列高度；Tab 栏和内容都在它右侧。
    let rail_width = if app.sidebar {
        if area.width > app.sidebar_width + 40 {
            app.sidebar_width
        } else {
            0
        }
    } else if app.config.collapsed_sidebar == CollapsedSidebar::Compact && area.width > 40 {
        4
    } else {
        0
    };
    let (sidebar, main) = if rail_width > 0 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(rail_width), Constraint::Min(20)])
            .split(area);
        (Some(cols[0]), cols[1])
    } else {
        app.workspace_hits.clear();
        app.agent_hits.clear();
        (None, area)
    };
    match sidebar {
        Some(rail) if app.sidebar => render_sidebar(frame, app, rail, &theme),
        Some(rail) => render_rail(frame, app, rail, &theme),
        None => {}
    }

    let tabs = app
        .workspace()
        .map(|workspace| workspace.tabs.len())
        .unwrap_or(0);
    let show_tab_bar = !(app.config.hide_tab_bar_when_single_tab && tabs <= 1);
    let bottom_bar = app.config.tab_bar_position == TabBarPosition::Bottom;
    let mode_line = app.mode_line();
    let mut constraints = Vec::new();
    if show_tab_bar && !bottom_bar {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(3));
    if show_tab_bar && bottom_bar {
        constraints.push(Constraint::Length(1));
    }
    if mode_line.is_some() {
        constraints.push(Constraint::Length(1));
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(main);
    let mut index = 0;
    let tab_bar = if show_tab_bar && !bottom_bar {
        index += 1;
        Some(rows[0])
    } else {
        None
    };
    let body = rows[index];
    index += 1;
    let bottom_tab_bar = if show_tab_bar && bottom_bar {
        let rect = rows[index];
        index += 1;
        Some(rect)
    } else {
        None
    };
    if !show_tab_bar {
        app.tab_bar = Rect::new(main.x, main.y, 0, 0);
        app.tab_hits.clear();
        app.tab_add = Rect::new(main.x, main.y, 0, 0);
    }
    let status = if mode_line.is_some() {
        Some(rows[index])
    } else {
        None
    };
    if let Some(rect) = bottom_tab_bar {
        render_tab_bar(frame, app, rect, &theme);
    }

    if let Some(tab_bar) = tab_bar {
        render_tab_bar(frame, app, tab_bar, &theme);
    }

    app.reflow(body);
    let slots = app.slots.clone();
    for slot in &slots {
        render_pane(frame, app, *slot, &theme);
    }
    render_overlays(frame, app, &theme);
    if let Some(reader) = &app.reader {
        render_reader(frame, reader, app.content, &theme);
    }
    if let Some(chooser) = &app.chooser {
        render_chooser(frame, chooser, app.content, &theme);
    }
    if let Some(help) = &app.help {
        let rows = app.help_view();
        clear_area(frame, app.content, &theme);
        render_help(frame, help, &rows, app.content, &theme);
    }
    if let Some(settings) = &app.settings {
        clear_area(frame, app.content, &theme);
        render_settings(frame, settings, app, app.content, &theme);
    }
    if let Some(goto) = &app.goto {
        let rows = app.goto_rows();
        clear_area(frame, area, &theme);
        render_goto(frame, goto, &rows, area, &theme);
    }
    if let Some(prompt) = &app.prompt {
        render_prompt(frame, prompt, app.content, &theme);
    }
    if let (Some(status), Some(line)) = (status, mode_line) {
        render_mode_line(frame, app, status, &line, &theme);
    }
}

fn draw_mobile(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(area);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.color("bg"))),
        area,
    );
    render_mobile_header(frame, app, rows[0], theme);
    let content = rows[1];
    app.reflow(content);
    if app.switcher {
        render_mobile_switcher(frame, app, content, theme);
        return;
    }
    let slots = app.slots.clone();
    for slot in &slots {
        render_pane(frame, app, *slot, theme);
    }
    render_overlays(frame, app, theme);
    if let Some(reader) = &app.reader {
        render_reader(frame, reader, app.content, theme);
    }
    if let Some(chooser) = &app.chooser {
        render_chooser(frame, chooser, app.content, theme);
    }
}

/// 手机顶栏：左边一行状态，右边一个按钮；按钮在面板打开时是 `close`。
fn render_mobile_header(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    app.mobile_switch = Rect::default();
    app.mobile_close = Rect::default();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let button_width = 8.min(area.width);
    let button = Rect::new(
        area.right().saturating_sub(button_width),
        area.y,
        button_width,
        1,
    );
    let (text, background) = if app.switcher {
        app.mobile_close = button;
        (" close ", theme.color("badge_bg"))
    } else {
        app.mobile_switch = button;
        (" switch ", theme.color("accent"))
    };
    let status_width = button.x.saturating_sub(area.x).saturating_sub(1);
    let label = truncate(&mobile_status(app), status_width as usize);
    let pad = status_width.saturating_sub(label.chars().count() as u16);
    let spans = vec![
        Span::styled(
            label,
            Style::default()
                .bg(theme.color("tab_bar_bg"))
                .fg(theme.color("tab_bar_fg")),
        ),
        Span::raw(" ".repeat(pad as usize)),
        Span::styled(
            text,
            Style::default()
                .bg(background)
                .fg(theme.color("tab_active_fg"))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.color("tab_bar_bg"))),
        Rect::new(area.x, area.y, area.width, 1),
    );
}

fn mobile_status(app: &App) -> String {
    if let Some(prompt) = &app.prompt {
        let (before, after) = prompt
            .buffer
            .split_at(prompt.cursor.min(prompt.buffer.len()));
        return format!(" {} {}▏{}", prompt.label, before, after);
    }
    let mut line = String::new();
    if !matches!(app.connection, Connection::Ready { .. }) {
        line.push_str(&format!(" {} ", app.connection.label()));
    }
    match app.workspace() {
        Some(workspace) => {
            let label = app.workspace_label(app.active_ws);
            line.push(' ');
            line.push_str(&label);
            line.push_str(&format!(
                "  tab {}/{}",
                workspace.active + 1,
                workspace.tabs.len()
            ));
        }
        None => line.push_str(" no workspace"),
    }
    let attention = app.attention_count(app.active_ws);
    if attention > 0 {
        line.push_str(&format!("  {attention} 待处理"));
    }
    line
}

struct MobileRow {
    text: String,
    style: Style,
    target: Option<MobileTarget>,
}

/// 面板内容：workspaces、当前工作区的 tabs、agents 与动作，一行一个可点条目。
fn mobile_rows(app: &App, theme: &Theme, width: usize) -> Vec<MobileRow> {
    let mut rows = Vec::new();
    let section = |label: &str| MobileRow {
        text: format!(" {label} "),
        style: Style::default()
            .fg(theme.color("sidebar_title"))
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        target: None,
    };

    rows.push(section("workspaces"));
    for index in 0..app.workspaces.len() {
        let label = app.workspace_label(index);
        let attention = app.attention_count(index);
        let mut text = format!(
            "{} {}",
            if index == app.active_ws { "▸" } else { " " },
            truncate(&label, width.saturating_sub(6))
        );
        if attention > 0 {
            text.push_str(&format!("  {attention} 待处理"));
        }
        let style = if index == app.active_ws {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
        } else {
            Style::default().fg(theme.color("fg"))
        };
        rows.push(MobileRow {
            text,
            style,
            target: Some(MobileTarget::Workspace(index)),
        });
    }

    rows.push(section("tabs"));
    if let Some(workspace) = app.workspace() {
        for (index, tab) in workspace.tabs.iter().enumerate() {
            let style = if index == workspace.active {
                Style::default()
                    .bg(theme.color("selection_bg"))
                    .fg(theme.color("selection_fg"))
            } else {
                Style::default().fg(theme.color("fg"))
            };
            rows.push(MobileRow {
                text: format!(
                    "{} {}",
                    if index == workspace.active {
                        "▸"
                    } else {
                        " "
                    },
                    truncate(&tab.display(&app.panes), width.saturating_sub(4))
                ),
                style,
                target: Some(MobileTarget::Tab(index)),
            });
        }
    } else {
        rows.push(MobileRow {
            text: "  (no tabs)".to_string(),
            style: Style::default().fg(theme.color("dim")),
            target: None,
        });
    }

    rows.push(section("agents"));
    let symbols = app.config.indicators == crate::config::Indicators::Symbols;
    let mut agents = app.agent_rows();
    // 没有状态上报的 Pane 也列在这里，任何一个 Pane 都能点过去。
    let reporting: Vec<u32> = agents.iter().map(|(pane, _, _)| *pane).collect();
    let mut idle: Vec<(u32, Badge, String)> = app
        .panes
        .iter()
        .filter(|(pane, _)| !reporting.contains(pane))
        .map(|(pane, state)| (*pane, Badge::None, state.label()))
        .collect();
    idle.sort_by_key(|(pane, _, _)| *pane);
    agents.extend(idle);
    if agents.is_empty() {
        rows.push(MobileRow {
            text: "  (none)".to_string(),
            style: Style::default().fg(theme.color("dim")),
            target: None,
        });
    }
    for (pane, badge, label) in agents {
        let position = app
            .locate_public(pane)
            .map(|(workspace, tab)| {
                format!(
                    " w{}:t{}",
                    app.workspaces[workspace].id, app.workspaces[workspace].tabs[tab].id
                )
            })
            .unwrap_or_default();
        let text = format!(
            "  {} {}{position}",
            badge.glyph(symbols),
            truncate(&label, width.saturating_sub(4 + position.chars().count()))
        );
        let style = if pane == app.focused() {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
        } else {
            Style::default().fg(theme.color(badge.token()))
        };
        rows.push(MobileRow {
            text,
            style,
            target: Some(MobileTarget::Agent(pane)),
        });
    }

    rows.push(section("menu"));
    for (label, action) in MOBILE_ACTIONS {
        rows.push(MobileRow {
            text: format!("  {label}"),
            style: Style::default()
                .fg(theme.color("accent"))
                .add_modifier(Modifier::BOLD),
            target: Some(MobileTarget::Action(*action)),
        });
    }
    rows
}

const MOBILE_ACTIONS: &[(&str, MobileAction)] = &[
    ("新建 tab", MobileAction::NewTab),
    ("左右分屏", MobileAction::SplitRight),
    ("上下分屏", MobileAction::SplitDown),
    ("关闭当前 pane", MobileAction::ClosePane),
    ("复制模式", MobileAction::CopyMode),
    ("下一个待处理", MobileAction::NextAgent),
    ("退出（detach）", MobileAction::Detach),
];

fn render_mobile_switcher(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    app.mobile_targets.clear();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.color("bg"))),
        area,
    );
    if area.width == 0 || area.height == 0 {
        app.mobile_max_scroll = 0;
        return;
    }
    let viewport = area;
    let rows = mobile_rows(app, theme, viewport.width as usize);
    app.mobile_max_scroll = rows.len().saturating_sub(viewport.height as usize);
    if app.mobile_scroll > app.mobile_max_scroll {
        app.mobile_scroll = app.mobile_max_scroll;
    }
    let mut lines = Vec::new();
    for (offset, row) in rows
        .iter()
        .skip(app.mobile_scroll)
        .take(viewport.height as usize)
        .enumerate()
    {
        let y = viewport.y + offset as u16;
        if let Some(target) = row.target {
            app.mobile_targets
                .push((target, Rect::new(viewport.x, y, viewport.width, 1)));
        }
        lines.push(Line::from(Span::styled(row.text.clone(), row.style)));
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.color("bg"))),
        viewport,
    );
}

fn render_tab_bar(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    app.tab_bar = area;
    app.tab_hits.clear();
    app.tab_add = Rect::default();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let background = Style::default().bg(theme.color("tab_bar_bg"));
    let mut spans = vec![Span::styled("  ".to_string(), background)];
    let mut x = area.x.saturating_add(2);

    let tabs: Vec<(usize, String, Badge)> = app
        .workspace()
        .map(|workspace| {
            workspace
                .tabs
                .iter()
                .enumerate()
                .map(|(index, tab)| {
                    let attention = tab
                        .panes()
                        .iter()
                        .filter_map(|pane| app.panes.get(&pane.0))
                        .map(|pane| pane.agent.badge())
                        .min()
                        .unwrap_or(Badge::None);
                    (index, tab.display(&app.panes), attention)
                })
                .collect()
        })
        .unwrap_or_default();
    let active_tab = app
        .workspace()
        .map(|workspace| workspace.active)
        .unwrap_or(0);

    for (index, _label, badge) in tabs {
        let selected = index == active_tab;
        let zoom = if selected && app.zoom { "Z" } else { " " };
        let flag = match badge {
            Badge::Blocked | Badge::Error | Badge::Done => badge.glyph(false),
            _ => " ",
        };
        let text = format!(" {} {} {} ", index + 1, zoom, flag);
        let width = text.chars().count() as u16;
        let style = if selected {
            Style::default()
                .bg(theme.color("tab_active_bg"))
                .fg(theme.color("tab_active_fg"))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .bg(theme.color("tab_bar_bg"))
                .fg(if badge <= Badge::Done {
                    theme.color(badge.token())
                } else {
                    theme.color("tab_bar_fg")
                })
        };
        let visible = width.min(area.right().saturating_sub(x));
        app.tab_hits.push((index, Rect::new(x, area.y, visible, 1)));
        x = x.saturating_add(width);
        spans.push(Span::styled(text, style));
    }

    let add = " + ";
    let add_width = add.chars().count() as u16;
    if x < area.right() {
        app.tab_add = Rect::new(x, area.y, add_width.min(area.right().saturating_sub(x)), 1);
        spans.push(Span::styled(
            add.to_string(),
            Style::default()
                .bg(theme.color("tab_bar_bg"))
                .fg(theme.color("dim")),
        ));
        x = x.saturating_add(add_width);
    }

    let right_text = tab_bar_right_text(app);
    let used = x.saturating_sub(area.x);
    if !right_text.is_empty() && area.width > used + right_text.chars().count() as u16 {
        let pad = area
            .width
            .saturating_sub(used + right_text.chars().count() as u16);
        spans.push(Span::styled(" ".repeat(pad as usize), background));
        spans.push(Span::styled(
            right_text,
            Style::default()
                .bg(theme.color("tab_bar_bg"))
                .fg(theme.color("dim")),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).style(background), area);
}

/// 右对齐的状态条目：`zoom` / `hostname` / `datetime` / `text` / `cmd:`。
fn tab_bar_right_text(app: &App) -> String {
    let mut parts: Vec<String> = Vec::new();
    for token in &app.config.tab_bar_right {
        match token.as_str() {
            "zoom" => {
                if app.zoom {
                    parts.push("zoom".to_string());
                }
            }
            "hostname" => parts.push(hostname()),
            "datetime" => parts.push(datetime()),
            "text" => {}
            token if token.starts_with("cmd:") => {
                if let Some(output) = app.command_output.get(token) {
                    if !output.is_empty() {
                        parts.push(output.clone());
                    }
                }
            }
            _ => {}
        }
    }
    parts.join(&app.config.tab_bar_right_separator)
}

fn render_sidebar(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    app.sidebar_area = area;
    app.workspace_hits.clear();
    app.agent_hits.clear();
    app.terminal_hits.clear();
    app.sidebar_new = Rect::default();
    app.sidebar_menu = Rect::default();

    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme.color("border")))
        .style(Style::default().bg(theme.color("sidebar_bg")));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 4 || inner.height < 2 {
        return;
    }
    let width = inner.width as usize;
    let title = |text: String| {
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(theme.color("sidebar_title"))
                .add_modifier(Modifier::BOLD),
        ))
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut row = inner.y;
    lines.push(title(" spaces".to_string()));
    row = row.saturating_add(1);

    // 每个 space 两行：名字与分支。
    let spaces: Vec<String> = (0..app.workspaces.len())
        .map(|index| app.workspace_label(index))
        .collect();
    for (index, label) in spaces.into_iter().enumerate() {
        if row + 2 > inner.y + inner.height {
            break;
        }
        let active = index == app.active_ws;
        let attention = app.attention_count(index);
        let text = format!(
            " {} {}",
            if active { "·" } else { " " },
            truncate(
                &label,
                width
                    .saturating_sub(5)
                    .saturating_sub(if attention > 0 { 2 } else { 0 })
            )
        );
        let style = if active {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
        } else {
            Style::default().fg(theme.color("fg"))
        };
        app.workspace_hits
            .push((index, Rect::new(inner.x, row, inner.width, 2)));
        lines.push(Line::from(Span::styled(text, style)));
        row = row.saturating_add(1);

        if let Some(cwd) = app.workspace_cwd(index) {
            app.refresh_branch(&cwd);
        }
        let branch = app
            .workspace_cwd(index)
            .and_then(|cwd| app.branch(&cwd))
            .unwrap_or_default();
        let mut detail = format!("   {}", truncate(&branch, width.saturating_sub(4)));
        if attention > 0 {
            detail.push_str(&format!(" {attention}"));
        }
        let detail_style = if active {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("dim"))
        } else {
            Style::default().fg(theme.color("dim"))
        };
        lines.push(Line::from(Span::styled(detail, detail_style)));
        row = row.saturating_add(1);
    }

    if row < inner.y + inner.height {
        lines.push(Line::from(""));
        row = row.saturating_add(1);
    }

    // `new` 与 `menu`：与 herdr 的侧栏底行同位置同含义。
    if row < inner.y + inner.height {
        let new = " new";
        let menu = "● menu ";
        let menu_width = menu.chars().count() as u16;
        app.sidebar_new = Rect::new(
            inner.x,
            row,
            (new.chars().count() as u16).min(inner.width),
            1,
        );
        if inner.width > menu_width {
            app.sidebar_menu = Rect::new(inner.x + inner.width - menu_width, row, menu_width, 1);
        }
        let pad = width.saturating_sub(new.chars().count() + menu.chars().count());
        lines.push(Line::from(vec![
            Span::styled(
                new.to_string(),
                Style::default()
                    .fg(theme.color("accent"))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(pad)),
            Span::styled(
                "●".to_string(),
                Style::default()
                    .fg(theme.color("accent"))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " menu ".to_string(),
                Style::default().fg(theme.color("tab_bar_fg")),
            ),
        ]));
        row = row.saturating_add(1);
    }

    if row < inner.y + inner.height {
        lines.push(Line::from(Span::styled(
            "─".repeat(width),
            Style::default().fg(theme.color("border")),
        )));
        row = row.saturating_add(1);
    }

    if row < inner.y + inner.height {
        let sort = match app.config.agent_sort {
            crate::config::AgentSort::Spaces => "grouped",
            crate::config::AgentSort::Priority => "priority",
        };
        lines.push(nav_title(" agents", sort, width, theme));
        row = row.saturating_add(1);
    }

    // agents 段：先是有状态上报的 Pane，再是其它客户端创建的终端。
    let mut rows: Vec<(u32, Badge, String)> = app.agent_rows();
    let reporting: Vec<u32> = rows.iter().map(|(pane, _, _)| *pane).collect();
    let mut idle_panes: Vec<(u32, Badge, String)> = app
        .panes
        .iter()
        .filter(|(pane, _)| !reporting.contains(pane))
        .map(|(pane, state)| (*pane, Badge::None, state.label()))
        .collect();
    idle_panes.sort_by_key(|(pane, _, _)| *pane);
    rows.extend(idle_panes);
    let unowned: Vec<(String, String)> = app
        .terminals_without_pane()
        .into_iter()
        .map(|terminal| {
            let label = if terminal.title.is_empty() {
                terminal.address.clone()
            } else {
                format!("{} · {}", terminal.address, terminal.title)
            };
            (
                terminal.address.clone(),
                truncate(&label, width.saturating_sub(4)),
            )
        })
        .collect();
    let total = rows.len() + unowned.len();
    let available = (inner.y + inner.height).saturating_sub(row) as usize;
    let max_scroll = total.saturating_sub(available);
    if app.sidebar_scroll > max_scroll {
        app.sidebar_scroll = max_scroll;
    }
    let start = app.sidebar_scroll.min(total);
    for (pane, badge, label) in rows.iter().skip(start) {
        if row >= inner.y + inner.height {
            break;
        }
        let focused = *pane == app.focused();
        let text = format!(
            " {} {}",
            badge.glyph(app.config.indicators == crate::config::Indicators::Symbols),
            truncate(label, width.saturating_sub(4))
        );
        let style = if focused {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
        } else if *badge == Badge::None {
            Style::default().fg(theme.color("fg"))
        } else {
            Style::default().fg(theme.color(badge.token()))
        };
        app.agent_hits
            .push((*pane, Rect::new(inner.x, row, inner.width, 1)));
        lines.push(Line::from(Span::styled(text, style)));
        row = row.saturating_add(1);
    }
    let skip_panes = start.saturating_sub(rows.len());
    for (address, label) in unowned.iter().skip(skip_panes) {
        if row >= inner.y + inner.height {
            break;
        }
        app.terminal_hits
            .push((address.clone(), Rect::new(inner.x, row, inner.width, 1)));
        lines.push(Line::from(Span::styled(
            format!(" · {label}"),
            Style::default().fg(theme.color("dim")),
        )));
        row = row.saturating_add(1);
    }
    if total == 0 && row < inner.y + inner.height {
        lines.push(Line::from(Span::styled(
            "  (no agents)",
            Style::default().fg(theme.color("dim")),
        )));
        row = row.saturating_add(1);
    }

    // 右下角的滚动指示，与 herdr 侧栏一致。
    if total > available && row < inner.y + inner.height {
        let glyph = if app.sidebar_scroll + available < total {
            "«"
        } else {
            "»"
        };
        let last = inner.y + inner.height - 1;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                glyph,
                Style::default().fg(theme.color("dim")),
            ))),
            Rect::new(inner.x + inner.width - 1, last, 1, 1),
        );
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// 段标题：左边名字，右边状态词。
fn nav_title(left: &str, right: &str, width: usize, theme: &Theme) -> Line<'static> {
    let pad = width.saturating_sub(left.chars().count() + right.chars().count());
    Line::from(vec![
        Span::styled(
            left.to_string(),
            Style::default()
                .fg(theme.color("sidebar_title"))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(pad)),
        Span::styled(right.to_string(), Style::default().fg(theme.color("dim"))),
    ])
}

fn render_pane(frame: &mut Frame, app: &mut App, slot: Slot, theme: &Theme) {
    let focused = slot.pane == app.focused();
    let note = app.note.clone();
    let drag = drag_selection(app, slot.pane);
    let copy_overlay = app
        .copy
        .as_ref()
        .filter(|copy| copy.pane == slot.pane)
        .map(|copy| (copy.selection(), copy.cursor));
    let copy_matches = app
        .copy
        .as_ref()
        .filter(|copy| copy.pane == slot.pane)
        .map(|copy| copy.matches.clone());
    let copy_active = app.copy.is_some();
    let state = app.panes.get(&slot.pane);
    let badge = state.map(|pane| pane.agent.badge()).unwrap_or(Badge::None);
    let symbols = app.config.indicators == crate::config::Indicators::Symbols;
    // herdr 不在边框上写终端地址：只有手动命名或开启 Agent 标签时才出现标题。
    let title = match state.map(|pane| &pane.state) {
        Some(PaneState::Live { .. }) => {
            let named = state.and_then(|pane| {
                pane.name
                    .as_deref()
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
            });
            match named {
                Some(name) => format!(" {} ", truncate(&name, 24)),
                None if app.config.show_agent_labels_on_pane_borders && badge != Badge::None => {
                    let label = state.map(|pane| pane.label()).unwrap_or_default();
                    format!(" {} {} ", badge.glyph(symbols), truncate(&label, 22))
                }
                None => String::new(),
            }
        }
        Some(PaneState::Ended { reason }) => format!(" closed: {} ", short(reason)),
        Some(PaneState::Waiting) | None => " starting ".to_string(),
    };
    let border_color = if slot.overlay {
        theme.color("accent")
    } else if focused {
        theme.color("border_focused")
    } else if badge == Badge::Blocked || badge == Badge::Error {
        theme.color(badge.token())
    } else {
        theme.color("border")
    };
    let title = if slot.overlay {
        format!("popup ·{title}")
    } else {
        title
    };
    let block = Block::default()
        .borders(slot.borders)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme.color("bg")))
        .title(Span::styled(title, Style::default().fg(border_color)));
    frame.render_widget(block, slot.outer);

    if slot.inner.width == 0 || slot.inner.height == 0 {
        return;
    }

    let Some(pane) = app.panes.get_mut(&slot.pane) else {
        return;
    };
    if !matches!(pane.state, PaneState::Live { .. } | PaneState::Ended { .. }) {
        let text = vec![
            Line::from(Span::styled(note, Style::default().fg(theme.color("fg")))),
            Line::from(""),
            Line::from(Span::styled(
                "ctrl+b ? 看键位",
                Style::default().fg(theme.color("dim")),
            )),
        ];
        frame.render_widget(
            Paragraph::new(text)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
            slot.inner,
        );
        return;
    }

    if pane.view.render(frame.buffer_mut(), slot.inner).is_err() {
        let text = "VT render failed\n\nuse `hc read` to look at it";
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), slot.inner);
        return;
    }
    let cursor = pane.view.cursor();
    let live = matches!(pane.state, PaneState::Live { .. });
    let scrollbar = pane.view.scrollbar();
    let images = pane.view.placement_images();

    let buffer = frame.buffer_mut();
    if let Some((start, end)) = drag {
        paint_selection(buffer, slot.inner, start, end, theme.color("selection_bg"));
    }
    for image in &images {
        paint_placement(buffer, slot.inner, image);
    }
    if let Some(matches) = copy_matches {
        for (x, y) in matches {
            paint_cell(
                buffer,
                slot.inner,
                (x, y),
                theme.color("badge_bg"),
                theme.color("badge_fg"),
            );
        }
    }
    if let Some((selection, at)) = copy_overlay {
        if let Some((start, end)) = selection {
            paint_selection(buffer, slot.inner, start, end, theme.color("selection_bg"));
        }
        paint_cell(
            buffer,
            slot.inner,
            at,
            theme.color("cursor_bg"),
            theme.color("cursor_fg"),
        );
    }
    if let Some((total, offset, len)) = scrollbar {
        if app.config.pane_scrollbars {
            paint_scrollbar(buffer, slot.inner, total, offset, len, theme);
        }
    }
    if focused && live && !copy_active {
        if let Some((x, y)) = cursor {
            if x < slot.inner.width && y < slot.inner.height {
                if app.config.host_cursor == HostCursor::Drawn {
                    paint_cell(
                        frame.buffer_mut(),
                        slot.inner,
                        (x, y),
                        theme.color("cursor_bg"),
                        theme.color("cursor_fg"),
                    );
                } else {
                    frame.set_cursor_position((slot.inner.x + x, slot.inner.y + y));
                }
            }
        }
    }
}

fn render_overlays(frame: &mut Frame, app: &App, theme: &Theme) {
    if let Some(toast) = &app.toast {
        render_toast(frame, toast, app.content, theme);
    }
}

fn render_toast(frame: &mut Frame, toast: &Toast, content: Rect, theme: &Theme) {
    if content.width < 12 || content.height < 3 {
        return;
    }
    let width = (toast.text.chars().count() as u16 + 4)
        .min(content.width)
        .max(12);
    let area = Rect::new(
        content.x + content.width.saturating_sub(width),
        content.y + content.height.saturating_sub(3),
        width,
        3,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.color("accent")))
        .style(Style::default().bg(theme.color("toast_bg")));
    frame.render_widget(block.clone(), area);
    frame.render_widget(
        Paragraph::new(truncate(&toast.text, width.saturating_sub(3) as usize))
            .style(
                Style::default()
                    .bg(theme.color("toast_bg"))
                    .fg(theme.color("toast_fg")),
            )
            .wrap(Wrap { trim: true }),
        block.inner(area),
    );
}

fn render_mode_line(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    line: &(String, String),
    theme: &Theme,
) {
    let background = Style::default()
        .bg(theme.color("status_bar_bg"))
        .fg(theme.color("status_bar_fg"));
    let mut spans = Vec::new();
    if !line.0.is_empty() {
        spans.push(Span::styled(
            format!(" {} ", line.0),
            Style::default()
                .bg(theme.color("accent"))
                .fg(theme.color("tab_active_fg"))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    } else {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        line.1.clone(),
        Style::default().fg(if line.0.is_empty() {
            theme.color("dim")
        } else {
            theme.color("status_bar_fg")
        }),
    ));
    let right = match &app.connection {
        Connection::Ready { .. } => String::new(),
        other => other.label(),
    };
    if !right.is_empty() {
        let used = line.0.chars().count() + line.1.chars().count() + 3;
        if area.width as usize > used + right.chars().count() {
            let pad = area.width as usize - used - right.chars().count();
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(
                right,
                Style::default().fg(theme.color("agent_working")),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(background), area);
}

fn clear_area(frame: &mut Frame, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let blank = " ".repeat(area.width as usize);
    let lines: Vec<Line> = (0..area.height)
        .map(|_| Line::from(Span::raw(blank.clone())))
        .collect();
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.color("sidebar_bg"))),
        area,
    );
}

fn centered(content: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(content.width).max(1);
    let height = height.min(content.height).max(1);
    Rect::new(
        content.x + (content.width - width) / 2,
        content.y + (content.height - height) / 2,
        width,
        height,
    )
}

fn render_prompt(frame: &mut Frame, prompt: &Prompt, content: Rect, theme: &Theme) {
    if content.width < 20 || content.height < 8 {
        return;
    }
    let width = 56.min(content.width - 2);
    let area = centered(content, width, 7);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.color("accent")))
        .style(Style::default().bg(theme.color("sidebar_bg")));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let confirm = prompt.kind.needs_confirm();
    let footer = if confirm {
        "↵ confirm    esc cancel"
    } else {
        "↵ save    ^c clear    esc cancel"
    };
    let (before, after) = prompt
        .buffer
        .split_at(prompt.cursor.min(prompt.buffer.len()));
    let input = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            before.to_string(),
            Style::default()
                .fg(theme.color("fg"))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("▏", Style::default().fg(theme.color("accent"))),
        Span::styled(after.to_string(), Style::default().fg(theme.color("fg"))),
    ]);
    let footer_pad = (inner.width as usize)
        .saturating_sub(footer.chars().count())
        .saturating_sub(3);
    let lines = vec![
        Line::from(Span::styled(
            format!(" {}", prompt.kind.title()),
            Style::default()
                .fg(theme.color("accent"))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        input,
        Line::from(vec![
            Span::raw(" ".repeat(footer_pad)),
            Span::styled(footer.to_string(), Style::default().fg(theme.color("dim"))),
        ]),
        Line::from(""),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.color("sidebar_bg"))),
        inner,
    );
}

fn render_help(
    frame: &mut Frame,
    help: &HelpOverlay,
    rows: &[crate::app::HelpRow],
    content: Rect,
    theme: &Theme,
) {
    let area = content;
    if area.width < 20 || area.height < 6 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.color("accent")))
        .style(Style::default().bg(theme.color("sidebar_bg")));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;

    let header = nav_title(" keybinds", "esc close", width, theme);
    let query_line = if help.query.is_empty() {
        Line::from(vec![
            Span::styled("  /", Style::default().fg(theme.color("accent"))),
            Span::styled(
                " press / to filter by command or shortcut",
                Style::default().fg(theme.color("dim")),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("  / ", Style::default().fg(theme.color("accent"))),
            Span::styled(help.query.clone(), Style::default().fg(theme.color("fg"))),
        ])
    };
    let rule = Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.color("border")),
    ));
    let footer = "search / · scroll j/k/↑↓/pgup/pgdn · close esc/enter";
    let body_rows = inner.height.saturating_sub(4) as usize;
    let scroll = if help.selected >= body_rows {
        help.selected + 1 - body_rows
    } else {
        0
    };

    let mut lines = vec![header, query_line, rule];
    for (index, row) in rows.iter().enumerate().skip(scroll).take(body_rows) {
        let selected = index == help.selected;
        let text = if row.section {
            format!(" {}", row.text)
        } else {
            format!(" {:28} {}", row.key, row.text)
        };
        // 右侧滚动条列。
        let pad = width.saturating_sub(text.chars().count() + 1);
        let style = if selected {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
        } else if row.section {
            Style::default()
                .fg(theme.color("accent"))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.color("fg"))
        };
        let mut spans = vec![Span::styled(text, style)];
        if width > 2 {
            spans.push(Span::styled(
                " ".repeat(pad),
                Style::default()
                    .bg(theme.color("sidebar_bg"))
                    .fg(theme.color("dim")),
            ));
            let track = if body_rows > 0 {
                (index - scroll) * 5 / body_rows.max(1)
            } else {
                0
            };
            spans.push(Span::styled(
                if track % 5 == 0 { "▐" } else { " " },
                Style::default().fg(theme.color("accent")),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(Span::styled(
        format!(" {footer}"),
        Style::default().fg(theme.color("dim")),
    )));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.color("sidebar_bg"))),
        inner,
    );
}

fn render_settings(
    frame: &mut Frame,
    settings: &SettingsOverlay,
    app: &App,
    content: Rect,
    theme: &Theme,
) {
    let area = Rect::new(
        content.x + 1,
        content.y,
        content.width.saturating_sub(2),
        content.height.saturating_sub(1),
    );
    if area.width < 24 || area.height < 8 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.color("accent")))
        .style(Style::default().bg(theme.color("sidebar_bg")));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;

    let sections = App::SETTINGS_SECTIONS;
    let mut tabs = String::new();
    for (index, name) in sections.iter().enumerate() {
        if index > 0 {
            tabs.push_str("   ");
        }
        tabs.push_str(name);
    }
    let mut tab_spans = vec![Span::styled(
        " settings",
        Style::default()
            .fg(theme.color("accent"))
            .add_modifier(Modifier::BOLD),
    )];
    tab_spans.push(Span::styled(
        " ".repeat(width.saturating_sub(" settings".chars().count() + 1)),
        Style::default(),
    ));
    let tab_line = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(tabs, Style::default().fg(theme.color("sidebar_title"))),
    ]);
    let _ = tab_spans;
    let rule = Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.color("border")),
    ));

    let len = app.settings_len(settings.section);
    let mut lines = vec![
        Line::from(Span::styled(
            " settings",
            Style::default()
                .fg(theme.color("accent"))
                .add_modifier(Modifier::BOLD),
        )),
        tab_line,
        rule,
    ];
    for index in 0..len {
        let selected = index == settings.selected;
        let (label, active) = settings_item(app, settings.section, index);
        let marker = if selected { "▸" } else { " " };
        let check = if active { "✓" } else { " " };
        let text = format!(
            " {marker} {label}{}",
            if active {
                format!(" {check}")
            } else {
                String::new()
            }
        );
        let style = if selected {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
        } else if active {
            Style::default()
                .fg(theme.color("accent"))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.color("fg"))
        };
        lines.push(Line::from(Span::styled(text, style)));
    }
    let footer_left = "↑↓ select  tab section";
    let footer_right = "↵ apply     esc close";
    let pad = width.saturating_sub(footer_left.chars().count() + footer_right.chars().count() + 2);
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {footer_left}"),
            Style::default().fg(theme.color("dim")),
        ),
        Span::raw(" ".repeat(pad)),
        Span::styled(
            footer_right.to_string(),
            Style::default().fg(theme.color("dim")),
        ),
    ]));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.color("sidebar_bg"))),
        inner,
    );
}

/// 设置项的文字与是否当前生效。
fn settings_item(app: &App, section: usize, index: usize) -> (String, bool) {
    match section {
        0 => {
            let name = crate::theme::names()[index];
            (name.to_string(), name == app.config.theme.name)
        }
        1 => {
            let symbols = index == 1;
            (
                if symbols { "symbols" } else { "dots" }.to_string(),
                (app.config.indicators == crate::config::Indicators::Symbols) == symbols,
            )
        }
        2 => {
            let names = ["off", "toast", "system"];
            let value = names[index.min(2)];
            let active = match app.config.notifications {
                crate::config::Delivery::Off => value == "off",
                crate::config::Delivery::Toast => value == "toast",
                crate::config::Delivery::System => value == "system",
            };
            (value.to_string(), active)
        }
        _ => {
            let on = index == 1;
            (
                if on { "on" } else { "off" }.to_string(),
                app.config.sound == on,
            )
        }
    }
}

fn render_goto(
    frame: &mut Frame,
    goto: &GotoOverlay,
    rows: &[crate::app::GotoRow],
    screen: Rect,
    theme: &Theme,
) {
    let area = Rect::new(
        screen.x + 2,
        screen.y + 1,
        screen.width.saturating_sub(4),
        screen.height.saturating_sub(2),
    );
    if area.width < 24 || area.height < 8 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.color("accent")))
        .style(Style::default().bg(theme.color("sidebar_bg")));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let terminals = rows.iter().filter(|row| !row.section).count();
    let title_right = format!("{terminals} terminals");
    let mut lines = vec![
        nav_title("─ Go to ", &format!("{title_right} "), width, theme),
        Line::from(vec![
            Span::styled(" / ", Style::default().fg(theme.color("accent"))),
            Span::styled(
                if goto.query.is_empty() {
                    "search agents and terminals".to_string()
                } else {
                    goto.query.clone()
                },
                Style::default().fg(if goto.query.is_empty() {
                    theme.color("dim")
                } else {
                    theme.color("fg")
                }),
            ),
        ]),
        Line::from(Span::styled(
            "─".repeat(width),
            Style::default().fg(theme.color("border")),
        )),
    ];
    let body_rows = inner.height.saturating_sub(5) as usize;
    let scroll = if goto.selected >= body_rows {
        goto.selected + 1 - body_rows
    } else {
        0
    };
    for (index, row) in rows.iter().enumerate().skip(scroll).take(body_rows) {
        let selected = index == goto.selected;
        let (left, right) = if row.section {
            (format!(" {}", row.text), row.right.clone())
        } else {
            (format!(" ├─ {}", row.text), row.right.clone())
        };
        let pad = width.saturating_sub(left.chars().count() + right.chars().count() + 1);
        let style = if selected {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
        } else if row.section {
            Style::default().fg(theme.color("sidebar_title"))
        } else {
            Style::default().fg(theme.color("fg"))
        };
        lines.push(Line::from(vec![
            Span::styled(left, style),
            Span::styled(
                " ".repeat(pad),
                Style::default().bg(theme.color("sidebar_bg")),
            ),
            Span::styled(format!("{right} "), Style::default().fg(theme.color("dim"))),
        ]));
    }
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            " No matching agents or terminals",
            Style::default().fg(theme.color("dim")),
        )));
    }
    if let Some(row) = rows.get(goto.selected) {
        if !row.context.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(" {}", row.context),
                Style::default().fg(theme.color("fg")),
            )));
            lines.push(Line::from(Span::styled(
                format!(" {}", row.cwd),
                Style::default().fg(theme.color("dim")),
            )));
        }
    }
    lines.push(Line::from(Span::styled(
        " / search · ↑↓/j/k rows · enter open · esc close",
        Style::default().fg(theme.color("dim")),
    )));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.color("sidebar_bg"))),
        inner,
    );
}

fn datetime() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    // 只用到本地时区的时分，手算 UTC+offset 会引入时区库依赖，这里给 UTC。
    let minutes = (now / 60) % (24 * 60);
    format!("{:02}:{:02}Z", minutes / 60, minutes % 60)
}

fn render_rail(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme.color("border")))
        .style(Style::default().bg(theme.color("sidebar_bg")));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = Vec::new();
    for (index, workspace) in app.workspaces.iter().enumerate() {
        let attention = app.attention_count(index);
        let label = if workspace.label.is_empty() {
            format!("w{}", workspace.id)
        } else {
            workspace.label.clone()
        };
        let color = if attention > 0 {
            theme.color("agent_done")
        } else if index == app.active_ws {
            theme.color("accent")
        } else {
            theme.color("dim")
        };
        lines.push(Line::from(Span::styled(
            truncate(&label, 3),
            Style::default().fg(color),
        )));
        if attention > 0 {
            lines.push(Line::from(Span::styled(
                format!("{attention}"),
                Style::default().fg(theme.color("agent_done")),
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_reader(frame: &mut Frame, reader: &Reader, content: Rect, theme: &Theme) {
    if content.width < 12 || content.height < 5 {
        return;
    }
    let width = (content.width as u32 * 92 / 100) as u16;
    let height = (content.height as u32 * 90 / 100) as u16;
    let area = Rect::new(
        content.x + (content.width - width) / 2,
        content.y + (content.height - height) / 2,
        width,
        height,
    );
    let status = if reader.complete {
        format!("read · {} · {} 行", reader.how, reader.lines.len())
    } else {
        format!("read · {} · 部分内容", reader.how)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.color("accent")))
        .style(Style::default().bg(theme.color("sidebar_bg")))
        .title(Span::styled(
            status,
            Style::default().fg(theme.color("accent")),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let visible = inner.height as usize;
    let window: Vec<Line> = reader
        .lines
        .iter()
        .skip(reader.scroll)
        .take(visible)
        .map(|line| Line::from(line.clone()))
        .collect();
    frame.render_widget(
        Paragraph::new(window)
            .style(
                Style::default()
                    .bg(theme.color("sidebar_bg"))
                    .fg(theme.color("fg")),
            )
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_chooser(frame: &mut Frame, chooser: &Chooser, content: Rect, theme: &Theme) {
    if content.width < 20 || content.height < 5 {
        return;
    }
    let width = content.width.min(64);
    let wanted = chooser.items.len() as u16 + 3;
    let height = wanted.min(content.height.saturating_sub(2)).max(3);
    let area = Rect::new(
        content.x + (content.width - width) / 2,
        content.y + (content.height - height) / 2,
        width,
        height,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.color("accent")))
        .style(Style::default().bg(theme.color("sidebar_bg")))
        .title(Span::styled(
            truncate(&chooser.title, width.saturating_sub(2) as usize),
            Style::default().fg(theme.color("accent")),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let visible = inner.height as usize;
    let start = if chooser.selected >= visible {
        chooser.selected + 1 - visible
    } else {
        0
    };
    let lines: Vec<Line> = chooser
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, item)| {
            let selected = index == chooser.selected;
            let marker = if selected { "▸" } else { " " };
            let text = format!(
                "{} {}. {}",
                marker,
                index + 1,
                truncate(&item.label, inner.width as usize - 5)
            );
            let style = if selected {
                Style::default()
                    .bg(theme.color("selection_bg"))
                    .fg(theme.color("selection_fg"))
            } else {
                Style::default().fg(theme.color("fg"))
            };
            Line::from(Span::styled(text, style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn drag_selection(app: &App, pane: u32) -> Option<((u16, u16), (u16, u16))> {
    let drag = app.drag?;
    if drag.pane != pane {
        return None;
    }
    let a = (drag.start.1, drag.start.0);
    let b = (drag.current.1, drag.current.0);
    Some(if a <= b {
        (drag.start, drag.current)
    } else {
        (drag.current, drag.start)
    })
}

fn paint_selection(
    buffer: &mut Buffer,
    area: Rect,
    start: (u16, u16),
    end: (u16, u16),
    color: Color,
) {
    for row in start.1..=end.1.min(area.height.saturating_sub(1)) {
        let from = if row == start.1 { start.0 } else { 0 };
        let to = if row == end.1 {
            end.0
        } else {
            area.width.saturating_sub(1)
        };
        for column in from..=to.min(area.width.saturating_sub(1)) {
            let cell = &mut buffer[(area.x + column, area.y + row)];
            cell.set_bg(color);
        }
    }
}

/// kitty 图像按单元格采样：每个 cell 取图像对应区域中心的颜色作为背景。
fn paint_placement(buffer: &mut Buffer, area: Rect, image: &PlacementImage) {
    if image.width == 0 || image.height == 0 || image.cols == 0 || image.rows == 0 {
        return;
    }
    let start_x = image.col.max(0) as u16;
    let start_y = image.row.max(0) as u16;
    let columns = image.cols.min(u32::from(u16::MAX)) as u16;
    let rows = image.rows.min(u32::from(u16::MAX)) as u16;
    for cell_y in 0..rows {
        for cell_x in 0..columns {
            let x = start_x.saturating_add(cell_x);
            let y = start_y.saturating_add(cell_y);
            if x >= area.width || y >= area.height {
                continue;
            }
            let pixel_x = u64::from(image.width) * u64::from(cell_x) / u64::from(columns.max(1));
            let pixel_y = u64::from(image.height) * u64::from(cell_y) / u64::from(rows.max(1));
            let pixel_x = (pixel_x as u32).min(image.width - 1);
            let pixel_y = (pixel_y as u32).min(image.height - 1);
            let offset =
                ((u64::from(pixel_y) * u64::from(image.width) + u64::from(pixel_x)) * 4) as usize;
            if offset + 3 >= image.rgba.len() {
                continue;
            }
            let color = Color::Rgb(
                image.rgba[offset],
                image.rgba[offset + 1],
                image.rgba[offset + 2],
            );
            let cell = &mut buffer[(area.x + x, area.y + y)];
            cell.set_symbol(" ");
            cell.set_bg(color);
        }
    }
}

fn paint_scrollbar(
    buffer: &mut Buffer,
    area: Rect,
    total: usize,
    offset: usize,
    len: usize,
    theme: &Theme,
) {
    if area.width < 3 || area.height < 3 || total == 0 || len == 0 {
        return;
    }
    let column = area.x + area.width - 1;
    let track = area.height as usize;
    let thumb = ((len * track) / total).max(1).min(track);
    let start = ((offset * track) / total).min(track.saturating_sub(thumb));
    for row in 0..track {
        let cell = &mut buffer[(column, area.y + row as u16)];
        let inside = row >= start && row < start + thumb;
        cell.set_symbol(if inside { "█" } else { "│" });
        cell.set_fg(if inside {
            theme.color("accent")
        } else {
            theme.color("border")
        });
    }
}

fn paint_cell(buffer: &mut Buffer, area: Rect, at: (u16, u16), bg: Color, fg: Color) {
    if at.0 >= area.width || at.1 >= area.height {
        return;
    }
    let cell = &mut buffer[(area.x + at.0, area.y + at.1)];
    cell.set_bg(bg);
    cell.set_fg(fg);
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default()
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn short(text: &str) -> String {
    truncate(text, 28)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_counts_characters_not_bytes() {
        assert_eq!(truncate("宽字符", 2), "宽…");
        assert_eq!(truncate("宽字符", 3), "宽字符");
        assert_eq!(truncate("abc", 5), "abc");
    }
}
