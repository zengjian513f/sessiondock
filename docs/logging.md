# Server log

Both binaries (`sessiondock`, `sessiondock-hub`) emit one JSON object per line
through `crates/sessiondock/src/log.rs` and the shared runtime writer.
With `SESSIONDOCK_STATE_DIR`, the default sink is its `runtime-logs` directory;
without it, the sink is stderr. `SESSIONDOCK_RUNTIME_LOG_DIR` selects a private
directory explicitly, or `-` selects stderr. The Hub normally retains its
external stderr configuration; under systemd, journald keeps that output with
the unit (`journalctl --user -u sessiondock -o cat`).

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

## Native SDK input and Agy result traces

`native_sdk.input_transition` records durable admission, state/consumption changes,
explicit stop and service recovery after the journal save succeeds. It carries
source, UID/native SID, original request ID, delivery ID, old/new state,
consumption, retry count and reason. The delivery ID joins Agy's
`agy.sdk.turn_started`, `write_receipt`, `input_written`, `step`, `result` and
`turn_finished` events; runtime ID, `native_pid` and stdout offset locate the existing
private sdkhost output and receipts. A reused write receipt is explicit and
does not mean input was sent again. Recovery/replay is labelled separately.

Agy logs completed user/assistant/error step boundaries, not token deltas.
`result` includes native status, output byte count, whether SessionDock sent an
interrupt, and the transcript reconciliation reason: user/error/final step
indices, missing or unreadable evidence, or the existing response-tail match.
`historical_error` and `effective_status` distinguish a proven earlier-turn
error from the current turn's completed native response; the raw `status` stays
available. The evidence carries `prior_error_step` separately from current errors.
`interrupt`, `kill_requested`, `kill_signal`, `output_closed`, `read_failed`,
`terminate` and `turn_finished`
distinguish explicit signals, transport failures and retained runtimes.
Original prompts, replies, native error text and stderr are not copied into
these metadata logs. Existing private native output and redacted live diagnostics
remain the place to inspect content. These events use the ordinary runtime log
sink and retention above; no debug switch is required.
These low-frequency transitions set `aggregation: "none"` so separate deliveries
and intermediate states remain separate records, including several results within
five seconds. They retain the writer's normal bounded admission, size limits and
loss counters. `pid` remains the logging service's PID; `native_pid` names Agy.

The Agy browser suite verifies persisted trace correlation for normal replies,
recovered contradictory results, genuine errors and explicit interruption,
including absence of message bodies and fixture credentials.

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
