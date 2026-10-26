//! Where hidden windows are parked.

/// Margin between the parked windows and the edge of the desktop.
const MARGIN: i32 = 100;

/// A point to the bottom right of every monitor, so a window whose top-left corner
/// is placed there cannot be seen on any of them, whatever the monitor layout.
pub fn offscreen_origin(monitors: &[tiler_core::Rect]) -> (i32, i32) {
    let right = monitors.iter().map(|m| m.right()).max().unwrap_or(0);
    let bottom = monitors.iter().map(|m| m.bottom()).max().unwrap_or(0);
    (right + MARGIN, bottom + MARGIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiler_core::Rect;

    #[test]
    fn beyond_every_monitor() {
        let monitors = [
            Rect::new(-1920, 200, 1920, 1080),
            Rect::new(0, 0, 2560, 1440),
            Rect::new(2560, -400, 1080, 1920),
        ];
        assert_eq!(offscreen_origin(&monitors), (3740, 1620));
    }

    #[test]
    fn no_monitors() {
        assert_eq!(offscreen_origin(&[]), (100, 100));
    }
}
