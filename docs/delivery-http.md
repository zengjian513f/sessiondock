# Delivery HTTP contract

`prepare_app(Config, CancellationToken).await` opens configured services and
builds the router. Startup and shutdown wait for delivery work to finish.
Configuration determines whether the delivery and terminal services are enabled.

`GET /api/session/outbox?uid=...&agent=...` returns
`{outbox, outbox_version}` for the selected native view. Unknown query fields are
ignored. A valid session with no visible receipts returns an empty outbox with
its current version; missing sessions and actual service errors remain errors.

The reliable-send routes (`POST /api/session/send`, `draft-status`,
`outbox/retry`, `outbox/discard`) are retired together with their executor;
SEND is `POST /api/session/conversation/send` ([conversation.md](conversation.md)).
Ordinary request admission queues behind the service worker. Growing outboxes
are returned without a separate response-size quota.

Responses are JSON with `Cache-Control: no-store`. Authentication, same-origin
checks and native scope remain in their respective transport layers. Storage
errors are reported for the affected operation and never fabricated as successful
CLI acceptance.

Synthetic HTTP and browser suites cover sends, retry identity, attachments,
parent/child scopes, confirmation, outbox projection, cancellation and shutdown.
The release record carries the consolidated validation and deployment result.
