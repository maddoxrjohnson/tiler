//! Stopping cleanly on Ctrl-C and when the console window is closed.
//!
//! For a console close, Windows terminates the process as soon as the handler
//! returns, so the handler waits (up to 4 s of the 5 s Windows allows) until the
//! server has put every hidden window back.

use std::sync::OnceLock;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};
use windows::core::BOOL;
use windows::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_CLOSE_EVENT};

struct Channels {
    stop: Sender<()>,
    done: Receiver<()>,
}

static CHANNELS: OnceLock<Channels> = OnceLock::new();

unsafe extern "system" fn handler(ctrl_type: u32) -> BOOL {
    let Some(ch) = CHANNELS.get() else {
        return false.into();
    };
    let _ = ch.stop.try_send(());
    if ctrl_type == CTRL_CLOSE_EVENT {
        let _ = ch.done.recv_timeout(Duration::from_secs(4));
    }
    true.into()
}

/// Installs the handler. Returns a receiver that fires when the server should stop
/// and a sender to call once cleanup is finished.
pub fn install() -> (Receiver<()>, Sender<()>) {
    let (stop_tx, stop_rx) = bounded(1);
    let (done_tx, done_rx) = bounded(1);
    let _ = CHANNELS.set(Channels {
        stop: stop_tx,
        done: done_rx,
    });
    // SAFETY: the handler is a plain function that lives for the whole program.
    if let Err(e) = unsafe { SetConsoleCtrlHandler(Some(handler), true) } {
        tracing::warn!(%e, "could not install the console handler");
    }
    (stop_rx, done_tx)
}
