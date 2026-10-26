//! The i3-style layout tree of a single workspace.
//!
//! A workspace owns one root [`Container`]. Containers hold windows and other
//! containers; every child has a weight that decides its share of the parent's
//! space. After every mutation the tree is [normalized](Container::normalize) so
//! that it keeps the invariants checked by [`Container::check_invariants`].

use std::collections::HashSet;
use std::fmt;

use crate::geometry::{Orientation, Rect, Sides, WindowId};

/// How a container arranges its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layout {
    /// Children share the space side by side, proportionally to their weights.
    Tiles,
    /// Children overlap, each offset by a small padding so their edges stay visible.
    Accordion,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Window(WindowId),
    Container(Container),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub orientation: Orientation,
    pub layout: Layout,
    pub children: Vec<Node>,
    /// One positive weight per child.
    pub weights: Vec<f64>,
}

/// The smallest share of a tiles container a child is ever given.
pub(crate) const FLOOR_SHARE: f64 = 1.0 / 20.0;

/// Indices from the root to a node. The last index is the node's position in its parent.
pub type Path = Vec<usize>;

/// Gaps between neighboring windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InnerGaps {
    /// Gap between windows that sit side by side.
    pub horizontal: i32,
    /// Gap between windows stacked on top of each other.
    pub vertical: i32,
}

impl Container {
    pub fn new(orientation: Orientation, layout: Layout) -> Self {
        Self {
            orientation,
            layout,
            children: Vec::new(),
            weights: Vec::new(),
        }
    }

    /// Builds a tiles container whose children all have weight 1.
    pub fn with_children(orientation: Orientation, children: Vec<Node>) -> Self {
        let weights = vec![1.0; children.len()];
        Self {
            orientation,
            layout: Layout::Tiles,
            children,
            weights,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Returns the path to the window, if it is in this tree.
    pub fn find(&self, id: WindowId) -> Option<Path> {
        for (i, child) in self.children.iter().enumerate() {
            match child {
                Node::Window(w) if *w == id => return Some(vec![i]),
                Node::Window(_) => {}
                Node::Container(c) => {
                    if let Some(mut rest) = c.find(id) {
                        rest.insert(0, i);
                        return Some(rest);
                    }
                }
            }
        }
        None
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.find(id).is_some()
    }

    /// All windows in depth-first, on-screen order.
    pub fn windows(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.collect_windows(&mut out);
        out
    }

    fn collect_windows(&self, out: &mut Vec<WindowId>) {
        for child in &self.children {
            match child {
                Node::Window(id) => out.push(*id),
                Node::Container(c) => c.collect_windows(out),
            }
        }
    }

    /// The container addressed by `path` (empty path = this container).
    ///
    /// Panics if the path does not lead to a container.
    pub fn container_at(&self, path: &[usize]) -> &Container {
        path.iter().fold(self, |c, &i| match &c.children[i] {
            Node::Container(child) => child,
            Node::Window(_) => panic!("path {path:?} leads to a window, not a container"),
        })
    }

    pub fn container_at_mut(&mut self, path: &[usize]) -> &mut Container {
        path.iter().fold(self, |c, &i| match &mut c.children[i] {
            Node::Container(child) => child,
            Node::Window(_) => panic!("path {path:?} leads to a window, not a container"),
        })
    }

    /// Inserts `node` into the container at `parent` at position `index`, giving it the
    /// average weight of its new siblings so it gets an equal share.
    pub fn insert_at(&mut self, parent: &[usize], index: usize, node: Node) {
        let parent = self.container_at_mut(parent);
        let weight = if parent.weights.is_empty() {
            1.0
        } else {
            parent.weights.iter().sum::<f64>() / parent.weights.len() as f64
        };
        parent.children.insert(index, node);
        parent.weights.insert(index, weight);
    }

    /// Inserts a window right after `anchor` in the anchor's parent, or at the end of the
    /// root when there is no anchor or it is not in this tree.
    pub fn insert_after(&mut self, anchor: Option<WindowId>, id: WindowId) {
        match anchor.and_then(|a| self.find(a)) {
            Some(path) => {
                let (index, parent) = path.split_last().expect("paths are never empty");
                self.insert_at(parent, index + 1, Node::Window(id));
            }
            None => self.insert_at(&[], self.children.len(), Node::Window(id)),
        }
    }

    /// Removes the node at `path` and returns it with its weight. Does not normalize.
    pub fn take(&mut self, path: &[usize]) -> (Node, f64) {
        let (&index, parent) = path.split_last().expect("cannot take the root");
        let parent = self.container_at_mut(parent);
        (parent.children.remove(index), parent.weights.remove(index))
    }

    /// Removes a window and normalizes. Returns false if the window was not in the tree.
    pub fn remove(&mut self, id: WindowId) -> bool {
        match self.find(id) {
            Some(path) => {
                self.take(&path);
                self.normalize();
                true
            }
            None => false,
        }
    }

    /// Restores the tree invariants:
    /// - empty non-root containers are removed;
    /// - a container with a single child is replaced by that child;
    /// - a container whose orientation equals its parent's is merged into the parent,
    ///   splitting its weight among its children proportionally.
    pub fn normalize(&mut self) {
        self.normalize_children();
        // The root itself must not be a wrapper around a single container.
        while self.children.len() == 1 && matches!(self.children[0], Node::Container(_)) {
            let Some(Node::Container(only)) = self.children.pop() else {
                unreachable!()
            };
            *self = only;
        }
    }

    fn normalize_children(&mut self) {
        let children = std::mem::take(&mut self.children);
        let weights = std::mem::take(&mut self.weights);
        for (child, weight) in children.into_iter().zip(weights) {
            self.push_normalized(child, weight, false);
        }
    }

    /// Appends `node`, unwrapping or merging it as needed. `inner_done` is true when the
    /// node's own subtree is already normalized.
    fn push_normalized(&mut self, node: Node, weight: f64, inner_done: bool) {
        let mut c = match node {
            Node::Window(_) => {
                self.children.push(node);
                self.weights.push(weight);
                return;
            }
            Node::Container(c) => c,
        };
        if !inner_done {
            c.normalize_children();
        }
        match c.children.len() {
            0 => {}
            1 => {
                let only = c.children.pop().expect("length checked");
                self.push_normalized(only, weight, true);
            }
            _ if c.orientation == self.orientation => {
                let total: f64 = c.weights.iter().sum();
                for (child, w) in c.children.into_iter().zip(c.weights) {
                    self.push_normalized(child, weight * w / total, true);
                }
            }
            _ => {
                self.children.push(Node::Container(c));
                self.weights.push(weight);
            }
        }
    }

    /// Checks the invariants that [`normalize`](Self::normalize) maintains.
    pub fn check_invariants(&self) -> Result<(), String> {
        self.check(true, &mut HashSet::new())
    }

    fn check(&self, is_root: bool, seen: &mut HashSet<WindowId>) -> Result<(), String> {
        if self.weights.len() != self.children.len() {
            return Err(format!("{self}: {} weights", self.weights.len()));
        }
        if let Some(w) = self.weights.iter().find(|w| !(w.is_finite() && **w > 0.0)) {
            return Err(format!("{self}: invalid weight {w}"));
        }
        if !is_root && self.children.len() < 2 {
            return Err(format!(
                "{self}: non-root container with fewer than 2 children"
            ));
        }
        if self.children.len() == 1 && matches!(self.children[0], Node::Container(_)) {
            return Err(format!("{self}: root wraps a single container"));
        }
        for child in &self.children {
            match child {
                Node::Window(id) => {
                    if !seen.insert(*id) {
                        return Err(format!("window {} appears twice", id.0));
                    }
                }
                Node::Container(c) => {
                    if c.orientation == self.orientation {
                        return Err(format!("{c}: same orientation as its parent {self}"));
                    }
                    c.check(false, seen)?;
                }
            }
        }
        Ok(())
    }

    /// Computes the rectangle of every window when the tree fills `area`.
    /// Like [`layout_rects`](Self::layout_rects), but every window gets at least its
    /// minimum `(width, height)` when the space allows it.
    pub fn layout_rects_with(
        &self,
        area: Rect,
        gaps: InnerGaps,
        accordion_padding: i32,
        min: &dyn Fn(WindowId) -> (i32, i32),
    ) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        self.layout_into(area, gaps, accordion_padding, min, &mut out);
        out
    }

    pub fn layout_rects(
        &self,
        area: Rect,
        gaps: InnerGaps,
        accordion_padding: i32,
    ) -> Vec<(WindowId, Rect)> {
        self.layout_rects_with(area, gaps, accordion_padding, &|_| (0, 0))
    }

    fn layout_into(
        &self,
        area: Rect,
        gaps: InnerGaps,
        padding: i32,
        min: &dyn Fn(WindowId) -> (i32, i32),
        out: &mut Vec<(WindowId, Rect)>,
    ) {
        let rects = self.child_rects_with(area, gaps, padding, min);
        debug_assert_eq!(rects.len(), self.children.len());
        for (child, rect) in self.children.iter().zip(rects) {
            match child {
                Node::Window(id) => out.push((*id, rect)),
                Node::Container(c) => c.layout_into(rect, gaps, padding, min, out),
            }
        }
    }

    /// The smallest size of a node along `axis`, given each window's minimum.
    fn min_along(
        node: &Node,
        axis: Orientation,
        gaps: InnerGaps,
        padding: i32,
        min: &dyn Fn(WindowId) -> (i32, i32),
    ) -> i32 {
        match node {
            Node::Window(id) => {
                let (w, h) = min(*id);
                match axis {
                    Orientation::H => w,
                    Orientation::V => h,
                }
            }
            Node::Container(c) => {
                let mins = c
                    .children
                    .iter()
                    .map(|n| Self::min_along(n, axis, gaps, padding, min));
                let n = c.children.len() as i32;
                match (c.orientation == axis, c.layout) {
                    (true, Layout::Tiles) => {
                        let gap = match axis {
                            Orientation::H => gaps.horizontal,
                            Orientation::V => gaps.vertical,
                        };
                        mins.sum::<i32>() + gap * (n - 1).max(0)
                    }
                    // Accordion children are inset by the padding on up to two sides.
                    (true, Layout::Accordion) => {
                        mins.max().unwrap_or(0) + padding * (n - 1).clamp(0, 2)
                    }
                    (false, _) => mins.max().unwrap_or(0),
                }
            }
        }
    }

    /// Each child's minimum size along this container's axis.
    pub(crate) fn child_mins(
        &self,
        gaps: InnerGaps,
        padding: i32,
        min: &dyn Fn(WindowId) -> (i32, i32),
    ) -> Vec<f64> {
        self.children
            .iter()
            .map(|c| f64::from(Self::min_along(c, self.orientation, gaps, padding, min)))
            .collect()
    }

    /// Sizes for tiles children: weighted shares, but no child below its minimum.
    /// Children whose share is too small are fixed at their minimum and the rest is
    /// shared again by weight. If even the minimums don't fit, they are scaled down
    /// proportionally.
    pub(crate) fn tile_sizes(&self, avail: f64, mins: &[f64]) -> Vec<f64> {
        // Every child keeps a small share, so windows without a known minimum still
        // get room when the others' minimums overflow.
        let floor = avail * FLOOR_SHARE;
        let mins: Vec<f64> = mins.iter().map(|m| m.max(floor)).collect();
        let mins = mins.as_slice();
        let total_min: f64 = mins.iter().sum();
        if total_min >= avail {
            return if total_min > 0.0 {
                mins.iter().map(|m| avail * m / total_min).collect()
            } else {
                vec![0.0; mins.len()]
            };
        }
        let mut fixed = vec![false; mins.len()];
        loop {
            let used: f64 = (0..mins.len()).filter(|&i| fixed[i]).map(|i| mins[i]).sum();
            let weight: f64 = (0..mins.len())
                .filter(|&i| !fixed[i])
                .map(|i| self.weights[i])
                .sum();
            let free = avail - used;
            let share = |i: usize| free * self.weights[i] / weight;
            let short: Vec<usize> = (0..mins.len())
                .filter(|&i| !fixed[i] && share(i) < mins[i])
                .collect();
            if short.is_empty() {
                return (0..mins.len())
                    .map(|i| if fixed[i] { mins[i] } else { share(i) })
                    .collect();
            }
            for i in short {
                fixed[i] = true;
            }
        }
    }

    /// The rectangles of this container's direct children when it fills `area`.
    pub(crate) fn child_rects_with(
        &self,
        area: Rect,
        gaps: InnerGaps,
        padding: i32,
        min: &dyn Fn(WindowId) -> (i32, i32),
    ) -> Vec<Rect> {
        let n = self.children.len();
        if n == 0 {
            return Vec::new();
        }
        let last = n - 1;
        match self.layout {
            Layout::Tiles => {
                let (gap, total) = match self.orientation {
                    Orientation::H => (gaps.horizontal, area.w),
                    Orientation::V => (gaps.vertical, area.h),
                };
                let avail = (total - gap * last as i32).max(0);
                let mins = self.child_mins(gaps, padding, min);
                let sizes = self.tile_sizes(f64::from(avail), &mins);
                let mut cumulative = 0.0;
                let mut start = 0;
                (0..n)
                    .map(|i| {
                        cumulative += sizes[i];
                        // Rounding cumulative edges spreads leftover pixels evenly and
                        // guarantees the children exactly fill the available space.
                        let end = if i == last {
                            avail
                        } else {
                            cumulative.round() as i32
                        };
                        let offset = start + gap * i as i32;
                        let size = end - start;
                        start = end;
                        match self.orientation {
                            Orientation::H => Rect::new(area.x + offset, area.y, size, area.h),
                            Orientation::V => Rect::new(area.x, area.y + offset, area.w, size),
                        }
                    })
                    .collect()
            }
            Layout::Accordion => (0..n)
                .map(|i| {
                    let before = if i > 0 { padding } else { 0 };
                    let after = if i < last { padding } else { 0 };
                    let sides = match self.orientation {
                        Orientation::H => Sides {
                            left: before,
                            right: after,
                            ..Sides::default()
                        },
                        Orientation::V => Sides {
                            top: before,
                            bottom: after,
                            ..Sides::default()
                        },
                    };
                    area.inset(sides)
                })
                .collect(),
        }
    }
}

impl fmt::Display for Container {
    /// Compact form used in tests and debug output, e.g. `H[1 V[2 3]]`.
    /// Accordion containers print as `HA[...]` / `VA[...]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let o = match self.orientation {
            Orientation::H => "H",
            Orientation::V => "V",
        };
        let l = match self.layout {
            Layout::Tiles => "",
            Layout::Accordion => "A",
        };
        write!(f, "{o}{l}[")?;
        for (i, child) in self.children.iter().enumerate() {
            if i > 0 {
                write!(f, " ")?;
            }
            match child {
                Node::Window(id) => write!(f, "{}", id.0)?,
                Node::Container(c) => write!(f, "{c}")?,
            }
        }
        write!(f, "]")
    }
}

/// Test helpers for building trees concisely.
#[cfg(test)]
pub(crate) mod build {
    use super::*;

    pub fn w(id: u64) -> Node {
        Node::Window(WindowId(id))
    }

    pub fn h(children: Vec<Node>) -> Node {
        Node::Container(Container::with_children(Orientation::H, children))
    }

    pub fn v(children: Vec<Node>) -> Node {
        Node::Container(Container::with_children(Orientation::V, children))
    }

    pub fn root(node: Node) -> Container {
        match node {
            Node::Container(c) => c,
            Node::Window(_) => panic!("root must be a container"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    fn ids(v: &[u64]) -> Vec<WindowId> {
        v.iter().copied().map(WindowId).collect()
    }

    #[test]
    fn find_and_windows() {
        let t = root(h(vec![w(1), v(vec![w(2), w(3)])]));
        assert_eq!(t.find(WindowId(3)), Some(vec![1, 1]));
        assert_eq!(t.find(WindowId(9)), None);
        assert_eq!(t.windows(), ids(&[1, 2, 3]));
    }

    #[test]
    fn insert_after_anchor_and_into_empty_root() {
        let mut t = Container::new(Orientation::H, Layout::Tiles);
        t.insert_after(None, WindowId(1));
        t.insert_after(Some(WindowId(1)), WindowId(2));
        t.insert_after(Some(WindowId(1)), WindowId(3));
        assert_eq!(t.to_string(), "H[1 3 2]");
        assert!(t.weights.iter().all(|&w| w == 1.0));

        let mut t = root(h(vec![w(1), v(vec![w(2), w(3)])]));
        t.insert_after(Some(WindowId(2)), WindowId(4));
        assert_eq!(t.to_string(), "H[1 V[2 4 3]]");
    }

    #[test]
    fn remove_last_window_leaves_empty_root() {
        let mut t = root(h(vec![w(1)]));
        assert!(t.remove(WindowId(1)));
        assert!(t.is_empty());
        assert!(!t.remove(WindowId(1)));
        t.check_invariants().unwrap();
    }

    #[test]
    fn remove_unwraps_single_child_container() {
        let mut t = root(h(vec![w(1), v(vec![w(2), w(3)])]));
        t.remove(WindowId(3));
        assert_eq!(t.to_string(), "H[1 2]");
        t.check_invariants().unwrap();
    }

    #[test]
    fn normalize_merges_same_orientation_and_scales_weights() {
        // H[1 V[2 H[3 4]]] -> remove 2 -> V has one child H[3 4] -> unwrap -> same
        // orientation as root -> merge: H[1 3 4], with 3 and 4 splitting V's weight.
        let mut t = root(h(vec![w(1), v(vec![w(2), h(vec![w(3), w(4)])])]));
        t.weights = vec![1.0, 2.0];
        t.remove(WindowId(2));
        assert_eq!(t.to_string(), "H[1 3 4]");
        assert_eq!(t.weights, vec![1.0, 1.0, 1.0]);
        t.check_invariants().unwrap();
    }

    #[test]
    fn root_with_single_container_child_takes_its_place() {
        let mut t = root(h(vec![v(vec![w(1), w(2)])]));
        t.normalize();
        assert_eq!(t.to_string(), "V[1 2]");
        t.check_invariants().unwrap();
    }

    #[test]
    fn check_invariants_rejects_bad_trees() {
        assert!(root(h(vec![w(1), h(vec![w(2), w(3)])]))
            .check_invariants()
            .is_err());
        assert!(root(h(vec![w(1), v(vec![w(2)])]))
            .check_invariants()
            .is_err());
        assert!(root(h(vec![w(1), w(1)])).check_invariants().is_err());
        let mut t = root(h(vec![w(1), w(2)]));
        t.weights = vec![1.0, 0.0];
        assert!(t.check_invariants().is_err());
    }

    #[test]
    fn layout_three_equal_tiles_with_gaps() {
        let t = root(h(vec![w(1), w(2), w(3)]));
        let gaps = InnerGaps {
            horizontal: 10,
            vertical: 10,
        };
        let r = t.layout_rects(Rect::new(0, 0, 1000, 500), gaps, 30);
        assert_eq!(
            r,
            vec![
                (WindowId(1), Rect::new(0, 0, 327, 500)),
                (WindowId(2), Rect::new(337, 0, 326, 500)),
                (WindowId(3), Rect::new(673, 0, 327, 500)),
            ]
        );
    }

    #[test]
    fn layout_respects_weights_and_nesting() {
        let mut t = root(h(vec![w(1), v(vec![w(2), w(3)])]));
        t.weights = vec![2.0, 1.0];
        let r = t.layout_rects(Rect::new(0, 0, 900, 600), InnerGaps::default(), 30);
        assert_eq!(
            r,
            vec![
                (WindowId(1), Rect::new(0, 0, 600, 600)),
                (WindowId(2), Rect::new(600, 0, 300, 300)),
                (WindowId(3), Rect::new(600, 300, 300, 300)),
            ]
        );
    }

    #[test]
    fn layout_accordion_pads_inner_edges() {
        let mut t = root(h(vec![w(1), w(2), w(3)]));
        t.layout = Layout::Accordion;
        let r = t.layout_rects(Rect::new(0, 0, 1000, 500), InnerGaps::default(), 30);
        assert_eq!(
            r,
            vec![
                (WindowId(1), Rect::new(0, 0, 970, 500)),
                (WindowId(2), Rect::new(30, 0, 940, 500)),
                (WindowId(3), Rect::new(30, 0, 970, 500)),
            ]
        );
    }

    fn widths_with(t: &Container, w: i32, mins: &[(u64, i32, i32)]) -> Vec<i32> {
        let min = |id: WindowId| {
            mins.iter()
                .find(|(m, _, _)| *m == id.0)
                .map_or((0, 0), |(_, w, h)| (*w, *h))
        };
        t.layout_rects_with(Rect::new(0, 0, w, 1000), InnerGaps::default(), 30, &min)
            .iter()
            .map(|(_, r)| r.w)
            .collect()
    }

    #[test]
    fn minimum_width_is_honored_and_the_rest_shrinks() {
        let t = root(h(vec![w(1), w(2)]));
        assert_eq!(widths_with(&t, 1000, &[(1, 700, 0)]), vec![700, 300]);
        // A minimum below the weighted share changes nothing.
        assert_eq!(widths_with(&t, 1000, &[(1, 300, 0)]), vec![500, 500]);
    }

    #[test]
    fn remaining_space_is_shared_by_weight() {
        let mut t = root(h(vec![w(1), w(2), w(3)]));
        t.weights = vec![1.0, 1.0, 3.0];
        // 1 needs 600; the other 600 px go 1:3 to windows 2 and 3.
        assert_eq!(widths_with(&t, 1200, &[(1, 600, 0)]), vec![600, 150, 450]);
    }

    #[test]
    fn impossible_minimums_are_scaled_proportionally() {
        let t = root(h(vec![w(1), w(2)]));
        assert_eq!(
            widths_with(&t, 1000, &[(1, 700, 0), (2, 400, 0)]),
            vec![636, 364]
        );
    }

    #[test]
    fn windows_without_a_minimum_keep_a_share_when_minimums_overflow() {
        let t = root(h(vec![w(1), w(2), w(3)]));
        let widths = widths_with(&t, 1000, &[(1, 700, 0), (2, 700, 0)]);
        assert!(widths[2] > 0, "{widths:?}");
        assert_eq!(widths.iter().sum::<i32>(), 1000);
    }

    #[test]
    fn accordion_minimum_includes_its_padding() {
        // H[1 V[2 HA[3 4]]]: window 3 needs 600 px but the accordion insets it by
        // 30 px, so the V column must be at least 630 wide.
        let mut acc = Container::with_children(Orientation::H, vec![w(3), w(4)]);
        acc.layout = Layout::Accordion;
        let column = Node::Container(Container::with_children(
            Orientation::V,
            vec![w(2), Node::Container(acc)],
        ));
        let t = root(h(vec![w(1), column]));
        let r = t.layout_rects_with(
            Rect::new(0, 0, 1000, 1000),
            InnerGaps::default(),
            30,
            &|id: WindowId| if id.0 == 3 { (600, 0) } else { (0, 0) },
        );
        let w3 = r.iter().find(|(id, _)| id.0 == 3).unwrap().1.w;
        assert_eq!(w3, 600, "{r:?}");
    }

    #[test]
    fn nested_minimums_propagate_to_the_container() {
        // V[2 3] must be as wide as its widest child.
        let t = root(h(vec![w(1), v(vec![w(2), w(3)])]));
        assert_eq!(widths_with(&t, 1000, &[(2, 600, 0)]), vec![400, 600, 600]);
        // Heights inside V are untouched by width minimums.
        let r = t.layout_rects_with(
            Rect::new(0, 0, 1000, 1000),
            InnerGaps::default(),
            30,
            &|id: WindowId| if id.0 == 3 { (0, 800) } else { (0, 0) },
        );
        assert_eq!((r[1].1.h, r[2].1.h), (200, 800));
    }

    #[test]
    fn layout_of_empty_tree_is_empty() {
        let t = Container::new(Orientation::H, Layout::Tiles);
        assert!(t
            .layout_rects(Rect::new(0, 0, 10, 10), InnerGaps::default(), 0)
            .is_empty());
    }
}
