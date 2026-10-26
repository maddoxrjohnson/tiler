//! The recovery file: which windows tiler has parked off-screen, and where they
//! were. Written whenever that changes, so a crash or a hard kill can be undone by
//! the next `tiler` start or by `tiler rescue`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parked {
    pub hwnd: i64,
    /// The visible frame before it was parked: x, y, width, height.
    pub rect: [i32; 4],
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RecoveryFile {
    /// The server that wrote the file.
    pub pid: u32,
    pub parked: Vec<Parked>,
}

/// `%LOCALAPPDATA%\tiler\recovery.json`.
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("tiler").join("recovery.json"))
}

/// Writes atomically (temp file + rename), so a crash mid-write can't leave a
/// truncated file.
pub fn save(path: &Path, file: &RecoveryFile) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(file)?)?;
    std::fs::rename(&tmp, path)
}

/// The saved file, or `None` if there is none or it can't be read.
pub fn load(path: &Path) -> Option<RecoveryFile> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("tiler-test-{}-{name}", std::process::id()))
            .join("recovery.json")
    }

    #[test]
    fn round_trip_and_remove() {
        let path = temp_path("roundtrip");
        let file = RecoveryFile {
            pid: 42,
            parked: vec![Parked {
                hwnd: 0x1234,
                rect: [8, 8, 948, 1064],
            }],
        };
        save(&path, &file).unwrap();
        assert_eq!(load(&path), Some(file));
        remove(&path);
        assert_eq!(load(&path), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn corrupt_or_missing_files_are_ignored() {
        let path = temp_path("corrupt");
        assert_eq!(load(&path), None);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{\"pid\": 1, \"parked\": [").unwrap();
        assert_eq!(load(&path), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

/// Putting parked windows back, from the recovery file or from memory.
#[cfg(windows)]
pub mod restore {
    use std::sync::Mutex;

    use tiler_core::Rect;
    use tiler_windows::win::{apply, monitors, window, Hwnd};

    use super::{Parked, RecoveryFile};

    /// The parked windows, mirrored for the panic hook (which can't reach the
    /// server's state).
    static PARKED: Mutex<Vec<Parked>> = Mutex::new(Vec::new());

    pub fn mirror(parked: &[Parked]) {
        if let Ok(mut p) = PARKED.lock() {
            p.clear();
            p.extend_from_slice(parked);
        }
    }

    /// Restores every parked window recorded in memory. Used by the panic hook.
    pub fn restore_mirrored() -> usize {
        let parked = PARKED.lock().map(|p| p.clone()).unwrap_or_default();
        restore_all(&parked)
    }

    pub fn rect_of(p: &Parked) -> Rect {
        Rect::new(p.rect[0], p.rect[1], p.rect[2], p.rect[3])
    }

    /// Puts each window back where it was, if it still exists and is still off
    /// screen. A rect that is no longer on any monitor goes to the primary one.
    pub fn restore_all(parked: &[Parked]) -> usize {
        let monitors = monitors::monitors();
        let Some(primary) = monitors.iter().find(|m| m.primary).or(monitors.first()) else {
            return 0;
        };
        let on_screen = |r: &Rect| monitors.iter().any(|m| m.bounds.intersects(r));
        let mut restored = 0;
        for p in parked {
            let hwnd = Hwnd(p.hwnd as isize);
            // Gone, or (if the handle was reused) a different window that is on
            // screen: leave it alone.
            let Some(now) = window::visible_rect(hwnd) else {
                continue;
            };
            if on_screen(&now) {
                continue;
            }
            let saved = rect_of(p);
            let target = if on_screen(&saved) {
                saved
            } else {
                let a = primary.work_area;
                Rect::new(a.x + 40, a.y + 40, saved.w.min(a.w), saved.h.min(a.h))
            };
            if apply::restore_to(hwnd, target) {
                restored += 1;
            }
        }
        restored
    }

    /// Restores the windows listed in a recovery file left by a server that did
    /// not exit cleanly, then deletes the file.
    pub fn from_file(path: &std::path::Path) -> usize {
        let Some(RecoveryFile { parked, .. }) = super::load(path) else {
            return 0;
        };
        let restored = restore_all(&parked);
        super::remove(path);
        restored
    }
}
