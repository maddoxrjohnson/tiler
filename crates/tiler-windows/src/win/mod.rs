//! The Win32 calls behind tiler. Everything here is a thin wrapper; decisions are
//! made in the platform-independent modules and in `tiler-core`.

pub mod apply;
pub mod hook;
pub mod monitors;
pub mod window;

pub use window::Hwnd;
