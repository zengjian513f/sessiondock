# Server log

Both binaries (`sessiondock`, `sessiondock-hub`) emit one JSON object per line
through `crates/sessiondock/src/log.rs` and the shared runtime writer.
With `SESSIONDOCK_STATE_DIR`, the default sink is its `runtime-logs` directory;
without it, the sink is stderr. `SESSIONDOCK_RUNTIME_LOG_DIR` selects a private
directory explicitly, or `-` selects stderr. The Hub normally retains its
external stderr configuration; under systemd, journald keeps that output with
the unit (`journalctl --user -u sessiondock -o cat`).
Before 2026-10-06 the services printed free-form text lines.

Every line has:

| Field | Meaning |
| --- | --- |
| `ts` | RFC 3339 UTC time with milliseconds |
| `timestamp_ms` | Writer receipt time in Unix milliseconds; used for retention |
| `level` | `info`, `warn` or `error` |
| `event` | dotted name, e.g. `server.listening`, `http.request`, `transfer.inventory` |

plus the event's own fields. Producers enqueue without waiting on the sink.
A dedicated thread writes complete lines; admission is bounded by 1,024 items
and 8 MiB, plus at most one in-flight item. Capacity losses and write failures
are cumulative counters in node health `runtime_log`. Once a minute, changed
loss totals are reported by `log.delivery`; failed summaries do not clear them.
Shutdown waits at most one second for normal draining; a blocked external sink
does not hold service exit. This does not change detached ptyhost descriptors.

The shared system journal remains an external sink: this application cannot
safely impose project-only 24-hour deletion there. No global vacuum is run.
Application-owned runtime files for the node, PTY hosts and resource-agent have
rolling 24-hour retention and bounded duplicate aggregation. Their directories,
five-second summaries, idle maintenance, failure counters and browser validation
are specified in [process-owned runtime diagnostics](diagnostics.md#process-owned-runtime-diagnostics).
Already-running old PTY hosts retain their prior logging behavior until natural
exit. Browser and Hub audit JSONL have separate retention contracts; external
journal and proxy archives remain outside application cleanup.
`tests/runtime_log_fault_browser.py` uses real page
navigation with private file, failing-device and full-pipe sinks to measure
response time, normal flush and bounded shutdown. A private recoverable FIFO
stays broken across the real 60-second summary interval; after recovery the
browser scenario checks that the delivered cumulative errors include the
previously failed summary.

## What is never logged

Credentials, tokens, conversation text, draft contents, attachments, native
records, request and response bodies, query strings and request headers other
than the two page identifiers below. Event fields are metadata: ids, counts,
durations, error codes and error messages from the service itself. This is the
same boundary the browser diagnostics keep ([diagnostics](diagnostics.md));
that separate JSONL store under `SESSIONDOCK_AUDIT_DIR` is unchanged.

OpenCode preparation failures retain exit status and output byte counts only;
CLI stdout and stderr text never become diagnostic fields.

Browser terminal input, successful input-frame sends and output receipts are
summarized by connection and transport/kind in the five-second audit batch.
Each summary retains counts, byte totals and elapsed time, with no key or output
text. Successful heartbeat probes produce no browser events; failures and
connection transitions remain observable. At most 64 summary buckets and 500
queued events are retained; capacity losses produce a count. Page navigation
flushes partial summaries through the existing bounded beacon path.

`tests/terminal_log_volume_browser.py` measures actual typing-to-render latency
with logging on/off, checks the private shell's command ledger, and compares
persisted event/byte counts. This small workload does not establish a universal
performance bound or cover sustained terminal output.

## Request log

A layer on the node's `/api` routes (including the authenticated node
listener) and on the Hub's dispatcher writes `event: "http.request"` with
`method`, `path` (no query string), `status`, `ms` (until the response head;
streams are not followed), `code` when the answer is an ApiError,
and `trace` / `page` from the `X-SessionDock-Trace` / `X-SessionDock-Page`
headers when present (at most 64 printable bytes each).

`SESSIONDOCK_LOG_REQUESTS` selects what is written ([environment](environment.md)):

| Value | Logged requests |
| --- | --- |
| `errors` (default) | 5xx answers (`error`) and requests of 2 s or more (`warn`) |
| `all` | every `/api` request: 5xx `error`, 4xx or slow `warn`, others `info` |
| `off` | none |

Requests rejected before routing (the local-only and Hub gates) are not
logged. The startup line (`server.listening`, `hub.listening`) reports the
listening URLs and the request mode.

The native list index emits `native_state.read_failed` at `warn` when its
model/turn scan encounters a real I/O failure or short read. It includes the
provider `source` and the service/OS error text, never native records or paths.
An mtime update alone does not produce this event or make a readable row fail.

Successful history opens taking at least 2 s also emit `history.open.slow`:
`uid`, `read_ms` (reader admission plus snapshot/body construction), `echo_ms`
(queued-send reconciliation), `prompt_ms` (live approval observation), and total
`ms`. These phases separate history work from runtime waits without recording
message text or native contents.

## Validation

[`tests/server_log_browser.py`](../tests/server_log_browser.py) drives the page on
a node and through the Hub and parses every line.

## History opening phases

`history.open.slow` retains the existing two-second threshold. In request-log
`all` mode, faster successful opens emit `history.open.ready` as well. Both
carry the validated browser `trace`/`page` identifiers and these durations:
`queued_ms` (reader admission plus blocking executor scheduling), `snapshot_ms`
(index/view acquisition and parsing), `body_ms` (message/media projection and
prompt/echo extraction), and `read_resume_ms` (return to the async task).
Their sum is `read_ms`; `echo_ms` and `prompt_ms` complete the handler timing.
These are elapsed wall times, not CPU measurements. Failed or cancelled opens
may have no completion record. `tests/server_log_browser.py` opens native
history through node and Hub pages and verifies the phase totals and IDs.

BUG-20261018-040613-165ffa: the browser sent the opening request 8 ms after
clicking and received its parsed body after 5902.6 ms, then rendered within
9 ms. The matching node trace recorded 5830 ms in the previously combined
read phase, 0 ms retiring echoes and 4 ms probing the prompt. The native turn
had completed before the click, and the lifecycle binding still named its
running instance; no delivery directory existed. A synthetic five-screenshot
Chromium open took approximately 0.4 seconds and did not reproduce the stall.
Follow-up phase logs captured 1519–2271 ms in snapshots with no admission delay.
The collector returned 71.8 MB / 103k boot-long connection records: liveness,
catalog acknowledgements and coordinator reads expanded them into a generic JSON
tree, cloned it and converted it again. Typed decoding on blocking workers and
bindings-only decoding for liveness remove those unnecessary allocations; the
coordinator still receives full evidence. In the synthetic 92 MB / 100k-record
Chromium regression, the old release peaked at 8776 MiB and cold opens took
468–1838 ms; the fixed release peaked at 285 MiB and took 34–343 ms.
This reproduces and fixes contention under collector load; the old combined
trace cannot retrospectively assign every millisecond of its 5830 ms stall.
See [collector regression](../tests/history_collector_browser.py).
