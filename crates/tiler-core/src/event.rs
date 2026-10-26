//! What the platform layer tells the core, and what the core asks it to do.

use crate::command::Command;
use crate::config::Config;
use crate::geometry::{MonitorId, Rect, WindowId};
use crate::keys::KeyChord;

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

/// A display, with the area not covered by the taskbar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub id: MonitorId,
    pub work_area: Rect,
    pub name: String,
    pub primary: bool,
}

/// Something that happened, reported by the platform layer or the CLI.
#[derive(Debug, Clone)]
pub enum Event {
    /// A window that should be considered for management appeared.
    WindowAppeared(WindowId, WindowInfo),
    WindowDestroyed(WindowId),
    /// The OS gave a window keyboard focus (click, Alt-Tab, taskbar...).
    WindowFocused(WindowId),
    WindowMinimized(WindowId),
    WindowRestored(WindowId),
    /// The user finished moving or resizing a window with the mouse.
    WindowMoveSizeEnd(WindowId, Rect),
    WindowTitleChanged(WindowId, String),
    /// The full, current set of monitors.
    MonitorsChanged(Vec<Monitor>),
    Command(Command),
    Hotkey(KeyChord),
    ConfigReloaded(Box<Config>),
}

/// Something the platform layer should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    SetRect(WindowId, Rect),
    /// Hide a window using the configured hide strategy.
    Hide(WindowId),
    /// Undo [`Action::Hide`]. Always followed by a `SetRect` for the same window.
    Show(WindowId),
    /// Give keyboard focus to a window, or to nothing (e.g. an empty workspace).
    Focus(Option<WindowId>),
    /// Ask a window to close.
    Close(WindowId),
    /// Output for the CLI client that sent the command.
    Reply(String),
    /// A command failed; shown to the CLI client or logged.
    Error(String),
    ReloadConfig,
    Rescue,
    DebugWindows,
    Quit,
    /// The binding mode changed, or the config was reloaded; the keyboard hook must
    /// re-read [`State::bindings`](crate::State::bindings).
    ModeChanged(String),
}
