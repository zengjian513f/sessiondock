# Bug reports and their CLI workers (M7)

`POST /api/bug-report` captures a self-contained diagnostic bundle
and starts a managed
CLI instance that investigates it. The Rust implementation lives in
`bug_report/mod.rs` (bundle), `bug_report/worker.rs` (launch + prompt
injection), `audit/query.rs` (server-side audit events and the time-window
query), `api/bug_report.rs` (route, validation, the `uid=bug-report` upload)
and `files/write.rs` (`bug_report_upload`). The route is `501
bug_report_disabled` and `capabilities.bug_report` is `false` until every
dependency is configured.

## Configuration

| Setting | Meaning |
| --- | --- |
| `SESSIONDOCK_BUG_REPORT_DIR` | Bundle root. Created as needed and chmodded to `0700` on Unix. |
| `SESSIONDOCK_BUG_REPORT_REPO` | The repository the worker investigates: the worker's cwd and the parent of `sessiondock_attachments/`. Ordinary filesystem paths are accepted. |

The two variables are all-or-nothing (`--check-config` prints
`bug_report_dir=` / `bug_report_repo=`). The route additionally needs the
audit directory (`events.jsonl` is the core of a bundle), the terminal
transport and the lifecycle service (initialized ledger + launcher);
otherwise the capability stays `false` and the route `501`.

### Which CLI the worker runs

The worker is the source's one
configured CLI, selected exactly as `POST /api/term/create` selects it
(`LifecycleService::entry_for`), on the CLI's own default model and effort.
There is no worker-specific profile and no model policy: the worker is the
same session the user would start from the picker, launched with the
repository as cwd. A source without a unique configured CLI answers
`503 本机找不到 <source> 命令`. (An earlier build had shipped a per-source
`bug_report_profiles` table pinned to the cheapest test model; that confused
the real-CLI *test* rule of AGENTS.md with production and was removed. A
leftover `bug_report_profiles` key in `launcher.json` is ignored.)

## Route contract (`POST /api/bug-report`)

Body: `{description (required, ≤ 50000 chars), uid, page_id|_page_id,
_trace_id, _build, source ∈ claude|codex|grok (default codex), terminal_name,
snapshot (object), attachments: [{path, number, name, kind, mime, size,
attachment_id}], cols (40–300), rows (12–120), origin ({node_id, node_name,
uid}, optional), captured (object, optional)}`; unknown fields are ignored,
non-numeric `cols`/`rows` are `400`. The body limit is 16 MiB (a `captured`
object carries another machine's audit window and terminal frame).

`origin` names the machine the problem was seen on, as the page names it:
the Hub node id and registered name, or nothing standalone (the local host
then). It reaches the manifest as `origin` and the prompt as `问题机器：<node
name>（主机 <hostname>）`; `origin.uid` is the session reference as the
reporter saw it (Hub-scoped in Hub mode) and is what the prompt's `相关会话`
shows when the body `uid` is empty.

### A worker on another machine (`POST /api/bug-report/capture`)

The report dialog offers the same machine picker as the new-session dialog,
so a problem seen on Lyra can be handled by a worker on Cygnus. The Hub
proxy insists that `uid`/`terminal_name` and `_node` name one machine, so the
page does it in two requests:

1. `POST /api/bug-report/capture` to the problem's machine (`_node` = origin,
   `uid`/`terminal_name` scoped to it): the same server-side context
   `create` would have gathered there — `{ok, hostname, captured_at, uid,
   terminal_name, session (list row), outbox (delivery ledger), terminal_capture
   (8000 rows, managed instance only), events (the 900 s audit window matching
   `uid`/`page_id`/`_trace_id`, newest 20 000 rows)}`. Gated like the report
   route (`403 terminal_disabled`, `501 bug_report_disabled`).
2. `POST /api/bug-report` to the worker's machine with `uid: ""`,
   `terminal_name: ""`, `origin` and `captured` = the answer of step 1. The
   worker's machine then skips its own session/ledger/terminal capture, writes
   `captured.session`/`outbox`/`terminal_capture` into the bundle and puts
   `captured.events` in front of its own audit window in `events.jsonl`
   (`event_count` counts both). `captured.hostname` becomes
   `origin.hostname`; a `captured` object without it is `400` unless it carries
   `captured.error`.

When the problem's machine is offline or the capture fails, the page sends
`captured: {error}` instead: the report is still saved, `origin.capture_error`
records why, and the prompt says the bundle only has the browser snapshot and
the worker machine's audit rows. Attachments are uploaded to the worker's
machine (they live in its repository). The prompt of a remote worker also
tells it that the session's native JSONL and ledgers are not on its machine.

| Status | When |
| --- | --- |
| `403 terminal_disabled` | terminal transport off |
| `501 bug_report_disabled` | any dependency above missing |
| `400` `不支持的处理会话类型: …` | unknown `source` |
| `503` `本机找不到 <source> 命令` | the source has no unique configured CLI |
| `400` (`请描述遇到的问题`, `问题描述不能超过 50000 字`, attachment messages) | validation before the directory exists |
| `500 bug_report_capture_failed {error, report_id, path}` | a bundle file could not be written after the directory was created |
| `500 bug_report_worker_failed {error, report_id, path}` | capture succeeded, the worker launch failed (manifest `status: failed`, audit `bug_report.worker_launch_failed`) |
| `202 {ok, report_id, path, worker: {name, source, sid, cwd, token, title, kind: "bug-report", report_id, record_id, launch_id, instance_id, profile, cols, rows}}` | worker started; the prompt injection continues in the background |

`worker.sid` is the declared Claude session id (`null` for Codex/Grok, whose
identity stays pending like any other launch); `token` is that sid or the
launch id. The extra identity fields let the legacy page
open the pending console exactly as after `term/create`.

## Server input preservation

Reports and ordinary messages use the same [conversation service](conversation.md).
There is one revisioned editing draft per logical session in the private state
directory. Browser storage holds preferences and draft identifiers only. A report
uses a provisional `report:<id>` identity until its processing launch is bound to
that same draft; the server owns the first SEND. No browser submission archive or
native-confirmation outbox is created.

Selecting/pasting a file saves metadata only. File bytes remain in browser RAM
until explicit Send, then stream into private server staging. An unuploaded file
needs reselection after refresh; leaving with unuploaded bytes or an unfinished
save warns. Upload/SEND errors retain the current draft. Concurrent edits use CAS;
a successful SEND clears only the submitted revision.

The report body adds `{draft_uid, draft_revision, request_id}` and attachment
`{upload_id, number}` references. A stable report request ID freezes diagnostics
once; a lost response returns the stored result, without another bundle or launch.
A crash with an incomplete capture returns `report_result_unknown` and retains
input for inspection.

## The bundle

`<dir>/BUG-YYYYMMDD-HHMMSS-hex6/` (`0700`, files `0600`, every write is a
private temp file renamed into place):

| File | Content |
| --- | --- |
| `description.md` | the trimmed description plus newline |
| `browser-state.json` | the request's `snapshot` object, redacted |
| `events.jsonl` | audit rows of the last 900 s whose `uid`, `page_id` or `trace_id` match the report, or whose `data.report_id` is the report — always including the report's own `bug_report.created` row; a remote `captured.events` window comes first |
| `environment.json` | `repository`, Rust `build`, `git rev-parse HEAD` / `status --short` / `diff --stat` run with cwd = repository (10 s bound, stdout tail 200 000 / stderr tail 40 000 chars) |
| `terminal.txt` | only when `terminal_name` is a managed instance: 8000 scrollback rows (screen as fallback) read through the instance's guard envelope |
| `attachments/NN-<name>` | hard link or copy of each validated upload |
| `worker-prompt.md` | the prompt (below) |
| `manifest.json` | `{schema: 1, report_id, created_at, status, description_file, events_file, event_count, event_window_seconds, uid, page_id, trace_id, build, hostname (the worker's machine), client_ip, origin: {node_id, node_name, hostname, uid, remote, capture_error}, session (list row of `uid`), outbox (delivery ledger snapshot), terminal_file, attachments[+bundle_file], browser_state_file, worker_prompt_file, repository}` plus, after launch, `worker`, `worker_source`, `tmux`, `launched_at`, `injection`, `submitted_at`, `confirmed_from`, `composer_cleared`, `error` |

Redaction follows `audit.sanitize`: keys
(authorization, cookie, api-key, password, secret, access/refresh token …)
become `<redacted>` at every level; paths are kept.

### Attachments

`resolve_attachments` takes at most 12 items, each `path` resolving
to an existing regular file below `<repo>/sessiondock_attachments/` (a symlink is
accepted when its resolved target remains below that root), `number` from the
item or the position, `mime` ≤ 100 chars,
`kind` ∈ image/video/audio else `file`, `name` ≤ 200 chars, `relative_path`
relative to the repository. The prompt lists them as `附件N: ./<relative_path>`.

The browser uses `POST /api/session/conversation/attachment` to stream private
uploads scoped by draft/session identity and upload ID. Identical retries reuse
metadata; different bytes under that ID return conflict and never overwrite.
Explicit report submission publishes the files to
`<repo>/sessiondock_attachments/<batch>/<name>` through the existing checked file
writer, then freezes diagnostic attachment copies. Ordinary conversation sends
publish to their own verified cwd. The old direct attachment endpoint remains
available to legacy callers; it is not the new browser upload path.

## The worker (`bug_report/worker.rs`)

1. Create the source's ordinary configured CLI with repository cwd. Keep its
   model/effort defaults. The new frontend view is conversation mode; the PTY
   starts in the backend and can be opened manually.
2. Bind the processing launch to the report's server draft, retaining original
   text, uploaded references and the diagnostic task prompt.
3. Call the common conversation sender. A blank startup screen may be retried
   before any write. A choice menu/approval refuses SEND and retains the draft;
   the user answers in the terminal. No Enter resend or native-confirmation
   polling takes place.
4. Successful guarded paste + Enter is `submitted`, with `injection.basis: SEND`.
   Failure records `failed` and `draft_retained`. Native JSONL is read later for
   history rendering independently. An exited unbound CLI can be restarted from
   the retained conversation draft; both launches share that draft identity.

### Prompt

`worker_prompt` is rewritten for this repository: read the bundle, locate the
first event that diverges across layers, keep the user's working-tree changes,
make the minimal complete fix, run only the validation proportionate to the
change, and **never push, deploy, restart a deployed service or touch
production directories** — explain in the session instead. Push /
Hub-sync steps are gone. The header names the machine the problem was seen
on (`问题机器：Lyra（主机 lyra）`); a worker on another machine is told that
the bundle's session row, ledger, terminal frame and audit rows were fetched
from that machine and that the session's native files are not local, and a
failed capture is spelled out with its reason.

## Audit events (`audit/query.rs`)

Server-side events share the browser intake's JSONL segments and queue
(`AuditService::record`, `try_send`, dropped and counted when full): same
fields as a browser row with `client: "server"`, `client_ts: null` and a
`category`. `bug_report.created` carries `uid`/`page_id`/`trace_id` of the
report and `data.report_id`; the worker events carry `trace_id = report_id`.
`query(directory, since, until, filter, limit)` reads the segments of the
window's dates line by line (≤ 100 000 rows).

## Bundle details

- Audit rows carry structured metadata only; there is no `content` blob, so
  `events.jsonl` has no message text/composer content.
- The report id stamp is UTC.
- `terminal.txt` exists only for a managed instance.
- `submitted` means the common guarded SEND completed; missing native history
  does not introduce an unconfirmed state. Old manifests remain readable.
- Report attachments: ≤ 512 MiB per upload; no media preview token in the
  upload response (`media: null`).
- `cols`/`rows` are recorded in the manifest only; the PTY size follows the
  console that attaches.

## Validation

- `cargo test -p sessiondock --lib bug_report --lib audit::query --lib api::bug_report --locked`
  (bundle, attachments, composer probes, manifest merge).
- `cargo test -p sessiondock --test bug_report_http --locked` (fake Claude:
  501 unconfigured, 503 for a source without a CLI, raw attachment upload, 202 shape, bundle
  files, `submitted` from the synthetic native record, second report sees the
  first in its window, the capture route's answer and the `captured` validation).
- `python3 tests/bug_report_http_suite.py` (binary, fake Claude + fake Codex:
  9 scenarios including Codex SEND without a native rollout and the audit trail).
- `python3 tests/bug_report_node_browser.py` (hub page over three fake nodes:
  the dialog's machine picker defaults to the problem's machine, a worker on
  another machine goes through `/api/bug-report/capture` and hands `captured`
  over, a failed capture becomes `captured: {error}`, the chosen machine's
  missing CLIs are greyed out).
- `python3 tests/check_config_suite.py` (`bug_report_*` cases) and
  `python3 tests/meta_capabilities_suite.py`.
- `python3 tests/bug_report_real.py` (`# run_validation: real-cli`): the real
  Claude worker with `claude-haiku-4-5-20251001 --effort low` in a temporary
  `CLAUDE_CONFIG_DIR`, prompt confirmed from the real `user` record, model id
  asserted from the assistant record, instance killed, bundle and session
  files deleted.
