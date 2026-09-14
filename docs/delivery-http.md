# Delivery HTTP contract

`prepare_app(Config, CancellationToken).await` opens configured services and
builds the router. Startup and shutdown wait for delivery work to finish.
Configuration determines whether the delivery and terminal services are enabled.

`GET /api/session/outbox?uid=...&agent=...` returns
`{outbox, outbox_version}` for the selected native view. Unknown query fields are
ignored. A valid session with no visible receipts returns an empty outbox with
its current version; missing sessions and actual service errors remain errors.

`POST /api/session/send` submits prompt text and opaque media metadata under a
request ID. The executor uses the session's terminal association and persists
its receipt, writes input, then checks the native queue or transcript using the
provider's confirmation rules. Retrying an existing ID returns its receipt;
changing that ID's payload is a conflict. Uploaded attachment paths are already
part of the text.

Discard, retry and other supported mutations are described in
[delivery-executor.md](delivery-executor.md) and
[delivery-codex-executor.md](delivery-codex-executor.md). The terminal protocol
retains its actual input and framing limits. Ordinary request admission queues
behind the service worker. Growing outboxes are returned without a separate
response-size quota.

Responses are JSON with `Cache-Control: no-store`. Authentication, same-origin
checks and native scope remain in their respective transport layers. Storage
errors are reported for the affected operation and never fabricated as successful
CLI acceptance.

Synthetic HTTP and browser suites cover sends, retry identity, attachments,
parent/child scopes, confirmation, outbox projection, cancellation and shutdown.
The release record carries the consolidated validation and deployment result.
