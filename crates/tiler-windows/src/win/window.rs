//! Reading facts about top-level windows.

use std::ffi::c_void;

use tiler_core::{Rect, WindowId};
use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindow, GetWindowLongW, GetWindowPlacement, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE, GWL_STYLE,
    GW_OWNER, WINDOWPLACEMENT,
};

use crate::facts::WindowFacts;
use crate::frame::Borders;

/// A window handle that can be sent between threads and used as a [`WindowId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hwnd(pub isize);

impl Hwnd {
    pub fn raw(self) -> HWND {
        HWND(self.0 as *mut c_void)
    }

    pub fn id(self) -> WindowId {
        WindowId(self.0 as u64)
    }

    pub fn from_id(id: WindowId) -> Hwnd {
        Hwnd(id.0 as isize)
    }
}

impl From<HWND> for Hwnd {
    fn from(h: HWND) -> Self {
        Hwnd(h.0 as isize)
    }
}

pub(crate) fn rect_from(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top)
}

/// All top-level windows, in z-order (topmost first).
pub fn enumerate() -> Vec<Hwnd> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the &mut Vec passed below, alive for the whole call.
        let list = unsafe { &mut *(lparam.0 as *mut Vec<Hwnd>) };
        list.push(hwnd.into());
        true.into()
    }
    let mut list: Vec<Hwnd> = Vec::new();
    // SAFETY: the callback only touches `list`, which outlives EnumWindows.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut list as *mut _ as isize)) };
    list
}

/// The outer rect (including invisible borders), as `GetWindowRect` reports it.
pub fn outer_rect(hwnd: Hwnd) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: valid out pointer; an invalid hwnd just returns an error.
    unsafe { GetWindowRect(hwnd.raw(), &mut r) }.ok()?;
    Some(rect_from(r))
}

/// The visible frame as drawn by DWM; falls back to the outer rect.
pub fn visible_rect(hwnd: Hwnd) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: the out buffer is exactly the size DWM expects for this attribute.
    let dwm = unsafe {
        DwmGetWindowAttribute(
            hwnd.raw(),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    match dwm {
        Ok(()) => Some(rect_from(r)),
        Err(_) => outer_rect(hwnd),
    }
}

/// Where the window goes when restored from minimized or maximized.
pub fn normal_rect(hwnd: Hwnd) -> Option<Rect> {
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: length is set and the struct outlives the call.
    unsafe { GetWindowPlacement(hwnd.raw(), &mut wp) }.ok()?;
    Some(rect_from(wp.rcNormalPosition))
}

pub fn borders(hwnd: Hwnd) -> Borders {
    match (outer_rect(hwnd), visible_rect(hwnd)) {
        (Some(outer), Some(visible)) => Borders::between(outer, visible),
        _ => Borders::default(),
    }
}

fn is_cloaked(hwnd: Hwnd) -> bool {
    let mut cloaked: u32 = 0;
    // SAFETY: DWMWA_CLOAKED writes a u32.
    let r = unsafe {
        DwmGetWindowAttribute(
            hwnd.raw(),
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    r.is_ok() && cloaked != 0
}

fn text(read: impl FnOnce(&mut [u16]) -> i32) -> String {
    let mut buf = [0u16; 512];
    let n = read(&mut buf).max(0) as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

pub fn title(hwnd: Hwnd) -> String {
    // SAFETY: the buffer is valid for its whole length.
    text(|b| unsafe { GetWindowTextW(hwnd.raw(), b) })
}

pub fn class(hwnd: Hwnd) -> String {
    // SAFETY: the buffer is valid for its whole length.
    text(|b| unsafe { GetClassNameW(hwnd.raw(), b) })
}

/// Closes a handle when dropped.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: we own this handle and close it exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Whether a process's token is elevated, or `None` if we cannot tell.
fn token_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    // SAFETY: valid process handle and out pointer.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    let token = OwnedHandle(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: the buffer is a TOKEN_ELEVATION of the size we pass.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    ok.ok()?;
    Some(elevation.TokenIsElevated != 0)
}

/// Whether this process runs as administrator.
pub fn is_process_elevated() -> bool {
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing.
    token_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false)
}

/// Executable name and elevation of the process that owns a window. Processes we
/// cannot open (usually elevated ones) are reported as elevated.
fn process_info(hwnd: Hwnd) -> (String, bool) {
    let mut pid = 0u32;
    // SAFETY: valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd.raw(), Some(&mut pid)) };
    // SAFETY: plain call; failure is handled.
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return (String::new(), true);
    };
    let handle = OwnedHandle(handle);
    let mut buf = [0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: buf is valid for `len` u16s.
    let path = match unsafe {
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    } {
        Ok(()) => String::from_utf16_lossy(&buf[..len as usize]),
        Err(_) => String::new(),
    };
    let exe = path.rsplit('\\').next().unwrap_or_default().to_owned();
    if exe.is_empty() {
        tracing::debug!(?hwnd, "could not read the process image name");
    }
    // A token we may not open usually belongs to an elevated process; assume so,
    // since managing an elevated window would silently fail (UIPI).
    (exe, token_elevated(handle.0).unwrap_or(true))
}

/// Everything the heuristics need, or `None` if the window is gone or invisible.
/// Invisible windows are rejected first because they are common and the process
/// lookup below costs several system calls.
pub fn facts(hwnd: Hwnd) -> Option<WindowFacts> {
    let h = hwnd.raw();
    // SAFETY: plain query.
    if !unsafe { IsWindowVisible(h) }.as_bool() {
        return None;
    }
    let rect = visible_rect(hwnd)?;
    // SAFETY: these read-only queries accept any hwnd and fail softly.
    let (style, ex_style, has_owner, visible, minimized) = unsafe {
        (
            GetWindowLongW(h, GWL_STYLE) as u32,
            GetWindowLongW(h, GWL_EXSTYLE) as u32,
            GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()),
            IsWindowVisible(h).as_bool(),
            IsIconic(h).as_bool(),
        )
    };
    let (process, elevated) = process_info(hwnd);
    Some(WindowFacts {
        style,
        ex_style,
        has_owner,
        visible,
        cloaked: is_cloaked(hwnd),
        minimized,
        class: class(hwnd),
        title: title(hwnd),
        process,
        elevated,
        rect,
    })
}
