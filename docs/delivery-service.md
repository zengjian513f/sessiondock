# Isolated asynchronous delivery reads

`delivery::service::DeliveryService` adds bounded asynchronous access to
[DeliveryEngine](delivery-engine.md). It exposes only committed outbox
projections and bounded receipt/log diagnostics. The Web adapter now uses this
library for [the read-only outbox endpoint](delivery-http.md). There is still no
sender, executor, command submission, CLI invocation, native acknowledgment
adapter, automatic initialization, or simulated queue acceptance. This library
does not complete migration M5.

## API and startup

`DeliveryService::open(explicit_path, limits, shutdown_token).await` opens an
**existing** ledger. Missing or corrupt data fails closed. Opening calls the
engine's two-provider fresh-epoch recovery, including its durable recovery
commits, before returning a service; therefore opening is deliberately not a
disk-read-only operation. Subsequent successful queries do not change the ledger.
Failed opens release their handle; interrupted recovery is handled only by the
next explicit engine open. Windows opens the same store (WP-W, [delivery-store.md](delivery-store.md)).

The caller must configure a dedicated directory with the permissions and
nonoverlap restrictions described by [the store](delivery-store.md). This service
does not create directories or discover other application roots. It does not
resolve native ownership: the future transport must validate the exact native
scope before calling it. Codex UID/optional agent and Claude's complete
`{uid, session_id, agent_id}` scope are passed directly to Engine; no scope is
guessed from a terminal display name.

The public reads are:

- `codex_outbox(uid, agent_id)` and `claude_outbox(scope)`: the existing complete
  `{outbox, outbox_version}` wire projection, including its tombstone visibility,
  unsupported-media and conservative state rules.
- `receipts(provider, offset, limit)`: the existing committed receipt metadata
  array, with Engine's maximum of 128 entries.
- `logs(after_sequence, limit)`: the existing bounded log array. This remains
  usable after an engine freeze; it includes no prompt or native proof.

All return `Result<EncodedJson, service::Error>`. `EncodedJson` is a non-Clone,
non-Debug holder of already encoded bytes. Errors are typed, with no raw path,
prompt, or adapter reason. A future API can distinguish Busy/Closed, a response
budget failure, engine errors, and worker failure without inventing a second
delivery schema. No capabilities are enabled by this library.

## Work and response ownership

One detached coordinator owns one engine and a bounded Tokio request channel.
It starts at most **one** blocking read at a time and awaits it before starting
another. Every engine open, read/fingerprint check, large JSON serialization,
and normal lock-release cleanup runs in `spawn_blocking`; none runs on the Tokio
reactor. At most eight blocking opens are admitted process-wide, including opens
whose awaiting caller has disappeared.

`Limits::capacity` defaults to 8, with an allowed range of 1–16. A semaphore
counts **queued requests + active work + successful responses still held by
callers**. It is acquired before enqueueing; saturation returns Busy immediately.
Admission is retained during blocking work even when the waiting request future
or response receiver is dropped. A successful response takes ownership of its
permit, including while buffered in an unread oneshot. Failed/discarded responses
release their permit without retaining a large JSON result.

`EncodedJson::as_bytes()` borrows bytes while retaining admission. For a future
HTTP body, `into_parts()` transfers `(Vec<u8>, ResponseGuard)`. The body must own
the guard until completion or Drop; releasing the guard while constructing a
body would defeat the aggregate response bound. There is intentionally no
unguarded `into_bytes()` method. Callers that explicitly copy bytes outside the
holder are responsible for budgeting those copies.

`max_json_bytes` defaults to 16 MiB and is capped at 32 MiB. Serde writes directly
into a bounded writer that checks the next write before appending and caps its
requested buffer growth. It does not first build an unbounded JSON Value or
serialized Vec and inspect its size afterward. Oversized responses return
ResponseLimit; no partial body, truncated outbox, or silently removed row is
returned. Escaping counts toward the encoded-byte limit. The aggregate retained
response-byte ceiling is capacity × max_json_bytes. Engine/store snapshots and
temporary typed projections have their existing separate memory bounds; this is
not a claim that total process memory equals the response-byte ceiling.

## Cancellation and shutdown

An admitted request belongs to the coordinator. Dropping a future HTTP request
would only discard its response receiver; it cannot abort a started blocking
read, drop the engine early, or return its permit while work is still running.
No request-handler task owns the engine or a DispatchBatch.

`shutdown().await` closes admission, rejects queued reads that have not started,
waits for any active blocking read, releases the engine/OS lock in a blocking
cleanup, and then reports completion. Repeated shutdown calls are allowed. A
successful response may outlive shutdown, retaining its response permit until
the holder/body is dropped; it does not retain the engine or OS lock. Cancelling
the supplied parent token also stops admission. Explicit service shutdown uses a
child token and never cancels the caller's parent token.

Share the service using Arc. The coordinator owns neither the public handle nor
its request sender, so there is no Arc/channel cycle. Last-handle Drop cancels
the child token and closes admission; the independent coordinator finishes its
active worker before releasing the lock. Drop cannot wait, so applications that
need an orderly process shutdown must await `shutdown()` while their Tokio
runtime is still alive. The library does not abort underlying blocking workers.

## Validation

Synthetic Unix tests exercise the unchanged legacy JSON shape, exact scopes,
tombstone retention, bounded diagnostics, no writes after open, both recovery
epochs, corrupt/missing open handling, escaped-output byte budgets, full queues,
slow blocking reads on a single-thread Tokio runtime, discarded response
receivers, response/body admission ownership, shutdown with queued/active work,
last-handle Drop without a lock leak, external cancellation, and engine freeze
after external disk edits with logs still available. Test-only barriers park a
blocking worker and prove that capacity and the OS lock remain held until it
finishes. Fixtures use private temporary directories and never invoke native CLI
processes or contact production services. HTTP/body integration is future work;
these tests do not claim browser validation.
