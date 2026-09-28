//! Server-side terminal writes for conversation SEND and the bug-report
//! worker: the host terminal driver (composer recognition, leases, paste and
//! Enter) and managed-instance target resolution. No durable state.

pub mod driver;
pub mod target;
