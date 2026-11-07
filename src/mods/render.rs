//! 表面绘制顺序、帧预算与覆盖层矩形。
//!
//! 每帧按 z 序绘制：region / pane 表面在最下，dialog 压暗内容区，popup 与
//! window 在最上。单表面 8 ms、全部表面 32 ms；超预算的表面保留上一帧并
//! 记账，下一帧优先。

use std::time::{Duration, Instant};

use ratatui_core::layout::Rect;
use ratatui_core::style::Style;
use ratatui_core::terminal::Frame;
use ratatui_widgets::paragraph::Paragraph;

use super::{ModSurface, SurfacePlace, SurfaceState};

/// 单表面一帧的预算。
pub const SURFACE_BUDGET: Duration = Duration::from_millis(8);
/// 全部表面一帧的总预算。
pub const FRAME_BUDGET: Duration = Duration::from_millis(32);

/// 一帧的绘制入口：按 z 序绘制全部表面。
///
/// dialog 先压暗内容区再绘制；超出帧预算的表面保留上一帧并记账。
pub fn draw_surfaces(app: &mut crate::app::App, frame: &mut Frame) {
    if app.config.no_mods || app.mods.surfaces.is_empty() {
        return;
    }
    let mut order: Vec<(u8, String, Rect)> = Vec::new();
    let mut areas: Vec<(String, Rect)> = Vec::new();
    for surface in &app.mods.surfaces {
        let Some(rect) = surface_rect(app, surface) else {
            continue;
        };
        areas.push((surface.id.clone(), rect));
        order.push((z_order(surface.place), surface.id.clone(), rect));
    }
    for (id, rect) in areas {
        if let Some(surface) = app.mods.surface_mut(&id) {
            surface.area = rect;
        }
    }
    order.sort_by_key(|(z, _, _)| *z);
    let mut spent = Duration::ZERO;
    for (_, id, rect) in order {
        if spent >= FRAME_BUDGET {
            break;
        }
        let Some(surface) = app.mods.surface_mut(&id) else {
            continue;
        };
        if surface.place == SurfacePlace::Dialog {
            let theme = app.config.theme.clone();
            crate::ui::clear_area(frame, app.content, &theme);
        }
        if !matches!(surface.state, SurfaceState::Loading | SurfaceState::Running) {
            if let Some(error) = surface.last_error.clone() {
                let area = Rect::new(rect.x, rect.y, rect.width, rect.height.min(1));
                frame.render_widget(
                    Paragraph::new(format!(" {error} "))
                        .style(Style::default().fg(app.config.theme.color("agent_error"))),
                    area,
                );
            }
            continue;
        }
        let mod_id = surface.mod_id.clone();
        let started = Instant::now();
        surface.view.render(frame, rect);
        let elapsed = started.elapsed();
        // 引擎回调收集的异常在本帧之后计数；连续三次进入 Failed。
        let errors: Vec<String> = {
            let mut errors = surface
                .errors
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            std::mem::take(&mut *errors)
        };
        spent += elapsed;
        if elapsed > SURFACE_BUDGET {
            // 记账：超出预算的这一帧未刷新的提交保留到下一帧。
            surface.last_error = Some(format!("frame budget {elapsed:?}"));
        }
        if !errors.is_empty() {
            app.record_mod_errors(&id, &mod_id, errors);
        }
    }
}

fn z_order(place: SurfacePlace) -> u8 {
    match place {
        SurfacePlace::Region | SurfacePlace::Pane => 0,
        SurfacePlace::Dialog => 1,
        SurfacePlace::Popup => 2,
        SurfacePlace::Window => 3,
    }
}

/// 表面本帧的矩形；region 用外框布局，pane 用槽位，覆盖层用自身位置。
fn surface_rect(app: &crate::app::App, surface: &ModSurface) -> Option<Rect> {
    let rect = match surface.place {
        SurfacePlace::Region => app
            .mod_regions
            .iter()
            .find(|region| region.id == surface.mod_id && !region.folded)
            .map(|region| region.rect),
        SurfacePlace::Pane => app
            .slots
            .iter()
            .find(|slot| Some(&slot.pane.to_string()) == surface.key.as_ref())
            .map(|slot| slot.inner),
        SurfacePlace::Dialog | SurfacePlace::Popup | SurfacePlace::Window => Some(surface.area),
    }?;
    (rect.width > 0 && rect.height > 0).then_some(rect)
}

/// dialog 居中并占内容区大部分的矩形。
pub fn dialog_rect(content: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(content.width.saturating_sub(4)).max(20);
    let height = height.min(content.height.saturating_sub(2)).max(5);
    Rect {
        x: content.x + (content.width.saturating_sub(width)) / 2,
        y: content.y + (content.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// popup 靠近给定锚点的矩形，越界时收回内容区。
pub fn popup_rect(content: Rect, anchor: (u16, u16), width: u16, height: u16) -> Rect {
    let width = width.min(content.width).max(10);
    let height = height.min(content.height).max(3);
    let x = (anchor.0 + 1).min(content.x + content.width.saturating_sub(width));
    let y = (anchor.1 + 1).min(content.y + content.height.saturating_sub(height));
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_is_centered_and_clamped() {
        let content = Rect::new(4, 2, 80, 24);
        let rect = dialog_rect(content, 60, 10);
        assert_eq!(rect.x, 4 + 10);
        assert_eq!(rect.y, 2 + 7);
        // 内容区太窄时收窄，不越界。
        let rect = dialog_rect(Rect::new(0, 0, 30, 10), 60, 10);
        assert_eq!(rect.width, 26);
        assert!(rect.right() <= 30);
    }

    #[test]
    fn popup_stays_inside_content() {
        let content = Rect::new(0, 0, 60, 20);
        let rect = popup_rect(content, (58, 18), 20, 8);
        assert!(rect.right() <= 60);
        assert!(rect.bottom() <= 20);
        assert_eq!(rect.width, 20);
    }

    #[test]
    fn overlays_draw_above_regions() {
        assert!(z_order(SurfacePlace::Window) > z_order(SurfacePlace::Popup));
        assert!(z_order(SurfacePlace::Popup) > z_order(SurfacePlace::Dialog));
        assert_eq!(
            z_order(SurfacePlace::Dialog),
            z_order(SurfacePlace::Region) + 1
        );
    }
}
