//! SDK-agnostic reconnectable process transport.
#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
#[cfg(not(unix))]
mod unsupported;
#[cfg(not(unix))]
pub use unsupported::*;

#[cfg(unix)]
pub mod cold;
