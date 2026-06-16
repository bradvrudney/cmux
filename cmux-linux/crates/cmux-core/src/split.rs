//! Binary split tree for arranging terminal panes within a tab.
//!
//! This mirrors the upstream `bonsplit` model: every tab holds a tree whose
//! leaves are panes and whose internal nodes are splits with an orientation and
//! a divider ratio. Splitting replaces a leaf with an internal node; closing a
//! pane collapses its parent so the sibling takes the freed space.

use serde::{Deserialize, Serialize};

use crate::ids::PaneId;

/// How a split arranges its two children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    /// Children sit side by side (`first | second`); the divider is vertical.
    Horizontal,
    /// Children stack (`first` on top, `second` below); the divider is horizontal.
    Vertical,
}

/// Direction of focus movement, used for spatial pane navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

/// A node in the split tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Leaf(PaneId),
    Split {
        orientation: Orientation,
        /// Fraction (0,1) of the space given to `first`.
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(p) => out.push(*p),
            Node::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    fn contains(&self, pane: PaneId) -> bool {
        match self {
            Node::Leaf(p) => *p == pane,
            Node::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }
}

/// A rectangle in normalized or pixel space. `x`/`y` are the top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// A split boundary that can be dragged to change its ratio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Divider {
    /// Pre-order index among split nodes; pass to [`SplitTree::set_ratio_by_index`].
    pub split_index: usize,
    pub orientation: Orientation,
    /// The full region the split occupies (so a UI can map a pointer to a ratio).
    pub region: Rect,
    pub ratio: f32,
}

/// The two child regions of a split occupying `rect` at `ratio`.
fn child_rects(rect: Rect, orientation: Orientation, ratio: f32) -> (Rect, Rect) {
    match orientation {
        Orientation::Horizontal => {
            let fw = rect.w * ratio;
            (
                Rect::new(rect.x, rect.y, fw, rect.h),
                Rect::new(rect.x + fw, rect.y, rect.w - fw, rect.h),
            )
        }
        Orientation::Vertical => {
            let fh = rect.h * ratio;
            (
                Rect::new(rect.x, rect.y, rect.w, fh),
                Rect::new(rect.x, rect.y + fh, rect.w, rect.h - fh),
            )
        }
    }
}

/// The split tree for one tab. Empty when the tab has no panes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SplitTree {
    root: Option<Node>,
}

impl SplitTree {
    /// A tree with a single pane.
    pub fn single(pane: PaneId) -> Self {
        Self {
            root: Some(Node::Leaf(pane)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn root(&self) -> Option<&Node> {
        self.root.as_ref()
    }

    /// All panes, left-to-right / top-to-bottom (tree pre-order).
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            root.collect_leaves(&mut out);
        }
        out
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.as_ref().map_or(false, |r| r.contains(pane))
    }

    pub fn first_leaf(&self) -> Option<PaneId> {
        self.leaves().first().copied()
    }

    /// Split `target` into two, inserting `new_pane`. When `before` is true the
    /// new pane becomes the `first` child (left/top); otherwise `second`.
    /// Returns `true` if the split happened.
    pub fn split(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        orientation: Orientation,
        before: bool,
    ) -> bool {
        match &mut self.root {
            None => false,
            Some(root) => Self::split_node(root, target, new_pane, orientation, before),
        }
    }

    fn split_node(
        node: &mut Node,
        target: PaneId,
        new_pane: PaneId,
        orientation: Orientation,
        before: bool,
    ) -> bool {
        match node {
            Node::Leaf(p) if *p == target => {
                let existing = Node::Leaf(*p);
                let inserted = Node::Leaf(new_pane);
                let (first, second) = if before {
                    (inserted, existing)
                } else {
                    (existing, inserted)
                };
                *node = Node::Split {
                    orientation,
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { first, second, .. } => {
                Self::split_node(first, target, new_pane, orientation, before)
                    || Self::split_node(second, target, new_pane, orientation, before)
            }
        }
    }

    /// Remove `pane`. Its sibling is promoted into the parent's slot. Returns
    /// `true` if the pane was found and removed.
    pub fn close(&mut self, pane: PaneId) -> bool {
        match self.root.take() {
            None => false,
            Some(root) => match Self::close_node(root, pane) {
                CloseResult::NotFound(node) => {
                    self.root = Some(node);
                    false
                }
                CloseResult::Removed(maybe_node) => {
                    self.root = maybe_node;
                    true
                }
            },
        }
    }

    fn close_node(node: Node, pane: PaneId) -> CloseResult {
        match node {
            Node::Leaf(p) if p == pane => CloseResult::Removed(None),
            Node::Leaf(p) => CloseResult::NotFound(Node::Leaf(p)),
            Node::Split {
                orientation,
                ratio,
                first,
                second,
            } => {
                match Self::close_node(*first, pane) {
                    CloseResult::Removed(None) => CloseResult::Removed(Some(*second)),
                    CloseResult::Removed(Some(new_first)) => CloseResult::Removed(Some(Node::Split {
                        orientation,
                        ratio,
                        first: Box::new(new_first),
                        second,
                    })),
                    CloseResult::NotFound(first) => match Self::close_node(*second, pane) {
                        CloseResult::Removed(None) => CloseResult::Removed(Some(first)),
                        CloseResult::Removed(Some(new_second)) => {
                            CloseResult::Removed(Some(Node::Split {
                                orientation,
                                ratio,
                                first: Box::new(first),
                                second: Box::new(new_second),
                            }))
                        }
                        CloseResult::NotFound(second) => CloseResult::NotFound(Node::Split {
                            orientation,
                            ratio,
                            first: Box::new(first),
                            second: Box::new(second),
                        }),
                    },
                }
            }
        }
    }

    /// Adjust the divider ratio of the split that directly parents `pane`.
    /// `ratio` is clamped to a sane visible range.
    pub fn set_ratio_for(&mut self, pane: PaneId, ratio: f32) -> bool {
        let ratio = ratio.clamp(0.05, 0.95);
        self.root
            .as_mut()
            .map_or(false, |r| Self::set_ratio_node(r, pane, ratio))
    }

    fn set_ratio_node(node: &mut Node, pane: PaneId, ratio: f32) -> bool {
        if let Node::Split {
            ratio: r,
            first,
            second,
            ..
        } = node
        {
            let directly_parents = matches!(**first, Node::Leaf(p) if p == pane)
                || matches!(**second, Node::Leaf(p) if p == pane);
            if directly_parents {
                *r = ratio;
                return true;
            }
            return Self::set_ratio_node(first, pane, ratio)
                || Self::set_ratio_node(second, pane, ratio);
        }
        false
    }

    /// Reset every split's divider to an even 0.5 ratio (upstream "equalize
    /// splits"). Leaves are unaffected.
    pub fn equalize(&mut self) {
        if let Some(root) = &mut self.root {
            Self::equalize_node(root);
        }
    }

    fn equalize_node(node: &mut Node) {
        if let Node::Split {
            ratio,
            first,
            second,
            ..
        } = node
        {
            *ratio = 0.5;
            Self::equalize_node(first);
            Self::equalize_node(second);
        }
    }

    /// Enumerate the dividers (split boundaries) within `viewport`, in
    /// pre-order over split nodes. The `split_index` aligns with
    /// [`SplitTree::set_ratio_by_index`] so a dragged divider can be addressed.
    pub fn dividers(&self, viewport: Rect) -> Vec<Divider> {
        let mut out = Vec::new();
        let mut idx = 0usize;
        if let Some(root) = &self.root {
            Self::collect_dividers(root, viewport, &mut idx, &mut out);
        }
        out
    }

    fn collect_dividers(node: &Node, rect: Rect, idx: &mut usize, out: &mut Vec<Divider>) {
        if let Node::Split {
            orientation,
            ratio,
            first,
            second,
        } = node
        {
            let my_index = *idx;
            *idx += 1;
            out.push(Divider {
                split_index: my_index,
                orientation: *orientation,
                region: rect,
                ratio: *ratio,
            });
            let (r1, r2) = child_rects(rect, *orientation, *ratio);
            Self::collect_dividers(first, r1, idx, out);
            Self::collect_dividers(second, r2, idx, out);
        }
    }

    /// Set the ratio of the split at `split_index` (pre-order, as produced by
    /// [`SplitTree::dividers`]). The ratio is clamped to a visible range.
    pub fn set_ratio_by_index(&mut self, split_index: usize, ratio: f32) -> bool {
        let ratio = ratio.clamp(0.05, 0.95);
        let mut idx = 0usize;
        match &mut self.root {
            Some(root) => Self::set_ratio_idx(root, split_index, &mut idx, ratio),
            None => false,
        }
    }

    fn set_ratio_idx(node: &mut Node, target: usize, idx: &mut usize, ratio: f32) -> bool {
        if let Node::Split {
            ratio: r,
            first,
            second,
            ..
        } = node
        {
            let my = *idx;
            *idx += 1;
            if my == target {
                *r = ratio;
                return true;
            }
            return Self::set_ratio_idx(first, target, idx, ratio)
                || Self::set_ratio_idx(second, target, idx, ratio);
        }
        false
    }

    /// Compute the rectangle of every pane within `viewport`.
    pub fn layout(&self, viewport: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            Self::layout_node(root, viewport, &mut out);
        }
        out
    }

    fn layout_node(node: &Node, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match node {
            Node::Leaf(p) => out.push((*p, rect)),
            Node::Split {
                orientation,
                ratio,
                first,
                second,
            } => match orientation {
                Orientation::Horizontal => {
                    let fw = rect.w * ratio;
                    Self::layout_node(first, Rect::new(rect.x, rect.y, fw, rect.h), out);
                    Self::layout_node(
                        second,
                        Rect::new(rect.x + fw, rect.y, rect.w - fw, rect.h),
                        out,
                    );
                }
                Orientation::Vertical => {
                    let fh = rect.h * ratio;
                    Self::layout_node(first, Rect::new(rect.x, rect.y, rect.w, fh), out);
                    Self::layout_node(
                        second,
                        Rect::new(rect.x, rect.y + fh, rect.w, rect.h - fh),
                        out,
                    );
                }
            },
        }
    }

    /// Find the pane spatially adjacent to `from` in `dir`, using a unit-square
    /// layout. Returns `None` if there is no pane in that direction.
    pub fn neighbor(&self, from: PaneId, dir: FocusDir) -> Option<PaneId> {
        let layout = self.layout(Rect::new(0.0, 0.0, 1.0, 1.0));
        let (_, src) = layout.iter().find(|(p, _)| *p == from)?;
        let (sx, sy) = src.center();

        layout
            .iter()
            .filter(|(p, _)| *p != from)
            .filter(|(_, r)| match dir {
                FocusDir::Left => r.x + r.w <= src.x + 1e-3,
                FocusDir::Right => r.x >= src.x + src.w - 1e-3,
                FocusDir::Up => r.y + r.h <= src.y + 1e-3,
                FocusDir::Down => r.y >= src.y + src.h - 1e-3,
            })
            .min_by(|(_, a), (_, b)| {
                let da = dist(a.center(), (sx, sy), dir);
                let db = dist(b.center(), (sx, sy), dir);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(p, _)| *p)
    }
}

/// Distance metric that weights the primary axis so the nearest pane in the
/// movement direction wins, breaking ties by cross-axis proximity.
fn dist((cx, cy): (f32, f32), (sx, sy): (f32, f32), dir: FocusDir) -> f32 {
    let (primary, cross) = match dir {
        FocusDir::Left | FocusDir::Right => ((cx - sx).abs(), (cy - sy).abs()),
        FocusDir::Up | FocusDir::Down => ((cy - sy).abs(), (cx - sx).abs()),
    };
    primary * 2.0 + cross
}

enum CloseResult {
    /// Pane not present here; node returned unchanged.
    NotFound(Node),
    /// Pane removed; `Some` is the replacement subtree, `None` means the subtree
    /// is now empty.
    Removed(Option<Node>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_then_split_horizontal() {
        let mut t = SplitTree::single(PaneId(1));
        assert_eq!(t.leaves(), vec![PaneId(1)]);
        assert!(t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false));
        assert_eq!(t.leaves(), vec![PaneId(1), PaneId(2)]);
    }

    #[test]
    fn split_before_inserts_first() {
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Vertical, true);
        assert_eq!(t.leaves(), vec![PaneId(2), PaneId(1)]);
    }

    #[test]
    fn split_unknown_target_is_noop() {
        let mut t = SplitTree::single(PaneId(1));
        assert!(!t.split(PaneId(99), PaneId(2), Orientation::Horizontal, false));
        assert_eq!(t.leaves(), vec![PaneId(1)]);
    }

    #[test]
    fn close_collapses_to_sibling() {
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false);
        t.split(PaneId(2), PaneId(3), Orientation::Vertical, false);
        assert_eq!(t.leaves(), vec![PaneId(1), PaneId(2), PaneId(3)]);
        assert!(t.close(PaneId(2)));
        assert_eq!(t.leaves(), vec![PaneId(1), PaneId(3)]);
    }

    #[test]
    fn close_last_pane_empties_tree() {
        let mut t = SplitTree::single(PaneId(1));
        assert!(t.close(PaneId(1)));
        assert!(t.is_empty());
        assert!(!t.close(PaneId(1)));
    }

    #[test]
    fn layout_splits_space_by_ratio() {
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false);
        let layout = t.layout(Rect::new(0.0, 0.0, 100.0, 50.0));
        let r1 = layout.iter().find(|(p, _)| *p == PaneId(1)).unwrap().1;
        let r2 = layout.iter().find(|(p, _)| *p == PaneId(2)).unwrap().1;
        assert_eq!((r1.x, r1.w), (0.0, 50.0));
        assert_eq!((r2.x, r2.w), (50.0, 50.0));
        assert_eq!(r1.h, 50.0);
    }

    #[test]
    fn set_ratio_changes_layout() {
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false);
        assert!(t.set_ratio_for(PaneId(1), 0.25));
        let layout = t.layout(Rect::new(0.0, 0.0, 100.0, 100.0));
        let r1 = layout.iter().find(|(p, _)| *p == PaneId(1)).unwrap().1;
        assert_eq!(r1.w, 25.0);
    }

    #[test]
    fn ratio_is_clamped() {
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false);
        t.set_ratio_for(PaneId(1), 5.0);
        let layout = t.layout(Rect::new(0.0, 0.0, 100.0, 100.0));
        let r1 = layout.iter().find(|(p, _)| *p == PaneId(1)).unwrap().1;
        assert_eq!(r1.w, 95.0);
    }

    #[test]
    fn dividers_enumerated_with_indices() {
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false); // split 0
        t.split(PaneId(2), PaneId(3), Orientation::Vertical, false); // split 1 (under second)
        let d = t.dividers(Rect::new(0.0, 0.0, 1.0, 1.0));
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].split_index, 0);
        assert_eq!(d[0].orientation, Orientation::Horizontal);
        assert_eq!(d[1].split_index, 1);
        assert_eq!(d[1].orientation, Orientation::Vertical);
    }

    #[test]
    fn set_ratio_by_index_changes_layout() {
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false);
        assert!(t.set_ratio_by_index(0, 0.25));
        let layout = t.layout(Rect::new(0.0, 0.0, 100.0, 100.0));
        let r1 = layout.iter().find(|(p, _)| *p == PaneId(1)).unwrap().1;
        assert_eq!(r1.w, 25.0);
        // Out-of-range index is a no-op.
        assert!(!t.set_ratio_by_index(9, 0.5));
    }

    #[test]
    fn set_ratio_by_index_is_clamped() {
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false);
        t.set_ratio_by_index(0, 5.0);
        let d = t.dividers(Rect::new(0.0, 0.0, 1.0, 1.0));
        assert_eq!(d[0].ratio, 0.95);
    }

    #[test]
    fn equalize_resets_all_ratios() {
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false);
        t.split(PaneId(2), PaneId(3), Orientation::Vertical, false);
        t.set_ratio_by_index(0, 0.2);
        t.set_ratio_by_index(1, 0.8);
        t.equalize();
        let d = t.dividers(Rect::new(0.0, 0.0, 1.0, 1.0));
        assert!(d.iter().all(|d| d.ratio == 0.5));
    }

    #[test]
    fn neighbor_navigation_horizontal() {
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false);
        assert_eq!(t.neighbor(PaneId(1), FocusDir::Right), Some(PaneId(2)));
        assert_eq!(t.neighbor(PaneId(2), FocusDir::Left), Some(PaneId(1)));
        assert_eq!(t.neighbor(PaneId(1), FocusDir::Left), None);
        assert_eq!(t.neighbor(PaneId(1), FocusDir::Up), None);
    }

    #[test]
    fn neighbor_navigation_grid() {
        // Build a 2x2 grid: split into left|right, then split each vertically.
        let mut t = SplitTree::single(PaneId(1));
        t.split(PaneId(1), PaneId(2), Orientation::Horizontal, false); // 1 | 2
        t.split(PaneId(1), PaneId(3), Orientation::Vertical, false); // 1 over 3
        t.split(PaneId(2), PaneId(4), Orientation::Vertical, false); // 2 over 4
        // Layout: top-left=1, top-right=2, bottom-left=3, bottom-right=4
        assert_eq!(t.neighbor(PaneId(1), FocusDir::Right), Some(PaneId(2)));
        assert_eq!(t.neighbor(PaneId(1), FocusDir::Down), Some(PaneId(3)));
        assert_eq!(t.neighbor(PaneId(4), FocusDir::Left), Some(PaneId(3)));
        assert_eq!(t.neighbor(PaneId(4), FocusDir::Up), Some(PaneId(2)));
    }
}
