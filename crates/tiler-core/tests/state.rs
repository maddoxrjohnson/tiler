//! Behavior of the core state machine, driven only through events.

use tiler_core::{
    parse_command, Action, Config, Event, Monitor, MonitorId, Rect, State, WindowId, WindowInfo,
};

const SCREEN: Rect = Rect::new(0, 0, 1920, 1080);
// Default config: outer gaps 8, inner gaps 8.
const FULL: Rect = Rect::new(8, 8, 1904, 1064);
const LEFT: Rect = Rect::new(8, 8, 948, 1064);
const RIGHT: Rect = Rect::new(964, 8, 948, 1064);

fn monitor(id: u64, x: i32) -> Monitor {
    Monitor {
        id: MonitorId(id),
        bounds: Rect::new(x, 0, 1920, 1080),
        work_area: Rect::new(x, 0, 1920, 1080),
        scale: 1.0,
        name: format!("Monitor {id}"),
        primary: id == 1,
    }
}

fn info(process: &str) -> WindowInfo {
    WindowInfo {
        process: process.into(),
        app_name: process.trim_end_matches(".exe").into(),
        class: "AppWindow".into(),
        title: format!("{process} window"),
        float_hint: false,
        rect: Rect::new(100, 100, 800, 600),
    }
}

struct Harness {
    state: State,
}

impl Harness {
    fn new(config: Config, monitors: Vec<Monitor>) -> Self {
        let mut state = State::new(config);
        state.handle(Event::MonitorsChanged(monitors));
        Harness { state }
    }

    fn single() -> Self {
        Self::new(Config::default(), vec![monitor(1, 0)])
    }

    fn ev(&mut self, e: Event) -> Vec<Action> {
        let actions = self.state.handle(e);
        self.state.check_invariants().unwrap();
        actions
    }

    fn open(&mut self, id: u64) -> Vec<Action> {
        self.ev(Event::WindowAppeared(WindowId(id), info("app.exe")))
    }

    fn cmd(&mut self, c: &str) -> Vec<Action> {
        self.ev(Event::Command(parse_command(c).unwrap()))
    }
}

fn set_rect(id: u64, r: Rect) -> Action {
    Action::SetRect(WindowId(id), r)
}

fn reply(actions: &[Action]) -> &str {
    actions
        .iter()
        .find_map(|a| match a {
            Action::Reply(s) => Some(s.as_str()),
            _ => None,
        })
        .expect("a reply")
}

#[test]
fn opening_windows_tiles_them() {
    let mut h = Harness::single();
    assert_eq!(h.open(1), vec![set_rect(1, FULL)]);
    let a = h.open(2);
    assert_eq!(a, vec![set_rect(1, LEFT), set_rect(2, RIGHT)]);
}

#[test]
fn switching_workspaces_hides_and_shows() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    let a = h.cmd("workspace 2");
    assert_eq!(
        a,
        vec![
            Action::Hide(WindowId(1)),
            Action::Hide(WindowId(2)),
            Action::Focus(None)
        ]
    );
    let a = h.cmd("workspace 1");
    assert_eq!(
        a,
        vec![
            Action::Show(WindowId(1)),
            set_rect(1, LEFT),
            Action::Show(WindowId(2)),
            set_rect(2, RIGHT),
            Action::Focus(Some(WindowId(2))),
        ]
    );
    // Back-and-forth goes to the previous workspace.
    let a = h.cmd("workspace-back-and-forth");
    assert!(a.contains(&Action::Hide(WindowId(1))));
}

#[test]
fn hotkeys_run_bound_commands() {
    let mut h = Harness::single();
    h.open(1);
    let a = h.ev(Event::Hotkey("alt-2".parse().unwrap()));
    assert!(a.contains(&Action::Hide(WindowId(1))));
    // Unbound chords do nothing.
    assert!(h.ev(Event::Hotkey("ctrl-q".parse().unwrap())).is_empty());
}

#[test]
fn modes_change_bindings() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    assert_eq!(
        h.cmd("mode resize"),
        vec![Action::ModeChanged("resize".into())]
    );
    let a = h.ev(Event::Hotkey("l".parse().unwrap()));
    // Window 2 is focused; growing it by 50 shrinks window 1 by 50.
    assert!(a.contains(&set_rect(1, Rect::new(8, 8, 898, 1064))));
    assert!(a.contains(&set_rect(2, Rect::new(914, 8, 998, 1064))));
    assert_eq!(
        h.cmd("mode nope"),
        vec![Action::Error("unknown mode 'nope'".into())]
    );
}

#[test]
fn destroying_focused_window_refocuses_and_relayouts() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    let a = h.ev(Event::WindowDestroyed(WindowId(2)));
    assert_eq!(a, vec![set_rect(1, FULL), Action::Focus(Some(WindowId(1)))]);
}

#[test]
fn last_window_closed_then_new_window_fills_workspace() {
    let mut h = Harness::single();
    h.open(1);
    h.ev(Event::WindowDestroyed(WindowId(1)));
    assert_eq!(h.open(2), vec![set_rect(2, FULL)]);
}

#[test]
fn unknown_workspace_is_created() {
    let mut h = Harness::single();
    h.open(1);
    h.cmd("workspace foo");
    let out = h.cmd("list-workspaces --focused");
    assert_eq!(reply(&out), "foo");
}

#[test]
fn zero_monitors_does_nothing_until_monitors_return() {
    let mut h = Harness::single();
    h.open(1);
    assert!(h.ev(Event::MonitorsChanged(vec![])).is_empty());
    assert!(h.open(2).is_empty());
    assert!(h
        .cmd("workspace 2")
        .iter()
        .all(|a| matches!(a, Action::Focus(_))));
    let a = h.ev(Event::MonitorsChanged(vec![monitor(1, 0)]));
    assert!(a
        .iter()
        .any(|x| matches!(x, Action::SetRect(..) | Action::Hide(_))));
}

#[test]
fn unknown_and_duplicate_window_events_are_ignored() {
    let mut h = Harness::single();
    assert!(h.ev(Event::WindowDestroyed(WindowId(42))).is_empty());
    assert!(h.ev(Event::WindowFocused(WindowId(42))).is_empty());
    assert!(h.ev(Event::WindowMinimized(WindowId(42))).is_empty());
    assert!(h
        .ev(Event::WindowMoveSizeEnd(WindowId(42), SCREEN))
        .is_empty());
    h.open(1);
    assert!(h.open(1).is_empty());
}

#[test]
fn focusing_a_window_on_a_hidden_workspace_switches_to_it() {
    let mut h = Harness::single();
    h.open(1);
    h.cmd("workspace 2");
    let a = h.ev(Event::WindowFocused(WindowId(1)));
    assert_eq!(a, vec![Action::Show(WindowId(1)), set_rect(1, FULL)]);
    assert_eq!(reply(&h.cmd("list-workspaces --focused")), "1");
}

#[test]
fn move_node_to_workspace() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    let a = h.cmd("move-node-to-workspace 3");
    assert_eq!(
        a,
        vec![
            Action::Hide(WindowId(2)),
            set_rect(1, FULL),
            Action::Focus(Some(WindowId(1)))
        ]
    );
    let a = h.cmd("move-node-to-workspace --focus-follows-window 3");
    assert!(a.contains(&Action::Focus(Some(WindowId(1)))));
    assert_eq!(reply(&h.cmd("list-workspaces --focused")), "3");
}

#[test]
fn two_monitors_and_moving_workspaces() {
    let mut h = Harness::new(Config::default(), vec![monitor(1, 0), monitor(2, 1920)]);
    assert_eq!(reply(&h.cmd("list-workspaces")), "1\n2");
    h.open(1);
    let a = h.cmd("move-workspace-to-monitor next");
    assert!(a.contains(&set_rect(1, Rect::new(1928, 8, 1904, 1064))));
    assert_eq!(reply(&h.cmd("list-workspaces --focused")), "1");
    // Workspace 2 swapped onto the first monitor; focus-monitor moves back to it.
    h.cmd("focus-monitor left");
    assert_eq!(reply(&h.cmd("list-workspaces --focused")), "2");
    assert_eq!(
        reply(&h.cmd("list-monitors")),
        "1 | Monitor 1 | 1920x1080 (primary)\n2 | Monitor 2 | 1920x1080"
    );
}

#[test]
fn moving_past_the_edge_goes_to_the_next_monitor() {
    let mut h = Harness::new(Config::default(), vec![monitor(1, 0), monitor(2, 1920)]);
    h.open(1);
    let a = h.cmd("move right");
    assert!(a.contains(&set_rect(1, Rect::new(1928, 8, 1904, 1064))));
    assert_eq!(reply(&h.cmd("list-workspaces --focused")), "2");
}

#[test]
fn rules_and_float_hint() {
    let config = Config::from_toml(
        "[mode.main.binding]\nalt-1 = 'workspace 1'\n\
         [[on-window-detected]]\nif.process = 'calc.exe'\nrun = 'layout floating'\n\
         [[on-window-detected]]\nif.process = 'chat.exe'\nrun = 'move-node-to-workspace 9'\n\
         [[on-window-detected]]\nif.process = 'overlay.exe'\nrun = 'ignore'\n",
    )
    .unwrap();
    let mut h = Harness::new(config, vec![monitor(1, 0)]);
    h.ev(Event::WindowAppeared(WindowId(1), info("app.exe")));
    let a = h.ev(Event::WindowAppeared(WindowId(2), info("calc.exe")));
    assert_eq!(a, vec![set_rect(2, Rect::new(100, 100, 800, 600))]);
    let a = h.ev(Event::WindowAppeared(WindowId(3), info("chat.exe")));
    assert_eq!(a, vec![Action::Hide(WindowId(3))]);
    assert!(h
        .ev(Event::WindowAppeared(WindowId(4), info("overlay.exe")))
        .is_empty());
    let dialog = WindowInfo {
        float_hint: true,
        ..info("app.exe")
    };
    let a = h.ev(Event::WindowAppeared(WindowId(5), dialog));
    assert_eq!(a, vec![set_rect(5, Rect::new(100, 100, 800, 600))]);
    // Toggling floating on the dialog tiles it next to window 1.
    let a = h.cmd("layout floating tiling");
    assert!(a.contains(&set_rect(1, Rect::new(0, 0, 960, 1080))));
}

#[test]
fn drag_resize_changes_weights_and_title_drag_snaps_back() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    let grown = Rect::new(8, 8, 1048, 1064);
    let a = h.ev(Event::WindowMoveSizeEnd(WindowId(1), grown));
    assert!(a.contains(&set_rect(2, Rect::new(1064, 8, 848, 1064))));
    let moved = Rect::new(300, 300, 848, 1064);
    let a = h.ev(Event::WindowMoveSizeEnd(WindowId(2), moved));
    assert_eq!(a, vec![set_rect(2, Rect::new(1064, 8, 848, 1064))]);
}

#[test]
fn minimize_and_restore() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    let a = h.ev(Event::WindowMinimized(WindowId(2)));
    assert_eq!(a, vec![set_rect(1, FULL)]);
    let a = h.ev(Event::WindowRestored(WindowId(2)));
    assert_eq!(a, vec![set_rect(1, LEFT), set_rect(2, RIGHT)]);
}

#[test]
fn fullscreen_toggle() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    let a = h.cmd("fullscreen");
    assert_eq!(
        a,
        vec![set_rect(2, SCREEN), Action::Focus(Some(WindowId(2)))]
    );
    let a = h.cmd("fullscreen");
    assert_eq!(
        a,
        vec![set_rect(2, RIGHT), Action::Focus(Some(WindowId(2)))]
    );
}

#[test]
fn server_commands_become_actions() {
    let mut h = Harness::single();
    assert_eq!(h.cmd("reload-config"), vec![Action::ReloadConfig]);
    assert_eq!(h.cmd("rescue"), vec![Action::Rescue]);
    assert_eq!(h.cmd("quit"), vec![Action::Quit]);
    h.open(7);
    assert_eq!(
        reply(&h.cmd("list-windows")),
        "7 | 1 | app.exe | app.exe window"
    );
}

#[test]
fn config_reload_applies_new_gaps() {
    let mut h = Harness::single();
    h.open(1);
    let config = Config::from_toml("[mode.main.binding]\nalt-1 = 'workspace 1'\n").unwrap();
    let a = h.ev(Event::ConfigReloaded(Box::new(config)));
    // ModeChanged is always sent so the keyboard hook refreshes its bindings.
    assert_eq!(
        a,
        vec![set_rect(1, SCREEN), Action::ModeChanged("main".into())]
    );
}

fn floating(rect: Rect) -> WindowInfo {
    WindowInfo {
        float_hint: true,
        rect,
        ..info("tool.exe")
    }
}

#[test]
fn floating_window_survives_monitor_removal() {
    let mut h = Harness::new(Config::default(), vec![monitor(1, 0), monitor(2, 1920)]);
    h.cmd("focus-monitor next");
    let a = h.ev(Event::WindowAppeared(
        WindowId(5),
        floating(Rect::new(2200, 100, 800, 600)),
    ));
    assert_eq!(a, vec![set_rect(5, Rect::new(2200, 100, 800, 600))]);
    h.ev(Event::MonitorsChanged(vec![monitor(1, 0)]));
    let a = h.cmd("workspace 2");
    assert!(
        a.contains(&set_rect(5, Rect::new(280, 100, 800, 600))),
        "{a:?}"
    );
}

#[test]
fn floating_window_follows_its_workspace_to_another_monitor() {
    let mut h = Harness::new(Config::default(), vec![monitor(1, 0), monitor(2, 1920)]);
    h.ev(Event::WindowAppeared(
        WindowId(5),
        floating(Rect::new(100, 100, 800, 600)),
    ));
    let a = h.cmd("move-workspace-to-monitor next");
    assert!(
        a.contains(&set_rect(5, Rect::new(2020, 100, 800, 600))),
        "{a:?}"
    );
}

#[test]
fn floating_window_is_kept_on_screen() {
    let mut h = Harness::single();
    let a = h.ev(Event::WindowAppeared(
        WindowId(5),
        floating(Rect::new(1700, 100, 800, 600)),
    ));
    assert_eq!(a, vec![set_rect(5, Rect::new(1120, 100, 800, 600))]);
}

#[test]
fn restoring_a_window_on_a_hidden_workspace_reveals_it() {
    let mut h = Harness::single();
    h.open(1);
    h.ev(Event::WindowMinimized(WindowId(1)));
    h.cmd("workspace 2");
    let a = h.ev(Event::WindowRestored(WindowId(1)));
    assert_eq!(a, vec![set_rect(1, FULL)]);
    assert_eq!(reply(&h.cmd("list-workspaces --focused")), "1");
}

#[test]
fn automatic_focus_after_a_rule_hides_a_window_is_ignored_once() {
    let config = Config::from_toml(
        "[mode.main.binding]\nalt-1 = 'workspace 1'\n\
         [[on-window-detected]]\nif.process = 'chat.exe'\nrun = 'move-node-to-workspace 9'\n",
    )
    .unwrap();
    let mut h = Harness::new(config, vec![monitor(1, 0)]);
    h.open(1);
    assert_eq!(
        h.ev(Event::WindowAppeared(WindowId(2), info("chat.exe"))),
        vec![Action::Hide(WindowId(2))]
    );
    // Windows activates the new window on its own: stay on workspace 1.
    assert!(h.ev(Event::WindowFocused(WindowId(2))).is_empty());
    // A later, deliberate focus (Alt-Tab, taskbar) does switch.
    h.ev(Event::WindowFocused(WindowId(1)));
    let a = h.ev(Event::WindowFocused(WindowId(2)));
    assert!(a.contains(&Action::Show(WindowId(2))));
}

#[test]
fn automatic_focus_after_closing_a_window_is_ignored_once() {
    let mut h = Harness::single();
    h.open(1);
    h.cmd("workspace 2");
    h.open(2);
    let a = h.ev(Event::WindowDestroyed(WindowId(2)));
    assert_eq!(a, vec![Action::Focus(None)]);
    // Windows activates the next window in z-order, which is hidden: ignore it.
    assert!(h.ev(Event::WindowFocused(WindowId(1))).is_empty());
}

#[test]
fn drag_resize_moves_only_the_dragged_edge() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    h.open(3);
    // 3 tiles in 1904 px with two 8 px gaps: widths 629, 630, 629.
    let dragged = Rect::new(645 - 46, 8, 630 + 46, 1064);
    let a = h.ev(Event::WindowMoveSizeEnd(WindowId(2), dragged));
    assert_eq!(
        a,
        vec![
            set_rect(1, Rect::new(8, 8, 583, 1064)),
            set_rect(2, Rect::new(599, 8, 676, 1064)),
        ]
    );
}

#[test]
fn returning_monitor_shows_the_workspace_it_had() {
    let mut h = Harness::new(Config::default(), vec![monitor(1, 0), monitor(2, 1920)]);
    h.cmd("focus-monitor next");
    h.open(1);
    h.cmd("workspace 3");
    h.open(2);
    h.ev(Event::MonitorsChanged(vec![monitor(1, 0)]));
    // Windows may give the monitor a new id when it comes back.
    let mut back = monitor(2, 1920);
    back.id = MonitorId(7);
    let a = h.ev(Event::MonitorsChanged(vec![monitor(1, 0), back]));
    assert!(
        a.contains(&set_rect(2, Rect::new(1928, 8, 1904, 1064))),
        "{a:?}"
    );
}

#[test]
fn focus_from_a_floating_window_goes_to_the_tiles() {
    let mut h = Harness::single();
    h.open(1);
    h.ev(Event::WindowAppeared(
        WindowId(2),
        floating(Rect::new(100, 100, 400, 300)),
    ));
    assert_eq!(h.cmd("focus left"), vec![Action::Focus(Some(WindowId(1)))]);
}

#[test]
fn debug_windows_is_passed_to_the_platform() {
    let mut h = Harness::single();
    assert_eq!(h.cmd("debug-windows"), vec![Action::DebugWindows]);
}

#[test]
fn a_window_that_will_not_shrink_gets_its_minimum_and_the_rest_adapts() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    let a = h.ev(Event::WindowMinSize(WindowId(1), 1200, 0));
    assert_eq!(
        a,
        vec![
            set_rect(1, Rect::new(8, 8, 1200, 1064)),
            set_rect(2, Rect::new(1216, 8, 696, 1064)),
        ]
    );
    // A minimum that already fits changes nothing.
    assert!(h.ev(Event::WindowMinSize(WindowId(2), 300, 0)).is_empty());
    // Closing the stubborn window frees the space.
    let a = h.ev(Event::WindowDestroyed(WindowId(1)));
    assert!(a.contains(&set_rect(2, FULL)));
}

#[test]
fn shrinking_a_window_by_mouse_lowers_its_minimum() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    h.ev(Event::WindowMinSize(WindowId(1), 1200, 0));
    // The user drags window 1 narrower than its recorded minimum: they know best.
    let a = h.ev(Event::WindowMoveSizeEnd(
        WindowId(1),
        Rect::new(8, 8, 1000, 1064),
    ));
    assert!(
        a.contains(&set_rect(1, Rect::new(8, 8, 1000, 1064))),
        "{a:?}"
    );
}

#[test]
fn dragging_a_tiled_window_to_another_monitor_moves_it_there() {
    let mut h = Harness::new(Config::default(), vec![monitor(1, 0), monitor(2, 1920)]);
    h.open(1);
    h.open(2);
    let a = h.ev(Event::WindowMoveSizeEnd(
        WindowId(2),
        Rect::new(2500, 100, 948, 1064),
    ));
    assert_eq!(
        a,
        vec![
            set_rect(1, FULL),
            set_rect(2, Rect::new(1928, 8, 1904, 1064)),
        ]
    );
    assert_eq!(reply(&h.cmd("list-workspaces --focused")), "2");
}

#[test]
fn dragging_a_floating_window_to_another_monitor_keeps_it_where_dropped() {
    let mut h = Harness::new(Config::default(), vec![monitor(1, 0), monitor(2, 1920)]);
    h.ev(Event::WindowAppeared(
        WindowId(5),
        floating(Rect::new(100, 100, 800, 600)),
    ));
    let dropped = Rect::new(2200, 100, 800, 600);
    assert!(h
        .ev(Event::WindowMoveSizeEnd(WindowId(5), dropped))
        .is_empty());
    assert_eq!(
        reply(&h.cmd("list-windows")),
        "5 | 2 | tool.exe | tool.exe window"
    );
}

#[test]
fn gaps_scale_with_monitor_dpi() {
    let mut m = monitor(1, 0);
    m.scale = 1.5;
    let mut h = Harness::new(Config::default(), vec![m]);
    // Default outer gap 8 becomes 12 at 150%.
    assert_eq!(h.open(1), vec![set_rect(1, Rect::new(12, 12, 1896, 1056))]);
}

#[test]
fn balance_sizes_and_reload_clear_recorded_minimums() {
    let mut h = Harness::single();
    h.open(1);
    h.open(2);
    h.ev(Event::WindowMinSize(WindowId(1), 1200, 0));
    let a = h.cmd("balance-sizes");
    assert!(a.contains(&set_rect(1, LEFT)), "{a:?}");
    h.ev(Event::WindowMinSize(WindowId(1), 1200, 0));
    let a = h.ev(Event::ConfigReloaded(Box::default()));
    assert!(a.contains(&set_rect(1, LEFT)), "{a:?}");
}

#[test]
fn minimums_keep_their_visual_size_across_monitor_dpi() {
    let mut hi = monitor(1, 0);
    hi.scale = 1.5;
    let mut h = Harness::new(Config::default(), vec![hi, monitor(2, 1920)]);
    h.open(1);
    h.open(2);
    // Measured on the 150% monitor: 1500 px there is 1000 px at 100%.
    h.ev(Event::WindowMinSize(WindowId(1), 1500, 0));
    let a = h.cmd("move-workspace-to-monitor next");
    assert!(
        a.contains(&set_rect(1, Rect::new(1928, 8, 1000, 1064))),
        "{a:?}"
    );
    assert!(
        a.contains(&set_rect(2, Rect::new(2936, 8, 896, 1064))),
        "{a:?}"
    );
}

#[test]
fn resizing_a_window_across_the_monitor_edge_is_not_a_drop() {
    let mut h = Harness::new(Config::default(), vec![monitor(1, 0), monitor(2, 1920)]);
    h.open(1);
    h.open(2);
    // Window 1 widened far enough that its center ends up on monitor 2.
    h.ev(Event::WindowMoveSizeEnd(
        WindowId(1),
        Rect::new(8, 8, 3900, 1064),
    ));
    assert_eq!(
        reply(&h.cmd("list-windows"))
            .lines()
            .filter(|l| l.contains("| 1 |"))
            .count(),
        2
    );
}
