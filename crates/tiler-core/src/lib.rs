//! Platform-independent core of the tiler window manager.
//!
//! Everything in this crate is pure logic: it receives [`Event`]s describing what
//! happened on the desktop and returns [`Action`]s describing what the platform
//! layer should do. It never talks to the operating system, so it can be tested
//! on any platform.
