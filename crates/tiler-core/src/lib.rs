//! Platform-independent core of the tiler window manager.
//!
//! Everything in this crate is pure logic: it receives [`Event`]s describing what
//! happened on the desktop and returns [`Action`]s describing what the platform
//! layer should do. It never talks to the operating system, so it can be tested
//! on any platform.

pub mod geometry;
pub mod tree;
pub mod tree_ops;

pub use geometry::{Direction, MonitorId, Orientation, Rect, Sides, WindowId};
pub use tree::{Container, InnerGaps, Layout, Node, Path};
pub use tree_ops::{LayoutCmd, ResizeAmount, ResizeDim};
