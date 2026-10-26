//! The core state machine: turns [`Event`]s into [`Action`]s.
//!
//! The state keeps the layout of every workspace and remembers where it last put
//! each window. After every event it computes where every window *should* be and
//! emits actions only for the differences, so the platform layer does the minimum
//! number of window moves.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use crate::command::{Command, LayoutArg, MonitorTarget};
use crate::config::{Config, Mode, MonitorPattern, RuleAction};
use crate::event::{Action, Event, Monitor, WindowInfo};
use crate::geometry::{Direction, MonitorId, Orientation, Rect, Sides, WindowId};
use crate::tree::InnerGaps;
use crate::workspace::Workspace;

/// Where a window should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    Hidden,
    At(Rect),
}

#[derive(Debug, Clone)]
struct Managed {
    info: WindowInfo,
    workspace: String,
    floating: bool,
    minimized: bool,
    floating_rect: Rect,
}

pub struct State {
    config: Config,
    mode: String,
    /// Sorted left to right, then top to bottom.
    monitors: Vec<Monitor>,
    /// The workspace shown on each monitor. Kept while there are no monitors so it can
    /// be restored when they come back.
    visible: BTreeMap<MonitorId, String>,
    focused_monitor: Option<MonitorId>,
    workspaces: BTreeMap<String, Workspace>,
    windows: HashMap<WindowId, Managed>,
    /// Windows a rule told us to leave alone.
    ignored: HashSet<WindowId>,
    prev_workspace: Option<String>,
    /// The last placement we asked the platform to apply for each window.
    applied: HashMap<WindowId, Placement>,
    /// Set when a command changed which window should have focus.
    focus_dirty: bool,
    /// Set right after a window closed, minimized, or was sent to a hidden workspace.
    /// Windows then activates some other window on its own; if that window is on a
    /// hidden workspace, switching to it would surprise the user, so the next such
    /// focus event is ignored. Any other focus event clears it.
    suppress_hidden_focus: bool,
    /// Sizes below which a window refuses to shrink, as reported by the platform.
    /// In DPI-independent units (pixels at 100% scale), so a window keeps the same
    /// visual minimum on monitors with different scaling.
    min_size: HashMap<WindowId, (f32, f32)>,
    /// Workspaces shown on monitors that went away, keyed by monitor name and
    /// position, so they come back when the monitor does (possibly with a new id).
    departed: Vec<(String, (i32, i32), String)>,
}

static EMPTY_MODE: LazyLock<Mode> = LazyLock::new(Mode::default);

impl State {
    pub fn new(config: Config) -> State {
        State {
            config,
            mode: "main".into(),
            monitors: Vec::new(),
            visible: BTreeMap::new(),
            focused_monitor: None,
            workspaces: BTreeMap::new(),
            windows: HashMap::new(),
            ignored: HashSet::new(),
            prev_workspace: None,
            applied: HashMap::new(),
            focus_dirty: false,
            suppress_hidden_focus: false,
            min_size: HashMap::new(),
            departed: Vec::new(),
        }
    }

    /// Applies an event and returns what the platform layer should do.
    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        let mut out = Vec::new();
        match event {
            Event::WindowAppeared(id, info) => self.on_appeared(id, info),
            Event::WindowDestroyed(id) => self.on_destroyed(id),
            Event::WindowFocused(id) => self.on_focused(id),
            Event::WindowMinimized(id) => self.on_minimized(id),
            Event::WindowRestored(id) => self.on_restored(id),
            Event::WindowMoveSizeEnd(id, rect) => self.on_move_size_end(id, rect),
            Event::WindowTitleChanged(id, title) => {
                if let Some(w) = self.windows.get_mut(&id) {
                    w.info.title = title;
                }
            }
            Event::WindowMinSize(id, w, h) => {
                if let Some(win) = self.windows.get(&id) {
                    let scale = self.workspace_scale(&win.workspace.clone());
                    let min = self.min_size.entry(id).or_default();
                    *min = (min.0.max(w as f32 / scale), min.1.max(h as f32 / scale));
                }
            }
            Event::MonitorsChanged(monitors) => self.on_monitors_changed(monitors),
            Event::Command(command) => self.run(command, &mut out),
            Event::Hotkey(chord) => {
                let commands = self.bindings().bindings.get(&chord).cloned();
                for command in commands.unwrap_or_default() {
                    self.run(command, &mut out);
                }
            }
            Event::ConfigReloaded(config) => {
                self.config = *config;
                // A reload is also the escape hatch for minimums recorded wrongly.
                self.min_size.clear();
                if !self.config.modes.contains_key(&self.mode) {
                    self.mode = "main".into();
                }
                out.push(Action::ModeChanged(self.mode.clone()));
            }
        }
        self.collect_garbage();
        let mut actions = self.sync();
        actions.append(&mut out);
        actions
    }

    /// The bindings of the current mode, for the keyboard hook.
    pub fn bindings(&self) -> &Mode {
        self.config.modes.get(&self.mode).unwrap_or(&EMPTY_MODE)
    }

    pub fn mode(&self) -> &str {
        &self.mode
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Every managed window, with the workspace it belongs to.
    pub fn managed_windows(&self) -> impl Iterator<Item = (WindowId, &str)> {
        self.windows
            .iter()
            .map(|(id, w)| (*id, w.workspace.as_str()))
    }

    // ---- window events -------------------------------------------------------------

    fn on_appeared(&mut self, id: WindowId, info: WindowInfo) {
        if self.windows.contains_key(&id) || self.ignored.contains(&id) {
            return;
        }
        let mut floating = info.float_hint;
        let mut target = None;
        let actions: Vec<RuleAction> = self
            .config
            .rules
            .iter()
            .filter(|r| r.matches(&info))
            .flat_map(|r| r.actions.iter().cloned())
            .collect();
        for action in actions {
            match action {
                RuleAction::Ignore => {
                    self.ignored.insert(id);
                    return;
                }
                RuleAction::Float => floating = true,
                RuleAction::Tile => floating = false,
                RuleAction::MoveToWorkspace(name) => target = Some(name),
            }
        }
        let workspace = target
            .or_else(|| self.focused_workspace().map(str::to_owned))
            .unwrap_or_else(|| "1".into());
        let shown_on = self.monitor_showing(&workspace);
        if shown_on.is_none() {
            self.suppress_hidden_focus = true;
        }
        let floating_rect = self.to_relative(info.rect, shown_on);
        self.windows.insert(
            id,
            Managed {
                floating_rect,
                info,
                workspace,
                floating,
                minimized: false,
            },
        );
        self.attach(id);
    }

    fn on_destroyed(&mut self, id: WindowId) {
        if self.ignored.remove(&id) || !self.windows.contains_key(&id) {
            return;
        }
        if self.focused_window() == Some(id) {
            self.focus_dirty = true;
        }
        self.suppress_hidden_focus = true;
        self.detach(id);
        self.windows.remove(&id);
        self.applied.remove(&id);
        self.min_size.remove(&id);
    }

    fn on_focused(&mut self, id: WindowId) {
        let suppress = std::mem::take(&mut self.suppress_hidden_focus);
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        if w.minimized {
            return;
        }
        let name = w.workspace.clone();
        if suppress && self.monitor_showing(&name).is_none() {
            return;
        }
        if let Some(m) = self.reveal(&name) {
            self.focused_monitor = Some(m);
        }
        self.workspace_mut(&name).touch(id);
    }

    /// Makes sure a workspace is on screen, preferring its pinned monitor, then the one
    /// it was last on, then the focused one. Returns the monitor showing it.
    fn reveal(&mut self, name: &str) -> Option<MonitorId> {
        if let Some(m) = self.monitor_showing(name) {
            return Some(m);
        }
        let last = self
            .workspaces
            .get(name)
            .and_then(|ws| ws.last_monitor)
            .filter(|m| self.monitor(*m).is_some());
        let target = self
            .forced_monitor(name)
            .or(last)
            .or(self.focused_monitor)?;
        self.prev_workspace = self.focused_workspace().map(str::to_owned);
        self.show_workspace(name, target);
        Some(target)
    }

    fn on_minimized(&mut self, id: WindowId) {
        if !self.windows.get(&id).is_some_and(|w| !w.minimized) {
            return;
        }
        self.detach(id);
        self.window_mut(id).minimized = true;
        self.applied.remove(&id);
        self.suppress_hidden_focus = true;
    }

    fn on_restored(&mut self, id: WindowId) {
        if !self.windows.get(&id).is_some_and(|w| w.minimized) {
            return;
        }
        self.window_mut(id).minimized = false;
        self.attach(id);
        let name = self.windows[&id].workspace.clone();
        if let Some(m) = self.reveal(&name) {
            self.focused_monitor = Some(m);
        }
    }

    fn on_move_size_end(&mut self, id: WindowId, rect: Rect) {
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        if w.minimized {
            return;
        }
        let name = w.workspace.clone();
        let floating = w.floating;

        // Dropped on another monitor: the window moves to the workspace shown there.
        let (cx, cy) = rect.center();
        let dropped_on = self
            .monitors
            .iter()
            .find(|m| {
                let a = m.bounds;
                (a.x..a.right()).contains(&cx) && (a.y..a.bottom()).contains(&cy)
            })
            .map(|m| m.id);
        // Only a pure move is a drop: a resize can also push the center across.
        let moved_only = match self.applied.get(&id) {
            Some(Placement::At(old)) => (old.w, old.h) == (rect.w, rect.h),
            _ => true,
        };
        let dropped_on = dropped_on.filter(|_| moved_only);
        if let Some(m) = dropped_on.filter(|m| Some(*m) != self.monitor_showing(&name)) {
            if let Some(target) = self.visible.get(&m).cloned() {
                self.move_node(id, &target);
                self.focused_monitor = Some(m);
                if floating {
                    self.window_mut(id).floating_rect = self.to_relative(rect, Some(m));
                    self.applied.insert(id, Placement::At(rect));
                } else {
                    self.applied.remove(&id);
                }
                return;
            }
        }

        if floating {
            let relative = self.to_relative(rect, self.monitor_showing(&name));
            self.window_mut(id).floating_rect = relative;
            self.applied.insert(id, Placement::At(rect));
            return;
        }
        // The user made the window smaller than its recorded minimum, so the
        // minimum was too high.
        let scale = self.workspace_scale(&name);
        if let Some(min) = self.min_size.get_mut(&id) {
            *min = (
                min.0.min(rect.w as f32 / scale),
                min.1.min(rect.h as f32 / scale),
            );
        }
        if let (Some(Placement::At(old)), Some(area)) =
            (self.applied.get(&id).copied(), self.workspace_area(&name))
        {
            // Only a size change is a resize; a pure move snaps back below. Each edge
            // that moved trades space with the neighbor on that side only.
            let mut edges = Vec::new();
            if rect.w != old.w {
                edges.push((Direction::Left, old.x - rect.x));
                edges.push((Direction::Right, rect.right() - old.right()));
            }
            if rect.h != old.h {
                edges.push((Direction::Up, old.y - rect.y));
                edges.push((Direction::Down, rect.bottom() - old.bottom()));
            }
            let gaps = self.workspace_gaps(&name);
            let mins = self.workspace_mins(&name);
            let min = |w: WindowId| mins.get(&w).copied().unwrap_or((0, 0));
            let root = &mut self.workspace_mut(&name).root;
            for (side, delta) in edges {
                if delta != 0 {
                    root.resize_edge_with(id, side, delta, area, gaps, &min);
                }
            }
        }
        // Forget the applied placement so the next sync snaps the window into place.
        self.applied.remove(&id);
    }

    fn on_monitors_changed(&mut self, mut monitors: Vec<Monitor>) {
        let gone: Vec<Monitor> = self
            .monitors
            .iter()
            .filter(|old| !monitors.iter().any(|m| m.id == old.id))
            .cloned()
            .collect();
        for m in gone {
            if let Some(ws) = self.visible.get(&m.id).cloned() {
                let key = (m.name, (m.work_area.x, m.work_area.y));
                self.departed.retain(|(n, p, _)| (n, p) != (&key.0, &key.1));
                self.departed.push((key.0, key.1, ws));
            }
        }
        if monitors.is_empty() {
            // Displays are being reconfigured; keep everything until they return.
            self.monitors.clear();
            return;
        }
        monitors.sort_by_key(|m| (m.work_area.x, m.work_area.y));
        self.monitors = monitors;
        let ids: Vec<MonitorId> = self.monitors.iter().map(|m| m.id).collect();
        self.visible.retain(|m, _| ids.contains(m));
        if !self.focused_monitor.is_some_and(|m| ids.contains(&m)) {
            self.focused_monitor = self
                .monitors
                .iter()
                .find(|m| m.primary)
                .or(self.monitors.first())
                .map(|m| m.id);
        }
        for id in ids {
            if !self.visible.contains_key(&id) {
                let name = match self.take_departed(id) {
                    Some(name) => name,
                    None => self.pick_workspace_for(id),
                };
                self.show_workspace(&name, id);
            }
        }
    }

    /// Takes the workspace that was on this monitor before it went away, if it is
    /// still hidden.
    fn take_departed(&mut self, monitor: MonitorId) -> Option<String> {
        let m = self.monitor(monitor)?;
        let key = (m.name.clone(), (m.work_area.x, m.work_area.y));
        let i = self.departed.iter().position(|(n, p, ws)| {
            (n, p) == (&key.0, &key.1) && !self.visible.values().any(|v| v == ws)
        })?;
        Some(self.departed.remove(i).2)
    }

    /// Chooses which workspace a monitor without one should show.
    fn pick_workspace_for(&self, monitor: MonitorId) -> String {
        let hidden = |name: &str| !self.visible.values().any(|v| v == name);
        let forced = self
            .config
            .workspace_monitor
            .keys()
            .find(|name| hidden(name) && self.forced_monitor(name) == Some(monitor));
        let previous = self
            .workspaces
            .iter()
            .find(|(name, ws)| {
                ws.last_monitor == Some(monitor)
                    && hidden(name)
                    && self.forced_monitor(name).map_or(true, |f| f == monitor)
            })
            .map(|(name, _)| name);
        if let Some(name) = forced.or(previous) {
            return name.clone();
        }
        (1..)
            .map(|n: u32| n.to_string())
            .find(|name| {
                hidden(name)
                    && self
                        .forced_monitor(name)
                        .map_or(true, |forced| forced == monitor)
            })
            .expect("there are infinitely many workspace numbers")
    }

    // ---- commands ------------------------------------------------------------------

    fn run(&mut self, command: Command, out: &mut Vec<Action>) {
        match command {
            Command::Focus(dir) => self.focus_direction(dir),
            Command::Move(dir) => self.move_direction(dir),
            Command::JoinWith(dir) => {
                if let Some((name, id)) = self.focused_tiled() {
                    self.workspace_mut(&name).root.join_with(id, dir);
                }
            }
            Command::Layout(args) => self.layout(&args),
            Command::Resize(dim, amount) => {
                if let Some((name, id)) = self.focused_tiled() {
                    if let Some(area) = self.workspace_area(&name) {
                        let gaps = self.workspace_gaps(&name);
                        let mins = self.workspace_mins(&name);
                        let min = |w: WindowId| mins.get(&w).copied().unwrap_or((0, 0));
                        self.workspace_mut(&name)
                            .root
                            .resize_with(id, dim, amount, area, gaps, &min);
                    }
                }
            }
            Command::BalanceSizes => {
                if let Some(name) = self.focused_workspace().map(str::to_owned) {
                    let ws = self.workspace_mut(&name);
                    ws.root.balance();
                    // Also forget minimums: a way out if one was recorded wrongly.
                    let ids: Vec<WindowId> = ws.windows().collect();
                    for id in ids {
                        self.min_size.remove(&id);
                    }
                }
            }
            Command::FlattenWorkspaceTree => {
                if let Some(name) = self.focused_workspace().map(str::to_owned) {
                    self.workspace_mut(&name).root.flatten();
                }
            }
            Command::Close => {
                if let Some(id) = self.focused_window() {
                    out.push(Action::Close(id));
                }
            }
            Command::Fullscreen => {
                if let (Some(name), Some(id)) = (
                    self.focused_workspace().map(str::to_owned),
                    self.focused_window(),
                ) {
                    let ws = self.workspace_mut(&name);
                    ws.fullscreen = if ws.fullscreen == Some(id) {
                        None
                    } else {
                        Some(id)
                    };
                    self.focus_dirty = true;
                }
            }
            Command::Workspace(name) => self.switch_workspace(&name),
            Command::WorkspaceBackAndForth => {
                if let Some(prev) = self.prev_workspace.clone() {
                    self.switch_workspace(&prev);
                }
            }
            Command::MoveNodeToWorkspace {
                name,
                focus_follows,
            } => {
                if let Some(id) = self.focused_window() {
                    if self.windows[&id].workspace != name {
                        self.move_node(id, &name);
                        self.focus_dirty = true;
                        if focus_follows {
                            self.switch_workspace(&name);
                        }
                    }
                }
            }
            Command::MoveWorkspaceToMonitor(target) => self.move_workspace_to_monitor(target),
            Command::FocusMonitor(target) => {
                if let Some(m) = self.resolve_monitor(target) {
                    self.focused_monitor = Some(m);
                    self.focus_dirty = true;
                }
            }
            Command::Mode(name) => {
                if self.config.modes.contains_key(&name) {
                    self.mode = name.clone();
                    out.push(Action::ModeChanged(name));
                } else {
                    out.push(Action::Error(format!("unknown mode '{name}'")));
                }
            }
            Command::ReloadConfig => out.push(Action::ReloadConfig),
            Command::Rescue => out.push(Action::Rescue),
            Command::Quit => out.push(Action::Quit),
            Command::DebugWindows => out.push(Action::DebugWindows),
            Command::ListWindows => out.push(Action::Reply(self.list_windows())),
            Command::ListWorkspaces { focused_only } => {
                let names: Vec<&str> = if focused_only {
                    self.focused_workspace().into_iter().collect()
                } else {
                    self.workspaces.keys().map(String::as_str).collect()
                };
                out.push(Action::Reply(names.join("\n")));
            }
            Command::ListMonitors => out.push(Action::Reply(self.list_monitors())),
        }
    }

    fn focus_direction(&mut self, dir: Direction) {
        // From a floating window, go back to the tiles of the same workspace first.
        if let Some(id) = self.focused_window().filter(|id| self.windows[id].floating) {
            let name = self.windows[&id].workspace.clone();
            let ws = self.workspace_mut(&name);
            if let Some(tiled) = ws.last_tiled() {
                ws.touch(tiled);
                self.focus_dirty = true;
                return;
            }
        }
        if let Some((name, id)) = self.focused_tiled() {
            let ws = self.workspace_mut(&name);
            if let Some(next) = ws.root.neighbor(id, dir) {
                ws.touch(next);
                self.focus_dirty = true;
                return;
            }
        }
        if let Some(m) = self.monitor_in_direction(dir) {
            self.focused_monitor = Some(m);
            self.focus_dirty = true;
        }
    }

    fn move_direction(&mut self, dir: Direction) {
        let Some((name, id)) = self.focused_tiled() else {
            return;
        };
        if self.workspace_mut(&name).root.move_window(id, dir) {
            self.focus_dirty = true;
            return;
        }
        // Already at the edge of the workspace: continue onto the next monitor.
        if let Some(m) = self.monitor_in_direction(dir) {
            let target = self.visible[&m].clone();
            self.move_node(id, &target);
            self.focused_monitor = Some(m);
            self.focus_dirty = true;
        }
    }

    fn layout(&mut self, args: &[LayoutArg]) {
        let Some(id) = self.focused_window() else {
            return;
        };
        let name = self.windows[&id].workspace.clone();
        for arg in args {
            let floating = self.windows[&id].floating;
            match arg {
                LayoutArg::Floating | LayoutArg::Tiling => {
                    let want_floating = *arg == LayoutArg::Floating;
                    if floating == want_floating {
                        continue;
                    }
                    if want_floating {
                        if let Some(Placement::At(r)) = self.applied.get(&id).copied() {
                            let relative = self.to_relative(r, self.monitor_showing(&name));
                            self.window_mut(id).floating_rect = relative;
                        }
                    }
                    self.detach(id);
                    self.window_mut(id).floating = want_floating;
                    self.attach(id);
                    self.focus_dirty = true;
                    return;
                }
                LayoutArg::Tree(cmd) => {
                    if !floating && self.workspace_mut(&name).root.set_layout(id, *cmd) {
                        return;
                    }
                }
            }
        }
    }

    fn switch_workspace(&mut self, name: &str) {
        let current = self.focused_workspace().map(str::to_owned);
        if current.as_deref() == Some(name) {
            return;
        }
        if let Some(m) = self.monitor_showing(name) {
            self.focused_monitor = Some(m);
        } else {
            let Some(m) = self.forced_monitor(name).or(self.focused_monitor) else {
                return;
            };
            self.show_workspace(name, m);
            self.focused_monitor = Some(m);
        }
        self.prev_workspace = current;
        self.focus_dirty = true;
    }

    fn move_workspace_to_monitor(&mut self, target: MonitorTarget) {
        let (Some(from), Some(to)) = (self.focused_monitor, self.resolve_monitor(target)) else {
            return;
        };
        if from == to {
            return;
        }
        let (Some(a), Some(b)) = (
            self.visible.get(&from).cloned(),
            self.visible.get(&to).cloned(),
        ) else {
            return;
        };
        self.show_workspace(&a, to);
        self.show_workspace(&b, from);
        self.focused_monitor = Some(to);
        self.focus_dirty = true;
    }

    // ---- helpers -------------------------------------------------------------------

    fn window_mut(&mut self, id: WindowId) -> &mut Managed {
        self.windows.get_mut(&id).expect("window is managed")
    }

    fn workspace_mut(&mut self, name: &str) -> &mut Workspace {
        if !self.workspaces.contains_key(name) {
            let ws = Workspace::new(self.config.default_root_layout);
            self.workspaces.insert(name.to_owned(), ws);
        }
        self.workspaces.get_mut(name).expect("just inserted")
    }

    fn focused_workspace(&self) -> Option<&str> {
        self.focused_monitor
            .and_then(|m| self.visible.get(&m))
            .map(String::as_str)
    }

    fn focused_window(&self) -> Option<WindowId> {
        self.focused_workspace()
            .and_then(|name| self.workspaces.get(name))
            .and_then(Workspace::focused)
    }

    /// The focused workspace and window, if that window is tiled.
    fn focused_tiled(&self) -> Option<(String, WindowId)> {
        let id = self.focused_window()?;
        let w = &self.windows[&id];
        (!w.floating).then(|| (w.workspace.clone(), id))
    }

    fn monitor_showing(&self, name: &str) -> Option<MonitorId> {
        self.visible
            .iter()
            .find(|(_, ws)| *ws == name)
            .map(|(m, _)| *m)
    }

    fn monitor(&self, id: MonitorId) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.id == id)
    }

    /// The area windows of a workspace are tiled into, if it is visible.
    fn workspace_area(&self, name: &str) -> Option<Rect> {
        let m = self.monitor(self.monitor_showing(name)?)?;
        Some(m.work_area.inset(self.scaled_gaps(m).0))
    }

    /// The DPI scale of the monitor showing a workspace (1.0 when hidden).
    fn workspace_scale(&self, name: &str) -> f32 {
        self.monitor_showing(name)
            .and_then(|m| self.monitor(m))
            .map_or(1.0, |m| m.scale)
    }

    /// Minimum sizes of a workspace's windows in that monitor's pixels.
    fn workspace_mins(&self, name: &str) -> HashMap<WindowId, (i32, i32)> {
        let scale = self.workspace_scale(name);
        self.min_size
            .iter()
            .map(|(id, &(w, h))| {
                (
                    *id,
                    ((w * scale).round() as i32, (h * scale).round() as i32),
                )
            })
            .collect()
    }

    /// The inner gaps of the monitor showing a workspace.
    fn workspace_gaps(&self, name: &str) -> InnerGaps {
        self.monitor_showing(name)
            .and_then(|m| self.monitor(m))
            .map_or(self.config.inner_gaps, |m| self.scaled_gaps(m).1)
    }

    /// Outer gaps, inner gaps and accordion padding for a monitor, scaled by its DPI
    /// so they look the same size on every monitor.
    fn scaled_gaps(&self, m: &Monitor) -> (Sides, InnerGaps, i32) {
        let s = |v: i32| (v as f32 * m.scale).round() as i32;
        let o = self.config.outer_gaps;
        let i = self.config.inner_gaps;
        (
            Sides {
                left: s(o.left),
                top: s(o.top),
                right: s(o.right),
                bottom: s(o.bottom),
            },
            InnerGaps {
                horizontal: s(i.horizontal),
                vertical: s(i.vertical),
            },
            s(self.config.accordion_padding),
        )
    }

    /// Shows a workspace on a monitor, replacing whatever that monitor showed.
    fn show_workspace(&mut self, name: &str, monitor: MonitorId) {
        self.visible.insert(monitor, name.to_owned());
        self.workspace_mut(name).last_monitor = Some(monitor);
    }

    /// Puts a managed window into its workspace's tree or floating list and makes it
    /// that workspace's most recently focused window.
    fn attach(&mut self, id: WindowId) {
        let w = &self.windows[&id];
        let (name, floating) = (w.workspace.clone(), w.floating);
        let orientation = self.auto_orientation(&name);
        let ws = self.workspace_mut(&name);
        if floating {
            ws.floating.push(id);
        } else {
            if ws.root.is_empty() {
                ws.root.orientation = orientation;
            }
            let anchor = ws.last_tiled();
            ws.root.insert_after(anchor, id);
        }
        ws.touch(id);
    }

    fn detach(&mut self, id: WindowId) {
        let name = self.windows[&id].workspace.clone();
        if let Some(ws) = self.workspaces.get_mut(&name) {
            ws.forget(id);
        }
    }

    fn move_node(&mut self, id: WindowId, target: &str) {
        self.detach(id);
        self.window_mut(id).workspace = target.to_owned();
        self.attach(id);
    }

    /// The root orientation for a workspace whose tree is empty.
    fn auto_orientation(&self, name: &str) -> Orientation {
        if let Some(o) = self.config.default_root_orientation {
            return o;
        }
        let monitor = self
            .monitor_showing(name)
            .or(self.focused_monitor)
            .and_then(|m| self.monitor(m));
        match monitor {
            Some(m) if m.work_area.h > m.work_area.w => Orientation::V,
            _ => Orientation::H,
        }
    }

    fn forced_monitor(&self, workspace: &str) -> Option<MonitorId> {
        let pattern = self.config.workspace_monitor.get(workspace)?;
        let found = match pattern {
            MonitorPattern::Index(i) => i.checked_sub(1).and_then(|i| self.monitors.get(i)),
            MonitorPattern::Main => self.monitors.iter().find(|m| m.primary),
            MonitorPattern::Secondary => self.monitors.iter().find(|m| !m.primary),
            MonitorPattern::Name(re) => self.monitors.iter().find(|m| re.is_match(&m.name)),
        };
        found.map(|m| m.id)
    }

    fn resolve_monitor(&self, target: MonitorTarget) -> Option<MonitorId> {
        let n = self.monitors.len();
        let i = self
            .monitors
            .iter()
            .position(|m| Some(m.id) == self.focused_monitor)?;
        match target {
            MonitorTarget::Next => Some(self.monitors[(i + 1) % n].id),
            MonitorTarget::Prev => Some(self.monitors[(i + n - 1) % n].id),
            MonitorTarget::Dir(dir) => self.monitor_in_direction(dir),
        }
    }

    /// The nearest monitor in `dir` from the focused one, comparing centers.
    fn monitor_in_direction(&self, dir: Direction) -> Option<MonitorId> {
        let current = self.monitor(self.focused_monitor?)?;
        let (cx, cy) = current.work_area.center();
        self.monitors
            .iter()
            .filter_map(|m| {
                let (x, y) = m.work_area.center();
                let (along, across) = match dir {
                    Direction::Left => (cx - x, (y - cy).abs()),
                    Direction::Right => (x - cx, (y - cy).abs()),
                    Direction::Up => (cy - y, (x - cx).abs()),
                    Direction::Down => (y - cy, (x - cx).abs()),
                };
                (along > 0).then_some((along, across, m.id))
            })
            .min()
            .map(|(_, _, id)| id)
    }

    /// Converts a screen rect to one relative to a monitor's work area: the given
    /// monitor, else the one containing the rect's center, else the focused one.
    fn to_relative(&self, rect: Rect, monitor: Option<MonitorId>) -> Rect {
        let (cx, cy) = rect.center();
        let contains = |m: &&Monitor| {
            let a = m.work_area;
            (a.x..a.right()).contains(&cx) && (a.y..a.bottom()).contains(&cy)
        };
        let m = monitor
            .and_then(|id| self.monitor(id))
            .or_else(|| self.monitors.iter().find(contains))
            .or_else(|| self.focused_monitor.and_then(|id| self.monitor(id)));
        match m {
            Some(m) => Rect::new(
                rect.x - m.work_area.x,
                rect.y - m.work_area.y,
                rect.w,
                rect.h,
            ),
            None => rect,
        }
    }

    /// Places a floating rect (relative to a work area) on a monitor, keeping it fully
    /// on screen when it fits.
    fn place_floating(relative: Rect, area: Rect) -> Rect {
        let clamp = |pos: i32, size: i32, start: i32, len: i32| {
            (start + pos).min(start + len - size).max(start)
        };
        Rect::new(
            clamp(relative.x, relative.w, area.x, area.w),
            clamp(relative.y, relative.h, area.y, area.h),
            relative.w,
            relative.h,
        )
    }

    /// Removes empty workspaces that are not on screen.
    fn collect_garbage(&mut self) {
        let visible: HashSet<&String> = self.visible.values().collect();
        self.workspaces
            .retain(|name, ws| !ws.is_empty() || visible.contains(name));
    }

    // ---- output --------------------------------------------------------------------

    fn desired_placements(&self) -> BTreeMap<WindowId, Placement> {
        let mut out = BTreeMap::new();
        for (name, ws) in &self.workspaces {
            let monitor = self.monitor_showing(name).and_then(|m| self.monitor(m));
            let Some(monitor) = monitor else {
                for id in ws.windows() {
                    out.insert(id, Placement::Hidden);
                }
                continue;
            };
            let (outer, inner, padding) = self.scaled_gaps(monitor);
            let area = monitor.work_area.inset(outer);
            let min = |id: WindowId| {
                self.min_size.get(&id).map_or((0, 0), |&(w, h)| {
                    (
                        (w * monitor.scale).round() as i32,
                        (h * monitor.scale).round() as i32,
                    )
                })
            };
            let tiled = ws.root.layout_rects_with(area, inner, padding, &min);
            for (id, rect) in tiled {
                out.insert(id, Placement::At(rect));
            }
            for id in &ws.floating {
                let rect = Self::place_floating(self.windows[id].floating_rect, monitor.work_area);
                out.insert(*id, Placement::At(rect));
            }
            if let Some(id) = ws.fullscreen {
                out.insert(id, Placement::At(monitor.work_area));
            }
        }
        out
    }

    /// Emits the actions that move every window from where it is to where it should be.
    fn sync(&mut self) -> Vec<Action> {
        if self.monitors.is_empty() {
            return Vec::new();
        }
        let mut hides = Vec::new();
        let mut moves = Vec::new();
        for (id, want) in self.desired_placements() {
            let previous = self.applied.insert(id, want);
            if previous == Some(want) {
                continue;
            }
            match want {
                Placement::Hidden => hides.push(Action::Hide(id)),
                Placement::At(rect) => {
                    if previous == Some(Placement::Hidden) {
                        moves.push(Action::Show(id));
                    }
                    moves.push(Action::SetRect(id, rect));
                }
            }
        }
        hides.append(&mut moves);
        if std::mem::take(&mut self.focus_dirty) {
            hides.push(Action::Focus(self.focused_window()));
        }
        hides
    }

    fn list_windows(&self) -> String {
        let mut ids: Vec<&WindowId> = self.windows.keys().collect();
        ids.sort();
        ids.iter()
            .map(|id| {
                let w = &self.windows[id];
                format!(
                    "{} | {} | {} | {}",
                    id.0, w.workspace, w.info.process, w.info.title
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn list_monitors(&self) -> String {
        self.monitors
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let primary = if m.primary { " (primary)" } else { "" };
                format!(
                    "{} | {} | {}x{}{primary}",
                    i + 1,
                    m.name,
                    m.work_area.w,
                    m.work_area.h
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Checks internal consistency. Used by tests.
    pub fn check_invariants(&self) -> Result<(), String> {
        let mut seen: HashMap<WindowId, &str> = HashMap::new();
        for (name, ws) in &self.workspaces {
            ws.root
                .check_invariants()
                .map_err(|e| format!("workspace {name}: {e}"))?;
            for id in ws.windows() {
                if let Some(other) = seen.insert(id, name) {
                    return Err(format!("window {} in {other} and {name}", id.0));
                }
                let w = self
                    .windows
                    .get(&id)
                    .ok_or_else(|| format!("unknown window {} in {name}", id.0))?;
                if w.workspace != *name || w.minimized {
                    return Err(format!("window {} misfiled in {name}", id.0));
                }
                if w.floating != ws.floating.contains(&id) {
                    return Err(format!("window {} floating flag mismatch", id.0));
                }
            }
            if let Some(id) = ws.mru.iter().find(|id| !ws.windows().any(|w| w == **id)) {
                return Err(format!("mru of {name} has stray window {}", id.0));
            }
        }
        for (id, w) in &self.windows {
            if !w.minimized && !seen.contains_key(id) {
                return Err(format!("window {} is in no workspace", id.0));
            }
            if !w.minimized && !self.monitors.is_empty() && !self.applied.contains_key(id) {
                return Err(format!("window {} was never placed or hidden", id.0));
            }
        }
        let mut shown = HashSet::new();
        for name in self.visible.values() {
            if !self.workspaces.contains_key(name) || !shown.insert(name) {
                return Err(format!("visible workspace {name} missing or shown twice"));
            }
        }
        Ok(())
    }
}
