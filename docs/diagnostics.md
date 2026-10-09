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

## Browser request cadence

Ordinary receipts are batched for up to 5 seconds, with up to 100 events and
48 KiB per HTTP batch. Reaching 100 queued events triggers an earlier flush;
error receipts bring the pending flush forward to 100 ms. New ordinary events
do not postpone the existing deadline. Failed requests wait at least 5 seconds
before retrying, including when new events fill the queue. Page hiding/unloading still flushes via
beacon, and long-frame beacons remain immediate.

Rust pages omit the `content` body before sizing/queueing receipts: the intake
only persists structured metadata, so transferring DOM/message/terminal
snapshots would add bandwidth and fragment batches without retaining evidence.
The trace IDs, timestamps, status, latency and other `data` fields remain.
This batching does not slow input readiness checks or conversation updates.

## Main-thread long frames

The page reports every main-thread frame of 1 s or longer as
`browser.main_thread.long_frame` (severity `warning`), at most 60 per page
load. The browser's `long-animation-frame` entry supplies the attribution in
`data.scripts[]`: `invoker` (for example `WebSocket.onmessage`,
`TimerHandler:setInterval`, `EventSource.onmessage`), `invoker_type`,
`function`, `url` (relative to the page) and `char` (source position), plus
per-script `duration_ms`, `forced_layout_ms` and `pause_ms`. The frame itself
carries `duration_ms`, `blocking_ms`, `render_ms`, `style_layout_ms`,
`heap_mb` (`[used, total, limit]`, Chromium only), `dom_nodes`, the selected
view, visibility, the audit queue length and the terminal state.

The record is sent with `sendBeacon` the moment the observer fires, not
through the batched queue: the browser process owns the request as soon as it
is queued, so the record survives even when the next frame never ends. A page
that freezes for good therefore leaves its last few, progressively longer
frames in the log with the function that ran in each. The page registers no
service worker for the same reason: a navigation to the same origin would wait
for a worker started inside the existing, possibly frozen, page process.

## Response bodies

Every audit post reads its response body to completion even though the page
only needs the status. An unread fetch response keeps its 2 MiB shared-memory
data pipe — one renderer file descriptor — until garbage collection; at one
post per second the renderer's 1024-fd soft limit fills within minutes, after
which the GPU command buffer cannot allocate shared memory and the tab freezes
in native code with no JavaScript on the stack. `tests/renderer_fd_browser.py`
guards it.

## Terminal opening and response phases

### Reproducible keyboard round trips

`python3 tests/terminal_rtt_browser.py --binary target/release/sessiondock`
opens the real console UI against a private raw-mode echo child, first through
the node and then through an authenticated test Hub. Chromium presses actual
keys; each reply has a sequence number. The suite checks every key returns
exactly once, reaches the grid, produces no page error, and leaves the synthetic
native record unchanged. Five warmup keys precede 40 individual keys and 40
keys in groups of eight. Timings are observations, not latency pass/fail limits.

`--output FILE` saves individual samples, browser/build information, phase
frame/byte/paint counts, and p50/p95/max for keydown → WebSocket send → matching
PTY reply → Canvas draw completion. Percentiles use nearest rank for p95.
The draw endpoint is **not compositor presentation or physical display latency**.
No clock synchronization between machines is needed: all timestamps come from
the same browser's `performance.now()`.

For a remote browser, `--serve FILE` keeps only the temporary loopback fixtures
alive and writes their URLs and synthetic UIDs. Forward those loopback ports
with SSH, then run the same script on the client with `--base URL --uid UID`.
Creating `FILE` with its suffix replaced by `.stop` shuts the fixtures down;
they also expire after 15 minutes. These options are for private fixtures,
never existing production sessions. `--ptyhost` and `--hub-binary` select saved
binaries independently; `SESSIONDOCK_TEST_WEB_DIR` selects saved frontend assets.

On 2026-10-09, Chromium on Cetus exercised an isolated shell on Lyra over an
SSH loopback forward on the existing network. The test Hub was colocated with
the node. Each cell below is the median **keydown-to-Canvas-draw** time in ms,
with 40 measured keys per phase. The first baseline preceded all experiments;
each experiment changed one candidate setting relative to the baseline setup.

| Trial | Node, individual | Node, grouped | Test Hub, individual | Test Hub, grouped |
| --- | ---: | ---: | ---: | ---: |
| Existing release binaries and frontend | 23.40 | 29.00 | 28.65 | 28.10 |
| Accept sockets with `TCP_NODELAY` | 23.65 | 27.65 | 27.25 | 29.00 |
| Draw synchronously instead of waiting for animation frame | 14.50 | 17.15 | 18.10 | 17.40 |
| Host grid quiet wait changed from 1 ms to zero | 24.05 | 26.75 | 26.05 | 28.70 |

Baseline node send-to-reply medians were 14.40/14.85 ms; reply-to-draw medians
were 8.40/14.10 ms for individual/grouped input. `TCP_NODELAY` and removing the
quiet wait did not establish a repeatable improvement in this setup. Drawing
synchronously advances the measured draw boundary, but this does not establish
earlier presentation and removes output coalescing. None of these experimental
runtime changes was retained.

Limits: this was a small synthetic-shell observation, not a measurement of the
production HTTPS proxy, the remote production Hub's upstream connection, or
Claude/Codex/Grok rendering. SSH forwarding changes TCP boundaries, so the
`TCP_NODELAY` result cannot rule out Nagle delays on the production node link.
The starting binaries were the existing release artifacts; experimental Rust
binaries were rebuilt from the current checkout, so unrelated intervening code
is another confounder. After the last browser run passed, Cetus SSH reset while
copying its result file and a planned fresh-build baseline repeat could not run.
The last Hub summary survived in the orchestration log; its raw samples were
not retrieved. These data neither identify the reported production slowdown
nor justify claiming a user-visible speedup.

### Claim and attach receipts

Browser `post()` receipts share `trace_id` and `page_id`:

- `browser.http.response.headers`: fetch returned response headers; records
  `status` and `headers_ms` from request start.
- `browser.http.response.received`: the body was read and JSON parsed; includes
  separate `headers_ms` and `body_ms` (body read only), plus total `duration_ms`.
- `browser.http.request.failed`: records `phase` (`headers`, `body`, or `parse`),
  nullable `status`/`headers_ms`, `timeout_ms`, visibility and online state.
  An online browser can still have an unreachable or stalled connection.

The node records `terminal.claim.received` on entry to the decoded claim
handler and `terminal.claim.response_ready` when it returns, including errors.
These carry the browser's trace/page/build headers, the target UID/name,
status and elapsed milliseconds. They use the existing nonblocking audit writer
and appear in bug-report bundles. A missing response-ready event may mean a
pending/cancelled request or a dropped audit record; it does not prove a hang.
Response-ready proves handler completion, **not delivery to the browser**.
No lease token, request body, response body or terminal text is recorded.

Terminal receipts share `connection_id` and terminal name:
`browser.terminal.connecting` → `browser.terminal.first_output` → (grid only)
`browser.terminal.snapshot_applied` → `browser.terminal.first_paint`.
First-output is immediate, unlike the existing 750 ms aggregated byte counts.
The grid records its first complete snapshot after parsing/application, then
its first nonempty canvas render with a nonzero layout rectangle. Hidden
canvases wait until rendered while shown; incomplete JSON does not count as a
snapshot. Paint records include pane visibility, document visibility, dimensions
and elapsed time from connection creation. This measures a canvas draw, not
physical display/compositor presentation. The first malformed grid line is
reported as `browser.terminal.grid_parse_error` with its length, never its text.
These first-frame/error markers reset on a new attachment, not on every diff.

The node records its side of each attach with the page's `page_id` and, in
`data.connection`, the page's connection id. Frames are aggregated into windows
that close 1 s after their first frame:

- `terminal.input.written`: browser frames the node received and wrote to the
  host (`frames`, `bytes`, `span_ms`), with `max_write_ms` covering the
  per-name gate wait plus the host write acknowledgement.
- `terminal.output.sent`: host output handed to the browser socket, with
  `max_write_ms` for the slowest socket write.
- `terminal.attach.closed`: the close code and reason.

A `max_write_ms` of 1 s or more is a warning. Page `browser.terminal.input` rows
without a matching `terminal.input.written` window mean the frames did not reach
the node. Prompt input receipts followed by no `terminal.output.sent` mean the
CLI produced no screen change. Output sent but not received points to the link
back to the browser. Only counts and timings are recorded, never terminal bytes.

`tests/terminal_diagnostics_browser.py` clicks the mobile console, sends shell
input, correlates node/browser claim and terminal I/O receipts, checks the
node's close receipt, and exercises real delayed HTTP
headers and bodies using isolated loopback fixtures.
