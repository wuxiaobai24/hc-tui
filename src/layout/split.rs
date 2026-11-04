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

    /// 把 `target` 这个叶子换成一个分屏，新 Pane 放在右侧/下侧。
    ///
    /// 返回 `false` 表示树里没有这个 Pane。
    pub fn split_pane(&mut self, target: PaneId, axis: Axis, new: PaneId, ratio: f32) -> bool {
        match self {
            SplitNode::Leaf(id) if *id == target => {
                let existing = SplitNode::Leaf(*id);
                *self = SplitNode::split(axis, ratio, existing, SplitNode::Leaf(new));
                true
            }
            SplitNode::Leaf(_) => false,
            SplitNode::Split { first, second, .. } => {
                first.split_pane(target, axis, new, ratio)
                    || second.split_pane(target, axis, new, ratio)
            }
        }
    }

    /// 移除一个叶子并折叠它的父分屏，把空间还给兄弟。根是它自己时返回 `None`。
    pub fn remove_pane(self, target: PaneId) -> Option<SplitNode> {
        match self {
            SplitNode::Leaf(id) if id == target => None,
            SplitNode::Leaf(_) => Some(self),
            SplitNode::Split {
                axis,
                ratio,
                first,
                second,
            } => match (first.remove_pane(target), second.remove_pane(target)) {
                (Some(first), Some(second)) => Some(SplitNode::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
        }
    }

    /// 调整离 `target` 最近、且轴为 `axis` 的那层分屏的比例。
    ///
    /// `delta` 为正时把 `target` 所在的那一侧变大。返回是否找到可调的层。
    pub fn adjust_ratio(&mut self, target: PaneId, axis: Axis, delta: f32) -> bool {
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Split {
                axis: node_axis,
                ratio,
                first,
                second,
            } => {
                let in_first = first.panes().contains(&target);
                let in_second = second.panes().contains(&target);
                if !in_first && !in_second {
                    return false;
                }
                // 先试更深的一层：最近的分隔线优先。
                let deeper = if in_first {
                    first.adjust_ratio(target, axis, delta)
                } else {
                    second.adjust_ratio(target, axis, delta)
                };
                if deeper {
                    return true;
                }
                if *node_axis != axis {
                    return false;
                }
                let signed = if in_first { delta } else { -delta };
                *ratio = (*ratio + signed).clamp(0.05, 0.95);
                true
            }
        }
    }
}

/// 方向键式的焦点移动：在 `rects` 里找 `from` 指定方向上的最近 Pane。
///
/// 只按几何位置判断，不看树结构：跨层分屏也能正确跳转。
pub fn neighbor(rects: &[(PaneId, Rect)], from: PaneId, direction: Direction) -> Option<PaneId> {
    let current = rects
        .iter()
        .find(|(pane, _)| *pane == from)
        .map(|(_, rect)| *rect)?;
    let mut best: Option<(PaneId, u16, u32)> = None;
    for (pane, rect) in rects {
        if *pane == from {
            continue;
        }
        let (edge, overlap) = match direction {
            Direction::Left => {
                if rect.x + rect.width > current.x {
                    continue;
                }
                let edge = current.x - (rect.x + rect.width);
                (edge, overlap_y(rect, &current))
            }
            Direction::Right => {
                if rect.x < current.x + current.width {
                    continue;
                }
                let edge = rect.x - (current.x + current.width);
                (edge, overlap_y(rect, &current))
            }
            Direction::Up => {
                if rect.y + rect.height > current.y {
                    continue;
                }
                let edge = current.y - (rect.y + rect.height);
                (edge, overlap_x(rect, &current))
            }
            Direction::Down => {
                if rect.y < current.y + current.height {
                    continue;
                }
                let edge = rect.y - (current.y + current.height);
                (edge, overlap_x(rect, &current))
            }
        };
        if overlap == 0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((_, best_edge, best_overlap)) => {
                edge < best_edge || (edge == best_edge && overlap > best_overlap)
            }
        };
        if better {
            best = Some((*pane, edge, overlap));
        }
    }
    best.map(|(pane, _, _)| pane)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

fn overlap_y(a: &Rect, b: &Rect) -> u32 {
    let top = a.y.max(b.y);
    let bottom = (a.y + a.height).min(b.y + b.height);
    u32::from(bottom.saturating_sub(top))
}

fn overlap_x(a: &Rect, b: &Rect) -> u32 {
    let left = a.x.max(b.x);
    let right = (a.x + a.width).min(b.x + b.width);
    u32::from(right.saturating_sub(left))
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
    fn splitting_a_leaf_adds_a_second_pane() {
        let mut tree = SplitNode::leaf(1);
        assert!(tree.split_pane(PaneId(1), Axis::Horizontal, PaneId(2), 0.5));
        assert_eq!(tree.panes(), vec![PaneId(1), PaneId(2)]);
        let rects = tree.layout(Rect::new(0, 0, 80, 24));
        assert_eq!(rects[0].1.width + rects[1].1.width, 80);
    }

    #[test]
    fn removing_a_pane_collapses_its_parent() {
        let tree = SplitNode::split(
            Axis::Horizontal,
            0.5,
            SplitNode::leaf(1),
            SplitNode::split(Axis::Vertical, 0.5, SplitNode::leaf(2), SplitNode::leaf(3)),
        );
        let tree = tree.remove_pane(PaneId(2)).unwrap();
        assert_eq!(tree.panes(), vec![PaneId(1), PaneId(3)]);
        let only = tree.remove_pane(PaneId(3)).unwrap();
        assert_eq!(only.panes(), vec![PaneId(1)]);
        assert!(only.remove_pane(PaneId(1)).is_none());
    }

    #[test]
    fn ratio_adjustment_hits_the_nearest_matching_split() {
        let mut tree = SplitNode::split(
            Axis::Horizontal,
            0.5,
            SplitNode::leaf(1),
            SplitNode::split(
                Axis::Horizontal,
                0.5,
                SplitNode::leaf(2),
                SplitNode::leaf(3),
            ),
        );
        assert!(tree.adjust_ratio(PaneId(2), Axis::Horizontal, 0.1));
        match &tree {
            SplitNode::Split { ratio, second, .. } => {
                assert_eq!(*ratio, 0.5);
                match second.as_ref() {
                    SplitNode::Split { ratio, .. } => assert!((ratio - 0.6).abs() < 1e-6),
                    _ => panic!("expected the inner split"),
                }
            }
            _ => panic!("expected a split"),
        }
        assert!(!tree.adjust_ratio(PaneId(1), Axis::Vertical, 0.1));
    }

    #[test]
    fn focus_neighbors_follow_geometry() {
        let tree = SplitNode::split(
            Axis::Horizontal,
            0.5,
            SplitNode::leaf(1),
            SplitNode::split(Axis::Vertical, 0.5, SplitNode::leaf(2), SplitNode::leaf(3)),
        );
        let rects = tree.layout(Rect::new(0, 0, 80, 24));
        assert_eq!(
            neighbor(&rects, PaneId(1), Direction::Right),
            Some(PaneId(2))
        );
        assert_eq!(
            neighbor(&rects, PaneId(2), Direction::Down),
            Some(PaneId(3))
        );
        assert_eq!(
            neighbor(&rects, PaneId(3), Direction::Left),
            Some(PaneId(1))
        );
        assert_eq!(neighbor(&rects, PaneId(1), Direction::Left), None);
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
