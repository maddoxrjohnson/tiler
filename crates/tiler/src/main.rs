//! `tiler` — the server and (soon) the CLI client.

mod translate;

#[cfg(windows)]
mod server;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: tiler server [--dry-run] [--config PATH]";

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
            other => return Err(format!("unknown option '{other}'\n{USAGE}")),
        }
    }
    Ok(opts)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("server") => match parse_server_options(&args[1..]) {
            Ok(opts) => run_server(opts),
            Err(e) => {
                eprintln!("tiler: {e}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
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
