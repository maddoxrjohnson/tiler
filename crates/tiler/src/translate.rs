//! Turning raw Win32 window events into core events.

use tiler_core::{Event, WindowId, WindowInfo};
use tiler_windows::{float_hint, manageable, WindowFacts};

/// The window events tiler cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Show,
    Uncloaked,
    Hide,
    Cloaked,
    Destroy,
    Foreground,
    MinimizeStart,
    MinimizeEnd,
    MoveSizeEnd,
    NameChange,
}

/// What the translator knows about a window besides the event itself.
pub struct Context {
    /// The core already manages this window.
    pub managed: bool,
    /// tiler itself hid this window (so a hide event is ours, not the app's).
    pub hidden_by_us: bool,
    pub we_are_elevated: bool,
}

pub fn window_info(f: &WindowFacts) -> WindowInfo {
    WindowInfo {
        process: f.process.clone(),
        app_name: f
            .process
            .strip_suffix(".exe")
            .unwrap_or(&f.process)
            .to_owned(),
        class: f.class.clone(),
        title: f.title.clone(),
        float_hint: float_hint(f),
        rect: f.rect,
    }
}

/// A Store (UWP) app frame whose app window isn't attached yet, so it still reports
/// the generic host process. Adopting it now would make rules miss it; its title
/// change or focus a moment later adopts it with the real process.
fn unresolved_uwp_frame(f: &WindowFacts) -> bool {
    f.class == "ApplicationFrameWindow"
        && f.process.eq_ignore_ascii_case("ApplicationFrameHost.exe")
}

/// A window that should start being managed, if `facts` says so.
pub fn appeared(id: WindowId, facts: Option<WindowFacts>, we_are_elevated: bool) -> Option<Event> {
    let f = facts?;
    (manageable(&f, we_are_elevated) && !f.minimized)
        .then(|| Event::WindowAppeared(id, window_info(&f)))
}

/// Translates one raw event into zero or more core events. `facts` is only called
/// when needed, since reading window facts costs several system calls.
pub fn translate(
    kind: Kind,
    id: WindowId,
    cx: &Context,
    facts: impl FnOnce() -> Option<WindowFacts>,
) -> Vec<Event> {
    match kind {
        // Some windows only become manageable after they are shown (their title or
        // style is set later), so give unmanaged windows another chance when they
        // gain focus or change title.
        Kind::Foreground if !cx.managed => appeared(id, facts(), cx.we_are_elevated)
            .into_iter()
            .chain([Event::WindowFocused(id)])
            .collect(),
        Kind::NameChange if !cx.managed => appeared(id, facts(), cx.we_are_elevated)
            .into_iter()
            .collect(),
        _ => translate_one(kind, id, cx, facts).into_iter().collect(),
    }
}

fn translate_one(
    kind: Kind,
    id: WindowId,
    cx: &Context,
    facts: impl FnOnce() -> Option<WindowFacts>,
) -> Option<Event> {
    match kind {
        Kind::Show | Kind::Uncloaked | Kind::MinimizeEnd if !cx.managed => appeared(
            id,
            facts().filter(|f| !unresolved_uwp_frame(f)),
            cx.we_are_elevated,
        ),
        Kind::Show | Kind::Uncloaked => None,
        // An app that hides its own window (e.g. "close to tray") is treated as closed.
        Kind::Hide if cx.managed && !cx.hidden_by_us => Some(Event::WindowDestroyed(id)),
        Kind::Cloaked | Kind::Destroy if cx.managed => Some(Event::WindowDestroyed(id)),
        // Sent for unmanaged windows too: the core uses any focus change.
        Kind::Foreground => Some(Event::WindowFocused(id)),
        Kind::MinimizeStart if cx.managed => Some(Event::WindowMinimized(id)),
        Kind::MinimizeEnd => Some(Event::WindowRestored(id)),
        Kind::MoveSizeEnd if cx.managed => facts().map(|f| Event::WindowMoveSizeEnd(id, f.rect)),
        Kind::NameChange if cx.managed => facts().map(|f| Event::WindowTitleChanged(id, f.title)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiler_core::Rect;
    use tiler_windows::facts::{WS_CAPTION, WS_THICKFRAME};

    fn app() -> WindowFacts {
        WindowFacts {
            style: WS_CAPTION | WS_THICKFRAME,
            visible: true,
            class: "Notepad".into(),
            title: "notes.txt - Notepad".into(),
            process: "notepad.exe".into(),
            rect: Rect::new(10, 10, 800, 600),
            ..Default::default()
        }
    }

    const ID: WindowId = WindowId(7);

    fn cx(managed: bool) -> Context {
        Context {
            managed,
            hidden_by_us: false,
            we_are_elevated: false,
        }
    }

    fn name(events: Vec<Event>) -> String {
        if events.is_empty() {
            return "none".into();
        }
        events
            .iter()
            .map(|e| format!("{e:?}").split('(').next().unwrap().to_owned())
            .collect::<Vec<_>>()
            .join("+")
    }

    #[test]
    fn new_window_appears_with_its_info() {
        let e = translate(Kind::Show, ID, &cx(false), || Some(app()));
        let [Event::WindowAppeared(id, info)] = e.as_slice() else {
            panic!("{e:?}")
        };
        let id = *id;
        assert_eq!(id, ID);
        assert_eq!(info.app_name, "notepad");
        assert!(!info.float_hint);
    }

    #[test]
    fn unmanageable_or_vanished_windows_do_not_appear() {
        let hidden = WindowFacts {
            visible: false,
            ..app()
        };
        assert_eq!(
            name(translate(Kind::Show, ID, &cx(false), || Some(hidden))),
            "none"
        );
        assert_eq!(name(translate(Kind::Show, ID, &cx(false), || None)), "none");
    }

    #[test]
    fn facts_are_not_read_for_events_that_do_not_need_them() {
        let e = translate(Kind::Destroy, ID, &cx(true), || panic!("facts read"));
        assert_eq!(name(e), "WindowDestroyed");
        let e = translate(Kind::Show, ID, &cx(true), || panic!("facts read"));
        assert_eq!(name(e), "none");
    }

    #[test]
    fn hiding_by_the_app_counts_as_closing_but_our_own_does_not() {
        assert_eq!(
            name(translate(Kind::Hide, ID, &cx(true), || None)),
            "WindowDestroyed"
        );
        let ours = Context {
            hidden_by_us: true,
            ..cx(true)
        };
        assert_eq!(name(translate(Kind::Hide, ID, &ours, || None)), "none");
        assert_eq!(name(translate(Kind::Hide, ID, &cx(false), || None)), "none");
    }

    #[test]
    fn minimize_and_restore() {
        assert_eq!(
            name(translate(Kind::MinimizeStart, ID, &cx(true), || None)),
            "WindowMinimized"
        );
        assert_eq!(
            name(translate(Kind::MinimizeEnd, ID, &cx(true), || None)),
            "WindowRestored"
        );
        // A window that was minimized when tiler started appears when restored.
        let e = translate(Kind::MinimizeEnd, ID, &cx(false), || Some(app()));
        assert_eq!(name(e), "WindowAppeared");
    }

    #[test]
    fn focus_is_always_reported() {
        assert_eq!(
            name(translate(Kind::Foreground, ID, &cx(false), || None)),
            "WindowFocused"
        );
        assert_eq!(
            name(translate(Kind::Foreground, ID, &cx(true), || panic!(
                "facts read"
            ))),
            "WindowFocused"
        );
    }

    fn uwp_frame() -> WindowFacts {
        WindowFacts {
            class: "ApplicationFrameWindow".into(),
            process: "ApplicationFrameHost.exe".into(),
            ..app()
        }
    }

    #[test]
    fn uwp_frames_wait_for_the_app_before_being_adopted() {
        // Shown before the app's own window is attached: the process would be the
        // generic host, so rules couldn't match. Wait.
        assert_eq!(
            name(translate(Kind::Show, ID, &cx(false), || Some(uwp_frame()))),
            "none"
        );
        assert_eq!(
            name(translate(Kind::Uncloaked, ID, &cx(false), || Some(
                uwp_frame()
            ))),
            "none"
        );
        // Once the app is attached, the real process is reported and it's adopted.
        let ready = WindowFacts {
            process: "CalculatorApp.exe".into(),
            ..uwp_frame()
        };
        assert_eq!(
            name(translate(Kind::NameChange, ID, &cx(false), || Some(ready))),
            "WindowAppeared"
        );
        // If it never resolves, focusing it still adopts it.
        assert_eq!(
            name(translate(Kind::Foreground, ID, &cx(false), || Some(
                uwp_frame()
            ))),
            "WindowAppeared+WindowFocused"
        );
    }

    #[test]
    fn late_manageable_windows_are_adopted_on_focus_or_title_change() {
        assert_eq!(
            name(translate(Kind::Foreground, ID, &cx(false), || Some(app()))),
            "WindowAppeared+WindowFocused"
        );
        assert_eq!(
            name(translate(Kind::NameChange, ID, &cx(false), || Some(app()))),
            "WindowAppeared"
        );
        let untitled = WindowFacts {
            title: String::new(),
            style: 0,
            ..app()
        };
        assert_eq!(
            name(translate(Kind::NameChange, ID, &cx(false), || Some(
                untitled
            ))),
            "none"
        );
    }

    #[test]
    fn mouse_resize_reports_the_visible_rect() {
        let e = translate(Kind::MoveSizeEnd, ID, &cx(true), || Some(app()));
        assert!(
            matches!(e.as_slice(), [Event::WindowMoveSizeEnd(_, r)] if *r == Rect::new(10, 10, 800, 600))
        );
    }
}
