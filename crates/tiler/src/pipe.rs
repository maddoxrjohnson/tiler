//! The server side of the CLI pipe.

use std::fs::File;
use std::io;
use std::os::windows::io::FromRawHandle;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use tiler_windows::win::identity::{self, OwnerOnly};
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

use crate::ipc::{read_message, write_message, Request, Response};

/// How long a client waits for the server loop to answer.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// A CLI request, where to send its response, and when the client stops waiting.
pub struct Incoming {
    pub request: Request,
    pub reply: Sender<Response>,
    /// Requests still queued after this were already answered with a timeout and
    /// must not run.
    pub deadline: Instant,
    /// Fires once the response has been written to the client.
    pub written: Receiver<()>,
}

/// This user's pipe name in this session.
pub fn name() -> String {
    let sid = identity::user_sid().unwrap_or_else(|| "unknown".into());
    crate::ipc::pipe_name(&sid, identity::session_id())
}

/// A listening pipe instance.
pub struct Instance(isize);

impl Instance {
    /// Hands the handle to the caller, who becomes responsible for closing it.
    fn into_handle(self) -> HANDLE {
        let handle = HANDLE(self.0 as _);
        std::mem::forget(self);
        handle
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: we own the handle and nothing else has taken it.
        let _ = unsafe { CloseHandle(HANDLE(self.0 as _)) };
    }
}

fn create_instance(first: bool) -> io::Result<Instance> {
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // Only this user may connect, even when the server runs elevated.
    let security = identity::user_sid().and_then(|sid| OwnerOnly::new(&sid));
    // SAFETY: valid name; the security attributes outlive the call.
    let handle = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(name()),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            security.as_ref().map(|s| &s.attributes as *const _),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(Instance(handle.0 as isize))
}

/// Claims the pipe name. Fails if another tiler server already owns it, which makes
/// this the single-instance check.
pub fn claim() -> io::Result<Instance> {
    create_instance(true)
}

/// Creates the next instance, retrying so a transient failure doesn't end the CLI.
fn next_instance() -> Instance {
    let mut delay = Duration::from_millis(100);
    loop {
        match create_instance(false) {
            Ok(i) => return i,
            Err(e) => {
                tracing::warn!(%e, "could not create a CLI pipe instance; retrying");
                thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_secs(5));
            }
        }
    }
}

/// Blocks until a client connects to this instance.
fn wait_for_client(instance: &Instance) {
    // SAFETY: an already-connected client also counts as connected.
    let _ = unsafe { ConnectNamedPipe(HANDLE(instance.0 as _), None) };
}

fn serve_one(instance: Instance, tx: &Sender<Incoming>) -> io::Result<()> {
    let handle = instance.into_handle();
    // SAFETY: we own the handle; File closes it when dropped.
    let mut pipe = unsafe { File::from_raw_handle(handle.0 as _) };
    let request: Request = read_message(&mut pipe)?;
    let (reply_tx, reply_rx) = bounded(1);
    let (written_tx, written_rx): (Sender<()>, Receiver<()>) = bounded(1);
    let incoming = Incoming {
        request,
        reply: reply_tx,
        deadline: Instant::now() + ANSWER_TIMEOUT,
        written: written_rx,
    };
    let response = match tx.send(incoming) {
        Ok(()) => reply_rx.recv_timeout(ANSWER_TIMEOUT).unwrap_or(Response {
            ok: false,
            output: "the server did not answer in time".into(),
        }),
        Err(_) => Response {
            ok: false,
            output: "the server is shutting down".into(),
        },
    };
    write_message(&mut pipe, &response)?;
    // Wait until the client has read everything before the handle is closed.
    let flushed = pipe.sync_all();
    let _ = written_tx.send(());
    flushed
}

/// Accepts CLI clients and serves each on its own short-lived thread, so a client
/// that stalls only stalls itself. The next instance is always created before a
/// client is handed off, so the pipe name never disappears while the server runs
/// (a client would otherwise think no server is running).
pub fn spawn_pipe_server(first: Instance, tx: Sender<Incoming>) {
    thread::Builder::new()
        .name("cli-pipe".into())
        .spawn(move || {
            let mut current = first;
            loop {
                let next = next_instance();
                wait_for_client(&current);
                let tx = tx.clone();
                let spawned = thread::Builder::new()
                    .name("cli-client".into())
                    .spawn(move || {
                        if let Err(e) = serve_one(current, &tx) {
                            tracing::debug!(%e, "CLI client failed");
                        }
                    });
                if let Err(e) = spawned {
                    tracing::warn!(%e, "could not start a CLI client thread");
                }
                current = next;
            }
        })
        .expect("failed to spawn the CLI pipe thread");
}
