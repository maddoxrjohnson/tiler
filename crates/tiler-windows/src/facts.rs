//! Which top-level windows tiler manages, and which of those should float.

use tiler_core::Rect;

// Window style bits, from WinUser.h. Duplicated here so this module builds on any OS.
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_THICKFRAME: u32 = 0x0004_0000;
pub const WS_EX_DLGMODALFRAME: u32 = 0x0000_0001;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const WS_EX_APPWINDOW: u32 = 0x0004_0000;
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

/// Window classes that belong to the shell and must never be managed.
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "XamlExplorerHostIslandWindow",
    "ForegroundStaging",
    "MultitaskingViewFrame",
];

/// Everything the heuristics need to know about a window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowFacts {
    pub style: u32,
    pub ex_style: u32,
    pub has_owner: bool,
    pub visible: bool,
    /// Hidden by DWM, e.g. a suspended UWP app or a window on another virtual desktop.
    pub cloaked: bool,
    pub minimized: bool,
    pub class: String,
    pub title: String,
    /// Executable file name, e.g. `firefox.exe`.
    pub process: String,
    /// The owning process runs as administrator.
    pub elevated: bool,
    /// Visible frame, without the invisible resize borders.
    pub rect: Rect,
}

/// True for windows a user would think of as "an app window".
pub fn manageable(f: &WindowFacts, we_are_elevated: bool) -> bool {
    let tool = f.ex_style & WS_EX_TOOLWINDOW != 0 && f.ex_style & WS_EX_APPWINDOW == 0;
    f.visible
        && !f.cloaked
        && !tool
        && f.style & WS_CHILD == 0
        && f.ex_style & WS_EX_NOACTIVATE == 0
        && (!f.title.is_empty() || f.style & WS_CAPTION == WS_CAPTION)
        // Windows cannot move an elevated window from a non-elevated process (UIPI).
        && (!f.elevated || we_are_elevated)
        && !SHELL_CLASSES.contains(&f.class.as_str())
        && f.rect.w > 0
        && f.rect.h > 0
}

/// The platform's guess that a managed window should float: dialogs, owned
/// windows, and windows the user cannot resize.
pub fn float_hint(f: &WindowFacts) -> bool {
    f.has_owner
        || f.class == "#32770"
        || f.style & WS_THICKFRAME == 0
        || f.ex_style & WS_EX_DLGMODALFRAME != 0
        || (f.style & WS_POPUP != 0 && f.style & WS_CAPTION != WS_CAPTION)
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_STYLE: u32 = WS_CAPTION | WS_THICKFRAME;

    fn app() -> WindowFacts {
        WindowFacts {
            style: APP_STYLE,
            visible: true,
            class: "Chrome_WidgetWin_1".into(),
            title: "Example - Google Chrome".into(),
            process: "chrome.exe".into(),
            rect: Rect::new(0, 0, 800, 600),
            ..Default::default()
        }
    }

    #[test]
    fn normal_app_window_is_managed_and_tiled() {
        assert!(manageable(&app(), false));
        assert!(!float_hint(&app()));
    }

    #[test]
    fn invisible_cloaked_child_and_zero_size_windows_are_not_managed() {
        for f in [
            WindowFacts {
                visible: false,
                ..app()
            },
            WindowFacts {
                cloaked: true,
                ..app()
            },
            WindowFacts {
                style: APP_STYLE | WS_CHILD,
                ..app()
            },
            WindowFacts {
                rect: Rect::new(0, 0, 0, 0),
                ..app()
            },
            WindowFacts {
                ex_style: WS_EX_NOACTIVATE,
                ..app()
            },
        ] {
            assert!(!manageable(&f, false), "{f:?}");
        }
    }

    #[test]
    fn tool_windows_are_skipped_unless_they_ask_for_the_taskbar() {
        let tool = WindowFacts {
            ex_style: WS_EX_TOOLWINDOW,
            ..app()
        };
        assert!(!manageable(&tool, false));
        let app_tool = WindowFacts {
            ex_style: WS_EX_TOOLWINDOW | WS_EX_APPWINDOW,
            ..app()
        };
        assert!(manageable(&app_tool, false));
    }

    #[test]
    fn shell_windows_are_never_managed() {
        let taskbar = WindowFacts {
            class: "Shell_TrayWnd".into(),
            ..app()
        };
        assert!(!manageable(&taskbar, false));
    }

    #[test]
    fn untitled_windows_need_a_caption() {
        let untitled = WindowFacts {
            title: String::new(),
            style: WS_POPUP,
            ..app()
        };
        assert!(!manageable(&untitled, false));
        let captioned = WindowFacts {
            title: String::new(),
            ..app()
        };
        assert!(manageable(&captioned, false));
    }

    #[test]
    fn elevated_windows_are_managed_only_when_we_are_elevated() {
        let admin = WindowFacts {
            elevated: true,
            ..app()
        };
        assert!(!manageable(&admin, false));
        assert!(manageable(&admin, true));
    }

    #[test]
    fn dialogs_owned_and_fixed_size_windows_float() {
        for f in [
            WindowFacts {
                has_owner: true,
                ..app()
            },
            WindowFacts {
                class: "#32770".into(),
                ..app()
            },
            WindowFacts {
                style: WS_CAPTION,
                ..app()
            },
            WindowFacts {
                ex_style: WS_EX_DLGMODALFRAME,
                ..app()
            },
            WindowFacts {
                style: WS_POPUP | WS_THICKFRAME,
                ..app()
            },
        ] {
            assert!(float_hint(&f), "{f:?}");
        }
    }
}
