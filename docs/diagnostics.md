# Bounded browser diagnostics intake (batch 23, M7)

`POST /api/audit/browser` accepts the legacy page's browser-side receipts only
when an explicit private `SESSIONDOCK_AUDIT_DIR` is configured. Unset keeps the
`audit` capability `false` and the route `501 not_implemented`; an empty value
fails startup. Diagnostics can never change request-handling semantics: every
budget is checked before a body is copied, producers never wait for disk, and
saturation is a counted drop, not an error the page would retry forever.

## Directory

The directory must already exist, be absolute without `..`, have no symlink or
reparse point among its ancestors, be mode `0700` on Unix, and be disjoint in
both directions from the web dir, native roots, ptyhost, state, delivery,
lifecycle, launcher config, Codex index and every file root. Files are created
`0600`. The service performs no other filesystem access.

## Wire contract (compatible with the Python endpoint)

Request body: `{page_id|_page_id, uid, _build, _trace_id, events:[{event, ts,
uid, trace_id, request_id, connection_id, severity, build, data, content}]}`.
`Content-Type` is not required (fetch and `sendBeacon` blobs both work) and no
request header is ever read or stored.

Responses:

- `202 {"ok":true,"accepted":N,"skipped":K,"dropped":bool}` — also when the
  queue is full (`dropped:true`, body not read), because the legacy page
  re-queues any non-2xx batch indefinitely.
- `400 invalid_audit_request` — non-JSON, non-object, `events` not an array,
  batch-level field type/length errors.
- `413 body_too_large` / `413 too_many_events` (more than 100 events).
- `429 rate_limited` with `Retry-After`.
- `503 shutdown` / `503 audit_busy` with `Retry-After: 1`.

Individual invalid events (name not matching `[A-Za-z0-9_.:-]{1,160}`, `ts`
not a string or longer than 64, `data` not an object, …) are skipped and
counted, matching the Python behaviour.

## Budgets (`audit::Limits`; tests lower them through `Config.audit_limits`)

| Budget | Default | Note |
| --- | --- | --- |
| Body | 4 MiB | Same as Python on purpose: `dom.snapshot` batches carry large `content` fields that are skipped by the deserializer and never allocated |
| Events per request | 100 | |
| `data` per event | 8 KiB serialized | Over budget → `{"truncated":true,"bytes","keys"}` |
| Strings / depth / arrays / object keys | 1024 chars / 8 / 256 / 128 | |
| Queue | 256 batches **and** 4 MiB | `try_send`; full → `202 dropped:true` |
| Parallel parses | 4 | |
| Token bucket | 10/s, burst 40, per client IP, at most 64 clients tracked | |
| Segment rotation | 8 MiB → `browser-YYYY-MM-DD.NNNN.jsonl` | |
| Total retention | 64 MiB | Oldest closed segments deleted first; the active segment and non-matching files are never deleted |
| Shutdown drain | 2 s (writer drain ≤ 1.5 s) | Beyond that counts as dropped |

Admission order: shutdown → token bucket → parse permit → queue bytes reserved
from `Content-Length` (capped at 1,024,000 bytes) → read body → parse → shrink
the reservation to the snapshot size → `try_send`. Durability: one `write_all`
per batch; `fdatasync` after one idle second, on rotation and on graceful
shutdown. Losing the last second on power failure is acceptable; blocking the
HTTP path is not.

## What is stored

Structured metadata only: `event`, `ts`, redacted `uid` (`source:<hash>`
form), trace/request/connection ids, severity, build, bounded `data`, plus the
server-side `seq`, `received_at`, `client` (peer IP) and `source`. Never stored:
`content`, unknown fields, request headers. Keys named like
`authorization`/`cookie`/`api_key`/`token`/`password` become `<redacted>`;
whole values shaped like absolute paths (`/x/…`, `~/`, `C:\`, `\\srv`) become
`<path>`.

## Observability

`GET /api/health` gains `"audit": {enabled, accepted_events, accepted_batches,
rejected_events, rejected_requests, rate_limited_requests, dropped_events,
dropped_batches, written_events, written_bytes, write_errors, retained_bytes,
queued_batches, queued_bytes}` (all zero with `enabled:false` when
unconfigured). Batches dropped at admission are unparsed, so they only count
in `dropped_batches`.

## Validation

`cargo test -p sessiondock audit --locked` (13 unit tests: validation,
rotation, retention, drop accounting, fault injection through `Gate`),
`cargo test -p sessiondock --test audit_http --locked` (7: 501 when
unconfigured, 202 + JSONL line, 413, 400, 429, saturation drops with a fast
request, health counters) and `python3 tests/audit_browser.py --binary
target/release/sessiondock` (real legacy page posts 12 event kinds through
fetch and the `pagehide` beacon; bounded records land in the JSONL; no message
text, `content` or corpus paths on disk; with the capability off the page makes
zero requests and the directory stays untouched). Playwright note: with
`context.route` active, polling must use `page.wait_for_timeout` so the page's
keepalive POSTs are not parked at the interception point.

Not implemented here: the Python per-route request/response tracing, SQLite/blob
storage and `/api/bug-report`. `debug_run` views are the read model's
([read-model.md](read-model.md#debug_run-视图python-agenthubdebug_runspy)).
