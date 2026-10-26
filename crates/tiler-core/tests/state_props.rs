//! Property test: no sequence of events may panic or leave the state inconsistent.

use proptest::prelude::*;
use tiler_core::{
    parse_command, Config, Event, Monitor, MonitorId, Rect, State, WindowId, WindowInfo,
};

const COMMANDS: &[&str] = &[
    "focus left",
    "focus right",
    "focus up",
    "focus down",
    "move left",
    "move right",
    "move up",
    "move down",
    "join-with right",
    "join-with down",
    "layout tiles accordion",
    "layout horizontal vertical",
    "layout floating tiling",
    "resize smart +80",
    "resize width -300",
    "resize height 200",
    "balance-sizes",
    "flatten-workspace-tree",
    "fullscreen",
    "close",
    "workspace 1",
    "workspace 2",
    "workspace web",
    "workspace-back-and-forth",
    "move-node-to-workspace 2",
    "move-node-to-workspace --focus-follows-window web",
    "move-workspace-to-monitor next",
    "focus-monitor prev",
    "mode resize",
    "mode main",
    "list-windows",
];

fn monitors() -> impl Strategy<Value = Vec<Monitor>> {
    (0usize..=3).prop_map(|n| {
        (0..n)
            .map(|i| Monitor {
                id: MonitorId(i as u64 + 1),
                work_area: Rect::new(i as i32 * 1920, 0, 1920, if i == 2 { 1920 } else { 1080 }),
                name: format!("M{i}"),
                primary: i == 0,
            })
            .collect()
    })
}

fn event() -> impl Strategy<Value = Event> {
    let id = (0u64..10).prop_map(WindowId);
    let rect = (0i32..3000, 0i32..1500, 1i32..2000, 1i32..1200)
        .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h));
    prop_oneof![
        4 => (id.clone(), any::<bool>(), rect.clone()).prop_map(|(id, float_hint, rect)| {
            Event::WindowAppeared(id, WindowInfo { process: "app.exe".into(), float_hint, rect, ..Default::default() })
        }),
        2 => id.clone().prop_map(Event::WindowDestroyed),
        2 => id.clone().prop_map(Event::WindowFocused),
        1 => id.clone().prop_map(Event::WindowMinimized),
        1 => id.clone().prop_map(Event::WindowRestored),
        1 => (id, rect).prop_map(|(id, r)| Event::WindowMoveSizeEnd(id, r)),
        1 => monitors().prop_map(Event::MonitorsChanged),
        6 => prop::sample::select(COMMANDS).prop_map(|c| Event::Command(parse_command(c).unwrap())),
        2 => prop::sample::select(vec!["alt-1", "alt-2", "alt-h", "alt-shift-l", "l", "esc"])
            .prop_map(|k| Event::Hotkey(k.parse().unwrap())),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_events_keep_state_consistent(
        initial in monitors(),
        events in prop::collection::vec(event(), 1..120),
    ) {
        let mut state = State::new(Config::default());
        state.handle(Event::MonitorsChanged(initial));
        for e in events {
            let desc = format!("{e:?}");
            state.handle(e);
            if let Err(err) = state.check_invariants() {
                return Err(TestCaseError::fail(format!("after {desc}: {err}")));
            }
        }
    }
}
