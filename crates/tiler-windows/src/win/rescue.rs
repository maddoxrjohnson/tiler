//! Bringing back windows stranded off-screen, with or without a running server.

use tiler_core::Rect;

use super::apply;
use super::monitors;
use super::window;
use crate::offscreen::rescue_target;

/// Moves app windows that are on no monitor (or would restore onto no monitor) onto
/// the primary monitor. Returns how many windows were moved.
pub fn rescue_offscreen_windows(we_are_elevated: bool) -> usize {
    let monitors = monitors::monitors();
    let Some(primary) = monitors.iter().find(|m| m.primary).or(monitors.first()) else {
        return 0;
    };
    let bounds: Vec<Rect> = monitors.iter().map(|m| m.bounds).collect();
    let mut moved = 0;
    for hwnd in window::enumerate() {
        let Some(f) = window::facts(hwnd) else {
            continue;
        };
        let normal = f.minimized.then(|| window::normal_rect(hwnd)).flatten();
        if let Some(target) = rescue_target(&f, normal, &bounds, primary.work_area, we_are_elevated)
        {
            tracing::info!(?hwnd, process = %f.process, "rescuing off-screen window");
            if apply::restore_to(hwnd, target) {
                moved += 1;
            }
        }
    }
    moved
}
