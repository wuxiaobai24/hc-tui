//! Ratatui 渲染层。

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    let (body, status) = (rows[0], rows[1]);

    let main = if app.sidebar {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(app.config.sidebar_width),
                Constraint::Min(10),
            ])
            .split(body);
        render_sidebar(frame, cols[0]);
        cols[1]
    } else {
        body
    };

    render_pane(frame, app, main);
    render_status(frame, app, status);
}

fn render_sidebar(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " workspaces ",
            Style::default().fg(Color::Cyan),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(Span::styled(
            "▾ demo",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("   Agents"),
        Line::from("   Shell"),
        Line::from("   Tests"),
        Line::from(""),
        Line::from(Span::styled(
            "  AGENTS",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(vec![
            Span::styled("  ! ", Style::default().fg(Color::Red)),
            Span::raw("codex"),
            Span::styled("  blocked", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::styled("  ● ", Style::default().fg(Color::Yellow)),
            Span::raw("pi"),
            Span::styled("  working", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::styled("  ○ ", Style::default().fg(Color::DarkGray)),
            Span::raw("claude"),
            Span::styled("  idle", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  (static demo)",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" demo ", Style::default().fg(Color::Cyan)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if app.view.resize(inner.width, inner.height, 8, 16).is_err() {
        return;
    }
    if app.view.render(frame.buffer_mut(), inner).is_err() {
        return;
    }
    if let Some((x, y)) = app.view.cursor() {
        if x < inner.width && y < inner.height {
            frame.set_cursor_position((inner.x + x, inner.y + y));
        }
    }
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let prefix = if app.prefix_active() {
        Span::styled(
            " PREFIX ",
            Style::default().bg(Color::Yellow).fg(Color::Black),
        )
    } else {
        Span::styled(
            " ctrl+b ",
            Style::default().bg(Color::Rgb(40, 40, 40)).fg(Color::Gray),
        )
    };
    let line = Line::from(vec![
        Span::styled(
            " hc-tui ",
            Style::default().bg(Color::Cyan).fg(Color::Black),
        ),
        prefix,
        Span::raw("  "),
        Span::styled(&app.status, Style::default().fg(Color::Gray)),
        Span::styled("   │ not connected", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::Rgb(24, 24, 24))),
        area,
    );
}
