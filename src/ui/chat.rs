//! Chat 视图：把 pi 会话的消息块铺成手机友好的一列。
//!
//! 只负责排版：块内文本按当前宽度重折，工具调用收成一行（可展开），输入行固定
//! 在底部，手机上多一条动作条。数据来自 [`crate::chat`]，输入由 `App` 回写到
//! Pane 的 attach 流。

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::chat::commands::Kind;
use crate::chat::pi::{BlockKind, ToolState};
use crate::chat::{Chat, ChatAction};
use crate::model::Badge;
use crate::theme::Theme;

/// 展开状态下工具结果最多显示的行数。
const RESULT_LINES: usize = 40;
/// 命令面板最多显示的行数。
const PALETTE_LINES: usize = 6;

/// 渲染层从 `App` 取到的上下文：Pane 名字、Agent 徽标、是否手机布局。
pub struct Meta {
    pub label: String,
    pub badge: Badge,
    pub mobile: bool,
}

impl Meta {
    pub fn of(app: &App) -> Self {
        let Some(pane) = app.chat.as_ref().map(|chat| chat.pane) else {
            return Self {
                label: String::new(),
                badge: Badge::None,
                mobile: app.mobile,
            };
        };
        let state = app.panes.get(&pane);
        Self {
            label: state.map(|pane| pane.label()).unwrap_or_default(),
            badge: state.map(|pane| pane.agent.badge()).unwrap_or(Badge::None),
            mobile: app.mobile,
        }
    }
}

/// 渲染一个已打开的 Chat。调用方保证 `app.chat` 是 `Some`。
pub fn render(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let meta = Meta::of(app);
    let Some(chat) = app.chat.as_mut() else {
        return;
    };
    paint(frame, chat, &meta, area, theme);
}

fn paint(frame: &mut Frame, chat: &mut Chat, meta: &Meta, area: Rect, theme: &Theme) {
    let label = &meta.label;
    let badge = meta.badge;
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(blank_lines(area)).style(Style::default().bg(theme.color("bg"))),
        area,
    );

    let palette_rows = chat.palette_matches().len().min(PALETTE_LINES);
    let mobile = meta.mobile;
    let mut constraints = vec![Constraint::Length(1), Constraint::Min(1)];
    if mobile {
        constraints.push(Constraint::Length(1));
    }
    if palette_rows > 0 {
        constraints.push(Constraint::Length(palette_rows as u16));
    }
    constraints.push(Constraint::Length(1));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    let mut index = 0;
    let head_area = rows[index];
    index += 1;
    let body_area = rows[index];
    index += 1;
    let action_area = if mobile {
        let area = rows[index];
        index += 1;
        Some(area)
    } else {
        None
    };
    let palette_area = if palette_rows > 0 {
        let area = rows[index];
        index += 1;
        Some(area)
    } else {
        None
    };
    let composer_area = rows[index];
    let width = area.width as usize;

    chat.actions.clear();

    // 顶栏：会话来源 + 需要人处理时亮红。
    let mut head = format!("chat · {label} · {}", chat.note);
    if chat.tool_running() {
        head.push_str(" · 运行中");
    }
    let head_style = match badge {
        Badge::Blocked | Badge::Error => Style::default()
            .fg(theme.color("agent_error"))
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(theme.color("accent"))
            .add_modifier(Modifier::BOLD),
    };
    render_line(
        frame,
        head_area,
        Line::from(Span::styled(truncate(&head, width), head_style)),
        theme,
    );

    let rendered = body_lines(chat, width, theme);
    let height = body_area.height as usize;
    chat.view_height = height;
    let max_scroll = rendered.lines.len().saturating_sub(height);
    if chat.scroll > max_scroll {
        chat.scroll = max_scroll;
    }
    let start = max_scroll.saturating_sub(chat.scroll);
    let mut window: Vec<Line> = rendered
        .lines
        .iter()
        .skip(start)
        .take(height)
        .cloned()
        .collect();
    // 内容不足一屏时贴底，最新的消息始终在输入行上方。
    let pad = height.saturating_sub(window.len());
    if pad > 0 {
        let mut padded = vec![Line::from(""); pad];
        padded.append(&mut window);
        window = padded;
    }
    frame.render_widget(
        Paragraph::new(window).style(Style::default().fg(theme.color("fg"))),
        body_area,
    );
    // 块的行区间与工具块的可点击区域（只登记落在视口里的行）。
    chat.view_start = start;
    chat.block_lines = rendered.spans.clone();
    chat.hits.clear();
    for (block, first, last) in &rendered.spans {
        if !chat
            .blocks
            .get(*block)
            .is_some_and(|block| block.tool.is_some())
        {
            continue;
        }
        let (first, last) = (*first, *last);
        if last < start || first >= start + height {
            continue;
        }
        let top = (first.max(start) - start) + pad;
        let bottom = (last.min(start + height - 1) - start) + pad;
        if bottom >= height {
            continue;
        }
        let rect = Rect::new(
            body_area.x,
            body_area.y + top as u16,
            body_area.width,
            (bottom - top + 1) as u16,
        );
        chat.hits.push((*block, rect));
    }

    if let Some(action_area) = action_area {
        render_actions(frame, chat, action_area, theme);
    }
    if let Some(palette_area) = palette_area {
        render_palette(frame, chat, palette_area, theme);
    }
    let hint = if chat.palette.is_some() {
        "发消息… ↑↓ 选命令 · Enter 采用"
    } else {
        "发消息… Enter 发送"
    };
    let composer = composer_line(&chat.input, chat.cursor, width, theme, hint);
    render_line(frame, composer_area, composer, theme);
}

fn render_line(frame: &mut Frame, area: Rect, line: Line<'static>, theme: &Theme) {
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.color("bg"))),
        area,
    );
}

/// 整块空白：每个单元格都写一遍，Pane 终端留下的字符不会从缝隙里透出来。
fn blank_lines(area: Rect) -> Vec<Line<'static>> {
    let blank = " ".repeat(area.width as usize);
    (0..area.height)
        .map(|_| Line::from(Span::raw(blank.clone())))
        .collect()
}

fn render_actions(frame: &mut Frame, chat: &mut Chat, area: Rect, theme: &Theme) {
    const ACTIONS: &[(ChatAction, &str)] = &[
        (ChatAction::Send, "发送"),
        (ChatAction::Interrupt, "中断"),
        (ChatAction::Palette, "命令"),
        (ChatAction::Terminal, "终端"),
    ];
    let width = area.width / ACTIONS.len() as u16;
    if width == 0 {
        return;
    }
    let mut spans = Vec::new();
    for (slot, (action, label)) in ACTIONS.iter().enumerate() {
        let x = area.x + slot as u16 * width;
        let rect = Rect::new(x, area.y, width, 1);
        chat.actions.push((*action, rect));
        let style = match action {
            ChatAction::Send => Style::default()
                .fg(theme.color("tab_active_fg"))
                .bg(theme.color("tab_active_bg"))
                .add_modifier(Modifier::BOLD),
            _ => Style::default()
                .fg(theme.color("sidebar_title"))
                .bg(theme.color("status_bar_bg")),
        };
        spans.push(Span::styled(centered(label, width as usize), style));
    }
    render_line(frame, area, Line::from(spans), theme);
}

fn render_palette(frame: &mut Frame, chat: &mut Chat, area: Rect, theme: &Theme) {
    let matches = chat.palette_matches();
    let selected = chat
        .palette
        .as_ref()
        .map(|palette| palette.index)
        .unwrap_or(0);
    let items = chat
        .palette
        .as_ref()
        .map(|palette| palette.items.clone())
        .unwrap_or_default();
    let width = area.width as usize;
    let rows = area.height as usize;
    let start = selected
        .saturating_add(1)
        .saturating_sub(rows)
        .min(matches.len().saturating_sub(rows));
    let mut lines = Vec::new();
    for (row, index) in matches.iter().skip(start).take(rows).enumerate() {
        let Some(item) = items.get(*index) else {
            continue;
        };
        let tail = match item.kind {
            Kind::Terminal => " · 终端",
            Kind::Local(_) => " · 本地",
            Kind::Prompt => "",
        };
        let text = truncate(
            &format!("{} {}{}", item.name, item.description, tail),
            width,
        );
        let style = if row + start == selected {
            Style::default()
                .fg(theme.color("selection_fg"))
                .bg(theme.color("selection_bg"))
        } else {
            Style::default().fg(theme.color("fg"))
        };
        lines.push(Line::from(Span::styled(text, style)));
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.color("sidebar_bg"))),
        area,
    );
}

struct Rendered {
    lines: Vec<Line<'static>>,
    /// 每个块：(块下标, 起始行, 结束行)，都是折行后的行号。
    spans: Vec<(usize, usize, usize)>,
}

/// 消息块 → 已折行的显示行。
fn body_lines(chat: &Chat, width: usize, theme: &Theme) -> Rendered {
    let width = width.max(2);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans = Vec::new();
    for (index, block) in chat.blocks.iter().enumerate() {
        let selected = chat.selected == Some(index);
        let highlight = selected.then(|| theme.color("selection_bg"));
        let start = lines.len();
        match block.kind {
            BlockKind::User => {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                let marker = Span::styled(
                    "› ",
                    Style::default()
                        .fg(theme.color("accent"))
                        .add_modifier(Modifier::BOLD),
                );
                push_wrapped(
                    &mut lines,
                    &block.text,
                    width - 2,
                    marker,
                    theme,
                    "fg",
                    highlight,
                );
            }
            BlockKind::Assistant => {
                push_wrapped(
                    &mut lines,
                    &block.text,
                    width,
                    Span::raw(""),
                    theme,
                    "fg",
                    highlight,
                );
            }
            BlockKind::Thinking => {
                let mut parts = wrap(&block.text, width - 2);
                let total = parts.len();
                parts.truncate(2);
                let marker = Span::styled("⋯ ", Style::default().fg(theme.color("dim")));
                for (row, part) in parts.into_iter().enumerate() {
                    let lead = if row == 0 {
                        marker.clone()
                    } else {
                        Span::raw("  ")
                    };
                    lines.push(with_background(
                        Line::from(vec![
                            lead,
                            Span::styled(part, Style::default().fg(theme.color("dim"))),
                        ]),
                        highlight,
                    ));
                }
                if total > 2 {
                    lines.push(with_background(
                        Line::from(Span::styled(
                            format!("  …思考共 {total} 行"),
                            Style::default().fg(theme.color("dim")),
                        )),
                        highlight,
                    ));
                }
            }
            BlockKind::Tool => {
                let Some(tool) = &block.tool else {
                    continue;
                };
                let color = match tool.state {
                    ToolState::Running => theme.color("agent_working"),
                    ToolState::Ok => theme.color("agent_done"),
                    ToolState::Error => theme.color("agent_error"),
                };
                let status = match tool.state {
                    ToolState::Running => "运行中",
                    ToolState::Ok => "完成",
                    ToolState::Error => "失败",
                };
                let mut tail = if tool.result.is_empty() {
                    status.to_string()
                } else {
                    format!("{status} · {}", tool.result)
                };
                if let Some(duration) = tool.duration_ms {
                    tail.push_str(&format!(" · {duration}ms"));
                }
                let expanded = chat.is_expanded(index);
                let marker = if expanded { "▾" } else { "▸" };
                // 窄屏上给结果留位置：参数摘要最多占三分之一宽。
                let call = truncate(
                    &format!("{} {}", tool.name, tool.args),
                    (width / 3).clamp(8, 40),
                );
                let text = format!("{marker} ⚙ {call} → {tail}");
                lines.push(with_background(
                    Line::from(Span::styled(
                        truncate(&text, width),
                        Style::default().fg(color),
                    )),
                    highlight,
                ));
                if expanded && !tool.detail.is_empty() {
                    let mut parts = wrap(&tool.detail, width.saturating_sub(4).max(1));
                    parts.truncate(RESULT_LINES);
                    for part in parts {
                        lines.push(with_background(
                            Line::from(Span::styled(
                                format!("    {part}"),
                                Style::default().fg(theme.color("dim")),
                            )),
                            highlight,
                        ));
                    }
                }
            }
            BlockKind::Note => {
                push_wrapped(
                    &mut lines,
                    &block.text,
                    width,
                    Span::styled("· ", Style::default().fg(theme.color("dim"))),
                    theme,
                    "dim",
                    highlight,
                );
            }
        }
        if lines.len() > start {
            spans.push((index, start, lines.len() - 1));
        }
    }
    Rendered { lines, spans }
}

fn with_background(
    line: Line<'static>,
    background: Option<ratatui::style::Color>,
) -> Line<'static> {
    match background {
        Some(color) => line.style(Style::default().bg(color)),
        None => line,
    }
}

/// 把一段文本按宽度折行，第一行带 `first` 前缀，其余行缩进同样宽度。
fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    first: Span<'static>,
    theme: &Theme,
    token: &str,
    highlight: Option<ratatui::style::Color>,
) {
    let style = Style::default().fg(theme.color(token));
    let indent = Span::raw(" ".repeat(first.width()));
    let width = width.max(1);
    for (index, part) in wrap(text, width).into_iter().enumerate() {
        let lead = if index == 0 {
            first.clone()
        } else {
            indent.clone()
        };
        lines.push(with_background(
            Line::from(vec![lead, Span::styled(part, style)]),
            highlight,
        ));
    }
}

/// 按显示宽度折行：优先在空格处断开，长串硬切。
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.replace('\t', "    ");
        if raw.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut line_width = 0usize;
        let mut last_space: Option<usize> = None;
        for ch in raw.chars() {
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if line_width + char_width > width && line_width > 0 {
                if ch == ' ' {
                    // 恰好落在边界上的空格：断在这里，空格本身丢掉。
                    out.push(line.trim_end().to_string());
                    line.clear();
                    line_width = 0;
                    last_space = None;
                    continue;
                }
                match last_space.take() {
                    Some(position) => {
                        let head = line[..position].trim_end().to_string();
                        let tail = line[position..].trim_start().to_string();
                        out.push(head);
                        line_width = display_width(&tail);
                        line = tail;
                    }
                    None => {
                        out.push(std::mem::take(&mut line));
                        line_width = 0;
                    }
                }
            }
            line.push(ch);
            line_width += char_width;
            if ch == ' ' {
                last_space = Some(line.len() - 1);
            }
        }
        out.push(line.trim_end().to_string());
    }
    out
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

/// 底部输入行：`› ` + 输入内容 + 光标块。内容超宽时横向滚到光标处。
fn composer_line(
    input: &str,
    cursor: usize,
    width: usize,
    theme: &Theme,
    hint: &str,
) -> Line<'static> {
    let prompt = Span::styled(
        "› ",
        Style::default()
            .fg(theme.color("accent"))
            .add_modifier(Modifier::BOLD),
    );
    if input.is_empty() {
        return Line::from(vec![
            prompt,
            Span::styled(
                truncate(hint, width.saturating_sub(2)),
                Style::default().fg(theme.color("dim")),
            ),
        ]);
    }
    let cursor = cursor.min(input.len());
    let chars: Vec<(char, usize)> = input
        .chars()
        .map(|ch| (ch, UnicodeWidthChar::width(ch).unwrap_or(0)))
        .collect();
    let cursor_index = input[..cursor].chars().count();
    let available = width.saturating_sub(2).max(1);
    let mut start = 0usize;
    loop {
        let used: usize = chars[start..cursor_index].iter().map(|(_, w)| *w).sum();
        if used < available || start >= cursor_index {
            break;
        }
        start += 1;
    }
    let mut spans = vec![prompt];
    let mut used = 0usize;
    for (index, (ch, w)) in chars.iter().enumerate().skip(start) {
        if used + w > available {
            break;
        }
        if index == cursor_index {
            spans.push(Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(theme.color("cursor_fg"))
                    .bg(theme.color("cursor_bg")),
            ));
        } else {
            spans.push(Span::raw(ch.to_string()));
        }
        used += w;
    }
    if cursor_index >= chars.len() {
        spans.push(Span::styled(
            " ",
            Style::default()
                .fg(theme.color("cursor_fg"))
                .bg(theme.color("cursor_bg")),
        ));
    }
    Line::from(spans)
}

/// 按显示宽度居中，宽字符不会把布局撑破。
fn centered(text: &str, width: usize) -> String {
    let used = display_width(text);
    if used >= width {
        return truncate(text, width);
    }
    let left = (width - used) / 2;
    let right = width - used - left;
    format!("{}{text}{}", " ".repeat(left), " ".repeat(right))
}

fn truncate(text: &str, width: usize) -> String {
    let mut used = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width {
            out.push('…');
            return out;
        }
        used += w;
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{composer_line, paint, truncate, wrap, Meta};
    use crate::chat::Chat;
    use crate::model::Badge;
    use crate::theme::builtin;
    use ratatui::{backend::TestBackend, layout::Rect, text::Line, widgets::Paragraph, Terminal};

    fn session_lines() -> Vec<String> {
        [
            serde_json::json!({"type":"session","version":3,"id":"s","cwd":"/tmp/app"}),
            serde_json::json!({"type":"message","id":"a","parentId":null,
                "message":{"role":"user","content":"给这个仓库加一个 chat 视图，要求手机上能看清楚"}}),
            serde_json::json!({"type":"message","id":"b","parentId":"a",
                "message":{"role":"assistant","content":[
                    {"type":"text","text":"先读一下现有的渲染层。"},
                    {"type":"toolCall","id":"t1","name":"bash","arguments":{"command":"wc -l src/app.rs"}}]}}),
            serde_json::json!({"type":"message","id":"c","parentId":"b",
                "message":{"role":"toolResult","toolCallId":"t1","toolName":"bash",
                    "content":[{"type":"text","text":"5644 src/app.rs\n展开第二行"}],"isError":false}}),
        ]
        .iter()
        .map(|value| value.to_string())
        .collect()
    }

    fn painted(mobile: bool) -> String {
        let mut chat = Chat::from_lines(1, session_lines());
        let theme = builtin("default").expect("default theme");
        let backend = TestBackend::new(40, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let meta = Meta {
            label: "pi".to_string(),
            badge: Badge::Working,
            mobile,
        };
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 40, 14);
                paint(frame, &mut chat, &meta, area, &theme);
            })
            .expect("draw");
        screen(terminal.backend().buffer(), 40, 14)
    }

    /// 取屏幕文本；宽字符的填充格也占一格，去掉空白后比较内容。
    fn screen(buffer: &ratatui::buffer::Buffer, width: usize, height: usize) -> String {
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x as u16, y as u16)].symbol());
            }
            out.push('\n');
        }
        out.chars().filter(|ch| !ch.is_whitespace()).collect()
    }

    #[test]
    fn chat_shows_messages_and_tool_cards() {
        let text = painted(false);
        assert!(text.contains("chat·pi"), "{text}");
        assert!(text.contains("›给这个仓库"), "{text}");
        assert!(text.contains("⚙bashwc-l"), "{text}");
        assert!(text.contains("→完成·5644"), "{text}");
    }

    #[test]
    fn mobile_layout_adds_the_action_bar() {
        let mut chat = Chat::from_lines(1, session_lines());
        let theme = builtin("default").expect("default theme");
        let backend = TestBackend::new(40, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let meta = Meta {
            label: "pi".to_string(),
            badge: Badge::Idle,
            mobile: true,
        };
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 40, 16);
                paint(frame, &mut chat, &meta, area, &theme);
            })
            .expect("draw");
        assert_eq!(chat.actions.len(), 4);
        let text = screen(terminal.backend().buffer(), 40, 16);
        assert!(text.contains("发送"), "{text}");
        assert!(text.contains("中断"), "{text}");
    }

    /// 用真实 transcript 冒烟：`HC_TUI_CHAT_SESSION=<path> cargo test --lib real_transcript`。
    #[test]
    fn real_transcript_renders() {
        let Ok(path) = std::env::var("HC_TUI_CHAT_SESSION") else {
            return;
        };
        let text = std::fs::read_to_string(&path).expect("session file");
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let mut chat = Chat::from_lines(1, lines);
        assert!(!chat.blocks.is_empty(), "no blocks from {path}");
        let theme = builtin("default").expect("default theme");
        let backend = TestBackend::new(60, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let meta = Meta {
            label: "pi".to_string(),
            badge: Badge::Working,
            mobile: true,
        };
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 60, 30);
                paint(frame, &mut chat, &meta, area, &theme);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..30 {
            for x in 0..60 {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        println!("{out}");
    }

    #[test]
    fn a_long_unbroken_word_is_hard_wrapped() {
        assert_eq!(wrap(&"x".repeat(50), 20).len(), 3);
    }

    #[test]
    fn wrap_breaks_on_spaces_before_hard_cutting() {
        assert_eq!(wrap("hello world again", 11), vec!["hello world", "again"]);
    }

    #[test]
    fn wrap_counts_wide_characters_as_two_cells() {
        assert_eq!(wrap("中文中文", 4), vec!["中文", "中文"]);
    }

    #[test]
    fn composer_keeps_the_prompt_when_empty() {
        let theme = builtin("default").expect("default theme");
        let line = composer_line("", 0, 40, &theme, "发消息…");
        assert_eq!(line.spans.len(), 2);
    }

    #[test]
    fn narrow_width_expanded_tool_does_not_panic() {
        let mut chat = Chat::from_lines(1, session_lines());
        chat.toggle_expanded(2);
        let theme = builtin("default").expect("default theme");
        for width in 2..6u16 {
            let backend = TestBackend::new(width, 10);
            let mut terminal = Terminal::new(backend).expect("terminal");
            let meta = Meta {
                label: "pi".to_string(),
                badge: Badge::Working,
                mobile: false,
            };
            let _ = terminal.draw(|frame| {
                let area = Rect::new(0, 0, width, 10);
                paint(frame, &mut chat, &meta, area, &theme);
            });
        }
    }

    #[test]
    fn expanding_a_tool_card_shows_the_full_result() {
        let mut chat = Chat::from_lines(1, session_lines());
        chat.toggle_expanded(2);
        let theme = builtin("default").expect("default theme");
        let backend = TestBackend::new(40, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let meta = Meta {
            label: "pi".to_string(),
            badge: Badge::Working,
            mobile: false,
        };
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 40, 16);
                paint(frame, &mut chat, &meta, area, &theme);
            })
            .expect("draw");
        let text = screen(terminal.backend().buffer(), 40, 16);
        assert!(text.contains("展开第二行"), "{text}");
    }

    #[test]
    fn chat_paints_over_the_terminal_underneath() {
        let mut chat = Chat::from_lines(1, session_lines());
        let theme = builtin("default").expect("default theme");
        let backend = TestBackend::new(40, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let meta = Meta {
            label: "pi".to_string(),
            badge: Badge::Working,
            mobile: false,
        };
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 40, 16);
                // 模拟同一帧里先画的 Pane 终端。
                let leftovers = vec![Line::from("X".repeat(40)); 16];
                frame.render_widget(Paragraph::new(leftovers), area);
                paint(frame, &mut chat, &meta, area, &theme);
            })
            .expect("draw");
        let text = screen(terminal.backend().buffer(), 40, 16);
        assert!(!text.contains('X'), "{text}");
    }

    #[test]
    fn truncate_respects_display_width() {
        assert_eq!(truncate("中文中文", 5), "中文…");
    }
}
