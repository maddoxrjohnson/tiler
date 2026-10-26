//! `tiler`: the server, and the CLI that talks to it.

#[cfg_attr(not(windows), allow(dead_code))]
mod ipc;
#[cfg_attr(not(windows), allow(dead_code))]
mod recovery;
#[cfg_attr(not(windows), allow(dead_code))]
mod translate;

#[cfg(windows)]
mod client;
#[cfg(windows)]
mod console;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod server;
#[cfg(windows)]
mod watch;

use std::path::PathBuf;
use std::process::ExitCode;

const SERVER_USAGE: &str = "usage: tiler [server] [--dry-run] [--background] [--config PATH]";

const USAGE: &str = "usage: tiler [server] [--dry-run] [--background] [--config PATH]
       tiler <command> [args...]

With no command, tiler starts the server.

Commands are the same as in the config file, e.g.
  tiler workspace 2          tiler focus left        tiler list-windows
  tiler move-node-to-workspace 3                      tiler reload-config
  tiler rescue               (works even when the server is not running)
  tiler quit

All commands: ";

/// Options for `tiler server`.
#[derive(Debug, Default)]
pub struct ServerOptions {
    /// Log the actions tiler would take instead of moving any window.
    pub dry_run: bool,
    /// Detach from the console window and log to a file (used when starting at
    /// login).
    pub background: bool,
    pub config: Option<PathBuf>,
}

fn parse_server_options(args: &[String]) -> Result<ServerOptions, String> {
    let mut opts = ServerOptions::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => opts.dry_run = true,
            "--background" => opts.background = true,
            "--config" => {
                let path = args.next().ok_or("--config needs a path")?;
                opts.config = Some(PathBuf::from(path));
            }
            other => return Err(format!("unknown option '{other}'\n{SERVER_USAGE}")),
        }
    }
    Ok(opts)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The server logs at info by default; CLI commands stay quiet unless asked.
    let is_client = args.first().is_some_and(|a| {
        !matches!(
            a.as_str(),
            "server" | "--dry-run" | "--background" | "--config"
        )
    });
    let default_level = if is_client { "warn" } else { "info" };
    let filter = tracing_subscriber::EnvFilter::try_from_env("TILER_LOG")
        .unwrap_or_else(|_| default_level.into());
    // In the background there is no console, so log to %LOCALAPPDATA%\tiler\tiler.log.
    let log_file = (!is_client && args.iter().any(|a| a == "--background"))
        .then(log_file)
        .flatten();
    match log_file {
        Some(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init(),
        None => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init(),
    }
    // Every command works in physical pixels. Without this, Windows virtualizes
    // window and monitor coordinates for scaled displays while DWM still reports
    // physical ones, and the two don't mix.
    #[cfg(windows)]
    enable_dpi_awareness();
    match args.first().map(String::as_str) {
        // `tiler` alone, or with only server options, starts the server.
        None | Some("server" | "--dry-run" | "--background" | "--config") => {
            let options = match args.first().map(String::as_str) {
                Some("server") => &args[1..],
                _ => &args[..],
            };
            match parse_server_options(options) {
                Ok(opts) => run_server(opts),
                Err(e) => {
                    eprintln!("tiler: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Some("help" | "--help" | "-h") => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Some(_) => run_client(&args),
    }
}

/// The background log file, appended to.
fn log_file() -> Option<std::fs::File> {
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("tiler");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("tiler.log"))
        .ok()
}

#[cfg(windows)]
fn enable_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    // SAFETY: called once at startup, before any window is created.
    if let Err(e) =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
    {
        tracing::warn!(%e, "could not enable per-monitor DPI awareness");
    }
}

fn usage() -> String {
    format!("{USAGE}{}", tiler_core::command::COMMAND_NAMES.join(", "))
}

#[cfg(windows)]
fn run_client(args: &[String]) -> ExitCode {
    // Parse locally first so typos are reported instantly, even with no server.
    if let Err(e) = tiler_core::parse_args(args) {
        eprintln!("tiler: {e}");
        return ExitCode::from(2);
    }
    match client::send(args) {
        Ok(response) => {
            if !response.output.is_empty() {
                if response.ok {
                    println!("{}", response.output);
                } else {
                    eprintln!("{}", response.output);
                }
            }
            if response.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(client::ClientError::NotRunning) if args[0] == "rescue" => {
            // Exact positions from a crashed server's recovery file first, then
            // anything else that is stranded off-screen.
            let recovered =
                recovery::default_path().map_or(0, |p| recovery::restore::from_file(&p));
            let elevated = tiler_windows::win::window::is_process_elevated();
            let moved = tiler_windows::win::rescue::rescue_offscreen_windows(elevated);
            println!(
                "tiler server is not running; moved {} off-screen window(s) back",
                recovered + moved
            );
            ExitCode::SUCCESS
        }
        Err(client::ClientError::NotRunning) => {
            eprintln!("tiler server is not running (start it with `tiler server`)");
            ExitCode::from(3)
        }
        Err(client::ClientError::Io(e)) => {
            eprintln!("tiler: could not talk to the server: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run_client(_: &[String]) -> ExitCode {
    eprintln!("tiler only runs on Windows");
    ExitCode::FAILURE
}

#[cfg(windows)]
fn run_server(opts: ServerOptions) -> ExitCode {
    server::run(opts)
}

#[cfg(not(windows))]
fn run_server(_: ServerOptions) -> ExitCode {
    eprintln!("tiler only runs on Windows");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn server_options() {
        let o = parse_server_options(&args(&["--dry-run", "--config", "x.toml"])).unwrap();
        assert!(o.dry_run);
        assert_eq!(o.config, Some(PathBuf::from("x.toml")));
        assert!(parse_server_options(&args(&["--bogus"])).is_err());
        assert!(parse_server_options(&args(&["--config"])).is_err());
    }
}
