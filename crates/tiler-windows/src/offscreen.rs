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

/// Where `tiler rescue` should move a window, or `None` to leave it alone.
///
/// Only app windows tiler could manage are considered: shells and apps keep plenty
/// of helper windows off-screen on purpose. A minimized window is judged by its
/// restore position (`normal`), since every minimized window sits far off-screen.
pub fn rescue_target(
    facts: &crate::facts::WindowFacts,
    normal: Option<tiler_core::Rect>,
    monitors: &[tiler_core::Rect],
    area: tiler_core::Rect,
) -> Option<tiler_core::Rect> {
    if !crate::facts::manageable(facts, true) {
        return None;
    }
    let current = if facts.minimized { normal? } else { facts.rect };
    if monitors.iter().any(|m| m.intersects(&current)) {
        return None;
    }
    Some(tiler_core::Rect::new(
        area.x + 40,
        area.y + 40,
        current.w.min(area.w),
        current.h.min(area.h),
    ))
}

#[cfg(test)]
mod rescue_tests {
    use super::*;
    use crate::facts::{WindowFacts, WS_CAPTION, WS_EX_TOOLWINDOW, WS_THICKFRAME};
    use tiler_core::Rect;

    const SCREEN: Rect = Rect::new(0, 0, 2240, 1400);
    const AREA: Rect = Rect::new(0, 0, 2240, 1328);

    fn app(rect: Rect) -> WindowFacts {
        WindowFacts {
            style: WS_CAPTION | WS_THICKFRAME,
            visible: true,
            title: "App".into(),
            rect,
            ..Default::default()
        }
    }

    #[test]
    fn parked_app_window_is_rescued_with_its_size() {
        let f = app(Rect::new(2340, 1500, 800, 600));
        assert_eq!(
            rescue_target(&f, None, &[SCREEN], AREA),
            Some(Rect::new(40, 40, 800, 600))
        );
    }

    #[test]
    fn on_screen_windows_are_left_alone() {
        assert_eq!(
            rescue_target(&app(Rect::new(100, 100, 800, 600)), None, &[SCREEN], AREA),
            None
        );
    }

    #[test]
    fn minimized_window_is_judged_by_its_restore_position() {
        // Every minimized window sits at about (-32000, -32000).
        let f = WindowFacts {
            minimized: true,
            ..app(Rect::new(-31991, -32000, 219, 30))
        };
        let on_screen_restore = Some(Rect::new(420, 200, 1400, 900));
        assert_eq!(rescue_target(&f, on_screen_restore, &[SCREEN], AREA), None);
        let parked_restore = Some(Rect::new(2340, 1500, 1400, 900));
        assert_eq!(
            rescue_target(&f, parked_restore, &[SCREEN], AREA),
            Some(Rect::new(40, 40, 1400, 900))
        );
        assert_eq!(rescue_target(&f, None, &[SCREEN], AREA), None);
    }

    #[test]
    fn helper_windows_kept_off_screen_on_purpose_are_ignored() {
        let popup = WindowFacts {
            ex_style: WS_EX_TOOLWINDOW,
            ..app(Rect::new(62, -588, 458, 65))
        };
        assert_eq!(rescue_target(&popup, None, &[SCREEN], AREA), None);
        let zero = app(Rect::new(0, 0, 0, 0));
        assert_eq!(rescue_target(&zero, None, &[SCREEN], AREA), None);
    }
}
