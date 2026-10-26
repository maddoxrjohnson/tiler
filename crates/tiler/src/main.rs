//! `tiler` — the server and (soon) the CLI client.

#[cfg_attr(not(windows), allow(dead_code))]
mod ipc;
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

const SERVER_USAGE: &str = "usage: tiler [server] [--dry-run] [--config PATH]";

const USAGE: &str = "usage: tiler [server] [--dry-run] [--config PATH]
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
    pub config: Option<PathBuf>,
}

fn parse_server_options(args: &[String]) -> Result<ServerOptions, String> {
    let mut opts = ServerOptions::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => opts.dry_run = true,
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
    match args.first().map(String::as_str) {
        // `tiler` alone, or with only server options, starts the server.
        None | Some("server" | "--dry-run" | "--config") => {
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
            let elevated = tiler_windows::win::window::is_process_elevated();
            let moved = tiler_windows::win::rescue::rescue_offscreen_windows(elevated);
            println!("tiler server is not running; moved {moved} off-screen window(s) back");
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
