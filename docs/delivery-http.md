# Read-only delivery HTTP contract

`prepare_app(Config, CancellationToken).await` validates configuration, opens a
configured existing delivery ledger with fresh-epoch recovery, and builds the
Web app off the reactor. It returns `PreparedApp { router, delivery }`; callers
must stop HTTP admission, cancel the token, and await `delivery.shutdown()`.
The binary does so, including bind failure. If asset loading fails after ledger
recovery, preparation waits for the writer lock to be released before returning
the error. Configuration absence keeps the service disabled; missing data is
never treated as permission to initialize it.

`GET /api/session/outbox?uid=...&agent=...` is a committed display projection, not
a send or acknowledgment operation. UID and agent are bounded; unknown fields,
duplicate query keys, malformed queries and unknown native views are errors.
Native scope comes from one validated inventory view, with provenance described
in [delivery-scope.md](delivery-scope.md). Claude child views use the real parent
session ID, not their display `sid`. Unknown/short/foreign agents and missing or
conflicting native IDs never become a successful empty queue. Codex child delivery
and Grok delivery remain unsupported. An existing supported scope with no visible
receipts legitimately returns an empty outbox with its actual epoch/revision.

The response retains `{outbox, outbox_version}` and uses the Engine's conservative
state mapping. Unsupported media returns an explicit error; corruption/freeze
returns 503 without substituting an empty queue or exposing stored prompt data
in an error. API responses are same-origin/loopback, JSON and `Cache-Control:
no-store`. Neither host credentials nor arbitrary native metadata is serialized.

`outbox_read` is enabled only after the configured service opens. `outbox` remains
false: legacy send/retry/discard/background-reconciliation controls are not
enabled by read access. No HTTP command, internal persistence callback, executor,
strong native acknowledgment, receipt/log diagnostic route or queue migration
endpoint is added. Existing unimplemented mutations still return 501.

The service admits eight requests/responses by default. The HTTP Body holds its
`ResponseGuard` while unpolled, while delivering 32 KiB chunks, and until EOF or
Drop; the ninth retained response returns 503. Serialization has a 16 MiB default
budget inside the single blocking worker; oversized responses fail before HTTP
success, rather than returning a truncated queue. Budgeting covers application
response storage, not every transport/OS buffer. Client cancellation does not
abort an already started blocking read or release its ownership early. Shutdown
waits for that work and releases the ledger lock even if a caller still retains
an already computed response Body.

`cargo test -p sessiondock --test delivery_http --locked` validates synthetic
Claude main/child and Codex receipts, identity failures, default disablement,
startup failures, response admission, errors, unchanged native bytes and unchanged
ledger bytes after open, epoch recovery, and lock cleanup. No test invokes a
model CLI or writes production data. This is not reliable-send acceptance or
Windows/macOS runtime validation.
