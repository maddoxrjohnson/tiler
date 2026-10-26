//! The recovery file: which windows tiler has parked off-screen, and where they
//! were. Written whenever that changes, so a crash or a hard kill can be undone by
//! the next `tiler` start or by `tiler rescue`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parked {
    pub hwnd: i64,
    /// The visible frame before it was parked: x, y, width, height.
    pub rect: [i32; 4],
    /// The owning executable, to recognise the window if its handle was reused.
    #[serde(default)]
    pub exe: String,
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
                exe: "Code.exe".into(),
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

    use tiler_core::{Monitor, Rect};
    use tiler_windows::win::{apply, monitors, window, Hwnd};

    use super::{Parked, RecoveryFile};

    /// The parked windows, mirrored for the panic hook (which can't reach the
    /// server's state).
    static PARKED: Mutex<Vec<Parked>> = Mutex::new(Vec::new());

    pub fn mirror(parked: &[Parked]) {
        let mut p = PARKED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        p.clear();
        p.extend_from_slice(parked);
    }

    /// Restores every parked window recorded in memory. Used by the panic hook.
    pub fn restore_mirrored() -> usize {
        let parked = PARKED
            .lock()
            .map(|p| p.clone())
            .unwrap_or_else(|e| e.into_inner().clone());
        restore_ours(&parked)
    }

    fn rect_of(p: &Parked) -> Rect {
        Rect::new(p.rect[0], p.rect[1], p.rect[2], p.rect[3])
    }

    /// Whether the middle of `r` is on one of the monitors (a sliver of overlap
    /// doesn't make a window usable).
    fn on_screen(monitors: &[Monitor], r: &Rect) -> bool {
        let (cx, cy) = r.center();
        monitors.iter().any(|m| {
            let b = m.bounds;
            (b.x..b.right()).contains(&cx) && (b.y..b.bottom()).contains(&cy)
        })
    }

    /// Where a window should go back to: its saved rect, or the primary monitor if
    /// that rect is no longer on any monitor.
    fn target(monitors: &[Monitor], saved: Rect) -> Option<Rect> {
        if on_screen(monitors, &saved) {
            return Some(saved);
        }
        let primary = monitors.iter().find(|m| m.primary).or(monitors.first())?;
        let a = primary.work_area;
        Some(Rect::new(
            a.x + 40,
            a.y + 40,
            saved.w.min(a.w),
            saved.h.min(a.h),
        ))
    }

    /// Restores windows this process parked. They are moved back unconditionally:
    /// a park that is still queued (busy app) must not be mistaken for a window
    /// that is already on screen. The moves are queued too, so they land after any
    /// pending park instead of overtaking it.
    pub fn restore_ours(parked: &[Parked]) -> usize {
        let monitors = monitors::monitors();
        let mut restored = 0;
        for p in parked {
            let hwnd = Hwnd(p.hwnd as isize);
            if window::visible_rect(hwnd).is_none() {
                continue; // Closed.
            }
            if let Some(t) = target(&monitors, rect_of(p)) {
                if apply::restore_to_queued(hwnd, t) {
                    restored += 1;
                }
            }
        }
        restored
    }

    /// Restores windows listed by a server that did not exit cleanly. Since the
    /// file may be old and window handles get reused, a window is only touched if
    /// it is visible, not minimized, still off screen, and owned by the same
    /// executable.
    fn restore_from_file(parked: &[Parked]) -> usize {
        let monitors = monitors::monitors();
        let mut restored = 0;
        for p in parked {
            let hwnd = Hwnd(p.hwnd as isize);
            let Some(f) = window::facts(hwnd) else {
                continue;
            };
            let same_app = p.exe.is_empty() || f.process.eq_ignore_ascii_case(&p.exe);
            if f.minimized || !same_app || on_screen(&monitors, &f.rect) {
                continue;
            }
            if let Some(t) = target(&monitors, rect_of(p)) {
                if apply::restore_to(hwnd, t) {
                    restored += 1;
                }
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
        let restored = restore_from_file(&parked);
        super::remove(path);
        restored
    }
}
