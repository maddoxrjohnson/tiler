//! Listening to window events from every process.

use std::sync::OnceLock;
use std::thread;

use crossbeam_channel::Sender;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetAncestor, GetMessageW, TranslateMessage, CHILDID_SELF, GA_ROOT, MSG,
    OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

pub use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_NAMECHANGE,
    EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MOVESIZEEND,
};

/// The event ranges tiler listens to. Narrow ranges matter: every process marshals
/// every event in range to us, and the full range includes caret and cursor moves.
const RANGES: &[(u32, u32)] = &[
    (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
    (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND),
    (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
    (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
    (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
    (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
];

use super::window::Hwnd;

/// A window event exactly as Windows reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEvent {
    pub event: u32,
    pub hwnd: Hwnd,
}

static SENDER: OnceLock<Sender<RawEvent>> = OnceLock::new();

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 || hwnd.is_invalid() {
        return;
    }
    // Skip child windows. Out-of-context events arrive after the fact, so a destroyed
    // window has no root any more: only drop events whose root is known to differ.
    // SAFETY: GetAncestor accepts any hwnd.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if !root.is_invalid() && root != hwnd {
        return;
    }
    if let Some(tx) = SENDER.get() {
        let _ = tx.send(RawEvent {
            event,
            hwnd: hwnd.into(),
        });
    }
}

/// Starts a thread that forwards every top-level window event to `tx`. Can only be
/// called once per process.
pub fn spawn_event_hook(tx: Sender<RawEvent>) {
    SENDER
        .set(tx)
        .expect("spawn_event_hook may only be called once");
    thread::Builder::new()
        .name("win-event-hook".into())
        .spawn(|| {
            for &(min, max) in RANGES {
                // SAFETY: out-of-context hooks need no module handle; the callback is a
                // plain function that lives for the whole program.
                let hook = unsafe {
                    SetWinEventHook(
                        min,
                        max,
                        None,
                        Some(on_event),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                    )
                };
                if hook.is_invalid() {
                    tracing::error!(
                        min,
                        max,
                        "SetWinEventHook failed; some window events will be missed"
                    );
                }
            }
            // Out-of-context hooks deliver events through this thread's message queue.
            let mut msg = MSG::default();
            // SAFETY: standard message loop on the thread that installed the hooks.
            // GetMessageW returns -1 on error and 0 on WM_QUIT; stop on both.
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })
        .expect("failed to spawn the event hook thread");
}
