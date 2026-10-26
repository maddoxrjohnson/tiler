//! The tiler server: receives window events, runs the core, applies its actions.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crossbeam_channel::{select, unbounded};
use tiler_core::{parse_args, Action, Config, Event, Monitor, Rect, State};
use tiler_windows::win::apply::{self, HideStrategy, OffscreenHide};
use tiler_windows::win::hook::{self, RawEvent};
use tiler_windows::win::keyboard::{self, Bindings};
use tiler_windows::win::rescue;
use tiler_windows::win::{monitors, window, Hwnd};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::console;
use crate::ipc::{Request, Response};
use crate::pipe;
use crate::translate::{self, Context, Kind};
use crate::watch;
use crate::ServerOptions;

/// How long to wait before placing a just-unmaximized window again, once its
/// normal (not maximized) borders can be measured.
const REPLACE_DELAY: Duration = Duration::from_millis(150);

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

struct Server {
    state: State,
    hide: Box<dyn HideStrategy>,
    monitors: Vec<Monitor>,
    dry_run: bool,
    we_are_elevated: bool,
    config_path: Option<PathBuf>,
    config_explicit: bool,
    /// Windows that were maximized when placed, with their target rect.
    replace_soon: HashMap<Hwnd, Rect>,
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
            }
            _ => {}
        }
        tracing::debug!(?event, "event");
        let actions = self.state.handle(event);
        let mut keep_running = true;
        for action in actions {
            keep_running &= self.execute(action);
        }
        keep_running
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
                if apply::is_maximized(hwnd) {
                    apply::unmaximize(hwnd);
                    self.replace_soon.insert(hwnd, rect);
                }
                apply::set_rect(hwnd, rect);
            }
            Action::Hide(id) => self.hide.hide(Hwnd::from_id(id), &self.monitors),
            Action::Show(id) => self.hide.show(Hwnd::from_id(id)),
            Action::Focus(id) => apply::focus(id.map(Hwnd::from_id)),
            Action::Close(id) => apply::close(Hwnd::from_id(id)),
            Action::Reply(text) => tracing::info!("{text}"),
            Action::Error(text) => tracing::warn!("{text}"),
            Action::ModeChanged(mode) => tracing::info!(%mode, "mode"),
            Action::ReloadConfig => self.reload_config(),
            Action::Rescue => {
                self.restore_hidden();
                let moved = rescue::rescue_offscreen_windows();
                self.output
                    .push(Ok(format!("moved {moved} off-screen window(s) back")));
            }
            Action::DebugWindows => {}
            Action::Quit => return false,
        }
        true
    }

    fn reload_config(&mut self) {
        match load_config(self.config_path.as_ref(), self.config_explicit) {
            Ok(config) => {
                tracing::info!("config reloaded");
                self.dispatch(Event::ConfigReloaded(Box::new(config)));
            }
            Err(e) => tracing::error!("config not reloaded: {e}"),
        }
    }

    /// Brings every window tiler hid back onto the primary monitor.
    fn restore_hidden(&mut self) {
        let area = self
            .monitors
            .iter()
            .find(|m| m.primary)
            .or(self.monitors.first())
            .map(|m| m.work_area);
        for hwnd in self.hide.hidden() {
            self.hide.show(hwnd);
            if let (Some(area), Some(r)) = (area, window::visible_rect(hwnd)) {
                let rect = Rect::new(area.x + 40, area.y + 40, r.w.min(area.w), r.h.min(area.h));
                apply::restore_to(hwnd, rect);
            }
        }
    }

    fn refresh_bindings(&mut self) {
        let chords: HashSet<_> = self.state.bindings().bindings.keys().copied().collect();
        if let Ok(mut b) = self.bindings.write() {
            *b = chords;
        }
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

    /// Places windows that were maximized again, now that they are restored.
    fn replace_unmaximized(&mut self) {
        for (hwnd, rect) in std::mem::take(&mut self.replace_soon) {
            apply::set_rect(hwnd, rect);
        }
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
        match translate::translate(kind, id, &cx, || window::facts(raw.hwnd)) {
            Some(event) => self.dispatch(event),
            None => true,
        }
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TILER_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    // SAFETY: called once at startup, before any window is created.
    if let Err(e) =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
    {
        tracing::warn!(%e, "could not enable per-monitor DPI awareness");
    }

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
        bindings: Bindings::default(),
        output: Vec::new(),
    };
    server.refresh_bindings();
    if server.dry_run {
        tracing::info!("dry run: no window will be moved");
    }

    let (stop_rx, cleanup_done) = console::install();
    // Start listening before adopting existing windows, so a window created in
    // between is not missed. The core ignores duplicate appearances.
    let (raw_tx, raw_rx) = unbounded();
    hook::spawn_event_hook(raw_tx);

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
    let (config_tx, config_rx) = unbounded();
    if let Some(path) = server.config_path.clone() {
        watch::spawn_config_watcher(path, config_tx);
    }
    tracing::info!("tiler is running; stop it with `tiler quit` or Ctrl-C");

    loop {
        let timeout = if server.replace_soon.is_empty() {
            Duration::from_secs(3600)
        } else {
            REPLACE_DELAY
        };
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
                Ok((request, reply)) => {
                    let (response, keep_running) = server.on_request(request);
                    let _ = reply.send(response);
                    if !keep_running { break }
                }
                Err(_) => break,
            },
            recv(config_rx) -> _ => server.reload_config(),
            recv(stop_rx) -> _ => break,
            default(timeout) => server.replace_unmaximized(),
        }
    }

    tracing::info!("stopping; restoring hidden windows");
    if !server.dry_run {
        server.restore_hidden();
    }
    let _ = cleanup_done.send(());
    ExitCode::SUCCESS
}
