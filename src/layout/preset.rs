//! 布局预设：把当前分屏里的 Pane 重新排成几种常见形状。
//!
//! 预设只是 [`SplitNode`] 的生成函数，不保存任何状态；当前用的是哪一个记录在
//! [`crate::model::Tab::layout`] 里，才能按顺序切换。

use crate::layout::split::{Axis, PaneId, SplitNode};

/// 预设的名字，顺序即切换顺序。
pub const NAMES: &[&str] = &[
    "balanced",
    "main-left",
    "main-top",
    "columns",
    "rows",
    "stack",
    "main-stack",
];

/// 按预设重排 `panes`（顺序即原布局里的先后）。
pub fn build(index: usize, panes: &[PaneId]) -> SplitNode {
    if panes.is_empty() {
        return SplitNode::leaf(0);
    }
    match index % NAMES.len() {
        0 => balanced(panes, Axis::Horizontal),
        1 => main_left(panes),
        2 => main_top(panes),
        3 => along(panes, Axis::Horizontal),
        4 => along(panes, Axis::Vertical),
        5 => SplitNode::stack(panes.to_vec(), 0),
        _ => main_stack(panes),
    }
}

/// 沿一条轴逐格切开，每格等宽/等高。
fn along(panes: &[PaneId], axis: Axis) -> SplitNode {
    match panes.len() {
        0 => SplitNode::leaf(0),
        1 => SplitNode::Leaf(panes[0]),
        // 每次切出 1/n，剩下的子矩形里再切 1/(n-1)，于是每格等宽。
        count => SplitNode::split(
            axis,
            1.0 / count as f32,
            SplitNode::Leaf(panes[0]),
            along(&panes[1..], axis),
        ),
    }
}

/// 二分到网格状：每次对半切，两层之间换轴。
fn balanced(panes: &[PaneId], axis: Axis) -> SplitNode {
    if panes.len() == 1 {
        return SplitNode::Leaf(panes[0]);
    }
    let mid = panes.len() / 2;
    let ratio = mid as f32 / panes.len() as f32;
    SplitNode::split(
        axis,
        ratio,
        balanced(&panes[..mid], axis.other()),
        balanced(&panes[mid..], axis.other()),
    )
}

/// 第一个 Pane 占左边，其余纵向排在右边。
fn main_left(panes: &[PaneId]) -> SplitNode {
    if panes.len() == 1 {
        return SplitNode::Leaf(panes[0]);
    }
    SplitNode::split(
        Axis::Horizontal,
        0.6,
        SplitNode::Leaf(panes[0]),
        along(&panes[1..], Axis::Vertical),
    )
}

/// 第一个 Pane 占上面，其余横向排在下面。
fn main_top(panes: &[PaneId]) -> SplitNode {
    if panes.len() == 1 {
        return SplitNode::Leaf(panes[0]);
    }
    SplitNode::split(
        Axis::Vertical,
        0.6,
        SplitNode::Leaf(panes[0]),
        along(&panes[1..], Axis::Horizontal),
    )
}

/// 第一个 Pane 占左边，其余收成一叠放在右边。
fn main_stack(panes: &[PaneId]) -> SplitNode {
    if panes.len() == 1 {
        return SplitNode::Leaf(panes[0]);
    }
    SplitNode::split(
        Axis::Horizontal,
        0.6,
        SplitNode::Leaf(panes[0]),
        SplitNode::stack(panes[1..].to_vec(), 0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_core::layout::Rect;

    fn panes(count: u32) -> Vec<PaneId> {
        (1..=count).map(PaneId).collect()
    }

    #[test]
    fn every_preset_keeps_all_panes_once() {
        for count in 1..=5 {
            let ids = panes(count);
            for (index, name) in NAMES.iter().enumerate() {
                let node = build(index, &ids);
                let mut got = node.panes();
                got.sort();
                assert_eq!(got, ids, "{name} with {count} panes");
            }
        }
    }

    #[test]
    fn columns_are_side_by_side_and_rows_stack() {
        let ids = panes(3);
        let columns = build(3, &ids);
        let rects = columns.layout(Rect::new(0, 0, 90, 24));
        assert_eq!(rects[0].rect.x, 0);
        assert_eq!(rects[1].rect.x, 30);
        assert_eq!(rects[2].rect.x, 60);
        assert!(rects.iter().all(|placement| placement.rect.height == 24));

        let rows = build(4, &ids);
        let rects = rows.layout(Rect::new(0, 0, 90, 24));
        assert!(rects.iter().all(|placement| placement.rect.width == 90));
        assert_eq!(rects[2].rect.y, 16);
    }

    #[test]
    fn main_left_gives_the_first_pane_the_wide_column() {
        let ids = panes(3);
        let node = build(1, &ids);
        let rects = node.layout(Rect::new(0, 0, 100, 20));
        assert_eq!(rects[0].rect, Rect::new(0, 0, 60, 20));
        assert_eq!(rects[1].rect.width, 40);
        assert_eq!(rects[2].rect.y, 10);
    }

    #[test]
    fn the_stack_presets_fold_the_rest() {
        let ids = panes(3);
        let flat = build(5, &ids);
        assert!(matches!(flat, SplitNode::Stack { .. }));
        let main_stack = build(6, &ids);
        let rects = main_stack.layout(Rect::new(0, 0, 100, 20));
        assert_eq!(rects[0].rect, Rect::new(0, 0, 60, 20));
        assert!(!rects[1].collapsed, "栈里第一个是展开项");
        assert!(rects[2].collapsed);
    }

    #[test]
    fn an_unknown_index_wraps_around() {
        let ids = panes(2);
        assert_eq!(build(NAMES.len() + 1, &ids), build(1, &ids));
    }
}
