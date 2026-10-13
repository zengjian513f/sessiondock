# Conversation and terminal boundary

The conversation service owns drafts, SEND receipts, native echo reconciliation
and input-readiness state. The independent ptyhost process owns terminal lifetime;
its client library does not authorize browser control or infer CLI acceptance.

Dropping a browser connection or stopping the web service does not terminate a
terminal host. Use an explicit private host directory and the exact session and
instance identity when observing or controlling a host.

A successful PTY write does not prove that the CLI has processed the text.
Requests interrupted after a write can have an unknown outcome: never blindly
retry terminal input after a transport error. Keep submission identity and
query/reconcile the existing receipt instead. Native history remains the source
of user-visible CLI messages; queued input is separate state until reconciled.

The source implementation lives in `crates/sessiondock/src/conversation/` and the
local protocol client in `crates/ptyhost-client/`. The included lifecycle,
conversation, terminal and SEND browser suites exercise these boundaries using
private hosts and fake CLIs; see [validation](validation.md).
