//! Basic geometry and identifier types shared by the whole crate.

/// Opaque identifier of a top-level window (an `HWND` value on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

/// Opaque identifier of a monitor (an `HMONITOR` value on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MonitorId(pub u64);

/// An axis-aligned rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Amounts to shrink a rectangle by on each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sides {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Sides {
    pub const fn all(v: i32) -> Self {
        Self {
            left: v,
            top: v,
            right: v,
            bottom: v,
        }
    }
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// Shrinks the rectangle by `sides`; width and height never go below zero.
    pub fn inset(&self, sides: Sides) -> Rect {
        Rect {
            x: self.x + sides.left,
            y: self.y + sides.top,
            w: (self.w - sides.left - sides.right).max(0),
            h: (self.h - sides.top - sides.bottom).max(0),
        }
    }

    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// The axis along which a container lays out its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// Children side by side, left to right.
    H,
    /// Children stacked, top to bottom.
    V,
}

impl Orientation {
    pub fn opposite(self) -> Orientation {
        match self {
            Orientation::H => Orientation::V,
            Orientation::V => Orientation::H,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub const ALL: [Direction; 4] = [
        Direction::Left,
        Direction::Right,
        Direction::Up,
        Direction::Down,
    ];

    /// The orientation a container must have for this direction to move within it.
    pub fn orientation(self) -> Orientation {
        match self {
            Direction::Left | Direction::Right => Orientation::H,
            Direction::Up | Direction::Down => Orientation::V,
        }
    }

    /// Whether this direction moves towards higher child indices (right or down).
    pub fn is_forward(self) -> bool {
        matches!(self, Direction::Right | Direction::Down)
    }

    pub fn name(self) -> &'static str {
        match self {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inset_shrinks_each_side() {
        let r = Rect::new(0, 0, 100, 50).inset(Sides {
            left: 1,
            top: 2,
            right: 3,
            bottom: 4,
        });
        assert_eq!(r, Rect::new(1, 2, 96, 44));
    }

    #[test]
    fn inset_never_goes_negative() {
        let r = Rect::new(10, 10, 5, 5).inset(Sides::all(10));
        assert_eq!(r.w, 0);
        assert_eq!(r.h, 0);
    }

    #[test]
    fn center_is_midpoint() {
        assert_eq!(Rect::new(10, 20, 100, 50).center(), (60, 45));
    }

    #[test]
    fn direction_orientation_and_forward() {
        assert_eq!(Direction::Left.orientation(), Orientation::H);
        assert_eq!(Direction::Down.orientation(), Orientation::V);
        assert!(Direction::Right.is_forward());
        assert!(!Direction::Up.is_forward());
        assert_eq!(Orientation::H.opposite(), Orientation::V);
    }
}
