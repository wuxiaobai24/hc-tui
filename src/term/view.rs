//! 本地 VT：把 core 的字节流还原成 Ratatui Buffer。
//!
//! 这是整个客户端最关键的一层。边界见 plan v1.1 §4：
//! 上层只认 [`TerminalView`]，不出现任何 `ghostty_vt::` 类型。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::event::{KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ghostty_vt::{
    ffi, CellColor, CellStyle, CellWide, KeyEncoder, MouseEncoder, RenderState, RowCells,
    RowIterator, Terminal,
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use super::input;

/// 本地假定的 cell 像素尺寸。FACTS 与鼠标坐标换算共用。
const CELL_WIDTH_PX: u32 = 8;
const CELL_HEIGHT_PX: u32 = 16;

/// 一个终端视图：本地 VT + 渲染状态。
///
/// 每个 Pane 一个。同一个 core 终端可以有多个视图（镜像需 `observe`）。
pub struct TerminalView {
    terminal: Terminal,
    render_state: RenderState,
    size: (u16, u16),
    key_encoder: KeyEncoder,
    mouse_encoder: MouseEncoder,
    /// 本地 VT 生成的应答字节（透传查询的回答）。
    responses: Arc<Mutex<Vec<u8>>>,
    /// kitty graphics 已解码的 RGBA 缓存：fingerprint → 像素。
    image_cache: HashMap<u64, Arc<Vec<u8>>>,
}

/// 一张要绘制到单元格上的 kitty 图像。
#[derive(Debug, Clone)]
pub struct PlacementImage {
    pub col: i32,
    pub row: i32,
    pub cols: u32,
    pub rows: u32,
    pub z: i32,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

impl TerminalView {
    pub fn new(cols: u16, rows: u16, scrollback_bytes: usize) -> Result<Self> {
        let mut terminal = Terminal::new(cols.max(1), rows.max(1), scrollback_bytes)?;
        // core 只转发它不认识的查询（kitty graphics 等）。这些序列留在 attach
        // 流里，必须由本地 VT 回答，否则程序会一直等。
        let responses: Arc<Mutex<Vec<u8>>> = Arc::default();
        let sink = Arc::clone(&responses);
        terminal.set_write_pty_callback(move |bytes: &[u8]| {
            let mut buffer = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            buffer.extend_from_slice(bytes);
        })?;
        // 启用后由 libghostty 自己保存图像并回答 kitty 查询。
        terminal.enable_kitty_graphics()?;
        Ok(Self {
            terminal,
            render_state: RenderState::new()?,
            size: (cols.max(1), rows.max(1)),
            key_encoder: KeyEncoder::new()?,
            mouse_encoder: MouseEncoder::new()?,
            responses,
            image_cache: HashMap::new(),
        })
    }

    /// 喂原始 VT 字节。首帧与流内重绘都走这里。
    pub fn write(&mut self, bytes: &[u8]) {
        self.terminal.write(bytes);
        // OSC 52 剪贴板与 OSC 7 目录变化不回传宿主终端；取走它们，既做了过滤，
        // 也避免内部回调状态随运行时间无限累积。
        let _ = self.terminal.take_clipboard_writes();
        let _ = self.terminal.take_pwd_changes();
        let _ = self.terminal.take_bell_count();
    }

    /// 当前视口里的 kitty 图像。PNG 只在第一次出现时解码。
    pub fn placement_images(&mut self) -> Vec<PlacementImage> {
        if !self
            .terminal
            .kitty_graphics_may_have_placements()
            .unwrap_or(false)
        {
            return Vec::new();
        }
        let Ok(placements) = self.terminal.kitty_image_placements() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for placement in placements {
            let rgba = match self.image_cache.get(&placement.data_fingerprint) {
                Some(cached) => Arc::clone(cached),
                None => {
                    let Some(decoded) = decode_placement(&placement) else {
                        continue;
                    };
                    let decoded = Arc::new(decoded);
                    self.image_cache
                        .insert(placement.data_fingerprint, Arc::clone(&decoded));
                    decoded
                }
            };
            out.push(PlacementImage {
                col: placement.render.viewport_col,
                row: placement.render.viewport_row,
                cols: placement.render.grid_cols.max(1),
                rows: placement.render.grid_rows.max(1),
                z: placement.z,
                width: placement.image_width.max(1),
                height: placement.image_height.max(1),
                rgba,
            });
        }
        out.sort_by_key(|image| image.z);
        out
    }

    /// 取走本地 VT 对透传查询生成的应答，交给 attach 流写回。
    pub fn take_responses(&mut self) -> Vec<u8> {
        let mut buffer = self
            .responses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *buffer)
    }

    /// 按键 → 子程序该收到的字节。编码依据本地 VT 的当前键盘模式。
    pub fn encode_key(&mut self, key: KeyEvent) -> Option<Vec<u8>> {
        let mapped = input::map_key(key)?;
        let mut event = ghostty_vt::KeyEvent::new().ok()?;
        event.set_action(match key.kind {
            KeyEventKind::Press => ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS,
            KeyEventKind::Repeat => ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_REPEAT,
            KeyEventKind::Release => ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_RELEASE,
        });
        event.set_key(mapped.key);
        event.set_mods(mapped.mods);
        event.set_consumed_mods(mapped.consumed);
        event.set_utf8(&mapped.text);
        self.key_encoder.set_from_terminal(&self.terminal);
        match self.key_encoder.encode(&event) {
            Ok(bytes) if !bytes.is_empty() => Some(bytes),
            _ => None,
        }
    }

    /// 鼠标只在子程序自己启用了跟踪时才转发；否则留给宿主做选择。
    pub fn mouse_tracking(&self) -> bool {
        self.terminal.mouse_tracking_enabled().unwrap_or(false)
    }

    /// 鼠标事件 → 子程序该收到的字节。坐标是相对内容区左上角的 cell 位置。
    pub fn encode_mouse(&mut self, mouse: MouseEvent, origin: (u16, u16)) -> Option<Vec<u8>> {
        let (action, button) = match mouse.kind {
            MouseEventKind::Down(button) => (
                ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_PRESS,
                Some(mouse_button(button)),
            ),
            MouseEventKind::Up(button) => (
                ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_RELEASE,
                Some(mouse_button(button)),
            ),
            MouseEventKind::Drag(button) => (
                ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_MOTION,
                Some(mouse_button(button)),
            ),
            MouseEventKind::Moved => (ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_MOTION, None),
            MouseEventKind::ScrollUp => (
                ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_PRESS,
                Some(ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FOUR),
            ),
            MouseEventKind::ScrollDown => (
                ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_PRESS,
                Some(ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FIVE),
            ),
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => return None,
        };

        let mut event = ghostty_vt::MouseEvent::new().ok()?;
        event.set_action(action);
        match button {
            Some(button) => event.set_button(button),
            None => event.clear_button(),
        }
        event.set_mods(input::to_mods(mouse.modifiers));
        event.set_position(
            mouse.column.saturating_sub(origin.0) as f32,
            mouse.row.saturating_sub(origin.1) as f32,
        );
        self.mouse_encoder.set_from_terminal(&self.terminal);
        self.mouse_encoder.set_size(
            u32::from(self.size.0) * CELL_WIDTH_PX,
            u32::from(self.size.1) * CELL_HEIGHT_PX,
            CELL_WIDTH_PX,
            CELL_HEIGHT_PX,
        );
        match self.mouse_encoder.encode(&event) {
            Ok(bytes) if !bytes.is_empty() => Some(bytes),
            _ => None,
        }
    }

    /// 粘贴文本。bracketed paste 由子程序当前模式决定是否包裹。
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        if self.terminal.mode_get(2004).unwrap_or(false) {
            let mut out = Vec::with_capacity(text.len() + 12);
            out.extend_from_slice(b"\x1b[200~");
            out.extend_from_slice(text.as_bytes());
            out.extend_from_slice(b"\x1b[201~");
            out
        } else {
            text.as_bytes().to_vec()
        }
    }

    /// 滚动本地视口。负数向上翻历史。
    pub fn scroll(&mut self, lines: isize) {
        self.terminal.scroll_viewport_delta(lines);
    }

    pub fn scroll_bottom(&mut self) {
        self.terminal.scroll_viewport_bottom();
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

    /// 视口每一行的文本，用于 Copy Mode 的移动与搜索。
    pub fn viewport_lines(&self) -> Vec<String> {
        let (cols, rows) = self.size;
        let mut lines = Vec::with_capacity(rows as usize);
        for row in 0..rows {
            let mut text = self
                .terminal
                .read_text_viewport(
                    (0, u32::from(row)),
                    (cols.saturating_sub(1), u32::from(row)),
                    false,
                )
                .unwrap_or_default();
            text.truncate(text.trim_end().len());
            lines.push(text);
        }
        lines
    }

    /// 视口坐标处的超链接目标。
    pub fn link_at(&self, x: u16, y: u32) -> Option<String> {
        self.terminal.viewport_hyperlink_uri(x, y).ok().flatten()
    }

    /// 视口的 ANSI 形式，用于交给外部编辑器。
    pub fn ansi_viewport(&self) -> String {
        let (cols, rows) = self.size;
        self.terminal
            .read_ansi_viewport(
                (0, 0),
                (cols.saturating_sub(1), u32::from(rows.saturating_sub(1))),
                false,
            )
            .unwrap_or_default()
    }

    /// 滚动条信息：`(总行数, 视口偏移, 视口行数)`。
    pub fn scrollbar(&self) -> Option<(usize, usize, usize)> {
        let scrollbar = self.terminal.scrollbar().ok()?;
        (scrollbar.total > scrollbar.len).then_some((
            scrollbar.total,
            scrollbar.offset,
            scrollbar.len,
        ))
    }

    /// 视口内一段选区的纯文本。`rectangle` 为真时按列取一个方块。
    pub fn selection_text(&self, start: (u16, u16), end: (u16, u16), rectangle: bool) -> String {
        self.terminal
            .read_text_viewport(
                (start.0, u32::from(start.1)),
                (end.0, u32::from(end.1)),
                rectangle,
            )
            .unwrap_or_default()
    }
}

/// kitty 图像数据 → RGBA。PNG 用 crate 自带的解码器。
fn decode_placement(placement: &ghostty_vt::KittyImagePlacement) -> Option<Vec<u8>> {
    match placement.format {
        ghostty_vt::KittyImageFormat::Rgba => Some(placement.data.clone()),
        ghostty_vt::KittyImageFormat::Rgb => {
            let mut rgba = Vec::with_capacity(placement.data.len() / 3 * 4);
            for pixel in placement.data.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
            Some(rgba)
        }
        ghostty_vt::KittyImageFormat::Png => {
            let decoder = png::Decoder::new(std::io::Cursor::new(&placement.data));
            let mut reader = decoder.read_info().ok()?;
            let mut buffer = vec![0; reader.output_buffer_size()];
            let info = reader.next_frame(&mut buffer).ok()?;
            buffer.truncate(info.buffer_size());
            match info.color_type {
                png::ColorType::Rgba => Some(buffer),
                png::ColorType::Rgb => {
                    let mut rgba = Vec::with_capacity(buffer.len() / 3 * 4);
                    for pixel in buffer.as_chunks::<3>().0 {
                        rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
                    }
                    Some(rgba)
                }
                _ => None,
            }
        }
    }
}

fn mouse_button(button: MouseButton) -> ffi::GhosttyMouseButton {
    match button {
        MouseButton::Left => ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_LEFT,
        MouseButton::Right => ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_RIGHT,
        MouseButton::Middle => ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_MIDDLE,
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
    use crossterm::event::{KeyCode, KeyModifiers};

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
    fn kitty_queries_are_answered_by_the_local_vt() {
        let mut view = TerminalView::new(20, 5, 1 << 20).unwrap();
        // 已启用 kitty graphics：查询由 libghostty 回答，客户端不再插话。
        view.write(b"\x1b_Gi=31,a=q,f=24;AAAA\x1b\\");
        let answer = String::from_utf8_lossy(&view.take_responses()).into_owned();
        assert!(answer.starts_with("\x1b_Gi=31;"), "{answer:?}");
        assert!(!answer.contains("ENOTSUPPORTED"), "{answer:?}");
    }

    #[test]
    fn kitty_query_split_across_frames_is_still_answered() {
        let mut view = TerminalView::new(20, 5, 1 << 20).unwrap();
        view.write(b"prefix \x1b_Gi=7,a=q");
        assert!(
            view.take_responses().is_empty(),
            "unfinished query must wait"
        );
        view.write(b",f=24;AAAA\x1b\\");
        let answer = String::from_utf8_lossy(&view.take_responses()).into_owned();
        assert!(answer.contains("i=7;"), "{answer:?}");
        assert!(!answer.contains("ENOTSUPPORTED"), "{answer:?}");
    }

    #[test]
    fn clipboard_and_pwd_sequences_are_filtered() {
        let mut view = TerminalView::new(20, 5, 1 << 20).unwrap();
        view.write(b"\x1b]52;c;aGVsbG8=\x07\x1b]7;file:///tmp\x07\x07text");
        assert!(
            view.take_responses().is_empty(),
            "host must not answer OSC 52/7"
        );
        assert!(view.take_responses().is_empty());
    }

    #[test]
    fn kitty_transmit_is_not_faked_by_the_client() {
        let mut view = TerminalView::new(20, 5, 1 << 20).unwrap();
        view.write(b"\x1b_Gi=9,a=t,f=24;AAAA\x1b\\");
        let answer = String::from_utf8_lossy(&view.take_responses()).into_owned();
        assert!(!answer.contains("ENOTSUPPORTED"), "{answer:?}");
    }

    fn encoded(view: &mut TerminalView, code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
        view.encode_key(KeyEvent::new(code, modifiers))
            .unwrap_or_default()
    }

    #[test]
    fn encodes_legacy_key_sequences() {
        let mut view = TerminalView::new(20, 5, 1 << 20).unwrap();
        assert_eq!(
            encoded(&mut view, KeyCode::Char('a'), KeyModifiers::NONE),
            b"a"
        );
        assert_eq!(
            encoded(&mut view, KeyCode::Char('c'), KeyModifiers::CONTROL),
            vec![0x03]
        );
        assert_eq!(
            encoded(&mut view, KeyCode::Enter, KeyModifiers::NONE),
            b"\r"
        );
        assert_eq!(
            encoded(&mut view, KeyCode::Up, KeyModifiers::NONE),
            b"\x1b[A"
        );
    }

    #[test]
    fn paste_is_plain_until_the_program_enables_brackets() {
        let mut view = TerminalView::new(20, 5, 1 << 20).unwrap();
        assert_eq!(view.encode_paste("hi"), b"hi");
        view.write(b"\x1b[?2004h");
        assert_eq!(view.encode_paste("hi"), b"\x1b[200~hi\x1b[201~");
    }

    #[test]
    fn cursor_position_comes_from_the_paint() {
        let mut view = TerminalView::new(70, 12, 1 << 20).unwrap();
        view.write(PAINT);
        let mut buf = Buffer::empty(Rect::new(0, 0, 70, 12));
        view.render(&mut buf, Rect::new(0, 0, 70, 12)).unwrap();
        assert_eq!(view.cursor(), Some((0, 4)));
    }

    #[test]
    fn viewport_lines_and_selection_read_back_the_paint() {
        let mut view = TerminalView::new(70, 12, 1 << 20).unwrap();
        view.write(PAINT);
        let lines = view.viewport_lines();
        assert!(lines[0].starts_with("bold red"), "{:?}", lines[0]);
        assert!(lines[1].contains("宽字"), "{:?}", lines[1]);

        let text = view.selection_text((0, 0), (7, 0), false);
        assert_eq!(text, "bold red");

        // 跨行选区会带换行。
        let text = view.selection_text((0, 0), (3, 1), false);
        assert!(text.contains('\n'), "{text:?}");
    }
}
