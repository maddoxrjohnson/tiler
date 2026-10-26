//! Messages between the `tiler` CLI and the server, sent over a named pipe.
//!
//! Each message is a 4-byte little-endian length followed by that many bytes of JSON.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Messages larger than this are rejected, so a bad length can't exhaust memory.
pub const MAX_MESSAGE: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The command and its arguments, e.g. `["workspace", "2"]`.
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub output: String,
}

/// The pipe of one user's server in one Windows session. Keyed by SID (not user
/// name) so another user can't claim it, and by session so a console and a remote
/// desktop session of the same user don't collide.
pub fn pipe_name(user_sid: &str, session: u32) -> String {
    format!(r"\\.\pipe\tiler-{user_sid}-{session}")
}

pub fn write_message<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "message too large",
        ));
    }
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

pub fn read_message<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn pipe_name_is_per_user_and_session() {
        assert_eq!(
            pipe_name("S-1-5-21-1-2-3-1001", 1),
            r"\\.\pipe\tiler-S-1-5-21-1-2-3-1001-1"
        );
        assert_ne!(pipe_name("S-1", 1), pipe_name("S-1", 2));
    }

    #[test]
    fn round_trip() {
        let mut buf = Vec::new();
        let req = Request {
            args: vec!["workspace".into(), "my notes".into()],
        };
        write_message(&mut buf, &req).unwrap();
        let resp = Response {
            ok: false,
            output: "unknown command 'x'".into(),
        };
        write_message(&mut buf, &resp).unwrap();
        let mut r = Cursor::new(buf);
        assert_eq!(read_message::<Request>(&mut r).unwrap(), req);
        assert_eq!(read_message::<Response>(&mut r).unwrap(), resp);
    }

    #[test]
    fn truncated_input_is_an_eof_error() {
        let mut buf = Vec::new();
        write_message(
            &mut buf,
            &Request {
                args: vec!["quit".into()],
            },
        )
        .unwrap();
        buf.truncate(buf.len() - 1);
        let e = read_message::<Request>(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn oversized_length_is_rejected_without_allocating() {
        let buf = (u32::MAX).to_le_bytes().to_vec();
        let e = read_message::<Request>(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn garbage_json_is_an_error() {
        let mut buf = 3u32.to_le_bytes().to_vec();
        buf.extend_from_slice(b"{x}");
        assert!(read_message::<Request>(&mut Cursor::new(buf)).is_err());
    }
}
