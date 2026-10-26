//! What the platform layer tells the core, and what the core asks it to do.

use crate::geometry::Rect;

/// Facts about a window that rules and heuristics need.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowInfo {
    /// Executable file name, e.g. `firefox.exe`.
    pub process: String,
    /// Human-readable application name, e.g. `Firefox`.
    pub app_name: String,
    /// Win32 window class.
    pub class: String,
    pub title: String,
    /// The platform's guess that this window should float (dialogs, tool windows,
    /// windows that cannot be resized).
    pub float_hint: bool,
    /// Where the window was when we first saw it.
    pub rect: Rect,
}
