//! Carrying out the core's actions.

use std::collections::HashSet;

use tiler_core::{Monitor, Rect};
use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetShellWindow, GetWindowPlacement, IsIconic, IsZoomed, PostMessageW, SetForegroundWindow,
    SetWindowPlacement, SetWindowPos, ShowWindowAsync, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, WINDOWPLACEMENT, WM_CLOSE,
};

use super::window::{self, Hwnd};
use crate::frame::outer_rect;
use crate::offscreen::offscreen_origin;

/// How windows on inactive workspaces are hidden. Selected by `hide-strategy`.
pub trait HideStrategy {
    fn hide(&mut self, hwnd: Hwnd, monitors: &[Monitor]);
    /// Undo `hide`. The core always follows this with a `set_rect`.
    fn show(&mut self, hwnd: Hwnd);
    /// Stop tracking a window without touching it (it closed, or was minimized).
    fn forget(&mut self, hwnd: Hwnd);
    fn is_hidden(&self, hwnd: Hwnd) -> bool;
    /// Windows currently hidden by this strategy.
    fn hidden(&self) -> Vec<Hwnd>;
    /// Whether hiding makes Windows report `EVENT_OBJECT_HIDE` for the window, so
    /// such events must not be mistaken for the app hiding itself.
    fn emits_hide_events(&self) -> bool;
}

/// Parks hidden windows beyond the bottom-right corner of the desktop. They stay
/// "visible" to Windows, so they remain in the taskbar and Alt-Tab.
#[derive(Default)]
pub struct OffscreenHide {
    hidden: HashSet<Hwnd>,
}

impl HideStrategy for OffscreenHide {
    fn hide(&mut self, hwnd: Hwnd, monitors: &[Monitor]) {
        let areas: Vec<Rect> = monitors.iter().map(|m| m.bounds).collect();
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

    fn forget(&mut self, hwnd: Hwnd) {
        self.hidden.remove(&hwnd);
    }

    fn is_hidden(&self, hwnd: Hwnd) -> bool {
        self.hidden.contains(&hwnd)
    }

    fn hidden(&self) -> Vec<Hwnd> {
        self.hidden.iter().copied().collect()
    }

    fn emits_hide_events(&self) -> bool {
        false
    }
}

/// Whether the window is maximized.
pub fn is_maximized(hwnd: Hwnd) -> bool {
    // SAFETY: plain query.
    unsafe { IsZoomed(hwnd.raw()) }.as_bool()
}

/// Asks a maximized window to restore itself without activating it. This is
/// asynchronous: until the window processes it, its borders are the maximized ones,
/// so callers should place the window again shortly afterwards.
pub fn unmaximize(hwnd: Hwnd) {
    // SAFETY: ShowWindowAsync never blocks on the target thread.
    let _ = unsafe { ShowWindowAsync(hwnd.raw(), SW_SHOWNOACTIVATE) };
}

/// Moves a window so its *visible* frame is exactly `visible`. Never blocks on the
/// target window's thread.
pub fn set_rect(hwnd: Hwnd, visible: Rect) {
    let r = outer_rect(visible, window::borders(hwnd));
    // SAFETY: ASYNCWINDOWPOS keeps a hung app from blocking tiler.
    let result = unsafe {
        SetWindowPos(
            hwnd.raw(),
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

/// Puts a window back on screen at `visible`, also when it is minimized or
/// maximized: such a window remembers its restore position separately, and that
/// position would otherwise still be the off-screen parking spot. Returns whether
/// Windows accepted the change.
pub fn restore_to(hwnd: Hwnd, visible: Rect) -> bool {
    restore_with(hwnd, visible, false)
}

/// Like [`restore_to`], but a normal window's move is queued like every other
/// tiler move, so it can't overtake a hide the app hasn't processed yet, and a
/// hung app can't block the caller.
pub fn restore_to_queued(hwnd: Hwnd, visible: Rect) -> bool {
    restore_with(hwnd, visible, true)
}

fn restore_with(hwnd: Hwnd, visible: Rect, queued: bool) -> bool {
    // SAFETY: plain queries.
    let (minimized, maximized) = unsafe {
        (
            IsIconic(hwnd.raw()).as_bool(),
            IsZoomed(hwnd.raw()).as_bool(),
        )
    };
    if !minimized && !maximized {
        let r = outer_rect(visible, window::borders(hwnd));
        // Synchronous unless queued, so the result says whether it worked; rescue
        // is rare and user-initiated.
        let flags = if queued {
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS
        } else {
            SWP_NOZORDER | SWP_NOACTIVATE
        };
        // SAFETY: plain window-management call.
        return unsafe { SetWindowPos(hwnd.raw(), None, r.x, r.y, r.w, r.h, flags) }.is_ok();
    }
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: length is set; the struct outlives both calls.
    unsafe {
        if GetWindowPlacement(hwnd.raw(), &mut wp).is_err() {
            return false;
        }
        // Keep minimized windows minimized; bring maximized ones back to normal.
        wp.showCmd = if minimized {
            SW_SHOWMINNOACTIVE.0 as u32
        } else {
            SW_SHOWNOACTIVATE.0 as u32
        };
        wp.rcNormalPosition = RECT {
            left: visible.x,
            top: visible.y,
            right: visible.right(),
            bottom: visible.bottom(),
        };
        match SetWindowPlacement(hwnd.raw(), &wp) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(?hwnd, %e, "could not reset the restore position");
                false
            }
        }
    }
}

/// Gives keyboard focus to a window, or to the desktop when `None`.
pub fn focus(hwnd: Option<Hwnd>) {
    // Windows only lets the process that received the last input event change the
    // foreground window. A zero-length relative mouse move makes that us without the
    // side effects of an Alt key press (which can open menus). It is a real input
    // event, so it also resets idle timers.
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
