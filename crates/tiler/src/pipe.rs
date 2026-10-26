//! The server side of the CLI pipe.

use std::fs::File;
use std::io;
use std::os::windows::io::FromRawHandle;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Sender};
use windows::core::HSTRING;
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

use crate::ipc::{pipe_name, read_message, write_message, Request, Response};

/// A CLI request and where to send its response.
pub type Incoming = (Request, Sender<Response>);

/// How long a client waits for the server loop to answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

fn create_instance(first: bool) -> io::Result<HANDLE> {
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: plain call with a valid, owned name string.
    let handle = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(pipe_name()),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            None,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

/// Claims the pipe name. Fails if another tiler server already owns it, which makes
/// this the single-instance check.
pub fn claim() -> io::Result<HANDLE> {
    create_instance(true)
}

fn serve_one(handle: HANDLE, tx: &Sender<Incoming>) -> io::Result<()> {
    // SAFETY: blocks until a client connects; an already-connected client is fine.
    let _ = unsafe { ConnectNamedPipe(handle, None) };
    // SAFETY: we own the handle; File closes it when dropped.
    let mut pipe = unsafe { File::from_raw_handle(handle.0 as _) };
    let request: Request = read_message(&mut pipe)?;
    let (reply_tx, reply_rx) = bounded(1);
    let response = match tx.send((request, reply_tx)) {
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
    pipe.sync_all()
}

/// Serves CLI clients one at a time, starting with the instance from [`claim`].
pub fn spawn_pipe_server(first: HANDLE, tx: Sender<Incoming>) {
    let first = first.0 as isize;
    thread::Builder::new()
        .name("cli-pipe".into())
        .spawn(move || {
            let mut handle = HANDLE(first as _);
            loop {
                if let Err(e) = serve_one(handle, &tx) {
                    tracing::debug!(%e, "CLI client failed");
                }
                handle = match create_instance(false) {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::error!(%e, "CLI pipe stopped");
                        return;
                    }
                };
            }
        })
        .expect("failed to spawn the CLI pipe thread");
}
