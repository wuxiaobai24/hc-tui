//! 本地 VT：把 core 的字节流还原成 Ratatui Buffer。
//!
//! 这是整个客户端最关键的一层。边界见 plan v1.1 §4：
//! 上层只认 [`TerminalView`]，不出现任何 `ghostty_vt::` 类型。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    theme: Option<crate::theme::Theme>,
    committed: Option<Buffer>,
    sync_started: Option<Instant>,
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
    pub source: [u32; 4],
    pub offset: [u32; 2],
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
            theme: None,
            committed: None,
            sync_started: None,
        })
    }

    pub fn apply_theme(&mut self, theme: &crate::theme::Theme) -> Result<()> {
        if self.theme.as_ref() == Some(theme) {
            return Ok(());
        }
        let rgb = |color| -> ghostty_vt::RgbColor {
            let Color::Rgb(r, g, b) = color else {
                unreachable!("resolved theme color")
            };
            ghostty_vt::RgbColor { r, g, b }
        };
        // Named/indexed custom colors are resolved against the theme palette too.
        let palette = theme.terminal_palette();
        let resolve = |color: Color, fallback: Color| -> ghostty_vt::RgbColor {
            rgb(resolve_color(color, &palette).unwrap_or(fallback))
        };
        self.terminal.set_default_colors(
            resolve(theme.color("fg"), Color::Rgb(224, 224, 224)),
            resolve(theme.color("bg"), Color::Rgb(18, 18, 18)),
            resolve(theme.color("cursor_bg"), Color::Rgb(200, 200, 200)),
        )?;
        let mut full = ghostty_vt::default_palette();
        for (i, color) in palette.into_iter().enumerate() {
            full[i] = resolve(color, Color::Rgb(0, 0, 0));
        }
        self.terminal.set_default_palette(&full)?;
        self.theme = Some(theme.clone());
        Ok(())
    }

    /// 喂原始 VT 字节。首帧与流内重绘都走这里。
    pub fn write(&mut self, bytes: &[u8]) {
        self.terminal.write(bytes);
        if self
            .terminal
            .mode_get(ghostty_vt::MODE_SYNCHRONIZED_OUTPUT)
            .unwrap_or(false)
        {
            self.sync_started.get_or_insert_with(Instant::now);
        } else {
            self.sync_started = None;
        }
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
            self.image_cache.clear();
            return Vec::new();
        }
        let Ok(placements) = self.terminal.kitty_image_placements() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let live: std::collections::HashSet<_> =
            placements.iter().map(|p| p.data_fingerprint).collect();
        self.image_cache.retain(|key, _| live.contains(key));
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
                source: [
                    placement.render.source_x,
                    placement.render.source_y,
                    placement.render.source_width,
                    placement.render.source_height,
                ],
                offset: [placement.x_offset, placement.y_offset],
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
        // 事件坐标是 surface 像素；编码器按 cell 尺寸换算回单元格。
        event.set_position(
            f32::from(mouse.column.saturating_sub(origin.0)) * CELL_WIDTH_PX as f32,
            f32::from(mouse.row.saturating_sub(origin.1)) * CELL_HEIGHT_PX as f32,
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
        // Keep the last complete frame while an application builds a synchronized
        // update across network chunks. A broken writer cannot freeze the pane forever.
        if self
            .sync_started
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            if let Some(committed) = &self.committed {
                for y in 0..area.height.min(committed.area.height) {
                    for x in 0..area.width.min(committed.area.width) {
                        buf[(area.x + x, area.y + y)].clone_from(&committed[(x, y)]);
                    }
                }
            }
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
                cell.reset();
                let basic = cells.basic_data().unwrap_or_default();
                match basic.wide {
                    // 宽字符占两列：主 cell 画字，尾 cell 留空。跳过会错位。
                    CellWide::SpacerTail | CellWide::SpacerHead => {
                        cell.set_symbol(" ");
                        cell.set_style(style_of(
                            &basic.style,
                            default_fg,
                            default_bg,
                            colors.as_ref().map(|c| &c.palette),
                        ));
                    }
                    _ => {
                        let symbol = cells.grapheme_text().unwrap_or_default();
                        cell.set_symbol(
                            if symbol.is_empty()
                                || (basic.wide == CellWide::Wide && x + 1 >= area.width)
                            {
                                " "
                            } else {
                                &symbol
                            },
                        );
                        cell.set_style(style_of(
                            &basic.style,
                            default_fg,
                            default_bg,
                            colors.as_ref().map(|c| &c.palette),
                        ));
                    }
                }
                x += 1;
            }
            while x < area.width {
                let cell = &mut buf[(area.x + x, area.y + y)];
                cell.reset();
                cell.set_symbol(" ");
                cell.set_style(
                    Style::default()
                        .fg(default_fg.unwrap_or(Color::Reset))
                        .bg(default_bg.unwrap_or(Color::Reset)),
                );
                x += 1;
            }
            y += 1;
        }
        while y < area.height {
            for x in 0..area.width {
                let cell = &mut buf[(area.x + x, area.y + y)];
                cell.reset();
                cell.set_symbol(" ");
                cell.set_style(
                    Style::default()
                        .fg(default_fg.unwrap_or(Color::Reset))
                        .bg(default_bg.unwrap_or(Color::Reset)),
                );
            }
            y += 1;
        }
        let local = Rect::new(0, 0, area.width, area.height);
        let committed = self.committed.get_or_insert_with(|| Buffer::empty(local));
        committed.resize(local);
        for y in 0..area.height {
            for x in 0..area.width {
                committed[(x, y)].clone_from(&buf[(area.x + x, area.y + y)]);
            }
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

pub(crate) fn theme_rgb(theme: &crate::theme::Theme, token: &str) -> Color {
    let palette = theme.terminal_palette();
    resolve_color(theme.color(token), &palette)
        .filter(|c| matches!(c, Color::Rgb(..)))
        .unwrap_or(match token {
            "bg" => Color::Rgb(18, 18, 18),
            "cursor_bg" => Color::Rgb(200, 200, 200),
            _ => Color::Rgb(224, 224, 224),
        })
}

fn resolve_color(color: Color, palette: &[Color; 16]) -> Option<Color> {
    let index = match color {
        Color::Rgb(..) => return Some(color),
        Color::Reset => return None,
        Color::Indexed(i) => {
            return Some(if i < 16 {
                match palette[i as usize] {
                    c @ Color::Rgb(..) => c,
                    _ => rgb(ghostty_vt::default_palette()[i as usize]),
                }
            } else {
                rgb(ghostty_vt::default_palette()[i as usize])
            });
        }
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
    };
    Some(match palette[index] {
        color @ Color::Rgb(..) => color,
        Color::Indexed(i) => rgb(ghostty_vt::default_palette()[i as usize]),
        _ => rgb(ghostty_vt::default_palette()[index]),
    })
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

fn style_of(
    cell: &CellStyle,
    default_fg: Option<Color>,
    default_bg: Option<Color>,
    palette: Option<&[ghostty_vt::RgbColor; 256]>,
) -> Style {
    let resolve = |color| match (color, palette) {
        (CellColor::Palette(index), Some(palette)) => rgb(palette[index as usize]),
        _ => cell_color(color),
    };
    let mut fg = cell.fg_color.map(resolve).or(default_fg);
    let mut bg = cell.bg_color.map(resolve).or(default_bg);
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
    fn virtual_kitty_placeholders_expose_the_correct_source_crop() {
        use base64::Engine;
        let mut view = TerminalView::new(20, 5, 1 << 20).unwrap();
        let pixels = [255, 0, 0, 255].repeat(8);
        let payload = base64::engine::general_purpose::STANDARD.encode(pixels);
        view.write(
            format!("\x1b_Ga=T,f=32,s=4,v=2,U=1,i=7,c=4,r=1,q=2;{payload}\x1b\\").as_bytes(),
        );
        view.write("\x1b[H\x1b[38;2;0;0;7m\u{10eeee}\u{0305}\u{0305}\x1b[0m".as_bytes());
        let images = view.placement_images();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].source, [0, 0, 1, 2]);
        assert_eq!(images[0].width, 4);
    }

    #[test]
    fn reconstructs_text_and_styles_from_a_real_first_paint() {
        let buf = rendered();

        let text: String = (0..8).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(text, "bold red");

        let style = buf[(0, 0)].style();
        assert_eq!(style.fg, Some(rgb(ghostty_vt::default_palette()[1])));
        assert!(style.add_modifier.contains(Modifier::BOLD));

        // 下划线绿
        let style = buf[(9, 0)].style();
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(style.fg, Some(rgb(ghostty_vt::default_palette()[2])));
    }

    #[test]
    fn wide_characters_keep_their_columns() {
        let buf = rendered();
        assert_eq!(buf[(0, 1)].symbol(), "宽");
        assert_eq!(buf[(1, 1)].symbol(), " ");
        assert_eq!(buf[(2, 1)].symbol(), "字");
    }

    #[test]
    fn synchronized_redraws_do_not_expose_half_frames_and_have_a_timeout() {
        let area = Rect::new(0, 0, 20, 3);
        let mut view = TerminalView::new(20, 3, 1 << 20).unwrap();
        let mut buffer = Buffer::empty(area);
        view.write(b"complete");
        view.render(&mut buffer, area).unwrap();
        let complete = buffer.clone();
        for bytes in [b"\x1b[?2026h\x1b[2J\x1b[H".as_slice(), b"par", b"tial"] {
            view.write(bytes);
            buffer.reset();
            view.render(&mut buffer, area).unwrap();
            assert_eq!(buffer, complete);
        }
        view.write(b"\x1b[?2026l");
        view.render(&mut buffer, area).unwrap();
        assert_eq!(buffer[(0, 0)].symbol(), "p");
        view.write(b"\x1b[?2026h\rtimeout");
        view.sync_started = Some(Instant::now() - Duration::from_secs(2));
        view.render(&mut buffer, area).unwrap();
        assert_eq!(buffer[(0, 0)].symbol(), "t");
    }

    #[test]
    fn wide_selection_keeps_highlight_on_both_columns() {
        let area = Rect::new(0, 0, 12, 2);
        let mut view = TerminalView::new(12, 2, 1 << 20).unwrap();
        view.write("\x1b[48;2;40;65;90m中文\x1b[0m".as_bytes());
        let mut buffer = Buffer::empty(area);
        view.render(&mut buffer, area).unwrap();
        for x in 0..4 {
            assert_eq!(
                buffer[(x, 0)].bg,
                Color::Rgb(40, 65, 90),
                "wide glyph column {x}"
            );
        }
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
    fn theme_updates_existing_text_and_resolves_ansi_without_changing_truecolor() {
        let area = Rect::new(0, 0, 20, 2);
        let mut view = TerminalView::new(20, 2, 1 << 20).unwrap();
        view.write(b"plain \x1b[31mred \x1b[38;2;12;34;56mRGB");
        let dark = crate::theme::builtin("catppuccin").unwrap();
        view.apply_theme(&dark).unwrap();
        let mut buf = Buffer::empty(area);
        view.render(&mut buf, area).unwrap();
        assert_eq!(buf[(0, 0)].fg, dark.color("fg"));
        assert_eq!(buf[(0, 0)].bg, dark.color("bg"));
        assert_eq!(buf[(6, 0)].fg, dark.terminal_palette()[1]);
        assert_eq!(buf[(10, 0)].fg, Color::Rgb(12, 34, 56));
        let light = crate::theme::builtin("catppuccin-latte").unwrap();
        view.apply_theme(&light).unwrap();
        view.render(&mut buf, area).unwrap();
        assert_eq!(buf[(0, 0)].fg, light.color("fg"));
        assert_eq!(buf[(0, 0)].bg, light.color("bg"));
        assert_eq!(buf[(6, 0)].fg, light.terminal_palette()[1]);
        assert_eq!(buf[(10, 0)].fg, Color::Rgb(12, 34, 56));
    }

    #[test]
    fn ascii_edits_and_scrolling_survive_real_backend_redraws_without_duplicates() {
        use ratatui::{backend::CrosstermBackend, TerminalOptions, Viewport};
        use std::io::Write;
        #[derive(Clone, Default)]
        struct Output(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
        impl Write for Output {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let output = Output::default();
        use base64::Engine;
        crossterm::style::force_color_output(true);
        let outer = Rect::new(0, 0, 110, 36);
        let mut terminal = ratatui::Terminal::with_options(
            CrosstermBackend::new(output.clone()),
            TerminalOptions {
                viewport: Viewport::Fixed(outer),
            },
        )
        .unwrap();
        let mut view = TerminalView::new(30, 6, 1 << 20).unwrap();
        let mut host = TerminalView::new(110, 36, 1 << 20).unwrap();
        let mut offset = 0;
        let mut cases: Vec<_> = [
            b"prompt> abcdef".as_slice(),
            b"\x08\x08XY",
            b"\r\x1b[2Kprompt> x",
            b"\r\n0123456789012345678901234567890123456789\r\n",
            b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\n",
            b"\x1b[2;1H\x1b[2Kupdated",
            b"\x1b[2J\x1b[Hclear",
            b"\x1b[H\x1b[2J\x1b[1;2mBD\x1b[22mnormal\x1b[2mDIM\x1b[1mBOTH\x1b[22mplain",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, bytes)| (format!("ASCII edit {i}"), 30, 6, bytes.to_vec()))
        .collect();
        let capture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/nvim-redraw.json")).unwrap();
        for entry in capture["frames"].as_array().unwrap() {
            cases.push((
                entry["label"].as_str().unwrap().to_string(),
                entry["cols"].as_u64().unwrap() as u16,
                entry["rows"].as_u64().unwrap() as u16,
                base64::engine::general_purpose::STANDARD
                    .decode(entry["data"].as_str().unwrap())
                    .unwrap(),
            ));
        }
        for (label, cols, rows, bytes) in cases {
            let area = Rect::new(3, 2, cols, rows);
            view.resize(cols, rows, 8, 16).unwrap();
            for chunk in bytes.chunks(31) {
                view.write(chunk);
            }
            terminal
                .draw(|frame| {
                    view.render(frame.buffer_mut(), area).unwrap();
                })
                .unwrap();
            let data = output.0.borrow();
            host.write(&data[offset..]);
            offset = data.len();
            let mut expected = Buffer::empty(outer);
            let mut actual = Buffer::empty(outer);
            view.render(&mut expected, area).unwrap();
            host.render(&mut actual, outer).unwrap();
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    assert_eq!(
                        actual[(x, y)].symbol(),
                        expected[(x, y)].symbol(),
                        "at {x},{y} after {label}"
                    );
                    if expected[(x, y)].symbol() != " " {
                        assert_eq!(
                            actual[(x, y)].fg,
                            expected[(x, y)].fg,
                            "foreground at {x},{y} after {label}"
                        );
                    }
                    assert_eq!(
                        actual[(x, y)].modifier,
                        expected[(x, y)].modifier,
                        "attributes at {x},{y} after {label}"
                    );
                    assert_eq!(
                        actual[(x, y)].bg,
                        expected[(x, y)].bg,
                        "background at {x},{y} after {label}"
                    );
                }
            }
        }
    }

    #[test]
    fn legacy_key_release_does_not_duplicate_printable_input() {
        let mut view = TerminalView::new(20, 2, 1 << 20).unwrap();
        let press = KeyEvent::new(crossterm::event::KeyCode::Char('a'), KeyModifiers::NONE);
        let mut release = press;
        release.kind = KeyEventKind::Release;
        assert_eq!(view.encode_key(press), Some(b"a".to_vec()));
        assert_eq!(view.encode_key(release), None);
    }

    #[test]
    fn inverse_swaps_foreground_and_background() {
        let cell = CellStyle {
            inverse: true,
            ..CellStyle::default()
        };
        let style = style_of(&cell, Some(Color::White), Some(Color::Black), None);
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
    fn mouse_coordinates_are_cells_not_pixels() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

        let mut view = TerminalView::new(80, 24, 1 << 20).unwrap();
        view.write(b"\x1b[?1000h\x1b[?1006h");
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 12,
            row: 8,
            modifiers: KeyModifiers::NONE,
        };
        let bytes = view.encode_mouse(mouse, (0, 0)).expect("encoded");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("[<0;13;9M"), "{text:?}");
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
