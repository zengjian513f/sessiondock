# Explicit local creation and pending consoles

The development backend now connects creation receipts, the launcher allowlist,
guarded host status, browser leases and the legacy pending view. Default startup
still launches nothing. Batch ten adds explicit operator native binding, not reliable delivery,
external CLI takeover, rename, authentication, or production deployment.

## Explicit configuration and startup

Set `SESSIONDOCK_LIFECYCLE_DIR` to an existing private receipt directory and
`SESSIONDOCK_LAUNCHER_CONFIG` to an explicit private JSON file. Both are required
for Web startup. `SESSIONDOCK_PTYHOST_DIR` must identify exactly the launcher's
host directory; the HTTP and lifecycle layers share one terminal lease registry.
Paths must remain outside frontend assets, native history, other state stores,
and downloadable roots. Launcher cwd roots must also exclude private/native and
frontend paths. No default home or CLI discovery is performed.

Initialize receipts separately, without starting Web or any CLI:

```sh
sessiondock --initialize-lifecycle /absolute/private/development/receipts
```

The directory must already exist, be empty and have Unix mode 0700. Repeating
initialization does not overwrite it. The launcher JSON is mode 0600, singly
linked and read through checked no-follow paths with a 64 KiB limit. Its shape
and platform limits are in [launcher configuration](lifecycle-launcher.md).
Each adapter is a versioned, server-owned executable/args/environment allowlist
entry, not a command template supplied by a browser. Changes to its meaning need
a new adapter ID, rather than reinterpreting an existing idempotency key.

Async `prepare_app` opens/reconciles existing receipts before serving routes.
The synchronous app factory rejects this configuration. Failed startup and
listener binding await service shutdown and release store locks. Shutdown closes
admission and waits owned work; it does not terminate established hosts. An
explicit shell test validates Linux Web restart survival, not service-manager
cgroup policies or Windows/macOS process independence.

## HTTP contract

All methods require the configured lifecycle service. Without it, they return
an explicit 501. Mutations are JSON-only with an 8 KiB body limit and reject
unknown/duplicate fields. The local-only security middleware remains in force.

| Route | Input | Meaning |
| --- | --- | --- |
| POST `/api/term/create` | `source`, `cwd`, `request_id`; optional `adapter_id`, `cols`, `rows` | Persist/replay a creation receipt, start at most once, verify guarded readiness |
| GET `/api/term/new-status` | `record_id`, `instance_id` | Refresh status of that exact recorded instance |
| POST `/api/term/kill` | `record_id`, `instance_id` | Persist cancellation, retire input authority, guarded stop, verify exit |
| POST `/api/term/discard` | `record_id`, `instance_id` | WP-E: drop a finished (Exited/Failed) or durably cancelled receipt from `term/list.pending` (Python `pending_store.discard`); 409 `launch_not_finished` while the instance may still run; the receipt stays queryable |
| POST `/api/session/stop` | `uid`; optional `request_id` | Batch 29: stop the managed instance declaring this UID (Ctrl-D ×2, then the guarded host stop); typed refusal for unmanaged sessions |
| POST `/api/term/bind` | `record_id`, `instance_id`, `uid`, `operator_confirmed: true` | Validate a real main-session scope, persist one immutable intent, bind and observe |
| GET `/api/term/list` | None | Native-bound sessions plus separate launch-only pending receipts; batch 24 adds `resume_sources{claude,codex,grok}` and `backends` |
| POST `/api/term/takeover` | `uid`, `request_id`; optional `adapter_id`, `force`, `cols`, `rows` | Batch 24: resume an inventory session through its source's unique resume-capable profile; reply carries `action: started|reused`; `force:true` is 501 `takeover_force_unsupported` |
| GET `/api/term/complete-dir` | `path`, optional `limit` | Batch 24: `{directories}` strictly inside the global cwd roots, symlinks neither followed nor listed, ≤50 (default 24); a root prefix only suggests the root itself; 400 `invalid_path` |
| POST `/api/term/backend` | `backend` | Batch 24: `{ok, backend:"ptyhost", backends}` for `ptyhost`/`host`, not persisted; `tmux` is 400 `backend_unsupported`, unknown 400 `backend_unknown` |

The limited legacy diagnostics `_build`, `_trace_id`, `_page_id` are accepted but
confer no authority and are not forwarded. `cols`/`rows` are validated display
hints; the terminal's attach/resize owns its actual size. They do not change the
immutable launch specification. Missing working directories are not created.
There is no `create_cwd`, executable, argv or environment request field.

With no adapter ID, source must select exactly one configured adapter. The legacy
source picker advertises only this unambiguous subset. Direct API callers can
name an explicitly configured adapter; configuration does not prove a real model
CLI supports particular arguments or native receipts. Isolated acceptance uses
only a fixed free shell.

Receipt replies include record/request IDs, routing name, declared source/cwd,
launch/instance IDs, state, running/stale flags, an explanation, and
`native_binding: "unbound"`. They intentionally do not contain a fabricated
native SID/UID, host credentials/endpoints, argv or environment. HTTP success
means a receipt result exists; only `running: true` is a freshly verified ready
instance. Failed/uncertain/cancelled receipts remain queryable, not silently
deleted or reported as running. A name-only kill is rejected.

WP-E adds `started` (Unix seconds the intent was persisted), `finished_at`
(Unix seconds the receipt became Exited/Failed), `discarded`, `discardable`
and, on `binding`, `method` (`operator` | `process`), `evidence` and
`bound_at`. `GET /api/term/list` lists a receipt under `pending` only while
it is not discarded and — once Exited/Failed — for at most 600 s after
`finished_at` (Python `pending_store.active` keeps a resolved row for 600 s;
a finished receipt migrated from an older ledger has no time and is archived
at once). Archived receipts still answer `term/new-status`.

## Automatic binding by process evidence (WP-E)

Python associates a new Codex/Grok pane with its native record once the
first prompt is on disk (`_new_session_status`: same cwd, not in the
`before` set, and for Codex the rollout held by a process of the pane). The
Rust service keeps only the process evidence — never cwd, time or file
name — in `lifecycle::autobind`, a background task that runs while a
`Running` receipt with launch kind `new_pending` has no binding:

1. one fresh guarded host observation names the exact instance and its
   child process (`summary.pid`);
2. the `/proc` scan of [liveness.md](liveness.md) (`SESSIONDOCK_PROC_SCAN=1`)
   pairs every indexed session of the receipt's source with its owned CLI
   main processes; a session counts when one of them is, or descends from,
   that child with no other CLI main process in between (Python
   `term.process_belongs_to`); Codex keeps its rollout open, a Grok TUI
   keeps `events.jsonl` of its session directory open;
3. exactly one such session, whose native scope the index verifies
   (Claude `sessionId`, Codex `session_meta.payload.id`, Grok
   `summary.json` `info.id`), is bound through the ordinary durable bind
   path with `method: "process"`; the evidence note (`cli_pids=[…] under
   host child pid … ; native record …`) and `bound_at` are persisted on the
   receipt's binding and audited as `lifecycle.autobind`; zero or several
   candidates leave the receipt pending (`lifecycle.autobind_ambiguous`).

The task needs the lifecycle service, the managed runtime
(`SESSIONDOCK_PTYHOST_DIR`) and the process scan; without the scan the
receipt stays pending until the operator dialog binds it. The legacy page
follows a confirmed binding exactly like a declared Claude identity: it
releases its launch-kind console, opens the native session and reclaims the
console through the native lease. Legacy `tests/lifecycle_http_suite.py`
keeps its fake-CLI expectations (no scan configured → no automatic binding).

Eight HTTP response permits cover queued work through serialization and retained
response bodies, including never-polled responses. Serialization runs off the
async reactor with a 2 MiB writer limit, and bodies stream in 32 KiB chunks while
retaining their permits.
The service has independent bounded work admission. Oversized/busy responses
fail explicitly; no unbounded background request queue is created.

## Launch identity kinds (batch 24)

`POST /api/term/create` accepts `resume_uid` and the receipt reports
`launch_kind` (`fixed`, `new_pending`, `new_assigned`, `resume`) with
`declared_sid`/`declared_uid`. A Claude new session is `new_assigned`: the
server mints one UUID v4 before the Prepared receipt is persisted (replays and
restarts reuse the same `--session-id`) and passes `--meta {source, launch_id,
instance_id, sid}` so the runtime catalog can match the native record as soon
as it appears. Codex/Grok new sessions are `new_pending` (`--meta` without an
identity) and still go through operator binding; `POST /api/term/bind` on a
launch with a declared identity is 409 `launch_identity_declared`. Resume
(`resume_uid` or takeover) resolves the full native SID from the frozen
session inventory's verified scope — never from a client-supplied SID or path —
substitutes `{sid}` as one argument and adds `sid`+`uid` to `--meta`; the
instance appears as that UID's session row and the legacy console uses the
`guarded_v1` native claim. One managed instance per native identity: a running
one is reused (`action:"reused"`), an uncertain and not cancelled one is 409
`launch_conflict`. Since WP-E the index also verifies a Grok main session's
scope (`summary.json` `info.id`), so Grok resume, stop, binding and the
recycle bin's run state work like Codex. Additional codes: 400
`invalid_launch_request` / `launch_adapter` (no unique entry for the source, or
no unique resume-capable profile) / `invalid_launch` (cwd outside roots) /
`terminal_size`; 404 unknown `resume_uid`; 409 `launch_source` /
`launch_cwd_unknown`. Ledger schema 3→4 migrates strictly (old records gain
`spec.launch={"kind":"fixed"}` and `session_id:null`; old files that already
carry the new fields fail closed).

Still 501: `takeover` with `force:true` and rename (both need external
process detection). `POST /api/session/stop` is implemented for managed
instances only, see below.

## Stopping a managed instance (batch 29)

`POST /api/session/stop {uid, request_id?}` (8 KiB, `deny_unknown_fields`,
legacy diagnostics accepted) is Python's `_stop_session` restricted to what
this build can prove. The UID is checked against the index's native catalog (404
`session_missing`, like Python's "会话不存在"), then resolved through one
**fresh** guarded runtime observation — never the `/api/live` cache, never a
name, cwd, time or PID guess — to the single `guarded_v1` bound target that
declares it (launch identity from `--session-id`/resume, or an operator
binding). Capability `session_stop` is true when both the terminal transport
and the lifecycle service are configured; otherwise the route is the generic
501 `not_implemented`.

Escalation, all performed by or through the host (the Web process never
signals a PID):

1. Fresh exact status; an instance that already exited answers
   `stage:"already_exited"` without sending anything.
2. `C-d` through the guarded input path (`guarded_v1` keys), then up to 1.2 s
   polling the exact instance for exit; repeated once — Python's
   `graceful_stop(timeout=2.4)` sends exactly two EOFs, no Ctrl-C.
   Exit here is `stage:"graceful"`.
3. Otherwise the existing guarded stop: for a launch receipt this is the
   durable `term/kill` path (persist cancel → retire the launch-derived
   leases → one host-performed `kill` = SIGHUP to the foreground group →
   exact exit evidence within the 3 s cancel budget); a guarded instance
   without a receipt (a host started elsewhere with `--meta`) receives the
   same host `kill` through its bound guard. Exit is `stage:"stopped"`.
4. No exit within those bounds is `stage:"uncertain"` (`stopped:false`): the
   cancel flag stays on the receipt, nothing is retried and Python's
   TERM/KILL of PIDs is deliberately not reproduced.

Reply: `200 {ok:true, uid, stage, stopped, name, instance_id, record_id,
graceful_attempts, replayed, tmux:false, external_detection:"not_implemented",
explanation}`. Python's `{ok, stopped, tmux}` keys keep their meaning; the
others are additive. Refusals are typed and never a silent success: 501
`session_stop_unmanaged` when the inventory session has no managed instance
(external/unmanaged CLIs are not probed), 409 `run_state_unknown` when a
managed record names the session but its host is unreachable, duplicated or
its identity unverifiable (nothing is sent), 400 `invalid_stop_request`,
409 `stop_request_conflict` when a remembered `request_id` names another UID.
An optional `request_id` makes the call idempotent: the service remembers the
last 256 acted-upon outcomes in memory (Python persists nothing here) and
replays them with `replayed:true`; typed refusals are re-evaluated each time.
No operator flag is required — Python's confirmation is the browser dialog.

Browser leases: like Python, stop neither needs nor fails on a browser
terminal lease. The EOF keys are server-originated host input; the WebSocket
ends with the host's own exit marker, and the receipt path retires
launch-derived leases exactly as `term/kill` does. A stop blocks the single
lifecycle coordinator for at most ≈2 × (1 s + 1.2 s) + 3 s, like a slow
cancel; `term/list` polls queue behind it.

Legacy (`session_stop:true`): the header "停止会话"/"删除会话" action and the
sidebar menu treat a session as stoppable when `S.live` has it **or** a
managed instance with this UID is listed (`S.live` is not polled under
`live:false`); the request carries a `request_id`; the outcome or the
server's refusal (unmanaged/external explanation, unknown host state) is
shown inline in `#session-stop-notice` instead of a bare alert, and a
confirmed stop drops the UID from `S.live`. Python-served pages are
unchanged.

Validation: `cargo test -p sessiondock --test session_stop --locked`
(isolated ptyhost + fake CLIs: graceful stop with `/api/live` exited,
`already_exited`, replay, request conflict, unmanaged 501, 400/404, and a
shell that ignores EOF escalating to the guarded stop within the bound;
skips when ptyhost is unbuilt) and `python3 tests/session_stop_browser.py`
(desktop + 390 px).

## Pending terminal identity and cancellation

Pending claim/attach supply `record_id`, `launch_id`, `instance_id` alongside the
usual name/page/lease token fields. Native-bound requests instead use UID and
instance. Mixing the two is rejected. The registry pins a typed target, and a
launch-declared host cannot be controlled using the raw/name-only path even if
no bound lease previously existed. A stale or replaced target never triggers
an unguarded retry.

Cancellation persists its intent before effects, retires the exact input lease
under the same IO gate used by input/claim, and requests a guarded stop. Lost ACK
or absent output is not proof of exit. Repeat cancellation does not signal a
possibly replaced process. A retry may complete idempotent local lease retirement
after a transient busy error, but never gains another kill authority.
A retired launch receives WS 4002 `launch retired`,
distinct from takeover and from actual process exit. The browser preserves its
rendered output, stops reconnecting and shows the specific pending explanation.
It does not clear drafts or delete receipts. Only a confirmed exit is recorded
as exit; uncertainty survives restart.

The legacy `tmux:<name>` value is only its existing pending-view key, never a
native UID sent to Rust's native APIs. Pending identity comes from the full
receipt/launch/instance tuple. Rust mode does not run Python's pending-to-native
resolution/automatic discard loop, and directory completion does not scan the
filesystem. Native binding uses the separate [one-time host protocol](host-native-binding.md)
and [durable service](lifecycle-binding.md). The confirmation dialog selects a
native UID; the server resolves its actual SID from one verified snapshot. It
rejects browser SID/source overrides, missing confirmation, conflicts and
subagents. Old immutable-metadata Grok consoles remain separate.

Receipts add nullable `binding` with source, actual SID/UID, state and method
`operator` or `process` (WP-E, see above); `native_binding` is unbound, intent,
confirmed or uncertain. Neither association is a native CLI receipt. Existing
pending sockets stay attached without auto-upgrade. The explicit release action
closes only this page's socket; then a native console can request a new lease
(the page does this itself when it follows a confirmed binding). Cross-kind
force is rejected.
Native claim and attach require fresh lifecycle authorization for launch-derived
targets, even after Web restart with a new in-memory registry. Cancelled launches
cannot regain native control simply because the host remains alive.

## Acceptance evidence

`tests/lifecycle_browser.py` builds an isolated corpus and private launcher
configuration for a fixed free shell. It uses the real legacy create dialog,
pending xterm keyboard, Web stop/restart, sidebar navigation and mobile stop
action. It checks same-request replay creates only one shell, conflicting specs
are rejected, pending status is explicit, no reliable-send composer is enabled,
cancellation does not reclaim ownership, and native fixture bytes are unchanged.
Host/client, store and coordinator tests cover the deeper identity, persistence,
lost response, shutdown, capacity and crash-recovery boundaries separately.

The `--native-binding` browser variant additionally uses the real mobile-sized
confirmation dialog, checks pending socket identity survives binding, explicitly
releases then opens the native console, and cancels it from isolated browser
storage. Its free shell deliberately ignores HUP: a subsequent Web restart must
still deny native claim while guarded host status proves the child is alive.
