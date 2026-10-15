# Server log

Both binaries (`sessiondock`, `sessiondock-hub`) write their log to stderr as
one JSON object per line; under systemd, journald keeps it with the unit
(`journalctl --user -u sessiondock -o cat`). The writer is `crates/sessiondock/src/log.rs`.
Before 2026-10-06 the services printed free-form text lines.

Every line has:

| Field | Meaning |
| --- | --- |
| `ts` | RFC 3339 UTC time with milliseconds |
| `level` | `info`, `warn` or `error` |
| `event` | dotted name, e.g. `server.listening`, `http.request`, `transfer.inventory` |

plus the event's own fields. A line is written with one `write` so concurrent
lines stay whole.

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
