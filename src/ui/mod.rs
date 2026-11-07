//! Ratatui 渲染层：Tab 栏、侧栏、BSP 分屏里的 Pane、状态栏。
//!
//! 只画 Pane 的内容区；边框、Tab 栏、侧栏、状态栏都不交给终端内容。宽度与中文
//! 宽字符的对齐由 `term::view` 保证。所有颜色都来自 [`crate::theme`] 的 token。

use ratatui_core::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    terminal::Frame,
    text::{Line, Span},
};
use ratatui_widgets::{
    block::Block,
    borders::Borders,
    clear::Clear,
    paragraph::{Paragraph, Wrap},
};

use crossterm::event::KeyCode;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{
    App, Chooser, Connection, GotoOverlay, HelpOverlay, MobileAction, MobileTarget, NavigationRow,
    Prompt, Reader, SettingsOverlay, Toast,
};
use crate::config::HostCursor;
use crate::i18n::{t, t1, t2, tv};
use crate::model::{Badge, PaneState, Slot};
use crate::term::view::PlacementImage;
use crate::theme::Theme;

use crate::app::interaction::PointerTarget;

mod chat;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.pointer_targets.clear();
    app.menu_hit = Rect::default();
    app.pane_drag_hits.clear();
    app.pane_menu_hits.clear();
    app.sync_terminal_theme();
    let theme = app.config.theme.clone();
    let mobile = app.config.mouse_capture
        && app.config.mobile_width_threshold > 0
        && area.width <= app.config.mobile_width_threshold;
    app.mobile = mobile;
    app.agent_group_hits.clear();
    app.agent_sort_hit = Rect::default();
    app.machine_hits.clear();
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

    // 外框树算出本轮区域：侧栏、Tab 栏、内容区、模式行与模块区域。
    let tabs = app
        .workspace()
        .map(|workspace| workspace.tabs.len())
        .unwrap_or(0);
    let mode_line = app.mode_line();
    let params = crate::mods::chrome::LayoutParams {
        config: &app.config,
        sidebar_visible: app.sidebar,
        tab_count: tabs,
        mode_line: mode_line.is_some(),
        area,
    };
    let layout = app.chrome.layout(params);
    app.mod_regions = layout.mods.clone();

    match (layout.sidebar, layout.rail) {
        (Some(rail), _) => render_sidebar(frame, app, rail, &theme),
        (None, Some(rail)) => render_rail(frame, app, rail, &theme),
        (None, None) => {
            app.workspace_hits.clear();
            app.agent_hits.clear();
        }
    }

    if let Some(rect) = layout.tab_bar {
        render_tab_bar(frame, app, rect, &theme);
    } else {
        let corner = Rect::new(layout.column.x, layout.column.y, 0, 0);
        app.tab_bar = corner;
        app.tab_hits.clear();
        app.tab_add = corner;
    }

    app.reflow(layout.body);
    let slots = app.slots.clone();
    for slot in &slots {
        render_pane(frame, app, *slot, &theme);
    }
    // 模块表面按 z 序绘制：region/pane 在下，dialog、popup、window 在上。
    crate::mods::render::draw_surfaces(app, frame);
    render_overlays(frame, app, &theme);
    if app.chat.is_some() {
        chat::render(frame, app, layout.body, &theme);
    }
    if let (Some(status), Some(line)) = (layout.status, mode_line) {
        render_mode_line(frame, app, status, &line, &theme);
    }
    render_modals(frame, app, area, &theme);
}

fn render_modals(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let mut targets = Vec::new();
    if app.copy.is_some() && app.prompt.is_none() && app.chooser.is_none() {
        let bar = Rect::new(
            app.content.x,
            area.bottom().saturating_sub(1),
            app.content.width,
            1,
        );
        let buttons = if app.copy.as_ref().is_some_and(|copy| copy.typing) {
            vec![(t(" Find "), KeyCode::Enter), (t(" Cancel "), KeyCode::Esc)]
        } else {
            vec![
                (t(" Copy "), KeyCode::Char('y')),
                (t(" Search "), KeyCode::Char('/')),
                (" < ", KeyCode::Char('N')),
                (" > ", KeyCode::Char('n')),
                (t(" Done "), KeyCode::Esc),
            ]
        };
        clear_area(frame, bar, theme);
        frame.render_widget(
            Paragraph::new(t("COPY")).style(Style::default().fg(theme.color("accent"))),
            Rect::new(bar.x, bar.y, 5.min(bar.width), 1),
        );
        let mut x = bar.x.saturating_add(6);
        for (label, key) in buttons {
            let rect = Rect::new(x, bar.y, bar.right().saturating_sub(x), 1);
            pointer_button(
                frame,
                rect,
                label,
                PointerTarget::Key(key),
                theme,
                &mut targets,
            );
            x = x.saturating_add(UnicodeWidthStr::width(label) as u16 + 1);
        }
        if let Some(search) = app.copy.as_ref().and_then(|copy| copy.search.as_ref()) {
            if x < bar.right() {
                frame.render_widget(
                    Paragraph::new(format!("/{search}"))
                        .style(Style::default().fg(theme.color("fg"))),
                    Rect::new(x, bar.y, bar.right() - x, 1),
                );
            }
        }
    }
    if let Some(reader) = &app.reader {
        render_reader(frame, reader, app.content, theme);
        pointer_close(frame, app.content, theme, &mut targets);
    }
    if let Some(chooser) = &app.chooser {
        targets.clear();
        render_chooser(frame, chooser, app.content, theme, &mut targets);
    }
    if let Some(help) = &app.help {
        let rows = app.help_view();
        clear_area(frame, app.content, theme);
        targets.clear();
        render_help(frame, help, &rows, app.content, theme);
        pointer_close(frame, app.content, theme, &mut targets);
    }
    if let Some(settings) = &app.settings {
        clear_area(frame, app.content, theme);
        targets.clear();
        render_settings(frame, settings, app, app.content, theme, &mut targets);
    }
    if let Some(goto) = &app.goto {
        let rows = app.goto_rows();
        clear_area(frame, area, theme);
        targets.clear();
        render_goto(frame, goto, &rows, area, theme, app.palette, &mut targets);
    }
    if let Some(prompt) = &app.prompt {
        targets.clear();
        render_prompt(frame, prompt, app.content, theme, &mut targets);
    }
    app.pointer_targets = targets;
}

fn draw_mobile(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let mut constraints = vec![Constraint::Length(1), Constraint::Min(3)];
    if app.copy.is_some() {
        constraints.push(Constraint::Length(1));
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
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
        render_modals(frame, app, area, theme);
        return;
    }
    if app.chat.is_some() {
        chat::render(frame, app, content, theme);
        render_modals(frame, app, area, theme);
        return;
    }
    let slots = app.slots.clone();
    for slot in &slots {
        render_pane(frame, app, *slot, theme);
    }
    render_overlays(frame, app, theme);
    render_modals(frame, app, area, theme);
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
        (t(" close "), theme.color("badge_bg"))
    } else {
        app.mobile_switch = button;
        (t(" switch "), theme.color("accent"))
    };
    let status_width = button.x.saturating_sub(area.x).saturating_sub(1);
    let label = truncate(&mobile_status(app), status_width as usize);
    let pad = status_width.saturating_sub(UnicodeWidthStr::width(label.as_str()) as u16);
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
            line.push_str(&t2(
                "  tab {}/{}",
                workspace.active + 1,
                workspace.tabs.len(),
            ));
        }
        None => line.push_str(t(" no workspace")),
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

    rows.push(section(t("workspaces")));
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
        let workspace = &app.workspaces[index];
        rows.push(MobileRow {
            text: truncate(&format!("  {}", workspace_summary(workspace)), width),
            style: Style::default().fg(theme.color("dim")),
            target: Some(MobileTarget::Workspace(index)),
        });
    }

    rows.push(section(t("tabs")));
    if let Some(workspace) = app.workspace() {
        for (index, tab) in workspace.tabs.iter().enumerate() {
            if tab.settled {
                continue;
            }
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
            text: t("  (no tabs)").to_string(),
            style: Style::default().fg(theme.color("dim")),
            target: None,
        });
    }

    rows.push(section(t("panes")));
    match app.workspace().and_then(|workspace| workspace.tab()) {
        Some(tab) => {
            for pane in tab.panes() {
                let Some(state) = app.panes.get(&pane.0) else {
                    continue;
                };
                let focused = pane.0 == tab.focus;
                let suffix = if tab.floating_of(pane.0).is_some() {
                    format!(" · {}", t("floating"))
                } else {
                    String::new()
                };
                let room = width.saturating_sub(2 + UnicodeWidthStr::width(suffix.as_str()));
                rows.push(MobileRow {
                    text: format!(
                        "{} {}{suffix}",
                        if focused { "▸" } else { " " },
                        truncate(&state.label(), room)
                    ),
                    style: if focused {
                        Style::default()
                            .bg(theme.color("selection_bg"))
                            .fg(theme.color("selection_fg"))
                    } else {
                        Style::default().fg(theme.color("fg"))
                    },
                    target: Some(MobileTarget::Agent(pane.0)),
                });
            }
        }
        None => rows.push(MobileRow {
            text: t("  (no panes)").to_string(),
            style: Style::default().fg(theme.color("dim")),
            target: None,
        }),
    }

    rows.push(section(t("agents")));
    let symbols = app.config.indicators == crate::config::Indicators::Symbols;
    let mut previous = String::new();
    for NavigationRow {
        group,
        group_id,
        pane,
        terminal,
        badge,
        label,
    } in app.navigation_rows()
    {
        if previous != group_id {
            rows.push(section(&group));
            previous = group_id.clone();
        }
        let target = pane.map(MobileTarget::Agent).or_else(|| {
            terminal
                .and_then(|address| app.terminals.iter().position(|t| t.address == address))
                .map(MobileTarget::Terminal)
        });
        rows.push(MobileRow {
            text: format!(
                "  {} {}",
                badge.glyph(symbols),
                truncate(&label, width.saturating_sub(4))
            ),
            style: Style::default().fg(theme.color(if badge == Badge::None {
                "fg"
            } else {
                badge.token()
            })),
            target,
        });
    }

    rows.push(section(t("menu")));
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
    ("全部操作 / Actions", MobileAction::Actions),
    ("新建 tab", MobileAction::NewTab),
    ("左右分屏", MobileAction::SplitRight),
    ("上下分屏", MobileAction::SplitDown),
    ("关闭当前 pane", MobileAction::ClosePane),
    ("复制模式", MobileAction::CopyMode),
    ("下一个待处理", MobileAction::NextAgent),
    ("chat 视图", MobileAction::Chat),
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
    let full_area = area;
    let menu_width = 8.min(area.width);
    let area = Rect::new(
        area.x,
        area.y,
        area.width.saturating_sub(menu_width),
        area.height,
    );
    app.menu_hit = Rect::new(area.right(), area.y, menu_width, 1);
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
                .filter(|(_, tab)| !tab.settled)
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

    for (index, label, badge) in tabs {
        let selected = index == active_tab;
        let zoom = if selected && app.zoom { "Z" } else { " " };
        let flag = match badge {
            Badge::Blocked | Badge::Error | Badge::Done => badge.glyph(false),
            _ => " ",
        };
        let label = if app
            .workspace()
            .and_then(|w| w.tabs.get(index))
            .is_some_and(|t| !t.label.is_empty())
        {
            truncate(&label, 20)
        } else {
            (index + 1).to_string()
        };
        let text = format!(" {label} {zoom} {flag} ");
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
    frame.render_widget(
        Paragraph::new(t(" menu ")).style(
            Style::default()
                .bg(theme.color("accent"))
                .fg(theme.color("tab_active_fg")),
        ),
        app.menu_hit,
    );
    app.tab_bar = full_area;
}

/// 右对齐的状态条目：`zoom` / `hostname` / `datetime` / `text` / `cmd:`。
fn tab_bar_right_text(app: &App) -> String {
    let mut parts: Vec<String> = Vec::new();
    for token in &app.config.tab_bar_right {
        match token.as_str() {
            "zoom" => {
                if app.zoom {
                    parts.push(t("zoom").to_string());
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

fn workspace_summary(workspace: &crate::model::Workspace) -> String {
    let tabs = workspace.tabs.len();
    let panes = workspace.panes().len();
    // 英文分单复数；中文只有一种说法，四份原文都映射到同一条译文。
    let text = match (tabs == 1, panes == 1) {
        (true, true) => "{} tab · {} pane",
        (true, false) => "{} tab · {} panes",
        (false, true) => "{} tabs · {} pane",
        (false, false) => "{} tabs · {} panes",
    };
    t2(text, tabs, panes)
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
    let muted = Style::default().fg(theme.color("dim"));
    let mut lines = vec![nav_title(t(" Spaces"), "+ ", width, theme), Line::default()];
    let mut row = inner.y + 2;
    let mut space_lines = Vec::new();
    let mut space_hits = Vec::new();
    let mut machine_hits = Vec::new();
    let mut machine_groups = Vec::new();
    let mut machines: Vec<String> = (0..app.workspaces.len())
        .map(|index| app.workspace_machine(index))
        .chain(app.machines.iter().map(|machine| machine.name.clone()))
        .collect();
    machines.sort_by(|a, b| (a != "local", a).cmp(&(b != "local", b)));
    machines.dedup();
    for machine in machines {
        if !space_lines.is_empty() {
            space_lines.push(Line::default());
        }
        let state = app
            .machines
            .iter()
            .find(|m| m.name == machine)
            .map(|m| m.state.as_str());
        let connected = machine == "local" || state == Some("connected");
        let name = if machine == "local" {
            t("This computer")
        } else {
            &machine
        };
        let group_id = format!("machine:{machine}");
        let folded = app.folded_agent_groups.contains(&group_id);
        let chevron = if folded { "▸" } else { "▾" };
        machine_groups.push((group_id, space_lines.len()));
        let status = if connected { "●" } else { "○" };
        let name = truncate(name, width.saturating_sub(7));
        let pad = width.saturating_sub(UnicodeWidthStr::width(name.as_str()) + 6);
        space_lines.push(Line::from(vec![
            Span::styled(format!(" {chevron} {name}{}", " ".repeat(pad)), muted),
            Span::styled(
                status,
                Style::default().fg(theme.color(if connected { "agent_done" } else { "dim" })),
            ),
            Span::styled("  ", muted),
        ]));
        if folded {
            continue;
        }
        let indices: Vec<_> = (0..app.workspaces.len())
            .filter(|&i| app.workspace_machine(i) == machine)
            .collect();
        if indices.is_empty() {
            machine_hits.push((machine.clone(), space_lines.len()));
            space_lines.push(Line::from(Span::styled(
                if connected {
                    t(" + New space there")
                } else {
                    t(" ↻ Reconnect")
                },
                muted,
            )));
        }
        for index in indices {
            let label = app.workspace_label(index);
            let active = index == app.active_ws;
            let style = if active {
                Style::default()
                    .bg(theme.color("sidebar_selected_bg"))
                    .fg(theme.color("fg"))
            } else {
                Style::default().fg(theme.color("fg"))
            };
            let attention = app.attention_count(index);
            if machine == "local" {
                if let Some(cwd) = app.workspace_cwd(index) {
                    app.refresh_branch(&cwd);
                }
            }
            let branch = app
                .workspace_cwd(index)
                .and_then(|cwd| app.branch(&cwd))
                .unwrap_or_default();
            let name = truncate(
                &label,
                width.saturating_sub(5 + if attention > 0 { 3 } else { 0 }),
            );
            let name_width = UnicodeWidthStr::width(name.as_str());
            let detail_width = width.saturating_sub(name_width + 6);
            let detail = if attention > 0 {
                format!("{attention}!")
            } else {
                truncate(&branch, detail_width)
            };
            let detail_width = UnicodeWidthStr::width(detail.as_str());
            let pad = width.saturating_sub(name_width + detail_width + 5);
            let initial = label.chars().next().unwrap_or(' ').to_ascii_uppercase();
            let icon = truncate(&initial.to_string(), 1);
            let colors = [
                "space_teal",
                "space_green",
                "space_blue",
                "space_pink",
                "space_orange",
                "space_purple",
            ];
            let identity = app.workspaces[index].origin.as_deref().unwrap_or(&label);
            let color = identity.bytes().fold(0usize, |hash, byte| {
                hash.wrapping_mul(31).wrapping_add(byte as usize)
            }) % colors.len();
            space_hits.push((index, space_lines.len()));
            space_lines.push(
                Line::from(vec![
                    Span::styled(" ", style),
                    Span::styled(
                        format!(" {icon} "),
                        Style::default()
                            .bg(theme.color(colors[color]))
                            .fg(theme.color("space_icon_fg"))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" {name}"), style.add_modifier(Modifier::BOLD)),
                    Span::styled(
                        format!("{}{detail}", " ".repeat(pad)),
                        style.fg(theme.color(if attention > 0 {
                            "agent_working"
                        } else {
                            "dim"
                        })),
                    ),
                ])
                .style(style),
            );
        }
    }
    // Spaces and Agents scroll independently, so a long machine list cannot hide Agents.
    let space_height = space_lines
        .len()
        .min(inner.height.saturating_sub(10).max(3) as usize);
    app.space_scroll = app
        .space_scroll
        .min(space_lines.len().saturating_sub(space_height));
    app.spaces_area = Rect::new(inner.x, row, inner.width, space_height as u16);
    app.agent_group_hits.clear();
    for (group, offset) in machine_groups {
        if offset >= app.space_scroll && offset < app.space_scroll + space_height {
            app.agent_group_hits.push((
                group,
                Rect::new(
                    inner.x,
                    row + (offset - app.space_scroll) as u16,
                    inner.width,
                    1,
                ),
            ));
        }
    }
    for (index, offset) in space_hits {
        if offset >= app.space_scroll && offset < app.space_scroll + space_height {
            app.workspace_hits.push((
                index,
                Rect::new(
                    inner.x,
                    row + (offset - app.space_scroll) as u16,
                    inner.width,
                    1,
                ),
            ));
        }
    }
    for (machine, offset) in machine_hits {
        if offset >= app.space_scroll && offset < app.space_scroll + space_height {
            app.machine_hits.push((
                machine,
                Rect::new(
                    inner.x,
                    row + (offset - app.space_scroll) as u16,
                    inner.width,
                    1,
                ),
            ));
        }
    }
    lines.extend(
        space_lines
            .into_iter()
            .skip(app.space_scroll)
            .take(space_height),
    );
    row += space_height as u16;
    lines.push(Line::default());
    row += 1;

    // `new` 与 `menu`：与 herdr 的侧栏底行同位置同含义。
    if row < inner.y + inner.height {
        let new = t(" new");
        let menu = format!("●{}", t(" menu "));
        let menu_width = UnicodeWidthStr::width(menu.as_str()) as u16;
        app.sidebar_new = Rect::new(
            inner.x,
            row,
            (UnicodeWidthStr::width(new) as u16).min(inner.width),
            1,
        );
        if inner.width > menu_width {
            app.sidebar_menu = Rect::new(inner.x + inner.width - menu_width, row, menu_width, 1);
        }
        let pad = width
            .saturating_sub(UnicodeWidthStr::width(new) + UnicodeWidthStr::width(menu.as_str()));
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
                t(" menu ").to_string(),
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
            crate::config::AgentSort::Spaces => t("grouped"),
            crate::config::AgentSort::Priority => t("priority"),
        };
        app.agent_sort_hit = Rect::new(inner.x, row, inner.width, 1);
        lines.push(nav_title(t(" Agents"), sort, width, theme));
        row = row.saturating_add(1);
    }

    let mut entries = Vec::new();
    let mut previous = String::new();
    for NavigationRow {
        group,
        group_id,
        pane,
        terminal,
        badge,
        label,
    } in app.navigation_rows()
    {
        if group_id != previous {
            entries.push((
                None,
                None,
                Badge::None,
                group.clone(),
                true,
                group_id.clone(),
            ));
            previous = group_id.clone();
        }
        if !app.folded_agent_groups.contains(&group_id) {
            entries.push((pane, terminal, badge, label, false, group_id));
        }
    }
    let total = entries.len();
    let available = (inner.y + inner.height).saturating_sub(row) as usize;
    app.sidebar_scroll = app.sidebar_scroll.min(total.saturating_sub(available));
    for (pane, terminal, badge, label, header, group_id) in
        entries.iter().skip(app.sidebar_scroll).take(available)
    {
        let focused = pane.is_some_and(|pane| pane == app.focused());
        let style = if *header {
            Style::default().fg(theme.color("dim"))
        } else if focused {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
        } else if *badge == Badge::None {
            Style::default().fg(theme.color("fg"))
        } else {
            Style::default().fg(theme.color(badge.token()))
        };
        let text = if *header {
            format!(
                " {} {}",
                if app.folded_agent_groups.contains(group_id) {
                    "▸"
                } else {
                    "▾"
                },
                truncate(label, width.saturating_sub(4))
            )
        } else {
            format!(
                " {} {}",
                badge.glyph(app.config.indicators == crate::config::Indicators::Symbols),
                truncate(label, width.saturating_sub(4))
            )
        };
        let hit = Rect::new(inner.x, row, inner.width, 1);
        if !*header && label.contains("▾ ") || !*header && label.contains("▸ ") {
            if let Some(address) = terminal {
                app.agent_group_hits.push((
                    format!("agent:{address}"),
                    Rect::new(inner.x, row, 4.min(inner.width), 1),
                ));
            }
        }
        if *header {
            app.agent_group_hits.push((group_id.clone(), hit));
        }
        if let Some(pane) = pane {
            app.agent_hits.push((*pane, hit));
        } else if let Some(terminal) = terminal {
            app.terminal_hits.push((terminal.clone(), hit));
        }
        let pad = width.saturating_sub(UnicodeWidthStr::width(text.as_str()));
        lines.push(Line::from(Span::styled(
            format!("{text}{}", " ".repeat(pad)),
            style,
        )));
        row = row.saturating_add(1);
    }
    if total == 0 && row < inner.y + inner.height {
        lines.push(Line::from(Span::styled(
            t("  (no agents)"),
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
    let pad = width.saturating_sub(UnicodeWidthStr::width(left) + UnicodeWidthStr::width(right));
    Line::from(vec![
        Span::styled(
            left.to_string(),
            Style::default()
                .fg(theme.color("dim"))
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
    let floating = app
        .workspace()
        .and_then(crate::model::Workspace::tab)
        .and_then(|tab| tab.floating_of(slot.pane));
    let popup = app.popup.is_some_and(|popup| popup.pane == slot.pane);
    let badge = state.map(|pane| pane.agent.badge()).unwrap_or(Badge::None);
    let symbols = app.config.indicators == crate::config::Indicators::Symbols;
    // herdr 不在边框上写终端地址：只有手动命名或开启 Agent 标签时才出现标题。
    let title = if slot.collapsed {
        // 折叠行就是标题行本身，标签必须一直显示。
        let label = state.map(|pane| pane.label()).unwrap_or_default();
        match badge {
            Badge::None => format!(" {} ", truncate(&label, 40)),
            _ => format!(" {} {} ", badge.glyph(symbols), truncate(&label, 38)),
        }
    } else if floating.is_some() {
        // 浮动 Pane 没有相邻边框，标题里必须带身份。
        let label = state.map(|pane| pane.label()).unwrap_or_default();
        let pin = if floating.is_some_and(|entry| entry.pinned) {
            t("pin ")
        } else {
            ""
        };
        match badge {
            Badge::None => format!(" {pin}{} ", truncate(&label, 36)),
            _ => format!(" {} {pin}{} ", badge.glyph(symbols), truncate(&label, 34)),
        }
    } else {
        match state.map(|pane| &pane.state) {
            Some(PaneState::Live { .. }) => {
                let named = state.and_then(|pane| {
                    pane.name
                        .as_deref()
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                });
                match named {
                    Some(name) => format!(" {} ", truncate(&name, 24)),
                    None if app.config.show_agent_labels_on_pane_borders
                        && badge != Badge::None =>
                    {
                        let label = state.map(|pane| pane.label()).unwrap_or_default();
                        format!(" {} {} ", badge.glyph(symbols), truncate(&label, 22))
                    }
                    None => String::new(),
                }
            }
            Some(PaneState::Ended { reason }) => {
                format!(" {} ", t1("closed: {}", short(reason)))
            }
            Some(PaneState::Waiting) | None => t(" starting ").to_string(),
        }
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
    let title = if popup {
        format!("{} ·{title}", t("popup"))
    } else {
        title
    };
    let block = Block::default()
        .borders(slot.borders)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme.color("bg")))
        .title(Span::styled(title, Style::default().fg(border_color)));
    // Floating panes and popups may cover terminal cells with active SGR attributes.
    if slot.overlay {
        frame.render_widget(Clear, slot.outer);
    }
    frame.render_widget(block, slot.outer);
    if app.config.mouse_capture
        && !app.mobile
        && !popup
        && slot.borders.contains(Borders::TOP)
        && slot.outer.width >= 22
    {
        let menu = Rect::new(slot.outer.right() - 7, slot.outer.y, 5, 1);
        frame.render_widget(
            Paragraph::new("[...]").style(Style::default().fg(theme.color("accent"))),
            menu,
        );
        app.pane_menu_hits.push((slot.pane, menu));
        if !slot.overlay {
            let grip = Rect::new(slot.outer.right() - 12, slot.outer.y, 4, 1);
            frame.render_widget(
                Paragraph::new(" :: ").style(Style::default().fg(theme.color("dim"))),
                grip,
            );
            app.pane_drag_hits.push((slot.pane, grip));
        }
    }

    if slot.collapsed {
        return;
    }

    if slot.inner.width == 0 || slot.inner.height == 0 {
        return;
    }

    let Some(pane) = app.panes.get_mut(&slot.pane) else {
        return;
    };
    if pane.module.is_some() {
        let _ = pane.view.render(frame.buffer_mut(), slot.inner);
        return;
    }
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
        let text = t("VT render failed\n\nuse `hc read` to look at it");
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
    if !crate::term::graphics::native_enabled() {
        for image in &images {
            paint_placement(buffer, slot.inner, image);
        }
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
    clear_area(frame, area, theme);
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

pub(crate) fn clear_area(frame: &mut Frame, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(
            Style::default()
                .bg(theme.color("sidebar_bg"))
                .fg(theme.color("fg")),
        ),
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

fn render_prompt(
    frame: &mut Frame,
    prompt: &Prompt,
    content: Rect,
    theme: &Theme,
    targets: &mut Vec<(PointerTarget, Rect)>,
) {
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
    clear_area(frame, area, theme);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let confirm = prompt.kind.needs_confirm();
    let footer = if confirm {
        t("↵ confirm    esc cancel")
    } else {
        t("↵ save    ^c clear    esc cancel")
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
        .saturating_sub(UnicodeWidthStr::width(footer))
        .saturating_sub(3);
    let lines = vec![
        Line::from(Span::styled(
            format!(" {}", prompt.kind.title()),
            Style::default()
                .fg(theme.color("accent"))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(truncate(&prompt.label, inner.width as usize)),
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
    let buttons = Rect::new(
        inner.x,
        inner.y.saturating_add(3).min(inner.bottom() - 1),
        inner.width,
        1,
    );
    clear_area(frame, buttons, theme);
    pointer_button(
        frame,
        buttons,
        if confirm { t(" Confirm ") } else { t(" Save ") },
        PointerTarget::Confirm,
        theme,
        targets,
    );
    let offset = (buttons.width / 2).max(10).min(buttons.width);
    pointer_button(
        frame,
        Rect::new(buttons.x + offset, buttons.y, buttons.width - offset, 1),
        t(" Cancel "),
        PointerTarget::Close,
        theme,
        targets,
    );
    pointer_close(frame, area, theme, targets);
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

    let header = nav_title(t(" keybinds"), t("esc close"), width, theme);
    let query_line = if help.query.is_empty() {
        Line::from(vec![
            Span::styled("  /", Style::default().fg(theme.color("accent"))),
            Span::styled(
                t(" press / to filter by command or shortcut"),
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
    let footer = t("search / · scroll j/k/↑↓/pgup/pgdn · close esc/enter");
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
        let pad = width.saturating_sub(UnicodeWidthStr::width(text.as_str()) + 1);
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
    targets: &mut Vec<(PointerTarget, Rect)>,
) {
    let len = app.settings_len(settings.section);
    let panel_width = content.width.saturating_sub(4).min(60);
    let panel_height = content.height.saturating_sub(2).min(len as u16 + 9);
    if panel_width < 24 || panel_height < 8 {
        return;
    }
    let area = Rect::new(
        content.x + (content.width - panel_width) / 2,
        content.y + (content.height - panel_height) / 2,
        panel_width,
        panel_height,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.color("border")))
        .style(
            Style::default()
                .bg(theme.color("sidebar_bg"))
                .fg(theme.color("fg")),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let tab_rows = if width < 48 { 2 } else { 1 };
    let mut tab_lines = Vec::new();
    let mut tabs = vec![Span::raw(" ")];
    let mut tab_x = inner.x + 1;
    for (index, name) in App::SETTINGS_SECTIONS.iter().enumerate() {
        if tab_rows == 2 && index == 2 {
            tab_lines.push(Line::from(std::mem::take(&mut tabs)));
            tabs.push(Span::raw(" "));
            tab_x = inner.x + 1;
        }
        let style = if index == settings.section {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.color("dim"))
        };
        let width =
            (UnicodeWidthStr::width(t(name)) as u16 + 2).min(inner.right().saturating_sub(tab_x));
        targets.push((
            PointerTarget::SettingsSection(index),
            Rect::new(
                tab_x,
                inner.y + 2 + if tab_rows == 2 { index as u16 / 2 } else { 0 },
                width,
                1,
            ),
        ));
        tab_x = tab_x.saturating_add(width + 1);
        tabs.push(Span::styled(format!(" {} ", t(name)), style));
        tabs.push(Span::raw(" "));
    }
    tab_lines.push(Line::from(tabs));
    let mut lines = vec![
        Line::from(Span::styled(
            t(" Settings"),
            Style::default()
                .fg(theme.color("fg"))
                .add_modifier(Modifier::BOLD),
        )),
        Line::default(),
    ];
    lines.extend(tab_lines);
    lines.push(Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.color("border")),
    )));
    let available = inner.height.saturating_sub(5 + tab_rows) as usize;
    let offset = settings
        .selected
        .saturating_sub(available.saturating_sub(1));
    for index in offset..len.min(offset + available) {
        targets.push((
            PointerTarget::Setting(index),
            Rect::new(
                inner.x,
                inner.y + 3 + tab_rows + (index - offset) as u16,
                inner.width,
                1,
            ),
        ));
        let selected = index == settings.selected;
        let (label, active) = settings_item(app, settings.section, index);
        let style = if selected {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
        } else {
            Style::default().fg(theme.color("fg"))
        };
        let marker = if selected { "▸" } else { " " };
        let check = if active { "✓" } else { " " };
        let label = truncate(&label, width.saturating_sub(7));
        let pad = width.saturating_sub(UnicodeWidthStr::width(label.as_str()) + 6);
        lines.push(
            Line::from(Span::styled(
                format!(" {marker} {label}{} {check} ", " ".repeat(pad)),
                style,
            ))
            .style(style),
        );
    }
    frame.render_widget(Paragraph::new(lines), inner);
    let footer = if width >= 48 {
        t(" ↑↓ select   ←→ section   ↵ apply   esc close")
    } else {
        t(" ↑↓ select  tab section  ↵ apply  esc close")
    };
    frame.render_widget(
        Paragraph::new(truncate(footer, width)).style(Style::default().fg(theme.color("dim"))),
        Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
    );
    pointer_close(frame, area, theme, targets);
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
                if symbols { t("symbols") } else { t("dots") }.to_string(),
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
            (t(value).to_string(), active)
        }
        _ => {
            let on = index == 1;
            (
                if on { t("on") } else { t("off") }.to_string(),
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
    palette: bool,
    targets: &mut Vec<(PointerTarget, Rect)>,
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
    let title_right = format!(
        "{terminals} {}",
        if palette {
            t("commands")
        } else {
            t("terminals")
        }
    );
    let mut lines = vec![
        nav_title(
            if palette {
                t("─ Commands ")
            } else {
                t("─ Go to ")
            },
            &format!("{title_right} "),
            width,
            theme,
        ),
        Line::from(vec![
            Span::styled(" / ", Style::default().fg(theme.color("accent"))),
            Span::styled(
                if goto.query.is_empty() {
                    if palette {
                        t("search commands")
                    } else {
                        t("search agents and terminals")
                    }
                    .to_string()
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
        if !row.section {
            targets.push((
                PointerTarget::Goto(index),
                Rect::new(
                    inner.x,
                    inner.y + 3 + (index - scroll) as u16,
                    inner.width,
                    1,
                ),
            ));
        }
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
            t(" No matching agents or terminals"),
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
        t(" / search · ↑↓ rows · enter open · esc close"),
        Style::default().fg(theme.color("dim")),
    )));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.color("sidebar_bg"))),
        inner,
    );
    pointer_close(frame, area, theme, targets);
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
        t2("read · {} · {} 行", tv(&reader.how), reader.lines.len())
    } else {
        t1("read · {} · 部分内容", tv(&reader.how))
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
    clear_area(frame, area, theme);
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

fn render_chooser(
    frame: &mut Frame,
    chooser: &Chooser,
    content: Rect,
    theme: &Theme,
    targets: &mut Vec<(PointerTarget, Rect)>,
) {
    if content.width < 20 || content.height < 5 {
        return;
    }
    let width = content.width.min(64);
    let row_height = if content.width <= 64 && content.height >= 12 {
        2
    } else {
        1
    };
    let machines = chooser.kind == crate::app::ChooserKind::Machines;
    let wanted =
        (chooser.items.len() as u16).saturating_mul(row_height) + 3 + 2 * u16::from(machines);
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
    clear_area(frame, area, theme);
    frame.render_widget(block, area);
    let visible =
        (inner.height.saturating_sub(2 * u16::from(machines)) / row_height).max(1) as usize;
    let start = if chooser.selected >= visible {
        chooser.selected + 1 - visible
    } else {
        0
    };
    for (row, index) in (start..chooser.items.len()).take(visible).enumerate() {
        targets.push((
            PointerTarget::Chooser(index),
            Rect::new(
                inner.x,
                inner.y + row as u16 * row_height,
                inner.width,
                row_height,
            ),
        ));
    }
    let lines: Vec<Line> = chooser
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .flat_map(|(index, item)| {
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
            let line = Line::from(Span::styled(text, style));
            if row_height == 2 {
                vec![line, Line::default().style(style)]
            } else {
                vec![line]
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    if machines && inner.height >= 2 {
        for (row, buttons) in [
            vec![
                (t(" Open "), KeyCode::Enter),
                (t(" Add "), KeyCode::Char('a')),
            ],
            vec![
                (t(" Connect "), KeyCode::Char('c')),
                (t(" Rename "), KeyCode::Char('r')),
                (t(" Remove "), KeyCode::Char('d')),
            ],
        ]
        .into_iter()
        .enumerate()
        {
            let mut x = inner.x;
            for (label, key) in buttons {
                pointer_button(
                    frame,
                    Rect::new(
                        x,
                        inner.bottom() - 2 + row as u16,
                        inner.right().saturating_sub(x),
                        1,
                    ),
                    label,
                    PointerTarget::Key(key),
                    theme,
                    targets,
                );
                x = x.saturating_add(UnicodeWidthStr::width(label) as u16 + 1);
            }
        }
    }
    pointer_close(frame, area, theme, targets);
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

/// 通用宿主使用上下半块，每格保留两个像素采样，并遵守源裁剪与透明度。
fn paint_placement(buffer: &mut Buffer, area: Rect, image: &PlacementImage) {
    if image.cols == 0 || image.rows == 0 || image.width == 0 || image.height == 0 {
        return;
    }
    let [sx, sy, sw, sh] = image.source;
    let sw = if sw == 0 {
        image.width.saturating_sub(sx)
    } else {
        sw.min(image.width.saturating_sub(sx))
    };
    let sh = if sh == 0 {
        image.height.saturating_sub(sy)
    } else {
        sh.min(image.height.saturating_sub(sy))
    };
    if sw == 0 || sh == 0 {
        return;
    }
    let left = i64::from(image.col).max(0);
    let top = i64::from(image.row).max(0);
    let right = (i64::from(image.col) + i64::from(image.cols)).min(i64::from(area.width));
    let bottom = (i64::from(image.row) + i64::from(image.rows)).min(i64::from(area.height));
    for y in top..bottom {
        for x in left..right {
            let cell = &mut buffer[(area.x + x as u16, area.y + y as u16)];
            let background = cell.bg;
            let blend = |pixel: &[u8]| {
                if pixel[3] == 0 {
                    return background;
                }
                let base = match background {
                    Color::Rgb(r, g, b) => [r, g, b],
                    _ => [0, 0, 0],
                };
                let alpha = u16::from(pixel[3]);
                let mut color = [0; 3];
                for i in 0..3 {
                    color[i] = ((u16::from(pixel[i]) * alpha + u16::from(base[i]) * (255 - alpha))
                        / 255) as u8;
                }
                Color::Rgb(color[0], color[1], color[2])
            };
            let px = sx
                + (((x - i64::from(image.col)) as u64 * 2 + 1) * u64::from(sw)
                    / (u64::from(image.cols) * 2))
                    .min(u64::from(sw - 1)) as u32;
            let sample = |half: u64| {
                let py = sy
                    + (((y - i64::from(image.row)) as u64 * 4 + half) * u64::from(sh)
                        / (u64::from(image.rows) * 4))
                        .min(u64::from(sh - 1)) as u32;
                let offset = (u64::from(py) * u64::from(image.width) + u64::from(px)) as usize * 4;
                image.rgba.get(offset..offset + 4)
            };
            if let (Some(upper), Some(lower)) = (sample(1), sample(3)) {
                if upper[3] == 0 && lower[3] == 0 {
                    continue;
                }
                cell.set_symbol("▀")
                    .set_fg(blend(upper))
                    .set_bg(blend(lower));
            }
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

fn truncate(text: &str, max_columns: usize) -> String {
    if text.width() <= max_columns {
        return text.to_string();
    }
    if max_columns == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut columns = 0;
    for ch in text.chars() {
        let width = ch.width().unwrap_or(0);
        if columns + width > max_columns - 1 {
            break;
        }
        out.push(ch);
        columns += width;
    }
    out.push('…');
    out
}

fn short(text: &str) -> String {
    truncate(text, 28)
}

fn pointer_button(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    target: PointerTarget,
    theme: &Theme,
    targets: &mut Vec<(PointerTarget, Rect)>,
) {
    let width = (UnicodeWidthStr::width(label) as u16).min(area.width);
    if width == 0 || area.height == 0 {
        return;
    }
    let rect = Rect::new(area.x, area.y, width, 1);
    frame.render_widget(
        Paragraph::new(label).style(
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg")),
        ),
        rect,
    );
    targets.push((target, rect));
}

fn pointer_close(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    targets: &mut Vec<(PointerTarget, Rect)>,
) {
    if area.width < 5 || area.height == 0 {
        return;
    }
    pointer_button(
        frame,
        Rect::new(area.right() - 5, area.y, 5, 1),
        " [x] ",
        PointerTarget::Close,
        theme,
        targets,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_clear_removes_underlying_terminal_attributes() {
        use ratatui_core::{backend::TestBackend, terminal::Terminal};
        let theme = crate::theme::builtin("catppuccin").unwrap();
        let mut terminal = Terminal::new(TestBackend::new(30, 10)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new("terminal text").style(
                        Style::default()
                            .fg(Color::Red)
                            .add_modifier(Modifier::DIM | Modifier::ITALIC | Modifier::REVERSED),
                    ),
                    frame.area(),
                );
                clear_area(frame, frame.area(), &theme);
                frame.render_widget(
                    Paragraph::new("settings text").style(Style::default().fg(theme.color("fg"))),
                    frame.area(),
                );
            })
            .unwrap();
        for x in 0..13 {
            let cell = &terminal.backend().buffer()[(x, 0)];
            assert_eq!(cell.modifier, Modifier::empty());
            assert_eq!(cell.fg, theme.color("fg"));
            assert_eq!(cell.bg, theme.color("sidebar_bg"));
        }
    }

    #[test]
    fn truncate_counts_terminal_columns() {
        assert_eq!(truncate("宽字符", 2), "…");
        assert_eq!(truncate("宽字符", 3), "宽…");
        assert_eq!(truncate("宽字符", 6), "宽字符");
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(truncate("abc", 5), "abc");
    }
}
