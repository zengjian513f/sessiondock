//! Pure provider domains and an explicitly opened development ledger store.
//! No HTTP, host access, timers, home discovery, or CLI launch.
//!
//! Effects describe work for a future trusted application adapter. In
//! particular, `Persisted` means a durable commit, not a successful write call.

pub mod claude;
pub mod claude_adapter;
pub mod codex;
pub mod codex_adapter;
pub mod driver;
pub mod engine;
pub mod executor;
pub mod service;
pub mod store;
