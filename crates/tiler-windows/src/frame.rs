//! Invisible window borders.
//!
//! On Windows 10 and 11, `GetWindowRect` includes invisible resize borders around
//! most windows, while users (and tiler's layout) think in terms of the visible
//! frame reported by DWM. `SetWindowPos` takes the outer rect, so tiler converts.

use tiler_core::Rect;

/// Thickness of the invisible border on each side: outer rect minus visible frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Borders {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Borders {
    /// Borders given a window's outer rect and its visible frame.
    pub fn between(outer: Rect, visible: Rect) -> Borders {
        Borders {
            left: visible.x - outer.x,
            top: visible.y - outer.y,
            right: outer.right() - visible.right(),
            bottom: outer.bottom() - visible.bottom(),
        }
    }
}

/// The outer rect to give `SetWindowPos` so the visible frame lands exactly on
/// `visible`.
pub fn outer_rect(visible: Rect, b: Borders) -> Rect {
    Rect::new(
        visible.x - b.left,
        visible.y - b.top,
        visible.w + b.left + b.right,
        visible.h + b.top + b.bottom,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typical_windows_11_borders() {
        // 7 px invisible borders on left, right and bottom; none on top.
        let outer = Rect::new(93, 100, 814, 607);
        let visible = Rect::new(100, 100, 800, 600);
        let b = Borders::between(outer, visible);
        assert_eq!(
            b,
            Borders {
                left: 7,
                top: 0,
                right: 7,
                bottom: 7
            }
        );
        assert_eq!(
            outer_rect(Rect::new(8, 8, 948, 1064), b),
            Rect::new(1, 8, 962, 1071)
        );
    }

    #[test]
    fn round_trip() {
        let b = Borders {
            left: 3,
            top: 1,
            right: 5,
            bottom: 2,
        };
        let visible = Rect::new(10, 20, 300, 400);
        assert_eq!(Borders::between(outer_rect(visible, b), visible), b);
    }
}
