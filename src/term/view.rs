//! 本地 VT：把 core 的字节流还原成 Ratatui Buffer。
//!
//! 这是整个客户端最关键的一层。边界见 plan v1.1 §4：
//! 上层只认 [`TerminalView`]，不出现任何 `ghostty_vt::` 类型。

use anyhow::Result;
use ghostty_vt::{CellColor, CellStyle, CellWide, RenderState, RowCells, RowIterator, Terminal};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

/// 一个终端视图：本地 VT + 渲染状态。
///
/// 每个 Pane 一个。同一个 core 终端可以有多个视图（镜像需 `observe`）。
pub struct TerminalView {
    terminal: Terminal,
    render_state: RenderState,
    size: (u16, u16),
}

impl TerminalView {
    pub fn new(cols: u16, rows: u16, scrollback_bytes: usize) -> Result<Self> {
        let terminal = Terminal::new(cols.max(1), rows.max(1), scrollback_bytes)?;
        let render_state = RenderState::new()?;
        Ok(Self {
            terminal,
            render_state,
            size: (cols.max(1), rows.max(1)),
        })
    }

    /// 喂原始 VT 字节。首帧与流内重绘都走这里。
    pub fn write(&mut self, bytes: &[u8]) {
        self.terminal.write(bytes);
    }

    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// 调整尺寸。`cell_*_px` 用于像素查询，暂时给 8x16。
    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<()> {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if (cols, rows) == self.size {
            return Ok(());
        }
        self.terminal
            .resize(cols, rows, cell_width_px, cell_height_px)?;
        self.size = (cols, rows);
        Ok(())
    }

    /// 把当前屏幕写进 `area`。不覆盖区域外的任何单元格。
    pub fn render(&mut self, buf: &mut Buffer, area: Rect) -> Result<()> {
        if area.width == 0 || area.height == 0 {
            return Ok(());
        }
        self.render_state.update(&self.terminal)?;

        let colors = self.render_state.colors().ok();
        let default_fg = colors.map(|c| rgb(c.foreground));
        let default_bg = colors.map(|c| rgb(c.background));

        let mut row_iterator = RowIterator::new()?;
        let mut row_cells = RowCells::new()?;
        let mut rows = self.render_state.populate_row_iterator(&mut row_iterator)?;

        let mut y = 0u16;
        while y < area.height && rows.next() {
            let mut cells = rows.populate_cells(&mut row_cells)?;
            let mut x = 0u16;
            while x < area.width && cells.next() {
                let cell = &mut buf[(area.x + x, area.y + y)];
                let basic = cells.basic_data().unwrap_or_default();
                match basic.wide {
                    // 宽字符占两列：主 cell 画字，尾 cell 留空。跳过会错位。
                    CellWide::SpacerTail | CellWide::SpacerHead => {
                        cell.set_symbol(" ");
                        cell.set_style(Style::default());
                    }
                    _ => {
                        let symbol = cells.grapheme_text().unwrap_or_default();
                        cell.set_symbol(if symbol.is_empty() { " " } else { &symbol });
                        cell.set_style(style_of(&basic.style, default_fg, default_bg));
                    }
                }
                x += 1;
            }
            while x < area.width {
                let cell = &mut buf[(area.x + x, area.y + y)];
                cell.set_symbol(" ");
                cell.set_style(Style::default());
                x += 1;
            }
            y += 1;
        }
        while y < area.height {
            for x in 0..area.width {
                let cell = &mut buf[(area.x + x, area.y + y)];
                cell.set_symbol(" ");
                cell.set_style(Style::default());
            }
            y += 1;
        }
        Ok(())
    }

    /// 光标位置（视图坐标）。不可见时为 `None`。
    pub fn cursor(&self) -> Option<(u16, u16)> {
        let cursor = self.render_state.cursor().ok()?;
        if !cursor.visible {
            return None;
        }
        cursor.viewport.map(|v| (v.x, v.y))
    }
}

fn rgb(color: ghostty_vt::RgbColor) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}

fn cell_color(color: CellColor) -> Color {
    match color {
        CellColor::Palette(index) => Color::Indexed(index),
        CellColor::Rgb(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

fn style_of(cell: &CellStyle, default_fg: Option<Color>, default_bg: Option<Color>) -> Style {
    let mut fg = cell.fg_color.map(cell_color).or(default_fg);
    let mut bg = cell.bg_color.map(cell_color).or(default_bg);
    if cell.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }

    let mut style = Style::default();
    if let Some(fg) = fg {
        style = style.fg(fg);
    }
    if let Some(bg) = bg {
        style = style.bg(bg);
    }

    let mut modifiers = Modifier::empty();
    if cell.bold {
        modifiers |= Modifier::BOLD;
    }
    if cell.italic {
        modifiers |= Modifier::ITALIC;
    }
    if cell.faint {
        modifiers |= Modifier::DIM;
    }
    if cell.underlined {
        modifiers |= Modifier::UNDERLINED;
    }
    if cell.strikethrough {
        modifiers |= Modifier::CROSSED_OUT;
    }
    if cell.invisible {
        modifiers |= Modifier::HIDDEN;
    }
    if cell.blink {
        modifiers |= Modifier::SLOW_BLINK;
    }
    style.add_modifier(modifiers)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-04 从真实 hc 抓取的 392 字节首帧：
    /// 粗体红、下划线绿、宽字符、真彩、反显、光标 (0,4)。
    const PAINT: &[u8] = include_bytes!("../../tests/fixtures/core-first-paint.bin");

    fn rendered() -> Buffer {
        let mut view = TerminalView::new(70, 12, 1 << 20).unwrap();
        view.write(PAINT);
        let mut buf = Buffer::empty(Rect::new(0, 0, 70, 12));
        view.render(&mut buf, Rect::new(0, 0, 70, 12)).unwrap();
        buf
    }

    #[test]
    fn reconstructs_text_and_styles_from_a_real_first_paint() {
        let buf = rendered();

        let text: String = (0..8).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(text, "bold red");

        let style = buf[(0, 0)].style();
        assert_eq!(style.fg, Some(Color::Indexed(1)));
        assert!(style.add_modifier.contains(Modifier::BOLD));

        // 下划线绿
        let style = buf[(9, 0)].style();
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(style.fg, Some(Color::Indexed(2)));
    }

    #[test]
    fn wide_characters_keep_their_columns() {
        let buf = rendered();
        assert_eq!(buf[(0, 1)].symbol(), "宽");
        assert_eq!(buf[(1, 1)].symbol(), " ");
        assert_eq!(buf[(2, 1)].symbol(), "字");
    }

    #[test]
    fn truecolor_and_reverse_are_preserved() {
        let buf = rendered();
        assert_eq!(buf[(0, 2)].style().fg, Some(Color::Rgb(12, 34, 56)));

        // 反显通过交换前景/背景实现（再叠 REVERSED 会被终端二次反转）。
        let reverse = buf[(0, 3)].style();
        assert!(reverse.fg.is_some() && reverse.bg.is_some());
        assert_ne!(reverse.fg, reverse.bg);
    }

    #[test]
    fn inverse_swaps_foreground_and_background() {
        let cell = CellStyle {
            inverse: true,
            ..CellStyle::default()
        };
        let style = style_of(&cell, Some(Color::White), Some(Color::Black));
        assert_eq!(style.fg, Some(Color::Black));
        assert_eq!(style.bg, Some(Color::White));
    }

    #[test]
    fn cursor_position_comes_from_the_paint() {
        let mut view = TerminalView::new(70, 12, 1 << 20).unwrap();
        view.write(PAINT);
        let mut buf = Buffer::empty(Rect::new(0, 0, 70, 12));
        view.render(&mut buf, Rect::new(0, 0, 70, 12)).unwrap();
        assert_eq!(view.cursor(), Some((0, 4)));
    }
}
