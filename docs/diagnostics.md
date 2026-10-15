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
without a matching `terminal.input.written` window locate a gap between browser
input and a recorded host write; they do not prove that no frame reached the
node. The input receipt is emitted after the gated write, and cancellation can
drop an unfinished aggregation window; audit delivery is also best effort.
Prompt input receipts followed by no `terminal.output.sent` are consistent with
no CLI screen change. Output sent but not received points to the link back to
the browser. Only counts and timings are recorded, never terminal bytes.

`tests/terminal_diagnostics_browser.py` clicks the mobile console, sends shell
input, correlates node/browser claim and terminal I/O receipts, checks the
node's close receipt, and exercises real delayed HTTP
headers and bodies using isolated loopback fixtures.

### Locating a stalled terminal across the Hub

The browser, Hub and node now keep the same page and connection identifiers
through these additional metadata-only receipts. A successful write at any
boundary means that boundary accepted bytes, not that the CLI processed them.

| Receipt | What it establishes |
| --- | --- |
| `browser.terminal.input` | The console produced input; `transport` selects HTTP or WebSocket. |
| `browser.terminal.frame.sent` | WebSocket `send()` returned, with input/resize/ping kind, per-connection frame/byte totals and `buffered_bytes`. |
| `browser.terminal.frame.send_failed` | `send()` threw; records the exception name, not its message or payload. |
| `browser.terminal.heartbeat.pong` | Pong ID, whether it matched the pending probe, and matching round-trip time. Timeout records also include the pending ID and cumulative send counts. |
| `hub.terminal.transport` | Hub `browser_read`, `node_written`, `node_read`, `browser_written`: cumulative bytes/calls, pending duration, EOF and I/O error kind for each boundary. |
| `terminal.transport.progress` | Node decoded input received versus written to the host; ping/pong counts; current input/output stage and duration. |
| `browser.terminal.transport.dropped` / `.closed` | Browser-side disposal and actual close, including replaced sockets and their final send totals. |

Hub/node snapshots are emitted on opening, every five seconds (including quiet
connections; Hub periodic snapshots start after the upgrade), and on exit.
Node state lives outside the cancellable I/O futures,
so replacement, shutdown and an unfinished write retain the counters and stage.
Input stages distinguish `attaching_host`, `waiting_browser`, `waiting_gate`,
`writing_host` and `queueing_pong`; output stages distinguish readiness control,
waiting for host output and writing to the browser. Node snapshots include the
audit service's cumulative dropped-event and disk-write-error counters.

Hub byte counts include WebSocket framing; browser/node input-byte counts do
not. Compare the two ends of each Hub direction, not Hub raw bytes against
decoded PTY bytes. Response bytes prefetched with the HTTP upgrade are included
in `node_read`. No WebSocket payload parsing, input replay or protocol change is
introduced by these meters.

With Hub audit enabled, the original snapshot is stored in its private
`hub-YYYY-MM-DD.jsonl`. An independent bounded diagnostic queue also forwards
it to the node's existing audit intake. There it is named
`browser.hub.terminal.transport` because that intake prefixes event names;
`data.observer: "hub"`, `observed_at`, page/connection and node ID establish its
origin. These copies therefore enter ordinary `events.jsonl` bug bundles,
including remote captures, without depending on the browser to relay them.
Forwarding failure or queue overflow is recorded locally as
`hub.terminal.audit_forward_failed` / `.audit_forward_dropped`; it never blocks
terminal traffic. If the node was unreachable, inspect the Hub's local file.
Logs are still best effort; an absent receipt alone is not proof of lost input.

For example: browser send counts increasing while Hub `browser_read` stays
unchanged places the gap before Hub receipt; Hub read increasing with node-write
stalled places it in Hub upstream forwarding; node decoded receive increasing
without host writes places it in the node stage recorded alongside those counts.
A node pong count increasing without a matching browser pong places the gap on
the return path. The two Hub return counters further split that path.

`tests/terminal_heartbeat_browser.py` exercises both direct node and real
authenticated Hub paths in Chromium. It blocks each direction separately and
also blocks the very first upstream heartbeat/input while the initial screen
still arrives. Assertions join all three layers, retain final state on
replacement, verify uncertain input is not replayed, and check that terminal
text and node credentials do not appear in these logs.

### BUG-20261013-232935-441af3: repeated PTY upstream stalls

The Cygnus report was captured on build `20261013T223823185417Z`, commit
`afbe3f7c`, with a clean checkout. The complete manifest, browser state,
environment, 842 audit events and terminal capture were reviewed alongside
the lifecycle/conversation ledgers, the native Claude JSONL and the node/Hub
service journals. Times below are UTC on 2026-10-13.

- At 23:28:40 the first claim completed in 52 ms and the browser painted the
  initial grid within 167 ms. The first recorded input at 23:28:44.093
  (event 513, 27 bytes) had no corresponding node write receipt. This is the
  first observable input-path divergence, before the report dialog existed.
- Across the first four connections the browser recorded 42 inputs. Each
  received its initial screen but timed out its first bidirectional heartbeat
  after 10 seconds (events 533, 625, 697 and 792). None had an input-write
  receipt. Replacement claims completed in 49–72 ms, and ordinary HTTP/SSE
  traffic continued. The browser reported zero buffered WebSocket bytes at
  timeout; this is not a remote-delivery acknowledgement.
- The fifth connection opened at 23:29:27.254. Its first input receipt was
  two zero-byte resize frames (event 820), not the user's text. The terminal
  capture still showed an unsubmitted draft; the native history checkpoint
  stayed at 9277741 throughout the report window. PTY keystrokes do not use a
  SEND delivery ledger; the deployment had no `delivery/` directory, and its
  current conversation ledger and running lifecycle record were inspected.
- The subsequent daily audit shows new input on the fifth connection at
  23:29:37.784 and a node write window at 23:29:38.810 (39 bytes).
  At 23:29:51.028 native JSONL recorded a new user message. The same managed
  instance remained running; this recovery preceded this investigation and
  involved no repair or restart by the investigator.

The evidence identifies the browser-input → node-write gap and failed
bidirectional transport checks. It does not identify whether browser/network,
HTTPS proxy, Hub forwarding or node receive processing caused the gap.
The journals contain slow successful HTTP requests but no event establishing
a WebSocket failure cause. Header-button errors after opening the report are
occlusion by that dialog, not evidence that the terminal keyboard was blocked.
No speculative runtime change, input replay or production-data edit was made.

On 2026-10-14, `python3 tests/terminal_heartbeat_browser.py` passed in headless
Chromium with private synthetic fixtures: separately stalled downstream/upstream
sockets recovered, the host survived, and uncertain input was not replayed.
`python3 tests/terminal_rtt_browser.py --binary target/debug/sessiondock` also
passed: node and authenticated Hub paths each returned and painted all 85
keys (5 warmup, 40 individual, 40 grouped). Both runs used existing debug
binaries and the current frontend; they were not production-network tests.
These checks exercise recovery and input delivery, not the cause of this
intermittent production incident. The unresolved investigation remains in
[TODO](../TODO.md).

## Embedded information and resource views

`dom.snapshot` (also used by bug reports) records `view` separately from the
terminal connection/mode. `terminal.visible` uses rendered geometry and CSS;
an open WebSocket may remain connected while the information/resource view
hides the terminal. `content_view` contains the pane and scrolling element's
client/scroll dimensions, positions, display and overflow. Information views
also include the iframe document's root and body measurements.

`content_view.changed` records open/close, and `content_view.loaded` records
document load, successful content load and failures. Resource polling records
state transitions, not repeated successes. Explicit refreshes retain receipts.
Passive wheel observers in the iframe document and resource pane aggregate
attempt counts/deltas and before/after layout for a fixed one-second window
into `content_view.scroll`. `moved` also detects movement that returned to its
starting position. No movement alone is not an error: compare direction,
position, dimensions and overflow to distinguish an edge from blocked scrolling.
Leaving a view flushes its pending window and removes its listeners. These
metadata-only events use the existing bounded 500-event browser queue and
batched intake; no text, input values or credentials are collected.

An intentionally modal-covered console button records `occluded_by_modal` in
its normal state receipt, rather than a `console.button.missing` error.
Missing, hidden or clipped buttons remain faults.

For BUG-20261015-135638-c5bbf6, all 917 bundled events, the manifest, browser
state, environment, terminal capture, lifecycle binding and native JSONL
metadata were reviewed. The first observable diagnostic divergence is snapshot
587 after information-button click 583: it still reports the CSS-hidden terminal
as visible. Resource click 701 and information click 730 confirm the active
surface; no iframe wheel evidence existed. All 26 heartbeat replies matched,
HTTP response receipts were 200, native progress continued, and the lifecycle
binding was running. The four button errors followed report-modal occlusion.
No delivery directory existed and none was created.

The private Chromium baseline reproduced the information root's `overflow:hidden`:
2853px content in a 714px viewport stayed at scrollTop 0 after mouse wheel input.
The resource pane scrolled from 0 to 426 normally. The fix gives the information
root vertical scrolling; resource layout remains unchanged. The
[content scroll suite](../tests/content_scroll_browser.py) checks real wheels,
view changes, retry, geometry, themes/zoom and persisted audit records, including
an HTTP-served old-CSS fault to verify blocked-scroll diagnostics.
