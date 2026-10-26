//! The TOML configuration file.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use regex::Regex;
use serde::Deserialize;

use crate::command::{parse_command, Command, LayoutArg};
use crate::event::WindowInfo;
use crate::geometry::{Orientation, Sides};
use crate::keys::KeyChord;
use crate::tree::{InnerGaps, Layout};

/// The configuration used when the user has no config file.
pub const DEFAULT_CONFIG: &str = include_str!("default-config.toml");

#[derive(Debug, Clone)]
pub struct Config {
    pub hide_strategy: HideStrategyKind,
    pub accordion_padding: i32,
    pub default_root_layout: Layout,
    /// `None` means "auto": horizontal on landscape monitors, vertical on portrait.
    pub default_root_orientation: Option<Orientation>,
    pub inner_gaps: InnerGaps,
    pub outer_gaps: Sides,
    pub modes: BTreeMap<String, Mode>,
    pub rules: Vec<WindowRule>,
    pub workspace_monitor: BTreeMap<String, MonitorPattern>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideStrategyKind {
    Offscreen,
}

#[derive(Debug, Clone, Default)]
pub struct Mode {
    pub bindings: HashMap<KeyChord, Vec<Command>>,
}

/// Which monitor a workspace is pinned to.
#[derive(Debug, Clone)]
pub enum MonitorPattern {
    /// 1-based monitor index, left to right.
    Index(usize),
    /// The primary monitor (`"main"`).
    Main,
    /// The first non-primary monitor (`"secondary"`).
    Secondary,
    /// A regex matched against the monitor name.
    Name(Regex),
}

#[derive(Debug, Clone)]
pub struct WindowRule {
    pub process: Option<String>,
    pub class: Option<String>,
    pub title_regex: Option<Regex>,
    pub app_name_regex: Option<Regex>,
    pub actions: Vec<RuleAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleAction {
    Float,
    Tile,
    MoveToWorkspace(String),
    /// Leave the window completely unmanaged.
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

impl Default for Config {
    fn default() -> Self {
        Config::from_toml(DEFAULT_CONFIG).expect("the built-in default config is valid")
    }
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Config, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError(e.to_string()))?;

        let hide_strategy = choice(
            "hide-strategy",
            raw.hide_strategy.as_deref().unwrap_or("offscreen"),
            &[("offscreen", HideStrategyKind::Offscreen)],
        )?;
        let default_root_layout = choice(
            "default-root-container-layout",
            raw.default_root_container_layout
                .as_deref()
                .unwrap_or("tiles"),
            &[("tiles", Layout::Tiles), ("accordion", Layout::Accordion)],
        )?;
        let default_root_orientation = choice(
            "default-root-container-orientation",
            raw.default_root_container_orientation
                .as_deref()
                .unwrap_or("auto"),
            &[
                ("auto", None),
                ("horizontal", Some(Orientation::H)),
                ("vertical", Some(Orientation::V)),
            ],
        )?;

        let gaps = raw.gaps.unwrap_or_default();
        let inner = gaps.inner.unwrap_or_default();
        let inner_gaps = InnerGaps {
            horizontal: inner.horizontal,
            vertical: inner.vertical,
        };
        let outer_gaps = match gaps.outer {
            None => Sides::default(),
            Some(RawOuter::All(v)) => Sides::all(v),
            Some(RawOuter::Sides(s)) => Sides {
                left: s.left,
                top: s.top,
                right: s.right,
                bottom: s.bottom,
            },
        };

        if !raw.mode.contains_key("main") {
            return Err(ConfigError("config must define [mode.main.binding]".into()));
        }
        let mut modes = BTreeMap::new();
        for (mode_name, raw_mode) in &raw.mode {
            let mut bindings = HashMap::new();
            for (key, commands) in &raw_mode.binding {
                let at = |msg: &dyn fmt::Display| {
                    ConfigError(format!("mode.{mode_name}.binding.{key}: {msg}"))
                };
                let chord: KeyChord = key.parse().map_err(|e: String| at(&e))?;
                let mut parsed = Vec::new();
                for text in commands.as_slice() {
                    let command = parse_command(text).map_err(|e| at(&e))?;
                    if let Command::Mode(target) = &command {
                        if !raw.mode.contains_key(target) {
                            return Err(at(&format_args!("unknown mode '{target}'")));
                        }
                    }
                    parsed.push(command);
                }
                bindings.insert(chord, parsed);
            }
            modes.insert(mode_name.clone(), Mode { bindings });
        }

        let rules = raw
            .on_window_detected
            .iter()
            .enumerate()
            .map(|(i, r)| WindowRule::from_raw(i, r))
            .collect::<Result<_, _>>()?;

        let workspace_monitor = raw
            .workspace_to_monitor_force_assignment
            .into_iter()
            .map(|(workspace, pattern)| {
                let pattern = match pattern {
                    RawMonitorPattern::Index(i) => MonitorPattern::Index(i),
                    RawMonitorPattern::Name(n) if n == "main" => MonitorPattern::Main,
                    RawMonitorPattern::Name(n) if n == "secondary" => MonitorPattern::Secondary,
                    RawMonitorPattern::Name(n) => {
                        MonitorPattern::Name(Regex::new(&n).map_err(|e| {
                            ConfigError(format!(
                                "workspace-to-monitor-force-assignment.{workspace}: {e}"
                            ))
                        })?)
                    }
                };
                Ok((workspace, pattern))
            })
            .collect::<Result<_, ConfigError>>()?;

        Ok(Config {
            hide_strategy,
            accordion_padding: raw.accordion_padding.unwrap_or(30),
            default_root_layout,
            default_root_orientation,
            inner_gaps,
            outer_gaps,
            modes,
            rules,
            workspace_monitor,
        })
    }
}

impl WindowRule {
    /// True when every condition the rule specifies matches the window.
    pub fn matches(&self, w: &WindowInfo) -> bool {
        let eq = |want: &Option<String>, have: &str| {
            want.as_ref().map_or(true, |s| s.eq_ignore_ascii_case(have))
        };
        let re =
            |want: &Option<Regex>, have: &str| want.as_ref().map_or(true, |r| r.is_match(have));
        eq(&self.process, &w.process)
            && eq(&self.class, &w.class)
            && re(&self.title_regex, &w.title)
            && re(&self.app_name_regex, &w.app_name)
    }

    fn from_raw(index: usize, raw: &RawRule) -> Result<WindowRule, ConfigError> {
        let at = |field: &str, msg: &dyn fmt::Display| {
            ConfigError(format!("on-window-detected[{index}].{field}: {msg}"))
        };
        let regex = |field: &str, pattern: &Option<String>| {
            pattern
                .as_deref()
                .map(|p| Regex::new(p).map_err(|e| at(field, &e)))
                .transpose()
        };
        let actions = raw
            .run
            .as_slice()
            .iter()
            .map(|text| {
                if text == "ignore" {
                    return Ok(RuleAction::Ignore);
                }
                let not_allowed = || {
                    at(
                        "run",
                        &format_args!(
                            "'{text}' is not allowed in window rules (use layout floating|tiling, move-node-to-workspace or ignore)"
                        ),
                    )
                };
                match parse_command(text).map_err(|e| at("run", &e))? {
                    Command::Layout(args) if args == [LayoutArg::Floating] => Ok(RuleAction::Float),
                    Command::Layout(args) if args == [LayoutArg::Tiling] => Ok(RuleAction::Tile),
                    Command::MoveNodeToWorkspace { name, .. } => Ok(RuleAction::MoveToWorkspace(name)),
                    _ => Err(not_allowed()),
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(WindowRule {
            process: raw.condition.process.clone(),
            class: raw.condition.class.clone(),
            title_regex: regex("if.title-regex", &raw.condition.title_regex)?,
            app_name_regex: regex("if.app-name-regex", &raw.condition.app_name_regex)?,
            actions,
        })
    }
}

fn choice<T: Copy>(field: &str, value: &str, options: &[(&str, T)]) -> Result<T, ConfigError> {
    options
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, v)| *v)
        .ok_or_else(|| {
            let names: Vec<&str> = options.iter().map(|(n, _)| *n).collect();
            ConfigError(format!(
                "{field}: expected one of {}, got '{value}'",
                names.join("|")
            ))
        })
}

// Raw serde mirror of the file format; converted and validated by `Config::from_toml`.

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawConfig {
    hide_strategy: Option<String>,
    accordion_padding: Option<i32>,
    default_root_container_layout: Option<String>,
    default_root_container_orientation: Option<String>,
    gaps: Option<RawGaps>,
    #[serde(default)]
    mode: BTreeMap<String, RawMode>,
    #[serde(default)]
    on_window_detected: Vec<RawRule>,
    #[serde(default)]
    workspace_to_monitor_force_assignment: BTreeMap<String, RawMonitorPattern>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawGaps {
    inner: Option<RawInner>,
    outer: Option<RawOuter>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawInner {
    #[serde(default)]
    horizontal: i32,
    #[serde(default)]
    vertical: i32,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawOuter {
    All(i32),
    Sides(RawSides),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSides {
    #[serde(default)]
    left: i32,
    #[serde(default)]
    top: i32,
    #[serde(default)]
    right: i32,
    #[serde(default)]
    bottom: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMode {
    #[serde(default)]
    binding: BTreeMap<String, OneOrMany>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn as_slice(&self) -> &[String] {
        match self {
            OneOrMany::One(s) => std::slice::from_ref(s),
            OneOrMany::Many(v) => v,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    #[serde(rename = "if", default)]
    condition: RawCondition,
    run: OneOrMany,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawCondition {
    process: Option<String>,
    class: Option<String>,
    title_regex: Option<String>,
    app_name_regex: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawMonitorPattern {
    Index(usize),
    Name(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> KeyChord {
        s.parse().unwrap()
    }

    fn err(text: &str) -> String {
        Config::from_toml(text).unwrap_err().0
    }

    const MINIMAL: &str = "[mode.main.binding]\nalt-1 = 'workspace 1'\n";

    #[test]
    fn default_config_is_valid() {
        let c = Config::default();
        assert_eq!(c.hide_strategy, HideStrategyKind::Offscreen);
        assert_eq!(
            c.inner_gaps,
            InnerGaps {
                horizontal: 8,
                vertical: 8
            }
        );
        assert_eq!(c.outer_gaps, Sides::all(8));
        let main = &c.modes["main"];
        assert_eq!(
            main.bindings[&chord("alt-1")],
            vec![Command::Workspace("1".into())]
        );
        assert!(!main.bindings.contains_key(&chord("alt-tab")));
        assert_eq!(c.modes["service"].bindings[&chord("esc")].len(), 2);
        assert!(c.default_root_orientation.is_none());
    }

    #[test]
    fn minimal_config_uses_defaults() {
        let c = Config::from_toml(MINIMAL).unwrap();
        assert_eq!(c.accordion_padding, 30);
        assert_eq!(c.inner_gaps, InnerGaps::default());
        assert_eq!(c.outer_gaps, Sides::default());
        assert_eq!(c.default_root_layout, Layout::Tiles);
        assert!(c.rules.is_empty());
    }

    #[test]
    fn outer_gap_can_be_a_single_number() {
        let c = Config::from_toml(&format!("[gaps]\nouter = 5\n{MINIMAL}")).unwrap();
        assert_eq!(c.outer_gaps, Sides::all(5));
    }

    #[test]
    fn main_mode_is_required() {
        assert_eq!(
            err("[mode.resize.binding]\nesc = 'mode main'\n"),
            "config must define [mode.main.binding]"
        );
    }

    #[test]
    fn binding_errors_name_the_key() {
        assert_eq!(
            err("[mode.main.binding]\nalt-hh = 'focus left'\n"),
            "mode.main.binding.alt-hh: unknown key 'hh'"
        );
        assert_eq!(
            err("[mode.main.binding]\nalt-h = 'focus lft'\n"),
            "mode.main.binding.alt-h: focus: expected one of left|right|up|down, got 'lft'"
        );
        assert_eq!(
            err("[mode.main.binding]\nalt-r = 'mode resiz'\n"),
            "mode.main.binding.alt-r: unknown mode 'resiz'"
        );
    }

    #[test]
    fn toml_errors_include_location() {
        let e = err("[mode.main.binding\n");
        assert!(e.contains("line 1"), "{e}");
        let e = err(&format!("unknown-key = 1\n{MINIMAL}"));
        assert!(e.contains("unknown field `unknown-key`"), "{e}");
    }

    #[test]
    fn enum_values_are_validated() {
        assert_eq!(
            err(&format!("hide-strategy = 'cloak'\n{MINIMAL}")),
            "hide-strategy: expected one of offscreen, got 'cloak'"
        );
        assert_eq!(
            err(&format!("default-root-container-orientation = 'diagonal'\n{MINIMAL}")),
            "default-root-container-orientation: expected one of auto|horizontal|vertical, got 'diagonal'"
        );
    }

    #[test]
    fn rules_parse_and_match() {
        let c = Config::from_toml(&format!(
            r#"{MINIMAL}
[[on-window-detected]]
if.process = 'Slack.exe'
run = 'move-node-to-workspace 9'

[[on-window-detected]]
if.app-name-regex = '^(Calculator|Settings)$'
if.title-regex = 'Calc'
run = ['layout floating']

[[on-window-detected]]
if.class = 'ConsoleWindowClass'
run = 'ignore'
"#
        ))
        .unwrap();
        assert_eq!(c.rules.len(), 3);
        let slack = WindowInfo {
            process: "slack.exe".into(),
            ..Default::default()
        };
        assert!(c.rules[0].matches(&slack));
        assert_eq!(
            c.rules[0].actions,
            vec![RuleAction::MoveToWorkspace("9".into())]
        );
        let calc = WindowInfo {
            app_name: "Calculator".into(),
            title: "Calculator".into(),
            ..Default::default()
        };
        assert!(c.rules[1].matches(&calc));
        assert!(!c.rules[1].matches(&WindowInfo {
            title: "Other".into(),
            ..calc.clone()
        }));
        assert_eq!(c.rules[1].actions, vec![RuleAction::Float]);
        assert_eq!(c.rules[2].actions, vec![RuleAction::Ignore]);
        assert!(c.rules[2].matches(&WindowInfo {
            class: "consolewindowclass".into(),
            ..Default::default()
        }));
    }

    #[test]
    fn rule_errors() {
        assert_eq!(
            err(&format!("{MINIMAL}[[on-window-detected]]\nrun = 'focus left'\n")),
            "on-window-detected[0].run: 'focus left' is not allowed in window rules (use layout floating|tiling, move-node-to-workspace or ignore)"
        );
        let e = err(&format!(
            "{MINIMAL}[[on-window-detected]]\nif.title-regex = '('\nrun = 'ignore'\n"
        ));
        assert!(
            e.starts_with("on-window-detected[0].if.title-regex: "),
            "{e}"
        );
    }

    #[test]
    fn workspace_monitor_assignment() {
        let c = Config::from_toml(&format!(
            "{MINIMAL}[workspace-to-monitor-force-assignment]\n1 = 1\n2 = 'main'\n3 = 'secondary'\n4 = 'DELL.*'\n"
        ))
        .unwrap();
        assert!(matches!(c.workspace_monitor["1"], MonitorPattern::Index(1)));
        assert!(matches!(c.workspace_monitor["2"], MonitorPattern::Main));
        assert!(matches!(
            c.workspace_monitor["3"],
            MonitorPattern::Secondary
        ));
        assert!(
            matches!(&c.workspace_monitor["4"], MonitorPattern::Name(r) if r.is_match("DELL U2720Q"))
        );
    }
}
