//! Host Kitty graphics output. Image ids belong to this viewer and are removed on exit.
use std::io::{self, Write};
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine};
use ratatui::layout::Rect;

use super::view::PlacementImage;
use crate::app::App;

pub fn native_enabled() -> bool {
    match std::env::var("HC_TUI_GRAPHICS").as_deref() {
        Ok("kitty") => true,
        Ok("blocks") => false,
        _ => {
            std::env::var_os("TMUX").is_none()
                && (std::env::var_os("KITTY_WINDOW_ID").is_some()
                    || std::env::var("TERM").unwrap_or_default().contains("kitty")
                    || matches!(
                        std::env::var("TERM_PROGRAM").as_deref(),
                        Ok("ghostty" | "herdr" | "WezTerm")
                    ))
        }
    }
}

#[derive(Default)]
pub struct Graphics {
    previous: Vec<(Rect, PlacementImage)>,
    count: usize,
}

impl Graphics {
    pub fn draw(&mut self, app: &mut App, output: &mut impl Write) -> io::Result<()> {
        if !native_enabled() {
            return Ok(());
        }
        let obscured = app.chat.is_some()
            || app.reader.is_some()
            || app.help.is_some()
            || app.settings.is_some()
            || app.goto.is_some()
            || app.chooser.is_some()
            || app.prompt.is_some()
            || app.switcher
            || app.popup.is_some()
            || app.toast.is_some()
            || app.copy.is_some()
            || app.drag.is_some();
        let mut images = Vec::new();
        if !obscured {
            for (index, slot) in app.slots.iter().enumerate() {
                if slot.collapsed {
                    continue;
                }
                let mut tiles = vec![slot.inner];
                for blocker in app.slots.iter().skip(index + 1) {
                    tiles = tiles
                        .into_iter()
                        .flat_map(|area| subtract(area, blocker.outer))
                        .collect();
                }
                if let Some(pane) = app.panes.get_mut(&slot.pane) {
                    for image in pane.view.placement_images() {
                        for tile in &tiles {
                            let mut image = image.clone();
                            image.col += i32::from(slot.inner.x) - i32::from(tile.x);
                            image.row += i32::from(slot.inner.y) - i32::from(tile.y);
                            images.push((*tile, image));
                        }
                    }
                }
            }
        }
        self.render(images, output)
    }

    fn render(
        &mut self,
        images: Vec<(Rect, PlacementImage)>,
        output: &mut impl Write,
    ) -> io::Result<()> {
        let unchanged = images.len() == self.previous.len()
            && images.iter().zip(&self.previous).all(|((a, x), (b, y))| {
                a == b
                    && x.col == y.col
                    && x.row == y.row
                    && x.cols == y.cols
                    && x.rows == y.rows
                    && x.width == y.width
                    && x.height == y.height
                    && x.source == y.source
                    && x.offset == y.offset
                    && x.z == y.z
                    && Arc::ptr_eq(&x.rgba, &y.rgba)
            });
        if unchanged {
            return Ok(());
        }
        self.clear(output)?;
        write!(output, "\x1b7")?;
        for (area, image) in &images {
            let Some((rect, pixels, width, height)) = clipped(*area, image) else {
                continue;
            };
            let id = 0x70000000 + self.count as u32;
            self.count += 1;
            write!(output, "\x1b[{};{}H", rect.y + 1, rect.x + 1)?;
            let encoded = STANDARD.encode(pixels);
            let chunks: Vec<_> = encoded.as_bytes().chunks(4096).collect();
            for (index, chunk) in chunks.iter().enumerate() {
                let more = u8::from(index + 1 < chunks.len());
                if index == 0 {
                    write!(output, "\x1b_Ga=T,f=32,s={width},v={height},i={id},c={},r={},C=1,q=2,z={},X={},Y={},m={more};", rect.width, rect.height, image.z, if image.col>=0{image.offset[0]}else{0},if image.row>=0{image.offset[1]}else{0})?;
                } else {
                    write!(output, "\x1b_Gm={more};")?;
                }
                output.write_all(chunk)?;
                output.write_all(b"\x1b\\")?;
            }
        }
        output.write_all(b"\x1b8")?;
        output.flush()?;
        self.previous = images;
        Ok(())
    }

    pub fn clear(&mut self, output: &mut impl Write) -> io::Result<()> {
        for index in 0..self.count {
            write!(
                output,
                "\x1b_Ga=d,d=I,i={},q=2\x1b\\",
                0x70000000 + index as u32
            )?;
        }
        self.count = 0;
        self.previous.clear();
        Ok(())
    }
}

fn clipped(area: Rect, image: &PlacementImage) -> Option<(Rect, Vec<u8>, u32, u32)> {
    if image.cols == 0 || image.rows == 0 || image.width == 0 || image.height == 0 {
        return None;
    }
    let left = image.col.max(0) as u32;
    let top = image.row.max(0) as u32;
    let right =
        (i64::from(image.col) + i64::from(image.cols)).clamp(0, i64::from(area.width)) as u32;
    let bottom =
        (i64::from(image.row) + i64::from(image.rows)).clamp(0, i64::from(area.height)) as u32;
    if right <= left || bottom <= top {
        return None;
    }
    let [crop_x, crop_y, crop_w, crop_h] = image.source;
    let crop_w = if crop_w == 0 {
        image.width.saturating_sub(crop_x)
    } else {
        crop_w.min(image.width.saturating_sub(crop_x))
    };
    let crop_h = if crop_h == 0 {
        image.height.saturating_sub(crop_y)
    } else {
        crop_h.min(image.height.saturating_sub(crop_y))
    };
    if crop_w == 0 || crop_h == 0 {
        return None;
    }
    let source_x = crop_x
        + ((i64::from(left) - i64::from(image.col)) as u64 * u64::from(crop_w)
            / u64::from(image.cols)) as u32;
    let source_y = crop_y
        + ((i64::from(top) - i64::from(image.row)) as u64 * u64::from(crop_h)
            / u64::from(image.rows)) as u32;
    let width = ((u64::from(right - left) * u64::from(crop_w) / u64::from(image.cols)) as u32)
        .max(1)
        .min(crop_x + crop_w - source_x);
    let height = ((u64::from(bottom - top) * u64::from(crop_h) / u64::from(image.rows)) as u32)
        .max(1)
        .min(crop_y + crop_h - source_y);
    let mut pixels = Vec::new();
    for row in source_y..source_y + height {
        let start = (u64::from(row) * u64::from(image.width) + u64::from(source_x)) as usize * 4;
        pixels.extend_from_slice(image.rgba.get(start..start + width as usize * 4)?);
    }
    Some((
        Rect::new(
            area.x + left as u16,
            area.y + top as u16,
            (right - left) as u16,
            (bottom - top) as u16,
        ),
        pixels,
        width,
        height,
    ))
}

fn subtract(area: Rect, cover: Rect) -> Vec<Rect> {
    let overlap = area.intersection(cover);
    if overlap.is_empty() {
        return vec![area];
    }
    let mut out = Vec::new();
    if overlap.y > area.y {
        out.push(Rect::new(area.x, area.y, area.width, overlap.y - area.y));
    }
    if overlap.bottom() < area.bottom() {
        out.push(Rect::new(
            area.x,
            overlap.bottom(),
            area.width,
            area.bottom() - overlap.bottom(),
        ));
    }
    if overlap.x > area.x {
        out.push(Rect::new(
            area.x,
            overlap.y,
            overlap.x - area.x,
            overlap.height,
        ));
    }
    if overlap.right() < area.right() {
        out.push(Rect::new(
            overlap.right(),
            overlap.y,
            area.right() - overlap.right(),
            overlap.height,
        ));
    }
    out
}

impl Drop for Graphics {
    fn drop(&mut self) {
        let _ = self.clear(&mut io::stdout());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_crop_is_respected_and_occlusion_tiles_do_not_overlap() {
        let image = PlacementImage {
            col: 0,
            row: 0,
            cols: 1,
            rows: 1,
            z: 0,
            width: 2,
            height: 2,
            rgba: Arc::new((0..16).collect()),
            source: [1, 1, 1, 1],
            offset: [0, 0],
        };
        let (_, pixels, w, h) = clipped(Rect::new(0, 0, 1, 1), &image).unwrap();
        assert_eq!(pixels, vec![12, 13, 14, 15]);
        assert_eq!((w, h), (1, 1));
        let tiles = subtract(Rect::new(0, 0, 10, 10), Rect::new(2, 2, 4, 4));
        assert_eq!(
            tiles
                .iter()
                .map(|r| u32::from(r.width) * u32::from(r.height))
                .sum::<u32>(),
            84
        );
        for tile in tiles {
            assert!(tile.intersection(Rect::new(2, 2, 4, 4)).is_empty());
        }
    }

    #[test]
    fn host_protocol_roundtrip_and_unchanged_frames_are_cached() {
        let image = PlacementImage {
            col: 0,
            row: 0,
            cols: 2,
            rows: 2,
            z: 0,
            width: 2,
            height: 2,
            rgba: Arc::new([255, 0, 0, 255].repeat(4)),
            source: [0, 0, 2, 2],
            offset: [0, 0],
        };
        let images = vec![(Rect::new(3, 4, 10, 10), image)];
        let mut graphics = Graphics::default();
        let mut output = Vec::new();
        graphics.render(images.clone(), &mut output).unwrap();
        let mut host = crate::term::view::TerminalView::new(30, 20, 1 << 20).unwrap();
        host.write(&output);
        let placements = host.placement_images();
        assert_eq!(placements.len(), 1);
        assert_eq!((placements[0].col, placements[0].row), (3, 4));
        assert_eq!(&*placements[0].rgba, &[255, 0, 0, 255].repeat(4));
        output.clear();
        graphics.render(images, &mut output).unwrap();
        assert!(output.is_empty());
        graphics.render(Vec::new(), &mut output).unwrap();
        host.write(&output);
        assert!(host.placement_images().is_empty());
    }

    #[test]
    fn clips_scrolled_image_to_pane() {
        let image = PlacementImage {
            col: -1,
            row: -1,
            cols: 2,
            rows: 2,
            z: 0,
            width: 2,
            height: 2,
            rgba: Arc::new((0..16).collect()),
            source: [0, 0, 2, 2],
            offset: [0, 0],
        };
        let (rect, pixels, w, h) = clipped(Rect::new(5, 7, 10, 10), &image).unwrap();
        assert_eq!(rect, Rect::new(5, 7, 1, 1));
        assert_eq!((w, h), (1, 1));
        assert_eq!(pixels, vec![12, 13, 14, 15]);
        assert!(clipped(Rect::new(0, 0, 0, 0), &image).is_none());
    }
    #[test]
    fn deletes_only_viewer_owned_images() {
        let mut graphics = Graphics::default();
        graphics.count = 2;
        let mut output = Vec::new();
        graphics.clear(&mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("d=I,i=1879048192"));
        assert!(text.contains("d=I,i=1879048193"));
        assert_eq!(graphics.count, 0);
    }
}
