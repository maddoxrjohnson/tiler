//! The `tiler <command>` side of the CLI pipe.

use std::fs::{File, OpenOptions};
use std::io;
use std::thread;
use std::time::{Duration, Instant};

use crate::ipc::{read_message, write_message, Request, Response};
use crate::pipe;

/// Windows error codes for opening a pipe.
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PIPE_BUSY: i32 = 231;

pub enum ClientError {
    /// No server is listening.
    NotRunning,
    Io(io::Error),
}

fn connect() -> Result<File, ClientError> {
    // Longer than the server's answer timeout, so a slow request ahead of us
    // doesn't make this one fail.
    let deadline = Instant::now() + pipe::ANSWER_TIMEOUT + Duration::from_secs(2);
    loop {
        match OpenOptions::new().read(true).write(true).open(pipe::name()) {
            Ok(f) => return Ok(f),
            Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => {
                return Err(ClientError::NotRunning)
            }
            // The server is answering another client; its next instance is coming.
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(ClientError::Io(e)),
        }
    }
}

/// Sends a command to the running server and returns its response.
pub fn send(args: &[String]) -> Result<Response, ClientError> {
    let mut pipe = connect()?;
    write_message(
        &mut pipe,
        &Request {
            args: args.to_vec(),
        },
    )
    .map_err(ClientError::Io)?;
    read_message(&mut pipe).map_err(ClientError::Io)
}
