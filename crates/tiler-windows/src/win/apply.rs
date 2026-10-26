//! Carrying out the core's actions.

use std::collections::HashSet;

use tiler_core::{Monitor, Rect};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetShellWindow, IsZoomed, PostMessageW, SetForegroundWindow, SetWindowPos, ShowWindow,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_RESTORE, WM_CLOSE,
};

use super::window::{self, Hwnd};
use crate::frame::outer_rect;
use crate::offscreen::offscreen_origin;

/// How windows on inactive workspaces are hidden. Selected by `hide-strategy`.
pub trait HideStrategy {
    fn hide(&mut self, hwnd: Hwnd, monitors: &[Monitor]);
    /// Undo `hide`. The core always follows this with a `set_rect`.
    fn show(&mut self, hwnd: Hwnd);
    /// Windows currently hidden by this strategy.
    fn hidden(&self) -> Vec<Hwnd>;
}

/// Parks hidden windows beyond the bottom-right corner of the desktop. They stay
/// "visible" to Windows, so they remain in the taskbar and Alt-Tab.
#[derive(Default)]
pub struct OffscreenHide {
    hidden: HashSet<Hwnd>,
}

impl HideStrategy for OffscreenHide {
    fn hide(&mut self, hwnd: Hwnd, monitors: &[Monitor]) {
        let areas: Vec<Rect> = monitors.iter().map(|m| m.work_area).collect();
        let (x, y) = offscreen_origin(&areas);
        // SAFETY: SetWindowPos on another process's window is allowed (UIPI permitting).
        let r = unsafe {
            SetWindowPos(
                hwnd.raw(),
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        };
        if let Err(e) = r {
            tracing::warn!(?hwnd, %e, "hide failed");
        }
        self.hidden.insert(hwnd);
    }

    fn show(&mut self, hwnd: Hwnd) {
        self.hidden.remove(&hwnd);
    }

    fn hidden(&self) -> Vec<Hwnd> {
        self.hidden.iter().copied().collect()
    }
}

/// Moves a window so its *visible* frame is exactly `visible`.
pub fn set_rect(hwnd: Hwnd, visible: Rect) {
    let h = hwnd.raw();
    // SAFETY: plain window-management calls on a window handle.
    unsafe {
        if IsZoomed(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
    }
    let r = outer_rect(visible, window::borders(hwnd));
    // SAFETY: as above. ASYNCWINDOWPOS keeps a hung app from blocking tiler.
    let result = unsafe {
        SetWindowPos(
            h,
            None,
            r.x,
            r.y,
            r.w,
            r.h,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        )
    };
    if let Err(e) = result {
        tracing::warn!(?hwnd, %e, "set_rect failed");
    }
}

/// Gives keyboard focus to a window, or to the desktop when `None`.
pub fn focus(hwnd: Option<Hwnd>) {
    // Windows only lets the process that received the last input event change the
    // foreground window. A zero-length mouse move makes that us.
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: MOUSEEVENTF_MOVE,
                ..Default::default()
            },
        },
    };
    // SAFETY: a single well-formed INPUT.
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    // SAFETY: plain calls; failure only means focus does not change.
    let target = hwnd
        .map(Hwnd::raw)
        .unwrap_or_else(|| unsafe { GetShellWindow() });
    if !unsafe { SetForegroundWindow(target) }.as_bool() {
        tracing::debug!(?hwnd, "SetForegroundWindow refused");
    }
}

/// Asks a window to close, as if the user clicked its close button.
pub fn close(hwnd: Hwnd) {
    // SAFETY: posting WM_CLOSE is always allowed.
    let _ = unsafe { PostMessageW(Some(hwnd.raw()), WM_CLOSE, WPARAM(0), LPARAM(0)) };
}
