# Bug reports and their CLI workers (M7)

`POST /api/bug-report` (Python `server._bug_report` / `bug_report.py` at
`e5b023a`) captures a self-contained diagnostic bundle and starts a managed
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

Python `WORKER_SOURCES` / `shutil.which`: the worker is the source's one
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

Body as Python: `{description (required, ≤ 50000 chars), uid, page_id|_page_id,
_trace_id, _build, source ∈ claude|codex|grok (default codex), terminal_name,
snapshot (object), attachments: [{path, number, name, kind, mime, size,
attachment_id}], cols (40–300), rows (12–120)}`; unknown fields are ignored,
non-numeric `cols`/`rows` are `400`.

| Status | When |
| --- | --- |
| `403 terminal_disabled` | terminal transport off (Python `TERMINAL` false) |
| `501 bug_report_disabled` | any dependency above missing |
| `400` `不支持的处理会话类型: …` | unknown `source` |
| `503` `本机找不到 <source> 命令` | the source has no unique configured CLI |
| `400` (`请描述遇到的问题`, `问题描述不能超过 50000 字`, attachment messages) | validation before the directory exists |
| `500 bug_report_capture_failed {error, report_id, path}` | a bundle file could not be written after the directory was created |
| `500 bug_report_worker_failed {error, report_id, path}` | capture succeeded, the worker launch failed (manifest `status: failed`, audit `bug_report.worker_launch_failed`) |
| `202 {ok, report_id, path, worker: {name, source, sid, cwd, token, title, kind: "bug-report", report_id, record_id, launch_id, instance_id, profile, cols, rows}}` | worker started; the prompt injection continues in the background |

`worker.sid` is the declared Claude session id (`null` for Codex/Grok, whose
identity stays pending like any other launch); `token` is that sid or the
launch id (Python's tmux token). The extra identity fields let the legacy page
open the pending console exactly as after `term/create`.

## The bundle

`<dir>/BUG-YYYYMMDD-HHMMSS-hex6/` (`0700`, files `0600`, every write is a
private temp file renamed into place):

| File | Content |
| --- | --- |
| `description.md` | the trimmed description plus newline |
| `browser-state.json` | the request's `snapshot` object, redacted |
| `events.jsonl` | audit rows of the last 900 s whose `uid`, `page_id` or `trace_id` match the report, or whose `data.report_id` is the report — always including the report's own `bug_report.created` row |
| `environment.json` | `repository`, Rust `build`, `git rev-parse HEAD` / `status --short` / `diff --stat` run with cwd = repository (10 s bound, stdout tail 200 000 / stderr tail 40 000 chars) |
| `terminal.txt` | only when `terminal_name` is a managed instance: 8000 scrollback rows (screen as fallback) read through the instance's guard envelope |
| `attachments/NN-<name>` | hard link or copy of each validated upload |
| `worker-prompt.md` | the prompt (below) |
| `manifest.json` | `{schema: 1, report_id, created_at, status, description_file, events_file, event_count, event_window_seconds, uid, page_id, trace_id, build, hostname, client_ip, session (list row of `uid`), outbox (delivery ledger snapshot), terminal_file, attachments[+bundle_file], browser_state_file, worker_prompt_file, repository}` plus, after launch, `worker`, `worker_source`, `tmux`, `launched_at`, `injection`, `submitted_at`, `confirmed_from`, `composer_cleared`, `error` |

Redaction follows Python `audit.sanitize`: keys in Python's `_SECRET_KEYS`
(authorization, cookie, api-key, password, secret, access/refresh token …)
become `<redacted>` at every level; paths are kept.

### Attachments

`resolve_attachments` follows Python: at most 12 items, each `path` resolving
to an existing regular file below `<repo>/sessiondock_attachments/` (a symlink is
accepted when its resolved target remains below that root), `number` from the
item or the position, `mime` ≤ 100 chars,
`kind` ∈ image/video/audio else `file`, `name` ≤ 200 chars, `relative_path`
relative to the repository. The prompt lists them as `附件N: ./<relative_path>`.

`POST /api/session/attachment?uid=bug-report&name=<file>[&id=N]` is Python's
raw upload special case: the request body is the file, written through the
file write service into `<repo>/sessiondock_attachments/<id>/<name>` (`id` is
`[1-9]\d{0,8}` or the next free batch number; the name is sanitized like
Python `_attachment_name`; identical content is reused, a clash becomes
`stem__N.suffix`; nothing is ever overwritten). Response: Python's `{ok, name,
original_name, path, relative_path, attachment_id, mime, kind, size, reused,
media: null}`. Bound: 512 MiB per file (`413`), matching Python. Every
other session `uid` uses the conversation attachment upload contract of
[files.md](files.md).

## The worker (`bug_report/worker.rs`)

1. `lifecycle::Service::create` with the source's configured CLI and cwd =
   repository (`request_id` `bug-report-<report_id>`, idempotent). The record
   must reach `Running`; the sidebar decoration `{kind: "bug-report",
   report_id, title: "处理 <id>", worker_status, worker_error}` is remembered
   per lifecycle record (`BugReportService::pending_decoration`, rebuilt from
   manifests at start, `worker_status`/`worker_error` mirroring every
   manifest `status`/`error` update) and merged into the worker's
   `/api/term/list` pending row; the legacy pending page and sidebar row show
   it (`正在注入缺陷报告提示词`, `提示词已提交`, `提示词注入失败：…`).
   Manifest `status: starting`; audit `bug_report.worker_started`.
2. Readiness (Python `_inject_worker`, 90 s): the screen is read through the
   launch guard without a lease so a page may open the console meanwhile.
   Claude/Codex use the delivery driver's composer models
   (`driver::inspect_for`), Grok Python's `_ScreenProbe` (a non-blank frame
   that stopped changing). The composer must be `empty` for 600 ms; an
   `editing` frame on a fresh instance is a failure (`新建 … 会话出现了意外草稿`).
   State changes are audited as `bug_report.worker_probe`.
3. Injection as server-originated host input through the launch guard
   (`request_launch` — the same path `session/stop` uses for its EOF
   keys; Python's `tmux send-keys` needed no page console either). No browser
   lease is claimed, so a page that opened the console from the toast keeps
   it and watches the prompt arrive; `manifest.injection.origin` records
   `sessiondock-bug-report`. The frame is rechecked, `paste_started_at` is
   persisted, the prompt is pasted (bracketed), the paste is verified on
   screen — the composer shows the exact text, the TUI's collapsed-paste
   placeholder (`[Pasted text #1 +N lines]`, `[Pasted Content …]`), or, when
   the block cannot be read whole, a changed frame carrying the report id or
   the prompt's last line — then `pasted_at`/`paste_verified`,
   `enter_started_at`, Enter, `entered_at`, `enter_acknowledged` are persisted
   in turn. An unacknowledged (timed-out) Enter is recorded and never repeated
   blindly; a crash between the persisted steps leaves `status: injecting` and
   is never resumed.
4. Python `_confirm_submission`: for at most 4 × 1 s the composer is watched;
   while it visibly still holds the pasted draft Enter is resent (audit
   `bug_report.worker_enter_retry`); any other frame is only watched. The
   result is the manifest's `composer_cleared`, diagnostic only.
5. Confirmation comes from a native `user` record, never from the screen:
   Claude — the declared session id resolved through the published list rows;
   Codex/Grok — sessions of that source whose cwd is the
   repository and that were created since the launch. A `user` text containing
   the report id (or, for a declared session, a collapsed-paste placeholder as
   its first input) is `submitted` with `confirmed_from: {uid, method:
   "native_user_record", text_match}`; nothing within 20 s is
   `submitted_unconfirmed` with Python's message `提示词已粘贴到 <CLI>，但未能确认已提交；请在终端里检查`;
   any failed step is `failed` with `error`. Audit: `bug_report.worker_submitted`,
   `bug_report.worker_unconfirmed` (warning), `bug_report.worker_failed` (error).

Each launched worker starts its own injection task, as in Python.

### Prompt

`worker_prompt` is rewritten for this repository: read the bundle, locate the
first event that diverges across layers, keep the user's working-tree changes,
make the minimal complete fix, run only the validation proportionate to the
change, and **never push, deploy, restart a deployed service or touch
production directories** — explain in the session instead. Python's push /
Hub-sync steps are gone.

## Audit events (`audit/query.rs`)

Server-side events share the browser intake's JSONL segments and queue
(`AuditService::record`, `try_send`, dropped and counted when full): same
fields as a browser row with `client: "server"`, `client_ts: null` and a
`category`. `bug_report.created` carries `uid`/`page_id`/`trace_id` of the
report and `data.report_id`; the worker events carry `trace_id = report_id`.
`query(directory, since, until, filter, limit)` reads the segments of the
window's dates line by line (≤ 100 000 rows).

## Differences from Python (DELTA)

- Audit rows carry structured metadata only; there is no `content` blob, so
  `events.jsonl` has no message text/composer content (Python's SQLite blobs).
- The report id stamp is UTC (Python: local time).
- `terminal.txt` exists only for a managed instance (Python: any tmux window).
- `submitted` means a native `user` record carries the prompt (Python: the
  composer cleared); a Codex/Grok worker whose rollout cannot be found is
  `submitted_unconfirmed`.
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
  first in its window).
- `python3 tests/bug_report_http_suite.py` (binary, fake Claude + fake Codex:
  9 scenarios including Codex `submitted_unconfirmed` and the audit trail).
- `python3 tests/check_config_suite.py` (`bug_report_*` cases) and
  `python3 tests/meta_capabilities_suite.py`.
- `python3 tests/bug_report_real.py` (`# run_validation: real-cli`): the real
  Claude worker with `claude-haiku-4-5-20251001 --effort low` in a temporary
  `CLAUDE_CONFIG_DIR`, prompt confirmed from the real `user` record, model id
  asserted from the assistant record, instance killed, bundle and session
  files deleted.
