//! BSP 分屏布局。
//!
//! 轴的含义：
//! - [`Axis::Horizontal`]：左右分（竖分隔线）
//! - [`Axis::Vertical`]：上下分（横分隔线）

use ratatui_core::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    pub fn other(self) -> Self {
        match self {
            Axis::Horizontal => Axis::Vertical,
            Axis::Vertical => Axis::Horizontal,
        }
    }
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
    /// 纵向堆叠。`active` 那个 Pane 占掉标题行以下的高度，其余 Pane 各自只占一行标题。
    Stack {
        panes: Vec<PaneId>,
        active: usize,
    },
}

/// 一个 Pane 在某一帧里的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub pane: PaneId,
    pub rect: Rect,
    /// 只占一行标题。终端尺寸沿用上次展开时的值，不跟着这条矩形缩。
    pub collapsed: bool,
}

/// 一次分隔线拖动的目标；路径在按下时固定，拖过其他 Pane 不会换目标。
#[derive(Debug, Clone)]
pub struct Divider {
    path: Vec<bool>,
    axis: Axis,
    area: Rect,
    ratio: f32,
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

    pub fn stack(panes: Vec<PaneId>, active: usize) -> Self {
        let active = active.min(panes.len().saturating_sub(1));
        SplitNode::Stack { panes, active }
    }

    /// 收集所有叶子及其矩形，按先序。
    pub fn layout(&self, area: Rect) -> Vec<Placement> {
        let mut out = Vec::new();
        self.collect(area, &mut out);
        out
    }

    fn collect(&self, area: Rect, out: &mut Vec<Placement>) {
        match self {
            SplitNode::Leaf(id) => out.push(Placement {
                pane: *id,
                rect: area,
                collapsed: false,
            }),
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
            SplitNode::Stack { panes, active } => {
                if panes.is_empty() {
                    return;
                }
                let active = (*active).min(panes.len() - 1);
                // 标题行全部集中在顶部，展开的 Pane 占剩下的高度。
                let titles = (panes.len() as u16 - 1).min(area.height.saturating_sub(1));
                let expanded = Rect::new(area.x, area.y + titles, area.width, area.height - titles);
                let mut row = area.y;
                for (index, pane) in panes.iter().enumerate() {
                    if index == active {
                        out.push(Placement {
                            pane: *pane,
                            rect: expanded,
                            collapsed: false,
                        });
                        continue;
                    }
                    let height = u16::from(row < area.y + titles);
                    out.push(Placement {
                        pane: *pane,
                        rect: Rect::new(area.x, row, area.width, height),
                        collapsed: true,
                    });
                    row += height;
                }
            }
        }
    }

    /// 命中分隔线的两侧边缘；嵌套分隔线优先于祖先。
    pub fn divider_at(&self, area: Rect, column: u16, row: u16) -> Option<Divider> {
        if !area.contains((column, row).into()) {
            return None;
        }
        let Self::Split {
            axis,
            ratio,
            first,
            second,
        } = self
        else {
            return None;
        };
        let (a, b) = split_rect(area, *axis, *ratio);
        for (child, rect, side) in [(first, a, false), (second, b, true)] {
            if let Some(mut divider) = child.divider_at(rect, column, row) {
                divider.path.insert(0, side);
                return Some(divider);
            }
        }
        let (pointer, boundary) = match axis {
            Axis::Horizontal => (column, b.x),
            Axis::Vertical => (row, b.y),
        };
        (pointer == boundary || pointer.checked_add(1) == Some(boundary)).then(|| Divider {
            path: Vec::new(),
            axis: *axis,
            area,
            ratio: *ratio,
        })
    }

    pub fn drag_divider(&mut self, divider: &Divider, dx: i32, dy: i32) -> bool {
        let mut node = self;
        for second in &divider.path {
            let Self::Split {
                first,
                second: other,
                ..
            } = node
            else {
                return false;
            };
            node = if *second { other } else { first };
        }
        let Self::Split { axis, ratio, .. } = node else {
            return false;
        };
        if *axis != divider.axis {
            return false;
        }
        let (delta, extent) = match axis {
            Axis::Horizontal => (dx, divider.area.width),
            Axis::Vertical => (dy, divider.area.height),
        };
        if extent < 2 {
            return false;
        }
        let next = (divider.ratio + delta as f32 / f32::from(extent)).clamp(0.05, 0.95);
        if *ratio == next {
            return false;
        }
        *ratio = next;
        true
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
            SplitNode::Stack { panes, .. } => out.extend(panes.iter().copied()),
        }
    }

    fn in_a_stack(&self, target: PaneId) -> bool {
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Split { .. } => false,
            SplitNode::Stack { panes, .. } => panes.contains(&target),
        }
    }

    /// 把一个 Stack 里 `target` 那一项切成展开项。不在 Stack 里返回 `false`。
    pub fn activate_in_stack(&mut self, target: PaneId) -> bool {
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Split { first, second, .. } => {
                first.activate_in_stack(target) || second.activate_in_stack(target)
            }
            SplitNode::Stack { panes, active } => {
                match panes.iter().position(|pane| *pane == target) {
                    Some(index) if *active != index => {
                        *active = index;
                        true
                    }
                    _ => false,
                }
            }
        }
    }

    /// 把 `target` 这个叶子换成一个分屏，新 Pane 放在右侧/下侧。
    ///
    /// 返回 `false` 表示树里没有这个 Pane。
    pub fn split_pane(&mut self, target: PaneId, axis: Axis, new: PaneId, ratio: f32) -> bool {
        // Stack 里的 Pane 不能就地分屏：整个 Stack 一起和新的 Pane 分左右/上下。
        if self.in_a_stack(target) {
            let existing = std::mem::replace(self, SplitNode::Leaf(new));
            *self = SplitNode::split(axis, ratio, existing, SplitNode::Leaf(new));
            return true;
        }
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
            SplitNode::Stack { .. } => false,
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
            SplitNode::Stack {
                mut panes,
                mut active,
            } => {
                let index = match panes.iter().position(|pane| *pane == target) {
                    Some(index) => index,
                    None => return Some(SplitNode::Stack { panes, active }),
                };
                panes.remove(index);
                if index < active {
                    active -= 1;
                }
                match panes.len() {
                    0 => None,
                    1 => Some(SplitNode::Leaf(panes[0])),
                    _ => Some(SplitNode::stack(panes, active)),
                }
            }
        }
    }

    /// 调整离 `target` 最近、且轴为 `axis` 的那层分屏的比例。
    ///
    /// `delta` 为正时把 `target` 所在的那一侧变大。返回是否找到可调的层。
    pub fn adjust_ratio(&mut self, target: PaneId, axis: Axis, delta: f32) -> bool {
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Stack { .. } => false,
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
    neighbor_by(rects, from, direction)
}

/// 同 [`neighbor`]，目标类型由调用方决定（Pane 或模块区域）。
pub fn neighbor_by<T: PartialEq + Copy>(
    rects: &[(T, Rect)],
    from: T,
    direction: Direction,
) -> Option<T> {
    let current = rects
        .iter()
        .find(|(pane, _)| *pane == from)
        .map(|(_, rect)| *rect)?;
    let mut best: Option<(T, u16, u32)> = None;
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

    fn rects(node: &SplitNode, area: Rect) -> Vec<(PaneId, Rect)> {
        node.layout(area)
            .into_iter()
            .map(|placement| (placement.pane, placement.rect))
            .collect()
    }

    #[test]
    fn a_single_leaf_fills_the_area() {
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(rects(&SplitNode::leaf(1), area), vec![(PaneId(1), area)]);
    }

    #[test]
    fn horizontal_split_is_side_by_side() {
        let node = SplitNode::split(
            Axis::Horizontal,
            0.5,
            SplitNode::leaf(1),
            SplitNode::leaf(2),
        );
        let rects = rects(&node, Rect::new(0, 0, 80, 24));
        assert_eq!(rects[0], (PaneId(1), Rect::new(0, 0, 40, 24)));
        assert_eq!(rects[1], (PaneId(2), Rect::new(40, 0, 40, 24)));
    }

    #[test]
    fn vertical_split_stacks() {
        let node = SplitNode::split(Axis::Vertical, 0.25, SplitNode::leaf(1), SplitNode::leaf(2));
        let rects = rects(&node, Rect::new(0, 0, 80, 24));
        assert_eq!(rects[0], (PaneId(1), Rect::new(0, 0, 80, 6)));
        assert_eq!(rects[1], (PaneId(2), Rect::new(0, 6, 80, 18)));
    }

    #[test]
    fn nested_splits_keep_preorder() {
        let inner = SplitNode::split(Axis::Vertical, 0.5, SplitNode::leaf(2), SplitNode::leaf(3));
        let node = SplitNode::split(Axis::Horizontal, 0.5, SplitNode::leaf(1), inner);
        let rects = rects(&node, Rect::new(0, 0, 80, 24));
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
        let rects = rects(&tree, Rect::new(0, 0, 80, 24));
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
        let rects = rects(&tree, Rect::new(0, 0, 80, 24));
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
        let rects = rects(&node, Rect::new(0, 0, 1, 1));
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[1].1.width, 0);
    }

    #[test]
    fn a_stack_puts_the_titles_on_top_of_the_expanded_pane() {
        let tree = SplitNode::stack(vec![PaneId(1), PaneId(2), PaneId(3)], 1);
        let placements = tree.layout(Rect::new(0, 0, 80, 24));
        assert_eq!(placements.len(), 3);
        assert_eq!(
            placements[0],
            Placement {
                pane: PaneId(1),
                rect: Rect::new(0, 0, 80, 1),
                collapsed: true,
            }
        );
        assert_eq!(
            placements[1],
            Placement {
                pane: PaneId(2),
                rect: Rect::new(0, 2, 80, 22),
                collapsed: false,
            }
        );
        assert_eq!(
            placements[2],
            Placement {
                pane: PaneId(3),
                rect: Rect::new(0, 1, 80, 1),
                collapsed: true,
            }
        );
    }

    #[test]
    fn a_stack_shorter_than_its_titles_drops_the_extra_rows() {
        let tree = SplitNode::stack(vec![PaneId(1), PaneId(2), PaneId(3)], 0);
        let placements = tree.layout(Rect::new(0, 0, 40, 2));
        assert_eq!(
            placements[0],
            Placement {
                pane: PaneId(1),
                rect: Rect::new(0, 1, 40, 1),
                collapsed: false,
            }
        );
        assert_eq!(
            placements[1],
            Placement {
                pane: PaneId(2),
                rect: Rect::new(0, 0, 40, 1),
                collapsed: true,
            }
        );
        assert_eq!(placements[2].rect.height, 0);
    }

    #[test]
    fn activating_a_stacked_pane_expands_it() {
        let mut tree = SplitNode::stack(vec![PaneId(1), PaneId(2)], 0);
        assert!(tree.activate_in_stack(PaneId(2)));
        assert!(!tree.activate_in_stack(PaneId(2)));
        match &tree {
            SplitNode::Stack { active, .. } => assert_eq!(*active, 1),
            _ => panic!("expected a stack"),
        }
    }

    #[test]
    fn removing_a_stacked_pane_folds_the_stack_back() {
        let tree = SplitNode::stack(vec![PaneId(1), PaneId(2), PaneId(3)], 2);
        let tree = tree.remove_pane(PaneId(2)).unwrap();
        match &tree {
            SplitNode::Stack { panes, active } => {
                assert_eq!(panes, &vec![PaneId(1), PaneId(3)]);
                assert_eq!(*active, 1);
            }
            _ => panic!("expected a stack"),
        }
        assert_eq!(
            tree.remove_pane(PaneId(3)).unwrap().panes(),
            vec![PaneId(1)]
        );
    }

    #[test]
    fn splitting_a_stacked_pane_wraps_the_whole_stack() {
        let mut tree = SplitNode::stack(vec![PaneId(1), PaneId(2)], 0);
        assert!(tree.split_pane(PaneId(2), Axis::Horizontal, PaneId(9), 0.5));
        assert_eq!(tree.panes(), vec![PaneId(1), PaneId(2), PaneId(9)]);
        let placements = tree.layout(Rect::new(0, 0, 80, 24));
        assert_eq!(placements[2].pane, PaneId(9));
        assert!(!placements[2].collapsed);
    }
}
