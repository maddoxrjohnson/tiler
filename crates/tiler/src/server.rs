//! The tiler server: receives window events, runs the core, applies its actions.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;

use crossbeam_channel::{select, unbounded};
use tiler_core::{Action, Config, Event, Monitor, State, WindowId};
use tiler_windows::win::apply::{self, HideStrategy, OffscreenHide};
use tiler_windows::win::hook::{self, RawEvent};
use tiler_windows::win::{monitors, window, Hwnd};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::translate::{self, Context, Kind};
use crate::ServerOptions;

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

pub fn config_path(opts: &ServerOptions) -> Option<PathBuf> {
    opts.config.clone().or_else(|| {
        std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".tiler.toml"))
    })
}

/// Reads the config file, falling back to the built-in default when there is none.
fn load_config(path: Option<&PathBuf>) -> Result<Config, String> {
    match path.filter(|p| p.exists()) {
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            Config::from_toml(&text).map_err(|e| format!("{}: {e}", p.display()))
        }
        None => Ok(Config::default()),
    }
}

struct Server {
    state: State,
    hide: Box<dyn HideStrategy>,
    monitors: Vec<Monitor>,
    dry_run: bool,
    we_are_elevated: bool,
    config_path: Option<PathBuf>,
}

impl Server {
    /// Feeds an event to the core and carries out the resulting actions. Returns
    /// false when the server should stop.
    fn dispatch(&mut self, event: Event) -> bool {
        if let Event::MonitorsChanged(m) = &event {
            self.monitors.clone_from(m);
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
        if self.dry_run {
            tracing::info!(?action, "dry run");
            return !matches!(action, Action::Quit);
        }
        tracing::debug!(?action, "apply");
        match action {
            Action::SetRect(id, rect) => apply::set_rect(Hwnd::from_id(id), rect),
            Action::Hide(id) => self.hide.hide(Hwnd::from_id(id), &self.monitors),
            Action::Show(id) => self.hide.show(Hwnd::from_id(id)),
            Action::Focus(id) => apply::focus(id.map(Hwnd::from_id)),
            Action::Close(id) => apply::close(Hwnd::from_id(id)),
            Action::Reply(text) => tracing::info!("{text}"),
            Action::Error(text) => tracing::warn!("{text}"),
            Action::ModeChanged(mode) => tracing::info!(%mode, "mode"),
            Action::ReloadConfig => self.reload_config(),
            Action::Rescue => self.restore_hidden(),
            Action::DebugWindows => debug_windows(),
            Action::Quit => return false,
        }
        true
    }

    fn reload_config(&mut self) {
        match load_config(self.config_path.as_ref()) {
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
                let w = r.w.min(area.w);
                let h = r.h.min(area.h);
                apply::set_rect(hwnd, tiler_core::Rect::new(area.x + 40, area.y + 40, w, h));
            }
        }
    }

    fn on_raw(&mut self, raw: RawEvent) -> bool {
        let Some(kind) = kind(raw.event) else {
            return true;
        };
        let id = raw.hwnd.id();
        let cx = Context {
            managed: self.managed().contains(&id),
            hidden_by_us: self.hide.hidden().contains(&raw.hwnd),
            we_are_elevated: self.we_are_elevated,
        };
        match translate::translate(kind, id, &cx, || window::facts(raw.hwnd)) {
            Some(event) => self.dispatch(event),
            None => true,
        }
    }

    fn managed(&self) -> HashSet<WindowId> {
        self.state.managed_windows().map(|(id, _)| id).collect()
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

    let config_path = config_path(&opts);
    let config = load_config(config_path.as_ref()).unwrap_or_else(|e| {
        tracing::error!("{e}; using the default config");
        Config::default()
    });

    let mut server = Server {
        state: State::new(config),
        hide: Box::new(OffscreenHide::default()),
        monitors: Vec::new(),
        dry_run: opts.dry_run,
        we_are_elevated: window::is_process_elevated(),
        config_path,
    };
    if server.dry_run {
        tracing::info!("dry run: no window will be moved");
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

    let (raw_tx, raw_rx) = unbounded();
    hook::spawn_event_hook(raw_tx);
    let (stop_tx, stop_rx) = unbounded();
    if let Err(e) = ctrlc::set_handler(move || {
        let _ = stop_tx.send(());
    }) {
        tracing::warn!(%e, "could not install the Ctrl-C handler");
    }
    tracing::info!("tiler is running; press Ctrl-C to stop");

    loop {
        select! {
            recv(raw_rx) -> raw => match raw {
                Ok(raw) => if !server.on_raw(raw) { break },
                Err(_) => break,
            },
            recv(stop_rx) -> _ => break,
        }
    }

    tracing::info!("stopping; restoring hidden windows");
    if !server.dry_run {
        server.restore_hidden();
    }
    ExitCode::SUCCESS
}
