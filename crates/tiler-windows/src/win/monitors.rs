//! Enumerating displays.

use std::ffi::c_void;

use tiler_core::{Monitor, MonitorId};
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

use super::window::rect_from;

/// Every monitor with its work area (the screen minus the taskbar).
pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(
        hmon: HMONITOR,
        _: HDC,
        _: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        // SAFETY: lparam is the &mut Vec passed below, alive for the whole call.
        let list = unsafe { &mut *(lparam.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: cbSize tells Windows the buffer is a MONITORINFOEXW.
        let ok = unsafe { GetMonitorInfoW(hmon, &mut info as *mut _ as *mut MONITORINFO) };
        if ok.as_bool() {
            let name_len = info.szDevice.iter().position(|c| *c == 0).unwrap_or(32);
            list.push(Monitor {
                id: MonitorId(hmon.0 as usize as u64),
                bounds: rect_from(info.monitorInfo.rcMonitor),
                work_area: rect_from(info.monitorInfo.rcWork),
                scale: 1.0,
                name: String::from_utf16_lossy(&info.szDevice[..name_len]),
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        true.into()
    }
    let mut list: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only touches `list`, which outlives the call.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut list as *mut _ as *mut c_void as isize),
        )
    };
    list
}
