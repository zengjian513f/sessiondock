# Asynchronous delivery service

`DeliveryService` serializes access to [DeliveryEngine](delivery-engine.md).
It supplies outbox reads, receipt diagnostics and executor access through
`with_engine`. HTTP handlers select the native session before submitting work.

## API and startup

`DeliveryService::open(path, limits, shutdown_token).await` opens the ledger and
restores both provider machines, initializing a missing ledger. Startup completes before requests can use the
service. The selected path follows [the store](delivery-store.md).

`codex_outbox`, `claude_outbox`, `receipts` and `logs` return `EncodedJson`.
Outbox responses preserve the complete visible rows and opaque media metadata.
Diagnostic responses contain receipt/log metadata instead of prompt text.

## Work and response ownership

One coordinator owns the engine and runs blocking operations off the Tokio
reactor. The internal channel queues requests until the coordinator can handle
them. Retained responses do not consume request-admission permits.

`EncodedJson::as_bytes()` borrows the result; `into_parts()` transfers the byte
vector and response guard to the HTTP body. Serialization does not impose an
outbox byte quota or truncate the response to fit a capacity policy.

## Cancellation and shutdown

Dropping a caller discards its response receiver. A started blocking operation
finishes under the coordinator. `shutdown().await` closes admission, waits for
active work and releases the engine. Repeated shutdown calls are allowed.

Last-handle Drop cancels the service's child token. The coordinator can finish
independently; orderly process shutdown awaits it before closing Tokio. The
parent cancellation token is not changed by explicit service shutdown.

## Validation

Default check is the delivery HTTP/browser suites that use this service
(`cargo test -p sessiondock --test delivery_send --locked`,
`python3 tests/send_browser.py` when the change is user-visible). Do not run
crate unit tests unless the user asks.
