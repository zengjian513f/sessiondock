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

## Validation

[`tests/server_log_browser.py`](../tests/server_log_browser.py) drives the page on
a node and through the Hub and parses every line.
