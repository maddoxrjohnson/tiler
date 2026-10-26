//! Commands that restructure a workspace tree: focus navigation, moving, joining,
//! changing layouts, and resizing.

use crate::geometry::{Direction, Orientation, Rect, WindowId};
use crate::tree::{Container, InnerGaps, Layout, Node};

/// The argument of the `layout` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutCmd {
    Tiles,
    Accordion,
    Horizontal,
    Vertical,
    HTiles,
    VTiles,
    HAccordion,
    VAccordion,
}

impl LayoutCmd {
    fn target(self, (orientation, layout): (Orientation, Layout)) -> (Orientation, Layout) {
        use LayoutCmd::*;
        match self {
            Tiles => (orientation, Layout::Tiles),
            Accordion => (orientation, Layout::Accordion),
            Horizontal => (Orientation::H, layout),
            Vertical => (Orientation::V, layout),
            HTiles => (Orientation::H, Layout::Tiles),
            VTiles => (Orientation::V, Layout::Tiles),
            HAccordion => (Orientation::H, Layout::Accordion),
            VAccordion => (Orientation::V, Layout::Accordion),
        }
    }
}

/// Which dimension `resize` changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResizeDim {
    Width,
    Height,
    /// The dimension along the window's parent container.
    Smart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResizeAmount {
    /// Grow (positive) or shrink (negative) by this many pixels.
    Relative(i32),
    /// Set the size to exactly this many pixels.
    Absolute(i32),
}

/// A window never shrinks below this share of its container when resized.
const MIN_SHARE: f64 = 1.0 / 20.0;

/// The smallest size a child of a container with `n` children may be resized to.
/// Never more than an equal share, so the bounds stay valid for any `n`.
fn min_size(total: f64, n: usize) -> f64 {
    (total * MIN_SHARE).min(total / n as f64)
}

/// The index next to `i` in `dir` within a container of `len` children.
fn step(i: usize, dir: Direction, len: usize) -> Option<usize> {
    if dir.is_forward() {
        (i + 1 < len).then_some(i + 1)
    } else {
        i.checked_sub(1)
    }
}

/// The window inside `node` that is closest to a source approaching from the
/// opposite side of `dir`.
fn nearest_window(node: &Node, dir: Direction) -> WindowId {
    match node {
        Node::Window(id) => *id,
        Node::Container(c) => {
            let index = if c.orientation == dir.orientation() && !dir.is_forward() {
                c.children.len() - 1
            } else {
                0
            };
            nearest_window(&c.children[index], dir)
        }
    }
}

impl Container {
    /// The window that `focus <dir>` moves to, if any.
    ///
    /// Walks up to the nearest ancestor laid out along `dir` that has a sibling in that
    /// direction, then descends into the sibling, taking the child nearest to the source.
    pub fn neighbor(&self, id: WindowId, dir: Direction) -> Option<WindowId> {
        let path = self.find(id)?;
        (0..path.len()).rev().find_map(|k| {
            let parent = self.container_at(&path[..k]);
            if parent.orientation != dir.orientation() {
                return None;
            }
            let j = step(path[k], dir, parent.children.len())?;
            Some(nearest_window(&parent.children[j], dir))
        })
    }

    /// Moves a window one step in `dir` (the `move` command). Returns false if nothing
    /// changed, e.g. the window is already at the edge of the workspace.
    ///
    /// - next to a sibling window in a container along `dir`: swap with it;
    /// - next to a sibling container: move into it, as its first child;
    /// - at the edge of its parent: move out, next to the nearest ancestor along `dir`;
    /// - no ancestor along `dir`: re-root the tree along `dir` and put the window at
    ///   the far edge.
    pub fn move_window(&mut self, id: WindowId, dir: Direction) -> bool {
        let Some(path) = self.find(id) else {
            return false;
        };
        let orientation = dir.orientation();
        for k in (0..path.len()).rev() {
            let parent_path = path[..k].to_vec();
            let parent = self.container_at(&parent_path);
            if parent.orientation != orientation {
                continue;
            }
            let i = path[k];
            if k == path.len() - 1 {
                let Some(j) = step(i, dir, parent.children.len()) else {
                    continue; // At the edge of the parent: try further up.
                };
                if matches!(parent.children[j], Node::Window(_)) {
                    let parent = self.container_at_mut(&parent_path);
                    parent.children.swap(i, j);
                    parent.weights.swap(i, j);
                } else {
                    let (node, _) = self.take(&path);
                    let mut target = parent_path;
                    target.push(if j > i { j - 1 } else { j });
                    self.insert_at(&target, 0, node);
                }
            } else {
                let (node, _) = self.take(&path);
                let index = if dir.is_forward() { i + 1 } else { i };
                self.insert_at(&parent_path, index, node);
            }
            self.normalize();
            return true;
        }

        if self.orientation == orientation || self.windows().len() < 2 {
            return false;
        }
        let (node, _) = self.take(&path);
        self.normalize();
        let rest = Node::Container(std::mem::replace(
            self,
            Container::new(orientation, Layout::Tiles),
        ));
        self.children = if dir.is_forward() {
            vec![rest, node]
        } else {
            vec![node, rest]
        };
        self.weights = vec![1.0, 1.0];
        self.normalize();
        true
    }

    /// Puts the window and its neighbor in `dir` under a new container with the
    /// opposite orientation (the `join-with` command).
    pub fn join_with(&mut self, id: WindowId, dir: Direction) -> bool {
        let Some(path) = self.find(id) else {
            return false;
        };
        for k in (0..path.len()).rev() {
            let parent = self.container_at_mut(&path[..k]);
            if parent.orientation != dir.orientation() {
                continue;
            }
            let i = path[k];
            let Some(j) = step(i, dir, parent.children.len()) else {
                continue;
            };
            let (a, b) = (i.min(j), i.max(j));
            let second = parent.children.remove(b);
            let first = parent.children.remove(a);
            let weight = parent.weights.remove(b) + parent.weights.remove(a);
            let joined =
                Container::with_children(parent.orientation.opposite(), vec![first, second]);
            parent.children.insert(a, Node::Container(joined));
            parent.weights.insert(a, weight);
            self.normalize();
            return true;
        }
        false
    }

    /// Changes the layout and/or orientation of the window's parent container.
    /// Returns false if the container already had that layout.
    pub fn set_layout(&mut self, id: WindowId, cmd: LayoutCmd) -> bool {
        let Some(path) = self.find(id) else {
            return false;
        };
        let parent = self.container_at_mut(&path[..path.len() - 1]);
        let current = (parent.orientation, parent.layout);
        let target = cmd.target(current);
        if target == current {
            return false;
        }
        (parent.orientation, parent.layout) = target;
        self.normalize();
        true
    }

    /// Resizes a window by changing its weight in the nearest ancestor laid out along
    /// the requested dimension. Siblings shrink or grow proportionally; no child goes
    /// below [`MIN_SHARE`] of the container.
    pub fn resize(
        &mut self,
        id: WindowId,
        dim: ResizeDim,
        amount: ResizeAmount,
        area: Rect,
        gaps: InnerGaps,
    ) -> bool {
        let Some(path) = self.find(id) else {
            return false;
        };
        let orientation = match dim {
            ResizeDim::Width => Orientation::H,
            ResizeDim::Height => Orientation::V,
            ResizeDim::Smart => self.container_at(&path[..path.len() - 1]).orientation,
        };
        let Some(k) = (0..path.len()).rev().find(|&k| {
            let c = self.container_at(&path[..k]);
            c.orientation == orientation && c.layout == Layout::Tiles && c.children.len() > 1
        }) else {
            return false;
        };
        let rect = self.container_rect(&path[..k], area, gaps);
        let parent = self.container_at_mut(&path[..k]);
        let n = parent.children.len();
        let (gap, length) = match orientation {
            Orientation::H => (gaps.horizontal, rect.w),
            Orientation::V => (gaps.vertical, rect.h),
        };
        let total = f64::from(length - gap * (n as i32 - 1));
        if total <= 0.0 {
            return false;
        }
        let i = path[k];
        let sum: f64 = parent.weights.iter().sum();
        let old = parent.weights[i];
        let current = total * old / sum;
        let target = match amount {
            ResizeAmount::Relative(delta) => current + f64::from(delta),
            ResizeAmount::Absolute(size) => f64::from(size),
        };
        let min = min_size(total, n);
        // `.max(min)` guards against rounding when every child is at an equal share.
        let target = target.clamp(min, (total - min * (n - 1) as f64).max(min));
        let new = sum * target / total;
        let scale = (sum - new) / (sum - old);
        for (j, w) in parent.weights.iter_mut().enumerate() {
            *w = if j == i { new } else { *w * scale };
        }
        (new - old).abs() > f64::EPSILON
    }

    /// Moves one edge of a window by `delta` pixels (positive = outwards), taking the
    /// space from (or giving it to) the neighbor on that side only. Used for mouse
    /// resizing. Returns false if the window has no neighbor on that side.
    pub fn resize_edge(
        &mut self,
        id: WindowId,
        side: Direction,
        delta: i32,
        area: Rect,
        gaps: InnerGaps,
    ) -> bool {
        let Some(path) = self.find(id) else {
            return false;
        };
        for k in (0..path.len()).rev() {
            let parent = self.container_at(&path[..k]);
            if parent.orientation != side.orientation() || parent.layout != Layout::Tiles {
                continue;
            }
            let i = path[k];
            let n = parent.children.len();
            let Some(j) = step(i, side, n) else {
                continue;
            };
            let rect = self.container_rect(&path[..k], area, gaps);
            let (gap, length) = match side.orientation() {
                Orientation::H => (gaps.horizontal, rect.w),
                Orientation::V => (gaps.vertical, rect.h),
            };
            let total = f64::from(length - gap * (n as i32 - 1));
            if total <= 0.0 {
                return false;
            }
            let parent = self.container_at_mut(&path[..k]);
            let sum: f64 = parent.weights.iter().sum();
            let (wi, wj) = (parent.weights[i], parent.weights[j]);
            let pair = total * (wi + wj) / sum;
            let min = min_size(total, n);
            let new_i = (total * wi / sum + f64::from(delta)).clamp(min, (pair - min).max(min));
            parent.weights[i] = sum * new_i / total;
            parent.weights[j] = sum * (pair - new_i) / total;
            return (parent.weights[i] - wi).abs() > f64::EPSILON;
        }
        false
    }

    /// The rectangle of the container at `path` when the tree fills `area`. Accordion
    /// padding is ignored because only tiles containers are resized.
    fn container_rect(&self, path: &[usize], area: Rect, gaps: InnerGaps) -> Rect {
        let mut container = self;
        let mut rect = area;
        for &i in path {
            rect = container.child_rects(rect, gaps, 0)[i];
            container = match &container.children[i] {
                Node::Container(c) => c,
                Node::Window(_) => unreachable!("path leads to a container"),
            };
        }
        rect
    }

    /// Resets every weight in the tree to 1 (the `balance-sizes` command).
    pub fn balance(&mut self) {
        self.weights.fill(1.0);
        for child in &mut self.children {
            if let Node::Container(c) = child {
                c.balance();
            }
        }
    }

    /// Puts every window directly under the root (the `flatten-workspace-tree` command).
    pub fn flatten(&mut self) {
        self.children = self.windows().into_iter().map(Node::Window).collect();
        self.weights = vec![1.0; self.children.len()];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::build::*;
    use Direction::*;

    fn fixture() -> Container {
        root(h(vec![w(1), v(vec![w(2), w(3)])]))
    }

    fn id(n: u64) -> WindowId {
        WindowId(n)
    }

    fn after(f: impl FnOnce(&mut Container) -> bool) -> (bool, String) {
        let mut t = fixture();
        let changed = f(&mut t);
        t.check_invariants().unwrap();
        (changed, t.to_string())
    }

    #[test]
    fn neighbor_navigation() {
        let t = fixture();
        assert_eq!(t.neighbor(id(1), Right), Some(id(2)));
        assert_eq!(t.neighbor(id(3), Left), Some(id(1)));
        assert_eq!(t.neighbor(id(2), Down), Some(id(3)));
        assert_eq!(t.neighbor(id(3), Up), Some(id(2)));
        assert_eq!(t.neighbor(id(1), Left), None);
        assert_eq!(t.neighbor(id(1), Up), None);
        assert_eq!(t.neighbor(id(9), Up), None);
    }

    #[test]
    fn neighbor_descends_to_nearest_edge() {
        // H[V[1 2] H... ] — going left from 3 into H[1 2]-like containers picks the last.
        let t = root(h(vec![w(1), w(2), v(vec![w(3), w(4)])]));
        assert_eq!(t.neighbor(id(3), Left), Some(id(2)));
        let t = root(v(vec![h(vec![w(1), w(2)]), w(3)]));
        assert_eq!(t.neighbor(id(3), Up), Some(id(1)));
    }

    #[test]
    fn move_swaps_with_sibling_window() {
        assert_eq!(
            after(|t| t.move_window(id(2), Down)),
            (true, "H[1 V[3 2]]".into())
        );
    }

    #[test]
    fn move_into_sibling_container() {
        assert_eq!(
            after(|t| t.move_window(id(1), Right)),
            (true, "V[1 2 3]".into())
        );
    }

    #[test]
    fn move_out_of_parent() {
        assert_eq!(
            after(|t| t.move_window(id(2), Left)),
            (true, "H[1 2 3]".into())
        );
        assert_eq!(
            after(|t| t.move_window(id(3), Right)),
            (true, "H[1 2 3]".into())
        );
    }

    #[test]
    fn move_reroots_when_no_ancestor_matches() {
        assert_eq!(
            after(|t| t.move_window(id(2), Up)),
            (true, "V[2 H[1 3]]".into())
        );
        // The remaining tree collapses to V[2 3], which merges into the new V root.
        assert_eq!(
            after(|t| t.move_window(id(1), Down)),
            (true, "V[2 3 1]".into())
        );
    }

    #[test]
    fn move_at_edge_is_noop() {
        assert_eq!(
            after(|t| t.move_window(id(1), Left)),
            (false, "H[1 V[2 3]]".into())
        );
        let mut single = root(h(vec![w(1)]));
        assert!(!single.move_window(id(1), Up));
    }

    #[test]
    fn join_with_wraps_pair() {
        let mut t = root(h(vec![w(1), w(2), w(3)]));
        assert!(t.join_with(id(2), Right));
        assert_eq!(t.to_string(), "H[1 V[2 3]]");
        assert!(!t.join_with(id(1), Left));
        t.check_invariants().unwrap();
    }

    #[test]
    fn set_layout_changes_parent_and_reports_noop() {
        let mut t = fixture();
        assert!(t.set_layout(id(2), LayoutCmd::Accordion));
        assert_eq!(t.to_string(), "H[1 VA[2 3]]");
        assert!(!t.set_layout(id(2), LayoutCmd::VAccordion));
        // Turning the V container horizontal merges it into the H root.
        assert!(t.set_layout(id(3), LayoutCmd::HTiles));
        assert_eq!(t.to_string(), "H[1 2 3]");
        t.check_invariants().unwrap();
    }

    #[test]
    fn resize_grows_and_shrinks_siblings() {
        let mut t = root(h(vec![w(1), w(2)]));
        let area = Rect::new(0, 0, 1000, 500);
        assert!(t.resize(
            id(1),
            ResizeDim::Smart,
            ResizeAmount::Relative(100),
            area,
            InnerGaps::default()
        ));
        let r = t.layout_rects(area, InnerGaps::default(), 0);
        assert_eq!(r[0].1.w, 600);
        assert_eq!(r[1].1.w, 400);
        assert!(t.resize(
            id(2),
            ResizeDim::Width,
            ResizeAmount::Absolute(250),
            area,
            InnerGaps::default()
        ));
        let r = t.layout_rects(area, InnerGaps::default(), 0);
        assert_eq!(r[1].1.w, 250);
    }

    #[test]
    fn resize_clamps_to_min_share() {
        let mut t = root(h(vec![w(1), w(2)]));
        let area = Rect::new(0, 0, 1000, 500);
        t.resize(
            id(1),
            ResizeDim::Width,
            ResizeAmount::Relative(-100_000),
            area,
            InnerGaps::default(),
        );
        let r = t.layout_rects(area, InnerGaps::default(), 0);
        assert_eq!(r[0].1.w, 50);
        t.check_invariants().unwrap();
    }

    #[test]
    fn resize_without_matching_ancestor_is_noop() {
        let mut t = root(h(vec![w(1), w(2)]));
        let area = Rect::new(0, 0, 1000, 500);
        assert!(!t.resize(
            id(1),
            ResizeDim::Height,
            ResizeAmount::Relative(50),
            area,
            InnerGaps::default()
        ));
    }

    #[test]
    fn resize_with_more_than_twenty_children_does_not_panic() {
        let mut t = root(h((1..=21).map(w).collect()));
        let area = Rect::new(0, 0, 1904, 1064);
        t.resize(
            id(1),
            ResizeDim::Width,
            ResizeAmount::Relative(-50),
            area,
            InnerGaps::default(),
        );
        t.resize(
            id(1),
            ResizeDim::Width,
            ResizeAmount::Relative(5000),
            area,
            InnerGaps::default(),
        );
        t.check_invariants().unwrap();
    }

    #[test]
    fn resize_edge_moves_only_that_edge() {
        let mut t = root(h(vec![w(1), w(2), w(3)]));
        let area = Rect::new(0, 0, 900, 500);
        assert!(t.resize_edge(id(2), Left, 50, area, InnerGaps::default()));
        let widths: Vec<i32> = t
            .layout_rects(area, InnerGaps::default(), 0)
            .iter()
            .map(|(_, r)| r.w)
            .collect();
        assert_eq!(widths, vec![250, 350, 300]);
        // Shrinking the right edge of 3 is impossible: it has no right neighbor.
        assert!(!t.resize_edge(id(3), Right, -20, area, InnerGaps::default()));
        // An edge can't push a neighbor below the minimum share.
        t.resize_edge(id(2), Left, 10_000, area, InnerGaps::default());
        let r = t.layout_rects(area, InnerGaps::default(), 0);
        assert_eq!(r[0].1.w, 45);
        t.check_invariants().unwrap();
    }

    #[test]
    fn resize_edge_uses_nearest_matching_ancestor() {
        // Window 2's top edge belongs to the V container; its left edge to the H root.
        let mut t = fixture();
        let area = Rect::new(0, 0, 1000, 1000);
        assert!(t.resize_edge(id(3), Up, 100, area, InnerGaps::default()));
        let r = t.layout_rects(area, InnerGaps::default(), 0);
        assert_eq!(r[1].1.h, 400);
        assert_eq!(r[2].1.h, 600);
        assert!(t.resize_edge(id(3), Left, 100, area, InnerGaps::default()));
        let r = t.layout_rects(area, InnerGaps::default(), 0);
        assert_eq!(r[0].1.w, 400);
    }

    #[test]
    fn balance_and_flatten() {
        let mut t = fixture();
        t.weights = vec![3.0, 1.0];
        t.balance();
        assert_eq!(t.weights, vec![1.0, 1.0]);
        t.flatten();
        assert_eq!(t.to_string(), "H[1 2 3]");
        t.check_invariants().unwrap();
    }
}
