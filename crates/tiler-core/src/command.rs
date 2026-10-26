//! The command language shared by config keybindings and the `tiler` CLI.
//!
//! `alt-h = 'focus left'` in the config and `tiler focus left` on the command line
//! go through the same parser and produce the same [`Command`].

use std::fmt;

use crate::geometry::Direction;
use crate::tree_ops::{LayoutCmd, ResizeAmount, ResizeDim};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Command {
    Focus(Direction),
    Move(Direction),
    JoinWith(Direction),
    /// Applies the first layout that is not already in effect, so
    /// `layout tiles accordion` toggles between the two.
    Layout(Vec<LayoutArg>),
    Resize(ResizeDim, ResizeAmount),
    BalanceSizes,
    FlattenWorkspaceTree,
    Close,
    Fullscreen,
    Workspace(String),
    WorkspaceBackAndForth,
    MoveNodeToWorkspace {
        name: String,
        focus_follows: bool,
    },
    MoveWorkspaceToMonitor(MonitorTarget),
    FocusMonitor(MonitorTarget),
    Mode(String),
    ReloadConfig,
    Rescue,
    ListWindows,
    ListWorkspaces {
        focused_only: bool,
    },
    ListMonitors,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutArg {
    Tree(LayoutCmd),
    Floating,
    Tiling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonitorTarget {
    Next,
    Prev,
    Dir(Direction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// Every command name, used for "did you mean" suggestions.
pub const COMMAND_NAMES: &[&str] = &[
    "focus",
    "move",
    "join-with",
    "layout",
    "resize",
    "balance-sizes",
    "flatten-workspace-tree",
    "close",
    "fullscreen",
    "workspace",
    "workspace-back-and-forth",
    "move-node-to-workspace",
    "move-workspace-to-monitor",
    "focus-monitor",
    "mode",
    "reload-config",
    "rescue",
    "list-windows",
    "list-workspaces",
    "list-monitors",
    "quit",
];

/// Parses a command written as a single string, e.g. `"workspace 'my notes'"`.
pub fn parse_command(input: &str) -> Result<Command, ParseError> {
    parse_args(&split_words(input)?)
}

/// Parses a command from already-split arguments, e.g. CLI arguments.
pub fn parse_args<S: AsRef<str>>(args: &[S]) -> Result<Command, ParseError> {
    let Some((name, rest)) = args.split_first() else {
        return Err(ParseError("empty command".into()));
    };
    let name = name.as_ref();
    let mut args = Args {
        command: name,
        rest: rest.iter().map(AsRef::as_ref).collect(),
    };
    let command = match name {
        "focus" => Command::Focus(args.direction()?),
        "move" => Command::Move(args.direction()?),
        "join-with" => Command::JoinWith(args.direction()?),
        "layout" => {
            let mut layouts = vec![args.layout()?];
            while !args.rest.is_empty() {
                layouts.push(args.layout()?);
            }
            Command::Layout(layouts)
        }
        "resize" => {
            let dim = args.choice(&[
                ("smart", ResizeDim::Smart),
                ("width", ResizeDim::Width),
                ("height", ResizeDim::Height),
            ])?;
            Command::Resize(dim, args.amount()?)
        }
        "balance-sizes" => Command::BalanceSizes,
        "flatten-workspace-tree" => Command::FlattenWorkspaceTree,
        "close" => Command::Close,
        "fullscreen" => Command::Fullscreen,
        "workspace" => Command::Workspace(args.word()?),
        "workspace-back-and-forth" => Command::WorkspaceBackAndForth,
        "move-node-to-workspace" => {
            let focus_follows = args.flag("--focus-follows-window");
            Command::MoveNodeToWorkspace {
                name: args.word()?,
                focus_follows,
            }
        }
        "move-workspace-to-monitor" => Command::MoveWorkspaceToMonitor(args.monitor()?),
        "focus-monitor" => Command::FocusMonitor(args.monitor()?),
        "mode" => Command::Mode(args.word()?),
        "reload-config" => Command::ReloadConfig,
        "rescue" => Command::Rescue,
        "list-windows" => Command::ListWindows,
        "list-workspaces" => Command::ListWorkspaces {
            focused_only: args.flag("--focused"),
        },
        "list-monitors" => Command::ListMonitors,
        "quit" => Command::Quit,
        _ => {
            return Err(ParseError(match suggest(name) {
                Some(s) => format!("unknown command '{name}'; did you mean '{s}'?"),
                None => format!("unknown command '{name}'"),
            }))
        }
    };
    args.finish()?;
    Ok(command)
}

/// The remaining arguments of the command being parsed.
struct Args<'a> {
    command: &'a str,
    rest: Vec<&'a str>,
}

impl<'a> Args<'a> {
    fn error(&self, msg: impl fmt::Display) -> ParseError {
        ParseError(format!("{}: {msg}", self.command))
    }

    fn word(&mut self) -> Result<String, ParseError> {
        if self.rest.is_empty() {
            return Err(self.error("missing argument"));
        }
        Ok(self.rest.remove(0).to_string())
    }

    /// Removes `flag` from anywhere in the arguments, returning whether it was present.
    fn flag(&mut self, flag: &str) -> bool {
        let before = self.rest.len();
        self.rest.retain(|a| *a != flag);
        self.rest.len() != before
    }

    fn choice<T: Copy>(&mut self, options: &[(&str, T)]) -> Result<T, ParseError> {
        let word = self.word()?;
        options
            .iter()
            .find(|(name, _)| *name == word)
            .map(|(_, value)| *value)
            .ok_or_else(|| {
                let names: Vec<&str> = options.iter().map(|(n, _)| *n).collect();
                self.error(format_args!(
                    "expected one of {}, got '{word}'",
                    names.join("|")
                ))
            })
    }

    fn direction(&mut self) -> Result<Direction, ParseError> {
        let options: Vec<(&str, Direction)> =
            Direction::ALL.iter().map(|d| (d.name(), *d)).collect();
        self.choice(&options)
    }

    fn monitor(&mut self) -> Result<MonitorTarget, ParseError> {
        use Direction::*;
        self.choice(&[
            ("next", MonitorTarget::Next),
            ("prev", MonitorTarget::Prev),
            ("left", MonitorTarget::Dir(Left)),
            ("right", MonitorTarget::Dir(Right)),
            ("up", MonitorTarget::Dir(Up)),
            ("down", MonitorTarget::Dir(Down)),
        ])
    }

    fn layout(&mut self) -> Result<LayoutArg, ParseError> {
        use LayoutCmd::*;
        self.choice(&[
            ("tiles", LayoutArg::Tree(Tiles)),
            ("accordion", LayoutArg::Tree(Accordion)),
            ("horizontal", LayoutArg::Tree(Horizontal)),
            ("vertical", LayoutArg::Tree(Vertical)),
            ("h_tiles", LayoutArg::Tree(HTiles)),
            ("v_tiles", LayoutArg::Tree(VTiles)),
            ("h_accordion", LayoutArg::Tree(HAccordion)),
            ("v_accordion", LayoutArg::Tree(VAccordion)),
            ("floating", LayoutArg::Floating),
            ("tiling", LayoutArg::Tiling),
        ])
    }

    fn amount(&mut self) -> Result<ResizeAmount, ParseError> {
        let word = self.word()?;
        let bad = || {
            self.error(format_args!(
                "expected a number like 50, +50 or -50, got '{word}'"
            ))
        };
        if let Some(n) = word.strip_prefix('+') {
            n.parse().map(ResizeAmount::Relative).map_err(|_| bad())
        } else if word.starts_with('-') {
            word.parse().map(ResizeAmount::Relative).map_err(|_| bad())
        } else {
            word.parse().map(ResizeAmount::Absolute).map_err(|_| bad())
        }
    }

    fn finish(&self) -> Result<(), ParseError> {
        match self.rest.first() {
            Some(extra) => Err(self.error(format_args!("unexpected argument '{extra}'"))),
            None => Ok(()),
        }
    }
}

/// The closest command name within an edit distance of 2, if any.
fn suggest(name: &str) -> Option<&'static str> {
    COMMAND_NAMES
        .iter()
        .map(|c| (edit_distance(name, c), *c))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Splits on whitespace, keeping single- or double-quoted sections together.
pub fn split_words(input: &str) -> Result<Vec<String>, ParseError> {
    let mut words = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else {
            return Ok(words);
        };
        let mut word = String::new();
        if first == '\'' || first == '"' {
            chars.next();
            loop {
                match chars.next() {
                    Some(c) if c == first => break,
                    Some(c) => word.push(c),
                    None => return Err(ParseError("unterminated quote".into())),
                }
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                word.push(c);
            }
        }
        words.push(word);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Direction::*;

    fn ok(s: &str) -> Command {
        parse_command(s).unwrap_or_else(|e| panic!("{s:?}: {e}"))
    }

    fn err(s: &str) -> String {
        parse_command(s).unwrap_err().0
    }

    #[test]
    fn tree_commands() {
        assert_eq!(ok("focus left"), Command::Focus(Left));
        assert_eq!(ok("move down"), Command::Move(Down));
        assert_eq!(ok("join-with up"), Command::JoinWith(Up));
        assert_eq!(
            ok("layout tiles accordion"),
            Command::Layout(vec![
                LayoutArg::Tree(LayoutCmd::Tiles),
                LayoutArg::Tree(LayoutCmd::Accordion)
            ])
        );
        assert_eq!(
            ok("layout floating tiling"),
            Command::Layout(vec![LayoutArg::Floating, LayoutArg::Tiling])
        );
        assert_eq!(
            ok("layout h_accordion v_tiles horizontal vertical"),
            Command::Layout(vec![
                LayoutArg::Tree(LayoutCmd::HAccordion),
                LayoutArg::Tree(LayoutCmd::VTiles),
                LayoutArg::Tree(LayoutCmd::Horizontal),
                LayoutArg::Tree(LayoutCmd::Vertical),
            ])
        );
        assert_eq!(ok("balance-sizes"), Command::BalanceSizes);
        assert_eq!(ok("flatten-workspace-tree"), Command::FlattenWorkspaceTree);
        assert_eq!(ok("close"), Command::Close);
        assert_eq!(ok("fullscreen"), Command::Fullscreen);
    }

    #[test]
    fn resize_amounts() {
        assert_eq!(
            ok("resize smart +50"),
            Command::Resize(ResizeDim::Smart, ResizeAmount::Relative(50))
        );
        assert_eq!(
            ok("resize width -50"),
            Command::Resize(ResizeDim::Width, ResizeAmount::Relative(-50))
        );
        assert_eq!(
            ok("resize height 400"),
            Command::Resize(ResizeDim::Height, ResizeAmount::Absolute(400))
        );
        assert_eq!(
            err("resize width abc"),
            "resize: expected a number like 50, +50 or -50, got 'abc'"
        );
    }

    #[test]
    fn workspace_and_monitor_commands() {
        assert_eq!(ok("workspace 3"), Command::Workspace("3".into()));
        assert_eq!(
            ok("workspace 'my notes'"),
            Command::Workspace("my notes".into())
        );
        assert_eq!(
            ok("workspace-back-and-forth"),
            Command::WorkspaceBackAndForth
        );
        assert_eq!(
            ok("move-node-to-workspace 2"),
            Command::MoveNodeToWorkspace {
                name: "2".into(),
                focus_follows: false
            }
        );
        assert_eq!(
            ok("move-node-to-workspace --focus-follows-window \"web\""),
            Command::MoveNodeToWorkspace {
                name: "web".into(),
                focus_follows: true
            }
        );
        assert_eq!(
            ok("move-workspace-to-monitor next"),
            Command::MoveWorkspaceToMonitor(MonitorTarget::Next)
        );
        assert_eq!(
            ok("focus-monitor left"),
            Command::FocusMonitor(MonitorTarget::Dir(Left))
        );
    }

    #[test]
    fn server_commands() {
        assert_eq!(ok("mode resize"), Command::Mode("resize".into()));
        assert_eq!(ok("reload-config"), Command::ReloadConfig);
        assert_eq!(ok("rescue"), Command::Rescue);
        assert_eq!(ok("list-windows"), Command::ListWindows);
        assert_eq!(
            ok("list-workspaces --focused"),
            Command::ListWorkspaces { focused_only: true }
        );
        assert_eq!(ok("list-monitors"), Command::ListMonitors);
        assert_eq!(ok("quit"), Command::Quit);
    }

    #[test]
    fn parse_args_matches_parse_command() {
        assert_eq!(
            parse_args(&["workspace", "my notes"]).unwrap(),
            Command::Workspace("my notes".into())
        );
    }

    #[test]
    fn helpful_errors() {
        assert_eq!(
            err("focs left"),
            "unknown command 'focs'; did you mean 'focus'?"
        );
        assert_eq!(err("frobnicate"), "unknown command 'frobnicate'");
        assert_eq!(
            err("focus lft"),
            "focus: expected one of left|right|up|down, got 'lft'"
        );
        assert_eq!(err("focus"), "focus: missing argument");
        assert_eq!(err("close now"), "close: unexpected argument 'now'");
        assert_eq!(err(""), "empty command");
        assert_eq!(err("workspace 'oops"), "unterminated quote");
    }

    #[test]
    fn split_words_handles_quotes() {
        assert_eq!(
            split_words(r#"  a 'b c'  "d e" f"#).unwrap(),
            vec!["a", "b c", "d e", "f"]
        );
    }
}
