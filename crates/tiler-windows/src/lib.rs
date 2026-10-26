//! Win32 platform layer for tiler.
//!
//! The decision logic (which windows to manage, frame-border math, where to park
//! hidden windows) is plain Rust and is tested on every OS. The code that actually
//! calls Win32 lives in [`win`] and only builds on Windows.

pub mod facts;
pub mod frame;
pub mod offscreen;

#[cfg(windows)]
pub mod win;

pub use facts::{float_hint, manageable, WindowFacts};
pub use frame::{outer_rect, Borders};
pub use offscreen::offscreen_origin;
