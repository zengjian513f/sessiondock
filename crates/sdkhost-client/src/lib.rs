//! SDK-agnostic reconnectable process transport.
#[cfg(any(unix, windows))]
mod client;
#[cfg(any(unix, windows))]
pub use client::*;
#[cfg(unix)]
#[path = "unix.rs"]
pub mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
pub mod platform;
#[cfg(unix)]
pub use platform::socket_path;
#[cfg(any(unix, windows))]
pub use platform::{os, private_dir};
#[cfg(not(any(unix, windows)))]
mod unsupported;
#[cfg(not(any(unix, windows)))]
pub use unsupported::*;
#[cfg(any(unix, windows))]
pub mod cold;
