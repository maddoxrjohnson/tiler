//! Platform-independent core of the tiler window manager.
//!
//! Everything in this crate is pure logic: it receives [`Event`]s describing what
//! happened on the desktop and returns [`Action`]s describing what the platform
//! layer should do. It never talks to the operating system, so it can be tested
//! on any platform.

pub mod command;
pub mod config;
pub mod event;
pub mod geometry;
pub mod keys;
pub mod state;
pub mod tree;
pub mod tree_ops;
mod workspace;

pub use command::{parse_args, parse_command, Command, LayoutArg, MonitorTarget, ParseError};
pub use config::{Config, ConfigError, Mode, MonitorPattern, RuleAction, WindowRule};
pub use event::{Action, Event, Monitor, WindowInfo};
pub use geometry::{Direction, MonitorId, Orientation, Rect, Sides, WindowId};
pub use keys::{Key, KeyChord, Mods};
pub use state::State;
pub use tree::{Container, InnerGaps, Layout, Node, Path};
pub use tree_ops::{LayoutCmd, ResizeAmount, ResizeDim};
