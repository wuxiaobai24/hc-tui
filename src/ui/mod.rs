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

use crate::app::{App, Chooser, Connection, Reader, Toast};
use crate::config::{CollapsedSidebar, HostCursor, TabBarPosition};
use crate::model::{Badge, PaneState, Slot};
use crate::term::view::PlacementImage;
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let theme = app.config.theme.clone();
    let bottom_bar = app.config.tab_bar_position == TabBarPosition::Bottom;
    let rows = if bottom_bar {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(area)
    };
    let (tab_bar, body, status) = if bottom_bar {
        (rows[1], rows[0], rows[2])
    } else {
        (rows[0], rows[1], rows[2])
    };

    frame.render_widget(
        Block::default().style(Style::default().bg(theme.color("bg"))),
        area,
    );

    render_tab_bar(frame, app, tab_bar, &theme);

    let rails = if !app.sidebar
        && app.config.collapsed_sidebar == CollapsedSidebar::Compact
        && body.width > 40
    {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(4), Constraint::Min(20)])
            .split(body);
        render_rail(frame, app, cols[0], &theme);
        cols[1]
    } else {
        body
    };

    let content = if app.sidebar && rails.width > app.sidebar_width + 40 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(app.sidebar_width), Constraint::Min(40)])
            .split(rails);
        render_sidebar(frame, app, cols[0], &theme);
        cols[1]
    } else {
        app.workspace_hits.clear();
        app.agent_hits.clear();
        rails
    };

    app.reflow(content);
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
    render_status(frame, app, status, &theme);
}

fn render_tab_bar(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    app.tab_bar = area;
    app.tab_hits.clear();
    let mut spans = Vec::new();
    let mut x = area.x;

    // 工作区徽标：`w1 name`，点它切回第一个工作区没有意义，所以不可点。
    let workspace_label = app
        .workspace()
        .map(|workspace| {
            if workspace.label.is_empty() {
                format!("w{}", workspace.id)
            } else {
                format!("w{} {}", workspace.id, workspace.label)
            }
        })
        .unwrap_or_default();
    let chip = format!(" {} ", truncate(&workspace_label, 20));
    let chip_width = chip.chars().count() as u16;
    spans.push(Span::styled(
        chip,
        Style::default()
            .bg(theme.color("accent"))
            .fg(theme.color("tab_active_fg"))
            .add_modifier(Modifier::BOLD),
    ));
    x = x.saturating_add(chip_width);

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

    for (index, label, badge) in tabs {
        let active = index == active_tab;
        let marker = if active && app.zoom { "." } else { " " };
        let flag = match badge {
            Badge::Blocked | Badge::Error => badge.glyph(false),
            Badge::Done => badge.glyph(false),
            _ => " ",
        };
        let text = format!(" {}{}{} {} ", index + 1, marker, flag, truncate(&label, 16));
        let width = text.chars().count() as u16;
        let style = if active {
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
        app.tab_hits.push((
            index,
            Rect::new(x, area.y, width.min(area.width.saturating_sub(x)), 1),
        ));
        x = x.saturating_add(width);
        spans.push(Span::styled(text, style));
    }

    // 右对齐的状态条目。
    let right: Vec<&str> = app
        .config
        .tab_bar_right
        .iter()
        .map(String::as_str)
        .filter(|token| !token.is_empty())
        .collect();
    let mut right_text = String::new();
    for token in right {
        match token {
            "zoom" => {
                if app.zoom {
                    right_text.push_str("zoom ");
                }
            }
            "hostname" => {
                right_text.push_str(&hostname());
                right_text.push(' ');
            }
            "text" => {}
            token if token.starts_with("cmd:") => {
                if let Some(output) = app.command_output.get(token) {
                    if !output.is_empty() {
                        right_text.push_str(output);
                        right_text.push(' ');
                    }
                }
            }
            _ => {}
        }
    }
    let used = x.saturating_sub(area.x);
    if !right_text.is_empty() && area.width > used + 4 {
        let pad = area
            .width
            .saturating_sub(used + right_text.chars().count() as u16);
        spans.push(Span::raw(" ".repeat(pad as usize)));
        spans.push(Span::styled(
            right_text,
            Style::default()
                .bg(theme.color("tab_bar_bg"))
                .fg(theme.color("dim")),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.color("tab_bar_bg"))),
        area,
    );
}

fn render_sidebar(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    app.sidebar_area = area;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.color("border")))
        .style(Style::default().bg(theme.color("sidebar_bg")));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.workspace_hits.clear();
    app.agent_hits.clear();
    if inner.width < 4 || inner.height < 2 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut row = inner.y;
    lines.push(Line::from(Span::styled(
        " workspaces",
        Style::default()
            .fg(theme.color("sidebar_title"))
            .add_modifier(Modifier::BOLD),
    )));
    row = row.saturating_add(1);

    for (index, workspace) in app.workspaces.iter().enumerate() {
        if row >= inner.y + inner.height {
            break;
        }
        let attention = app.attention_count(index);
        let active = index == app.active_ws;
        let label = if workspace.label.is_empty() {
            format!("w{}", workspace.id)
        } else {
            format!("w{} {}", workspace.id, workspace.label)
        };
        let badge = if attention > 0 {
            format!(" {attention}")
        } else {
            String::new()
        };
        let text = format!(
            " {} {} {}{}",
            if active { "▸" } else { " " },
            truncate(&label, (inner.width as usize).saturating_sub(6)),
            workspace.tabs.len(),
            badge
        );
        let style = if active {
            Style::default()
                .bg(theme.color("selection_bg"))
                .fg(theme.color("selection_fg"))
        } else {
            Style::default().fg(theme.color("fg"))
        };
        app.workspace_hits
            .push((index, Rect::new(inner.x, row, inner.width, 1)));
        lines.push(Line::from(Span::styled(text, style)));
        row = row.saturating_add(1);
    }

    if row < inner.y + inner.height {
        row += 1;
        lines.push(Line::from(""));
    }
    if !app.machines.is_empty() {
        if row < inner.y + inner.height {
            row += 1;
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            " machines",
            Style::default()
                .fg(theme.color("sidebar_title"))
                .add_modifier(Modifier::BOLD),
        )));
        row = row.saturating_add(1);
        for machine in &app.machines {
            if row >= inner.y + inner.height {
                break;
            }
            let color = match machine.state.as_str() {
                "connected" => theme.color("agent_done"),
                "connecting" => theme.color("agent_working"),
                "needs_login" => theme.color("badge_bg"),
                _ => theme.color("agent_error"),
            };
            lines.push(Line::from(vec![
                Span::styled("· ", Style::default().fg(color)),
                Span::styled(
                    truncate(&machine.name, (inner.width as usize).saturating_sub(4)),
                    Style::default().fg(theme.color("fg")),
                ),
                Span::styled(
                    format!(" {}", truncate(&machine.state, 8)),
                    Style::default().fg(color),
                ),
            ]));
            row = row.saturating_add(1);
        }
    }
    lines.push(Line::from(Span::styled(
        " agents",
        Style::default()
            .fg(theme.color("sidebar_title"))
            .add_modifier(Modifier::BOLD),
    )));
    row = row.saturating_add(1);

    // agents 段之后是「没有状态上报的 Pane」，两段共用同一个滚动偏移。
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

    let available = (inner.y + inner.height).saturating_sub(row) as usize;
    let max_scroll = rows.len().saturating_sub(available);
    if app.sidebar_scroll > max_scroll {
        app.sidebar_scroll = max_scroll;
    }
    let start = app.sidebar_scroll.min(rows.len());
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no panes)",
            Style::default().fg(theme.color("dim")),
        )));
        row = row.saturating_add(1);
    }
    for (pane, badge, label) in rows.iter().skip(start) {
        if row >= inner.y + inner.height {
            break;
        }
        let position = app
            .locate_public(*pane)
            .map(|(workspace, tab)| {
                format!(
                    "w{}:t{}",
                    app.workspaces[workspace].id, app.workspaces[workspace].tabs[tab].id
                )
            })
            .unwrap_or_default();
        let focused = *pane == app.focused();
        let name_width = (inner.width as usize)
            .saturating_sub(3 + position.chars().count())
            .max(4);
        let text = format!(
            "{} {} {}",
            badge.glyph(app.config.indicators == crate::config::Indicators::Symbols),
            truncate(label, name_width),
            position
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

    if row < inner.y + inner.height {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            if app.sidebar_scroll > 0 {
                format!(" ↑ {}/{} · ctrl+b ? help", start + 1, rows.len())
            } else {
                " ctrl+b ? help".to_string()
            },
            Style::default().fg(theme.color("dim")),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
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
    let title = match state.map(|pane| &pane.state) {
        Some(PaneState::Live { channel }) => {
            let name = state
                .map(|pane| {
                    let label = pane.label();
                    match pane.program() {
                        Some(program) => format!("{label} · {program}"),
                        None => label,
                    }
                })
                .unwrap_or_default();
            if badge == Badge::None {
                format!(" {} · ch{channel} ", truncate(&name, 24))
            } else {
                format!(
                    " {} {} ",
                    badge.glyph(app.config.indicators == crate::config::Indicators::Symbols),
                    truncate(&name, 22)
                )
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
        paint_scrollbar(buffer, slot.inner, total, offset, len, theme);
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

fn render_status(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let mode = if app.copy.is_some() {
        Some(("COPY", theme.color("accent")))
    } else if app.prompt.is_some() {
        Some(("PROMPT", theme.color("badge_bg")))
    } else if app.move_mode {
        Some(("MOVE", theme.color("badge_bg")))
    } else if app.workspace_mode {
        Some(("SPACES", theme.color("badge_bg")))
    } else if app.resize_mode {
        Some(("RESIZE", Color::Magenta))
    } else if app.prefix_active() {
        Some(("PREFIX", theme.color("badge_bg")))
    } else {
        None
    };
    let mut spans = vec![
        Span::styled(
            " hc-tui ",
            Style::default()
                .bg(theme.color("accent"))
                .fg(theme.color("tab_active_fg")),
        ),
        Span::styled(
            match mode {
                Some((label, _)) => format!(" {label} "),
                None => " ctrl+b ".to_string(),
            },
            Style::default()
                .bg(mode
                    .map(|(_, color)| color)
                    .unwrap_or(theme.color("border")))
                .fg(theme.color("tab_active_fg")),
        ),
    ];
    let (connection, color) = match &app.connection {
        Connection::Connecting => (app.connection.label(), theme.color("agent_working")),
        Connection::Ready { .. } => (app.connection.label(), theme.color("agent_done")),
        Connection::Reconnecting { .. } => (app.connection.label(), theme.color("agent_working")),
        Connection::Failed { .. } => (app.connection.label(), theme.color("agent_error")),
    };
    spans.push(Span::styled(
        format!("  {connection} "),
        Style::default().fg(color),
    ));
    spans.push(Span::styled(
        "│ ",
        Style::default().fg(theme.color("border")),
    ));

    if let Some(prompt) = &app.prompt {
        spans.push(Span::styled(
            format!("{}  ", prompt.label),
            Style::default().fg(theme.color("badge_bg")),
        ));
        let (before, after) = prompt
            .buffer
            .split_at(prompt.cursor.min(prompt.buffer.len()));
        spans.push(Span::styled(
            before.to_string(),
            Style::default()
                .fg(theme.color("fg"))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            "▏",
            Style::default()
                .fg(theme.color("accent"))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            after.to_string(),
            Style::default().fg(theme.color("fg")),
        ));
    } else {
        spans.push(Span::styled(
            app.note.clone(),
            Style::default().fg(theme.color("dim")),
        ));
    }

    if area.width > 60 {
        let pane = match app.focused_pane().map(|pane| (&pane.state, &pane.terminal)) {
            Some((PaneState::Live { channel }, terminal)) => {
                let size = app
                    .slots
                    .iter()
                    .find(|slot| slot.pane == app.focused())
                    .map(|slot| (slot.inner.width, slot.inner.height))
                    .unwrap_or((0, 0));
                format!(
                    "{} {}x{} ch{channel}",
                    terminal.as_deref().unwrap_or("?"),
                    size.0,
                    size.1
                )
            }
            Some((PaneState::Ended { reason }, _)) => format!("closed: {}", short(reason)),
            _ => "no stream".to_string(),
        };
        let workspace = app
            .workspace()
            .map(|workspace| (workspace.active + 1, workspace.tabs.len()))
            .unwrap_or((0, 0));
        spans.push(Span::styled(
            format!("  │ tab {}/{} · {}", workspace.0, workspace.1, pane),
            Style::default().fg(theme.color("dim")),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .style(Style::default().bg(theme.color("status_bar_bg")))
            .wrap(Wrap { trim: true }),
        area,
    );
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
