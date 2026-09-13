//! Pure browser ownership plus explicitly configured local PTY transport.
//! No process creation, CLI invocation, native UID association, or host kill.
//!
//! Native and launch leases are distinct authority types. Pending launches can
//! only use the launch claim/prepare path, never raw name control. Raw peers
//! must have no launch declaration; malformed metadata (including null) fails
//! closed, without manufacturing any native association. Retirement blocks an
//! exact launch tuple for this service lifetime in a bounded, non-evicting set.
//! Lifecycle recovery must separately preserve cancellation intent on restart.
//! Launch retirement closes a bridge with 4002 / "launch retired", without a
//! takeover notification; it is not an assertion that the process has exited.
//!
//! `input` adds raw HTTP text/key writes under the same lease and per-name
//! gate as the WebSocket path, bounded to 16 KiB and 16 requests per second per
//! exact instance. A success is the host's write acknowledgement only; it never
//! claims the CLI consumed the bytes and enables no send ledger or composer.

pub mod input;
pub mod ownership;
mod service;

pub use ownership::ExpectedTarget;
pub use service::{
    BridgeLimits, InputPayload, InputReceipt, MAX_PASTE_BYTES, PreparedAttachment, TerminalError,
    TerminalService, terminal_size,
};
