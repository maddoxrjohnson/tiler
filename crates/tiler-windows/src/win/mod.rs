//! The Win32 calls behind tiler. Everything here is a thin wrapper; decisions are
//! made in the platform-independent modules and in `tiler-core`.

pub mod apply;
pub mod display;
pub mod hook;
pub mod identity;
pub mod keyboard;
pub mod monitors;
pub mod rescue;
pub mod startup;
pub mod window;

pub use window::Hwnd;
