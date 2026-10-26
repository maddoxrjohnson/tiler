//! The tiler server: receives window events, runs the core, applies its actions.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crossbeam_channel::{select, unbounded};
use tiler_core::{parse_args, Action, Config, Event, Monitor, Rect, State};
use tiler_windows::win::apply::{self, HideStrategy, OffscreenHide};
use tiler_windows::win::hook::{self, RawEvent};
use tiler_windows::win::keyboard::{self, Bindings};
use tiler_windows::win::{display, monitors, window, Hwnd};
use tiler_windows::win::{rescue, startup};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::console;
use crate::ipc::{Request, Response};
use crate::pipe;
use crate::recovery::{self, Parked, RecoveryFile};
use crate::translate::{self, Context, Kind};
use crate::watch;
use crate::ServerOptions;

/// How long to wait before placing a just-unmaximized window again, once its
/// normal (not maximized) borders can be measured.
const REPLACE_DELAY: Duration = Duration::from_millis(150);

/// How long display change messages must stop before the monitors are re-read.
const DISPLAY_SETTLE: Duration = Duration::from_millis(300);

fn kind(event: u32) -> Option<Kind> {
    Some(match event {
        hook::EVENT_OBJECT_SHOW => Kind::Show,
        hook::EVENT_OBJECT_UNCLOAKED => Kind::Uncloaked,
        hook::EVENT_OBJECT_HIDE => Kind::Hide,
        hook::EVENT_OBJECT_CLOAKED => Kind::Cloaked,
        hook::EVENT_OBJECT_DESTROY => Kind::Destroy,
        hook::EVENT_SYSTEM_FOREGROUND => Kind::Foreground,
        hook::EVENT_SYSTEM_MINIMIZESTART => Kind::MinimizeStart,
        hook::EVENT_SYSTEM_MINIMIZEEND => Kind::MinimizeEnd,
        hook::EVENT_SYSTEM_MOVESIZEEND => Kind::MoveSizeEnd,
        hook::EVENT_OBJECT_NAMECHANGE => Kind::NameChange,
        _ => return None,
    })
}

fn default_config_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".tiler.toml"))
}

/// Reads the config file. A missing default file means "use the built-in config";
/// a missing file the user named explicitly is an error.
fn load_config(path: Option<&PathBuf>, explicit: bool) -> Result<Config, String> {
    match path {
        Some(p) if p.exists() => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            Config::from_toml(&text).map_err(|e| format!("{}: {e}", p.display()))
        }
        Some(p) if explicit => Err(format!("{}: file not found", p.display())),
        _ => Ok(Config::default()),
    }
}

/// A pending check of a window's size after placing it.
#[derive(Debug, Clone, Copy)]
struct Verify {
    asked: Rect,
    due: Instant,
    /// The window was already too big once and has been placed again.
    second_look: bool,
}

struct Server {
    state: State,
    hide: Box<dyn HideStrategy>,
    monitors: Vec<Monitor>,
    dry_run: bool,
    we_are_elevated: bool,
    config_path: Option<PathBuf>,
    config_explicit: bool,
    /// Windows that were maximized when placed: target rect and when to place them
    /// again (after the asynchronous restore).
    replace_soon: HashMap<Hwnd, (Rect, Instant)>,
    /// Windows just placed, to check whether they accepted the size.
    verify_soon: HashMap<Hwnd, Verify>,
    /// When to re-read the monitors, after a burst of display change messages.
    display_at: Option<Instant>,
    /// Where each hidden window was before it was hidden.
    parked_rects: HashMap<Hwnd, Rect>,
    /// What the recovery file currently says.
    saved_parked: Vec<Parked>,
    /// `None` in a dry run, which never parks anything.
    recovery_path: Option<PathBuf>,
    /// The chords the keyboard hook swallows: the current mode's bindings.
    bindings: Bindings,
    /// Replies and errors produced while handling the current CLI request.
    output: Vec<Result<String, String>>,
}

impl Server {
    /// Feeds an event to the core and carries out the resulting actions. Returns
    /// false when the server should stop.
    fn dispatch(&mut self, event: Event) -> bool {
        match &event {
            Event::MonitorsChanged(m) => self.monitors.clone_from(m),
            // The core forgets these windows' placement without emitting Show.
            Event::WindowDestroyed(id) | Event::WindowMinimized(id) => {
                self.hide.forget(Hwnd::from_id(*id));
                self.parked_rects.remove(&Hwnd::from_id(*id));
            }
            _ => {}
        }
        tracing::debug!(?event, "event");
        let actions = self.state.handle(event);
        let mut keep_running = true;
        for action in actions {
            keep_running &= self.execute(action);
        }
        self.save_recovery();
        keep_running
    }

    /// Writes the recovery file (and the panic hook's copy) when the set of parked
    /// windows changed.
    fn save_recovery(&mut self) {
        let mut parked: Vec<Parked> = self
            .hide
            .hidden()
            .into_iter()
            .filter_map(|h| {
                let r = self.parked_rects.get(&h)?;
                Some(Parked {
                    hwnd: h.0 as i64,
                    rect: [r.x, r.y, r.w, r.h],
                })
            })
            .collect();
        parked.sort_by_key(|p| p.hwnd);
        if parked == self.saved_parked {
            return;
        }
        recovery::restore::mirror(&parked);
        if let Some(path) = &self.recovery_path {
            let file = RecoveryFile {
                pid: std::process::id(),
                parked: parked.clone(),
            };
            let result = if parked.is_empty() {
                recovery::remove(path);
                Ok(())
            } else {
                recovery::save(path, &file)
            };
            if let Err(e) = result {
                tracing::warn!(%e, "could not write the recovery file");
            }
        }
        self.saved_parked = parked;
    }

    fn execute(&mut self, action: Action) -> bool {
        match &action {
            Action::Reply(text) => self.output.push(Ok(text.clone())),
            Action::Error(text) => self.output.push(Err(text.clone())),
            Action::ModeChanged(_) => self.refresh_bindings(),
            // Read-only, so it also runs in a dry run.
            Action::DebugWindows => {
                debug_windows();
                return true;
            }
            _ => {}
        }
        if self.dry_run {
            tracing::info!(?action, "dry run");
            return !matches!(action, Action::Quit);
        }
        tracing::debug!(?action, "apply");
        match action {
            Action::SetRect(id, rect) => {
                let hwnd = Hwnd::from_id(id);
                let due = Instant::now() + REPLACE_DELAY;
                if apply::is_maximized(hwnd) {
                    apply::unmaximize(hwnd);
                    self.replace_soon.insert(hwnd, (rect, due));
                    self.verify_soon.remove(&hwnd);
                } else {
                    self.verify_soon.insert(
                        hwnd,
                        Verify {
                            asked: rect,
                            due,
                            second_look: false,
                        },
                    );
                }
                apply::set_rect(hwnd, rect);
            }
            Action::Hide(id) => {
                let hwnd = Hwnd::from_id(id);
                // Remember where it was, so recovery can put it back exactly.
                if !self.hide.is_hidden(hwnd) {
                    if let Some(r) = window::visible_rect(hwnd) {
                        self.parked_rects.insert(hwnd, r);
                    }
                }
                self.hide.hide(hwnd, &self.monitors);
            }
            Action::Show(id) => {
                let hwnd = Hwnd::from_id(id);
                self.hide.show(hwnd);
                self.parked_rects.remove(&hwnd);
            }
            Action::Focus(id) => apply::focus(id.map(Hwnd::from_id)),
            Action::Close(id) => apply::close(Hwnd::from_id(id)),
            Action::Reply(text) => tracing::info!("{text}"),
            Action::Error(text) => tracing::warn!("{text}"),
            Action::ModeChanged(mode) => tracing::info!(%mode, "mode"),
            Action::ReloadConfig => self.reload_config(),
            Action::Rescue => {
                self.restore_hidden();
                let moved = rescue::rescue_offscreen_windows(self.we_are_elevated);
                self.output
                    .push(Ok(format!("moved {moved} off-screen window(s) back")));
            }
            Action::DebugWindows => {}
            Action::Quit => return false,
        }
        true
    }

    fn apply_start_at_login(&self) {
        if !self.dry_run {
            startup::set_start_at_login(self.state.config().start_at_login);
        }
    }

    fn reload_config(&mut self) {
        match load_config(self.config_path.as_ref(), self.config_explicit) {
            Ok(config) => {
                tracing::info!("config reloaded");
                self.dispatch(Event::ConfigReloaded(Box::new(config)));
                self.apply_start_at_login();
            }
            Err(e) => tracing::error!("config not reloaded: {e}"),
        }
    }

    /// Brings every window tiler hid back onto the primary monitor.
    /// Puts every window tiler hid back where it was when it was hidden.
    fn restore_hidden(&mut self) {
        let fallback = self
            .monitors
            .iter()
            .find(|m| m.primary)
            .or(self.monitors.first())
            .map(|m| m.work_area);
        for hwnd in self.hide.hidden() {
            self.hide.show(hwnd);
            match self.parked_rects.remove(&hwnd) {
                Some(r) => {
                    let p = Parked {
                        hwnd: hwnd.0 as i64,
                        rect: [r.x, r.y, r.w, r.h],
                    };
                    recovery::restore::restore_all(&[p]);
                }
                None => {
                    if let (Some(a), Some(r)) = (fallback, window::visible_rect(hwnd)) {
                        let rect = Rect::new(a.x + 40, a.y + 40, r.w.min(a.w), r.h.min(a.h));
                        let _ = apply::restore_to(hwnd, rect);
                    }
                }
            }
        }
        self.save_recovery();
    }

    fn refresh_bindings(&mut self) {
        let chords: HashSet<_> = self.state.bindings().bindings.keys().copied().collect();
        *self
            .bindings
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = chords;
    }

    /// Runs a CLI command. Returns the response and whether to keep running.
    fn on_request(&mut self, request: Request) -> (Response, bool) {
        let command = match parse_args(&request.args) {
            Ok(c) => c,
            Err(e) => {
                return (
                    Response {
                        ok: false,
                        output: e.to_string(),
                    },
                    true,
                )
            }
        };
        self.output.clear();
        let keep_running = self.dispatch(Event::Command(command));
        let output = std::mem::take(&mut self.output);
        let ok = output.iter().all(Result::is_ok);
        let text: Vec<String> = output
            .into_iter()
            .map(|r| r.unwrap_or_else(|e| e))
            .collect();
        (
            Response {
                ok,
                output: text.join("\n"),
            },
            keep_running,
        )
    }

    /// Runs the delayed work whose time has come.
    fn run_timers(&mut self) {
        let now = Instant::now();

        // Windows that were maximized are restored by now: place them again with
        // their normal borders, and check their size after that.
        let due: Vec<(Hwnd, Rect)> = self
            .replace_soon
            .iter()
            .filter(|(_, (_, at))| *at <= now)
            .map(|(h, (r, _))| (*h, *r))
            .collect();
        for (hwnd, rect) in due {
            self.replace_soon.remove(&hwnd);
            apply::set_rect(hwnd, rect);
            self.verify_soon.insert(
                hwnd,
                Verify {
                    asked: rect,
                    due: now + REPLACE_DELAY,
                    second_look: false,
                },
            );
        }

        let due: Vec<(Hwnd, Verify)> = self
            .verify_soon
            .iter()
            .filter(|(_, v)| v.due <= now)
            .map(|(h, v)| (*h, *v))
            .collect();
        for (hwnd, v) in due {
            self.verify_soon.remove(&hwnd);
            self.verify_size(hwnd, v, now);
        }

        if self.display_at.is_some_and(|t| t <= now) {
            self.display_at = None;
            let monitors = monitors::monitors();
            if monitors.is_empty() {
                // Mid-reconfiguration; parking with no monitors would put hidden
                // windows on screen. Try again shortly.
                self.display_at = Some(now + DISPLAY_SETTLE);
                return;
            }
            tracing::info!("monitor setup changed");
            self.dispatch(Event::MonitorsChanged(monitors));
            // The parking spot depends on the monitor layout; a new monitor could
            // now be showing parked windows.
            for hwnd in self.hide.hidden() {
                self.hide.hide(hwnd, &self.monitors);
            }
        }
    }

    /// The next time `run_timers` has something to do.
    fn next_timer(&self) -> Option<Instant> {
        self.replace_soon
            .values()
            .map(|(_, at)| *at)
            .chain(self.verify_soon.values().map(|v| v.due))
            .chain(self.display_at)
            .min()
    }

    /// Checks whether a window accepted the size it was given. A window that is
    /// bigger is placed again and looked at once more: it may simply not have
    /// processed the move yet, or may have resized itself (DPI change, restoring
    /// its saved size). Only a second refusal is recorded as a minimum size.
    fn verify_size(&mut self, hwnd: Hwnd, v: Verify, now: Instant) {
        if self.hide.is_hidden(hwnd) {
            return;
        }
        let Some(actual) = window::visible_rect(hwnd) else {
            return;
        };
        // A few pixels of slack: some apps round their size.
        let w = if actual.w > v.asked.w + 4 {
            actual.w
        } else {
            0
        };
        let h = if actual.h > v.asked.h + 4 {
            actual.h
        } else {
            0
        };
        if w == 0 && h == 0 {
            return;
        }
        if !v.second_look {
            apply::set_rect(hwnd, v.asked);
            self.verify_soon.insert(
                hwnd,
                Verify {
                    second_look: true,
                    due: now + REPLACE_DELAY,
                    ..v
                },
            );
            return;
        }
        tracing::debug!(?hwnd, w, h, "window has a minimum size");
        self.dispatch(Event::WindowMinSize(hwnd.id(), w, h));
    }

    fn on_raw(&mut self, raw: RawEvent) -> bool {
        let Some(kind) = kind(raw.event) else {
            return true;
        };
        let id = raw.hwnd.id();
        let cx = Context {
            managed: self.state.managed_windows().any(|(m, _)| m == id),
            hidden_by_us: self.hide.emits_hide_events() && self.hide.is_hidden(raw.hwnd),
            we_are_elevated: self.we_are_elevated,
        };
        let mut keep_running = true;
        for event in translate::translate(kind, id, &cx, || window::facts(raw.hwnd)) {
            keep_running &= self.dispatch(event);
        }
        keep_running
    }
}

fn debug_windows() {
    for hwnd in window::enumerate() {
        if let Some(f) = window::facts(hwnd) {
            if f.visible {
                tracing::info!(?hwnd, manageable = tiler_windows::manageable(&f, false), ?f);
            }
        }
    }
}

pub fn run(opts: ServerOptions) -> ExitCode {
    // If tiler panics, put parked windows back before the process goes away.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let restored = recovery::restore::restore_mirrored();
        eprintln!("tiler crashed; restored {restored} hidden window(s)");
        default_hook(info);
    }));

    let config_explicit = opts.config.is_some();
    let config_path = opts.config.clone().or_else(default_config_path);
    let config = match load_config(config_path.as_ref(), config_explicit) {
        Ok(config) => config,
        Err(e) if config_explicit => {
            eprintln!("tiler: {e}");
            return ExitCode::from(2);
        }
        Err(e) => {
            tracing::error!("{e}; using the default config");
            Config::default()
        }
    };

    // Claim the CLI pipe first: it doubles as the single-instance check.
    let pipe_handle = match pipe::claim() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("tiler: could not start (is another tiler server running?): {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut server = Server {
        state: State::new(config),
        hide: Box::new(OffscreenHide::default()),
        monitors: Vec::new(),
        dry_run: opts.dry_run,
        we_are_elevated: window::is_process_elevated(),
        config_path,
        config_explicit,
        replace_soon: HashMap::new(),
        verify_soon: HashMap::new(),
        display_at: None,
        parked_rects: HashMap::new(),
        saved_parked: Vec::new(),
        recovery_path: if opts.dry_run {
            None
        } else {
            recovery::default_path()
        },
        bindings: Bindings::default(),
        output: Vec::new(),
    };
    server.refresh_bindings();
    server.apply_start_at_login();
    if server.dry_run {
        tracing::info!("dry run: no window will be moved");
    }

    let (stop_rx, cleanup_done) = console::install();
    // Start listening before adopting existing windows, so a window created in
    // between is not missed. The core ignores duplicate appearances.
    let (raw_tx, raw_rx) = unbounded();
    hook::spawn_event_hook(raw_tx);

    // A previous server that crashed or was killed may have left windows parked.
    if let Some(path) = &server.recovery_path {
        let restored = recovery::restore::from_file(path);
        if restored > 0 {
            tracing::info!(restored, "restored windows left hidden by a previous run");
        }
    }

    server.dispatch(Event::MonitorsChanged(monitors::monitors()));
    // Adopt existing windows bottom-to-top so the topmost one ends up focused.
    for hwnd in window::enumerate().into_iter().rev() {
        if let Some(e) = translate::appeared(hwnd.id(), window::facts(hwnd), server.we_are_elevated)
        {
            server.dispatch(e);
        }
    }
    // SAFETY: plain query.
    let foreground = Hwnd::from(unsafe { GetForegroundWindow() });
    server.dispatch(Event::WindowFocused(foreground.id()));
    let (cli_tx, cli_rx) = unbounded();
    pipe::spawn_pipe_server(pipe_handle, cli_tx);
    let (hotkey_tx, hotkey_rx) = unbounded();
    if server.dry_run {
        tracing::info!("dry run: hotkeys are disabled so no keys are swallowed");
    } else {
        keyboard::spawn_keyboard_hook(server.bindings.clone(), hotkey_tx);
    }
    let (display_tx, display_rx) = unbounded();
    let (session_tx, session_rx) = unbounded();
    let (session_done_tx, session_done_rx) = unbounded();
    display::spawn_display_listener(display_tx, session_tx, session_done_rx);
    let (config_tx, config_rx) = unbounded();
    if let Some(path) = server.config_path.clone() {
        watch::spawn_config_watcher(path, config_tx);
    }
    tracing::info!("tiler is running; stop it with `tiler quit` or Ctrl-C");

    let mut quit_reply = None;
    loop {
        // Run due timers on every iteration: a steady stream of events would
        // otherwise keep the `default` arm below from ever firing.
        server.run_timers();
        let timeout = server.next_timer().map_or(Duration::from_secs(3600), |t| {
            t.saturating_duration_since(Instant::now())
        });
        select! {
            recv(raw_rx) -> raw => match raw {
                Ok(raw) => if !server.on_raw(raw) { break },
                Err(_) => break,
            },
            recv(hotkey_rx) -> chord => match chord {
                Ok(chord) => {
                    server.output.clear();
                    if !server.dispatch(Event::Hotkey(chord)) { break }
                }
                Err(_) => break,
            },
            recv(cli_rx) -> incoming => match incoming {
                // The client was already told it timed out; don't run it late.
                Ok(incoming) if Instant::now() > incoming.deadline => {}
                Ok(incoming) => {
                    let (response, keep_running) = server.on_request(incoming.request);
                    if keep_running {
                        let _ = incoming.reply.send(response);
                    } else {
                        // Answer `quit` after cleanup, so "ok" means it is done.
                        quit_reply = Some((response, incoming.reply, incoming.written));
                        break;
                    }
                }
                Err(_) => break,
            },
            recv(config_rx) -> _ => server.reload_config(),
            recv(display_rx) -> _ => {
                // Displays change in bursts; wait until they settle.
                server.display_at = Some(Instant::now() + DISPLAY_SETTLE);
            }
            recv(stop_rx) -> _ => break,
            recv(session_rx) -> _ => {
                tracing::info!("Windows is logging off or shutting down");
                break;
            }
            default(timeout) => server.run_timers(),
        }
    }

    tracing::info!("stopping; restoring hidden windows");
    if !server.dry_run {
        server.restore_hidden();
    }
    if let Some((response, reply, written)) = quit_reply {
        let _ = reply.send(response);
        // Let the pipe thread deliver the reply before the process exits.
        let _ = written.recv_timeout(Duration::from_secs(2));
    }
    if let Some(path) = &server.recovery_path {
        recovery::remove(path);
    }
    let _ = cleanup_done.send(());
    let _ = session_done_tx.send(());
    ExitCode::SUCCESS
}
