//! Hub federation (batches 38–40): node identity, the hub's node registry
//! with its health monitor, the hub→node HTTP client, the wire namespace,
//! the aggregation of the five read routes plus the per-machine writes, and
//! the single-machine proxy.
//! Python `sessiondock/hub.py` and `sessiondock/federation.py` are the oracle.
//!
//! Nothing here binds a listener or touches session roots. The node side
//! (second listener, `node_auth`) and the hub binary (`api::hub`)
//! wire these pieces: `Registry::open(path, networks, cache_dir)
//! .with_public_payload(namespace::public_payload)` +
//! `Monitor::spawn(registry, client, shutdown)`, then `aggregate::*` for the
//! merged routes and `proxy::*` for everything addressed to one machine.

pub mod aggregate;
pub mod client;
pub mod identity;
pub mod namespace;
pub mod proxy;
pub mod registry;

pub use client::{Client, ClientError, JSON_LIMIT, Target};
pub use identity::{NodeToken, PROTOCOL};
pub use registry::{Monitor, Network, Node, Registry, RegistryError};

/// Largest browser request body the hub reads before proxying (`hub.py`
/// `BODY_LIMIT`); attachment uploads stream separately.
pub const BODY_LIMIT: usize = 4 * 1024 * 1024;
