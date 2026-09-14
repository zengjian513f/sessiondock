# Browser diagnostics

`POST /api/audit/browser` accepts browser-side receipts when
`SESSIONDOCK_AUDIT_DIR` is configured. The directory is created as needed and
chmodded to `0700` on Unix. Files are
created with mode `0600`.

The wire shape is `{page_id|_page_id, uid, _build, _trace_id, events:[...]}`.
The route keeps the 4 MiB request-body limit and maximum of 100 events per
request. A malformed top-level body returns `400`; a larger body or event list
returns `413`. Non-object events and names outside
`[A-Za-z0-9_.:-]{1,160}` are skipped. Envelope fields are converted to text and
clipped to the database column limits.

The service stores structured metadata in daily
`browser-YYYY-MM-DD.jsonl` files. It does not retain the browser `content`
field. Diagnostic `data`: known credential keys are
replaced with `<redacted>`, recursion stops after depth 12, and paths, strings,
arrays, objects and serialized values have no additional Rust-only limits.

Writers use a nonblocking queue with the 20,000-item capacity. A full
queue remains best effort and returns `202` with `dropped:true`. There is no
per-client rate bucket, concurrent-parse gate, queued-byte cap, file-size
rotation threshold or retained-byte cap. Matching daily files older than
the 14-day retention window are deleted.

`GET /api/health` reports accepted, rejected, dropped, written and queued
counters. Shutdown closes admission and drains queued records within the
configured shutdown deadline.
