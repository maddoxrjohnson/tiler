//! Noticing monitor changes: plugging a display in or out, changing resolution or
//! scaling, or moving or auto-hiding the taskbar.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
    TranslateMessage, MSG, SPI_SETWORKAREA, WINDOW_EX_STYLE, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_ENDSESSION, WM_QUERYENDSESSION, WM_SETTINGCHANGE, WNDCLASSW, WS_OVERLAPPED,
};

struct Channels {
    display: Sender<()>,
    session_end: Sender<()>,
    cleanup_done: Receiver<()>,
}

static CHANNELS: OnceLock<Channels> = OnceLock::new();

/// Set once the session-end cleanup has been started.
static SESSION_ENDING: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Scaling changes on a monitor without the taskbar may not change any work
    // area, so the DPI override setting counts too.
    const SPI_SETLOGICALDPIOVERRIDE: u32 = 0x009F;
    let setting = wparam.0 as u32;
    let relevant = msg == WM_DISPLAYCHANGE
        || msg == WM_DPICHANGED
        || (msg == WM_SETTINGCHANGE
            && (setting == SPI_SETWORKAREA.0 || setting == SPI_SETLOGICALDPIOVERRIDE));
    let Some(ch) = CHANNELS.get() else {
        // SAFETY: default handling.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    if relevant {
        let _ = ch.display.send(());
    }
    // Logoff or shutdown. Console handlers don't get these once user32 is loaded,
    // so this is the chance to put hidden windows back. Start at the query, before
    // apps get WM_ENDSESSION and save their (parked) window positions. If the
    // shutdown is then cancelled, tiler has stopped, but no window is lost.
    // Windows waits for these messages to return (about 5 s).
    if msg == WM_QUERYENDSESSION || (msg == WM_ENDSESSION && wparam.0 != 0) {
        if !SESSION_ENDING.swap(true, Ordering::SeqCst) {
            let _ = ch.session_end.send(());
            let _ = ch.cleanup_done.recv_timeout(Duration::from_secs(4));
        }
        return LRESULT(if msg == WM_QUERYENDSESSION { 1 } else { 0 });
    }
    // SAFETY: default handling for everything else.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Starts a thread with a hidden window that sends to `display` whenever the
/// monitor setup may have changed, and to `session_end` when the user logs off or
/// Windows shuts down; it then waits for `cleanup_done` before letting Windows
/// continue. Broadcasts like WM_DISPLAYCHANGE only reach top-level windows, so a
/// message-only window would not work. Can only be called once.
pub fn spawn_display_listener(
    display: Sender<()>,
    session_end: Sender<()>,
    cleanup_done: Receiver<()>,
) {
    let channels = Channels {
        display,
        session_end,
        cleanup_done,
    };
    if CHANNELS.set(channels).is_err() {
        panic!("spawn_display_listener may only be called once");
    }
    thread::Builder::new()
        .name("display-listener".into())
        .spawn(|| {
            // SAFETY: registering a class and creating a never-shown window with a
            // static window procedure.
            let created = unsafe {
                GetModuleHandleW(None).and_then(|module| {
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(wndproc),
                        hInstance: module.into(),
                        lpszClassName: w!("tilerDisplayListener"),
                        ..Default::default()
                    };
                    RegisterClassW(&class);
                    CreateWindowExW(
                        WINDOW_EX_STYLE::default(),
                        w!("tilerDisplayListener"),
                        w!("tiler"),
                        WS_OVERLAPPED,
                        0,
                        0,
                        0,
                        0,
                        None,
                        None,
                        Some(module.into()),
                        None,
                    )
                })
            };
            if let Err(e) = created {
                tracing::error!(%e, "display listener failed; monitor changes will be missed");
                return;
            }
            let mut msg = MSG::default();
            // SAFETY: standard message loop for the window created on this thread.
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })
        .expect("failed to spawn the display listener thread");
}
