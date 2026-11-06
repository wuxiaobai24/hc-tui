use super::*;
use crate::layout::split::Divider;

#[derive(Clone)]
pub(super) enum LayoutDrag {
    Sidebar,
    Split {
        workspace: u32,
        tab: u32,
        content: Rect,
        panes: Vec<PaneId>,
        divider: Divider,
        start: (u16, u16),
    },
}

impl App {
    pub(super) fn on_layout_mouse(&mut self, mouse: MouseEvent) -> bool {
        if let Some(drag) = self.layout_drag.clone() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
                    match drag {
                        LayoutDrag::Sidebar => {
                            let max = self
                                .config
                                .sidebar_max_width
                                .min(self.content.right().saturating_sub(41));
                            let min = self.config.sidebar_min_width.min(max);
                            self.sidebar_width = mouse
                                .column
                                .saturating_sub(self.sidebar_area.x)
                                .saturating_add(1)
                                .clamp(min, max);
                            self.dirty = true;
                        }
                        LayoutDrag::Split {
                            workspace,
                            tab,
                            content,
                            panes,
                            divider,
                            start,
                        } => {
                            let valid = self.workspace().is_some_and(|w| w.id == workspace)
                                && self
                                    .tab()
                                    .is_some_and(|t| t.id == tab && t.tree.panes() == panes)
                                && self.content == content
                                && !self.zoom;
                            if !valid {
                                self.layout_drag = None;
                                return true;
                            }
                            let dx = i32::from(mouse.column) - i32::from(start.0);
                            let dy = i32::from(mouse.row) - i32::from(start.1);
                            if self
                                .workspace_mut()
                                .and_then(Workspace::tab_mut)
                                .is_some_and(|t| t.tree.drag_divider(&divider, dx, dy))
                            {
                                self.mark_layout();
                            }
                        }
                    }
                    if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
                        if matches!(self.layout_drag, Some(LayoutDrag::Sidebar)) {
                            self.config.sidebar_width = self.sidebar_width;
                            if let Err(error) = crate::config::write_setting(
                                &self.config.config_path,
                                "ui.sidebar_width",
                                &self.sidebar_width.to_string(),
                            ) {
                                self.note = format!("sidebar resized; {error}");
                            }
                        }
                        self.layout_drag = None;
                    }
                    return true;
                }
                MouseEventKind::Down(_) => self.layout_drag = None,
                _ => return true,
            }
        }
        if self.mobile || mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return false;
        }
        if self.sidebar
            && self.sidebar_area.width > 0
            && contains(self.sidebar_area, mouse.column, mouse.row)
            && mouse.column == self.sidebar_area.right().saturating_sub(1)
        {
            self.drag = None;
            self.layout_drag = Some(LayoutDrag::Sidebar);
            return true;
        }
        if self.zoom || self.copy.is_some() || self.floating_drag.is_some() {
            return false;
        }
        // Border hits belong to the layout; terminal contents and floating windows keep their input.
        let border = self
            .slots
            .iter()
            .rev()
            .find(|s| contains(s.outer, mouse.column, mouse.row))
            .is_some_and(|s| {
                !s.overlay && !s.collapsed && !contains(s.inner, mouse.column, mouse.row)
            });
        if !border {
            return false;
        }
        let Some(tab) = self.tab() else {
            return false;
        };
        let Some(divider) = tab.tree.divider_at(self.content, mouse.column, mouse.row) else {
            return false;
        };
        self.layout_drag = Some(LayoutDrag::Split {
            workspace: self.workspace().unwrap().id,
            tab: tab.id,
            content: self.content,
            panes: tab.tree.panes(),
            divider,
            start: (mouse.column, mouse.row),
        });
        self.drag = None;
        true
    }
}
