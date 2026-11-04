//! BSP 分屏布局。
//!
//! 轴的含义：
//! - [`Axis::Horizontal`]：左右分（竖分隔线）
//! - [`Axis::Vertical`]：上下分（横分隔线）

use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SplitNode {
    Leaf(PaneId),
    Split {
        axis: Axis,
        /// 第一块占的比例，0.0–1.0。
        ratio: f32,
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}

impl SplitNode {
    pub fn leaf(id: u32) -> Self {
        SplitNode::Leaf(PaneId(id))
    }

    pub fn split(axis: Axis, ratio: f32, first: SplitNode, second: SplitNode) -> Self {
        SplitNode::Split {
            axis,
            ratio: ratio.clamp(0.05, 0.95),
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// 收集所有叶子及其矩形，按先序。
    pub fn layout(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.collect(area, &mut out);
        out
    }

    fn collect(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            SplitNode::Leaf(id) => out.push((*id, area)),
            SplitNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (a, b) = split_rect(area, *axis, *ratio);
                first.collect(a, out);
                second.collect(b, out);
            }
        }
    }

    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_panes(&mut out);
        out
    }

    fn collect_panes(&self, out: &mut Vec<PaneId>) {
        match self {
            SplitNode::Leaf(id) => out.push(*id),
            SplitNode::Split { first, second, .. } => {
                first.collect_panes(out);
                second.collect_panes(out);
            }
        }
    }
}

fn split_rect(area: Rect, axis: Axis, ratio: f32) -> (Rect, Rect) {
    match axis {
        Axis::Horizontal => {
            if area.width < 2 {
                return (area, Rect::new(area.x, area.y, 0, area.height));
            }
            let first_width = ((area.width as f32) * ratio).round() as u16;
            let first_width = first_width.clamp(1, area.width - 1);
            (
                Rect::new(area.x, area.y, first_width, area.height),
                Rect::new(
                    area.x + first_width,
                    area.y,
                    area.width - first_width,
                    area.height,
                ),
            )
        }
        Axis::Vertical => {
            if area.height < 2 {
                return (area, Rect::new(area.x, area.y, area.width, 0));
            }
            let first_height = ((area.height as f32) * ratio).round() as u16;
            let first_height = first_height.clamp(1, area.height - 1);
            (
                Rect::new(area.x, area.y, area.width, first_height),
                Rect::new(
                    area.x,
                    area.y + first_height,
                    area.width,
                    area.height - first_height,
                ),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_leaf_fills_the_area() {
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(SplitNode::leaf(1).layout(area), vec![(PaneId(1), area)]);
    }

    #[test]
    fn horizontal_split_is_side_by_side() {
        let node = SplitNode::split(
            Axis::Horizontal,
            0.5,
            SplitNode::leaf(1),
            SplitNode::leaf(2),
        );
        let rects = node.layout(Rect::new(0, 0, 80, 24));
        assert_eq!(rects[0], (PaneId(1), Rect::new(0, 0, 40, 24)));
        assert_eq!(rects[1], (PaneId(2), Rect::new(40, 0, 40, 24)));
    }

    #[test]
    fn vertical_split_stacks() {
        let node = SplitNode::split(Axis::Vertical, 0.25, SplitNode::leaf(1), SplitNode::leaf(2));
        let rects = node.layout(Rect::new(0, 0, 80, 24));
        assert_eq!(rects[0], (PaneId(1), Rect::new(0, 0, 80, 6)));
        assert_eq!(rects[1], (PaneId(2), Rect::new(0, 6, 80, 18)));
    }

    #[test]
    fn nested_splits_keep_preorder() {
        let inner = SplitNode::split(Axis::Vertical, 0.5, SplitNode::leaf(2), SplitNode::leaf(3));
        let node = SplitNode::split(Axis::Horizontal, 0.5, SplitNode::leaf(1), inner);
        let rects = node.layout(Rect::new(0, 0, 80, 24));
        assert_eq!(
            rects.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![PaneId(1), PaneId(2), PaneId(3)]
        );
        assert_eq!(rects[1].1, Rect::new(40, 0, 40, 12));
        assert_eq!(rects[2].1, Rect::new(40, 12, 40, 12));
    }

    #[test]
    fn tiny_areas_do_not_underflow() {
        let node = SplitNode::split(
            Axis::Horizontal,
            0.5,
            SplitNode::leaf(1),
            SplitNode::leaf(2),
        );
        let rects = node.layout(Rect::new(0, 0, 1, 1));
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[1].1.width, 0);
    }
}
