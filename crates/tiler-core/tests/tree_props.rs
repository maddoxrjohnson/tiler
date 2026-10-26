//! Property tests: no sequence of tree operations may break the tree invariants or
//! lose/duplicate a window.

use std::collections::BTreeSet;

use proptest::prelude::*;
use tiler_core::{
    Container, Direction, InnerGaps, Layout, LayoutCmd, Orientation, Rect, ResizeAmount, ResizeDim,
    WindowId,
};

#[derive(Debug, Clone)]
enum Op {
    Insert(u64, u64),
    Remove(u64),
    Move(u64, Direction),
    Join(u64, Direction),
    Layout(u64, LayoutCmd),
    Resize(u64, ResizeDim, i32),
    Balance,
    Flatten,
}

fn direction() -> impl Strategy<Value = Direction> {
    prop::sample::select(Direction::ALL.to_vec())
}

fn layout_cmd() -> impl Strategy<Value = LayoutCmd> {
    use LayoutCmd::*;
    prop::sample::select(vec![
        Tiles, Accordion, Horizontal, Vertical, HTiles, VTiles, HAccordion, VAccordion,
    ])
}

fn resize_dim() -> impl Strategy<Value = ResizeDim> {
    prop::sample::select(vec![ResizeDim::Width, ResizeDim::Height, ResizeDim::Smart])
}

fn op() -> impl Strategy<Value = Op> {
    let id = 0u64..25;
    prop_oneof![
        3 => (id.clone(), id.clone()).prop_map(|(a, b)| Op::Insert(a, b)),
        1 => id.clone().prop_map(Op::Remove),
        2 => (id.clone(), direction()).prop_map(|(a, d)| Op::Move(a, d)),
        2 => (id.clone(), direction()).prop_map(|(a, d)| Op::Join(a, d)),
        1 => (id.clone(), layout_cmd()).prop_map(|(a, c)| Op::Layout(a, c)),
        1 => (id, resize_dim(), -2000i32..2000).prop_map(|(a, d, n)| Op::Resize(a, d, n)),
        1 => Just(Op::Balance),
        1 => Just(Op::Flatten),
    ]
}

const AREA: Rect = Rect::new(0, 0, 1920, 1080);
const GAPS: InnerGaps = InnerGaps {
    horizontal: 8,
    vertical: 8,
};

proptest! {
    #[test]
    fn invariants_hold(ops in prop::collection::vec(op(), 1..80)) {
        let mut tree = Container::new(Orientation::H, Layout::Tiles);
        let mut model = BTreeSet::new();
        for op in ops {
            match op {
                Op::Insert(id, anchor) => {
                    if model.insert(id) {
                        tree.insert_after(Some(WindowId(anchor)), WindowId(id));
                    }
                }
                Op::Remove(id) => {
                    prop_assert_eq!(tree.remove(WindowId(id)), model.remove(&id));
                }
                Op::Move(id, d) => { tree.move_window(WindowId(id), d); }
                Op::Join(id, d) => { tree.join_with(WindowId(id), d); }
                Op::Layout(id, c) => { tree.set_layout(WindowId(id), c); }
                Op::Resize(id, dim, n) => {
                    tree.resize(WindowId(id), dim, ResizeAmount::Relative(n), AREA, GAPS);
                }
                Op::Balance => tree.balance(),
                Op::Flatten => tree.flatten(),
            }
            if let Err(e) = tree.check_invariants() {
                return Err(TestCaseError::fail(format!("after {op:?}: {e}\ntree: {tree}")));
            }
            let windows: BTreeSet<u64> = tree.windows().iter().map(|w| w.0).collect();
            prop_assert_eq!(&windows, &model);

            let plain = tree.layout_rects(AREA, GAPS, 30);
            // Arbitrary minimums, some impossible to satisfy together.
            let min = |id: WindowId| ((id.0 as i32 * 97) % 900, (id.0 as i32 * 53) % 700);
            let with_min = tree.layout_rects_with(AREA, GAPS, 30, &min);
            prop_assert_eq!(plain.len(), model.len());
            prop_assert_eq!(with_min.len(), model.len());
            for (_, r) in plain.into_iter().chain(with_min) {
                prop_assert!(r.w >= 0 && r.h >= 0, "negative size {:?}", r);
                prop_assert!(r.x >= AREA.x && r.right() <= AREA.right(), "{:?} outside", r);
                prop_assert!(r.y >= AREA.y && r.bottom() <= AREA.bottom(), "{:?} outside", r);
            }
        }
    }

    #[test]
    fn neighbor_is_always_another_window(
        ops in prop::collection::vec(op(), 1..40),
        probe in 0u64..12,
        d in direction(),
    ) {
        let mut tree = Container::new(Orientation::H, Layout::Tiles);
        for op in ops {
            match op {
                Op::Insert(id, anchor) if !tree.contains(WindowId(id)) => {
                    tree.insert_after(Some(WindowId(anchor)), WindowId(id));
                }
                Op::Move(id, d) => { tree.move_window(WindowId(id), d); }
                Op::Join(id, d) => { tree.join_with(WindowId(id), d); }
                _ => {}
            }
        }
        if let Some(n) = tree.neighbor(WindowId(probe), d) {
            prop_assert_ne!(n, WindowId(probe));
            prop_assert!(tree.contains(n));
        }
    }
}
