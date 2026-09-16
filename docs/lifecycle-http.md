# Explicit local creation and pending consoles

The development backend now connects creation receipts, configured CLI profiles,
guarded host status, browser leases and the legacy pending view. Default startup
still launches nothing. Batch ten adds explicit operator native binding, not reliable delivery,
external CLI takeover, rename, authentication, or production deployment.

New sessions open the conversation page. The configured CLI and ptyhost still
start on the backend; the browser does not claim or attach a console until the
user switches to it. Reopening a session may restore the user’s saved terminal
choice. Saved pending input remains editable even when its CLI has exited.

## Explicit configuration and startup

Set `SESSIONDOCK_LIFECYCLE_DIR` to an existing private receipt directory and
`SESSIONDOCK_LAUNCHER_CONFIG` to an explicit private JSON file. Both are required
for Web startup. `SESSIONDOCK_PTYHOST_DIR` must identify exactly the launcher's
host directory; the HTTP and lifecycle layers share one terminal lease registry.
Legacy launcher cwd-root fields are accepted for compatibility but do not
authorize or restrict working directories.

Initialize receipts separately, without starting Web or any CLI:

```sh
sessiondock --initialize-lifecycle /absolute/private/development/receipts
```

The directory must already exist and contain no lifecycle ledger. Repeating
initialization does not overwrite it. The launcher JSON supplies server-owned
profiles. Its shape
and platform limits are in [launcher configuration](lifecycle-launcher.md).
Each source uses a server-owned executable/args/environment profile rather than
a command template supplied by a browser.

Async `prepare_app` opens/reconciles existing receipts before serving routes.
The synchronous app factory rejects this configuration. Failed startup and
listener binding await service shutdown and release store locks. Shutdown closes
admission and waits owned work; it does not terminate established hosts. An
explicit shell test validates Linux Web restart survival, not service-manager
cgroup policies or Windows/macOS process independence.

## HTTP contract

All methods require the configured lifecycle service. Without it, they return
an explicit 501. Mutations are JSON-only; create/takeover/stop
bodies ignore unrelated dictionary members. The local-only middleware applies.

| Route | Input | Meaning |
| --- | --- | --- |
| POST `/api/term/create` | `source`, `cwd`; optional `request_id`, `create_cwd`, `cols`, `rows` | Persist/replay a creation receipt, request confirmation before creating a missing directory, start at most once, verify guarded readiness |
| GET `/api/term/new-status` | `record_id`, `instance_id` | Refresh status of that exact recorded instance |
| POST `/api/term/kill` | `record_id`, `instance_id` | Persist cancellation, retire input authority, guarded stop, verify exit |
| POST `/api/term/discard` | `record_id`, `instance_id` | Drop a finished (Exited/Failed) or durably cancelled receipt from `term/list.pending` together with the input the conversation service retained for it (a draft shared with a bound native session stays); 409 `launch_not_finished` while the instance may still run; the receipt stays queryable |
| POST `/api/session/stop` | `uid` | Stop a managed instance through guarded host control or terminate process IDs attributed to this exact external native session; an already-stopped session succeeds with `stopped:false` |
| POST `/api/term/bind` | `record_id`, `instance_id`, `uid`, `operator_confirmed: true` | Validate a real main-session scope, persist one immutable intent, bind and observe |
| GET `/api/term/list` | optional `force=1` | Native-bound sessions plus separate launch-only pending receipts, `resume_sources{claude,codex,grok}` and `backends`; served from a 2 s per-view response cache that every mutation above drops at once, `force=1` bypasses it ([liveness.md](liveness.md#response-caches)) |
| POST `/api/term/takeover` | `uid`; optional `force`, `cols`, `rows` | Reuse a managed console, start a stopped session, or return `needs_confirm` for a running external CLI; confirmed force terminates only exact native-session process matches before resume |
| GET `/api/term/complete-dir` | `path`, optional `limit` | Absolute/`~/` completion without a configured root gate; directory symlinks are followed, ≤50 (default 24) |
| POST `/api/term/backend` | `backend` | `{ok, backend:"ptyhost", backends}` for `ptyhost`/`host`, not persisted; `tmux` is 400 `backend_unsupported`, unknown 400 `backend_unknown` |

The limited legacy diagnostics `_build`, `_trace_id`, `_page_id` are accepted but
confer no authority and are not forwarded. `cols`/`rows` are validated display
hints; the terminal's attach/resize owns its actual size. They do not change the
immutable launch specification. Missing working directories return
`needs_create`; `create_cwd:true` creates the confirmed absolute path. Executable,
argv, environment, SID, and legacy adapter fields cannot choose the command.

Source selects exactly one interactive configured CLI or the `shell` terminal.
The legacy source picker advertises only this unambiguous subset and labels
`shell` as SSH. `term/list.sources.shell` advertises shell creation; resume
sources remain the three AI CLIs. Shell receipts use fixed argv, stay in the
terminal list while running, and support the same guarded attach, reconnect,
kill and discard as other launch receipts without native binding.
Shell receipts leave the sidebar as soon as exit or launch failure is verified;
the minimal lifecycle receipt stays queryable for idempotency. Terminal output
is a bounded in-memory screen/scrollback, not a persisted conversation archive.
An existing running shell can be reattached; an exited shell cannot be resumed.

Receipt replies include record/request IDs, routing name, declared source/cwd,
launch/instance IDs, state, running/stale flags, an explanation, and
`native_binding: "unbound"`. They intentionally do not contain a fabricated
native SID/UID, host credentials/endpoints, argv or environment. HTTP success
means a receipt result exists; only `running: true` is a freshly verified ready
instance. Failed/uncertain/cancelled receipts remain queryable, not silently
deleted or reported as running. A name-only kill is rejected.

Receipts carry `started` (Unix seconds the intent was persisted), `finished_at`
(Unix seconds the receipt became Exited/Failed), `discarded`, `discardable`
and, on `binding`, `method` (`operator` | `process`), `evidence` and
`bound_at`. `GET /api/term/list` lists a receipt under `pending` only while
it is not discarded and — once Exited/Failed — for at most 600 s after
`finished_at` (a resolved AI row is kept for 600 s; finished `shell` rows leave immediately;
a finished receipt migrated from an older ledger has no time and is archived
at once). Archived receipts still answer `term/new-status`.

## Automatic binding by process evidence

A new Codex/Grok pane is associated with its native record once the
first prompt is on disk (`_new_session_status`: same cwd, not in the
`before` set, and for Codex the rollout held by a process of the pane). The
Rust service keeps only the process evidence — never cwd, time or file
name — in `lifecycle::autobind`, a background task that runs while a
`Running` receipt with launch kind `new_pending` has no binding:

1. one fresh guarded host observation names the exact instance and its
   child process (`summary.pid`);
2. the native process scan of [liveness.md](liveness.md)
   pairs every indexed session of the receipt's source with its owned CLI
   main processes; a session counts when one of them is, or descends from,
   that child with no other CLI main process in between;
   Codex keeps its rollout open, a Grok TUI
   keeps `events.jsonl` of its session directory open;
3. exactly one such session, whose native scope the index verifies
   (Claude `sessionId`, Codex `session_meta.payload.id`, Grok
   `summary.json` `info.id`), is bound through the ordinary durable bind
   path with `method: "process"`; the evidence note (`cli_pids=[…] under
   host child pid … ; native record …`) and `bound_at` are persisted on the
   receipt's binding and audited as `lifecycle.autobind`; zero or several
   candidates leave the receipt pending (`lifecycle.autobind_ambiguous`).

The task needs the lifecycle service, the managed runtime
(`SESSIONDOCK_PTYHOST_DIR`) and native process evidence; on an unsupported
platform the receipt stays pending, the terminal stays usable, and a minute
after the page's first Enter it says only that the record has not been found.
`POST /api/term/bind` is the sole operator path; the page offers no binding
or release control. The legacy page follows a confirmed binding exactly like a
declared Claude identity: it releases its launch-kind console, opens the native
session and reclaims the console through the native lease. Legacy `tests/lifecycle_http_suite.py`
keeps its fake-CLI expectations by injecting an empty synthetic process tree.

Eight HTTP response permits cover queued work through serialization and retained
response bodies, including never-polled responses. Serialization runs off the
async reactor, and bodies stream in 32 KiB chunks while
retaining their permits.
The service has independent bounded work admission. Oversized/busy responses
fail explicitly; no unbounded background request queue is created.

## Launch identity kinds

`POST /api/term/create` accepts `resume_uid` and the receipt reports
`launch_kind` (`fixed`, `new_pending`, `new_assigned`, `resume`) with
`declared_sid`/`declared_uid`. A Claude new session is `new_assigned`: the
server mints one UUID v4 before the Prepared receipt is persisted (replays and
restarts reuse the same `--session-id`) and passes `--meta {source, launch_id,
instance_id, sid}` so the runtime catalog can match the native record as soon
as it appears. Grok new sessions are also `new_assigned`; Codex new sessions
are `new_pending` (`--meta` without an identity) and still use native binding;
`POST /api/term/bind` on a
launch with a declared identity is 409 `launch_identity_declared`. Resume
(`resume_uid` or takeover) resolves the full native SID from the frozen
session inventory's verified scope — never from a client-supplied SID or path —
substitutes `{sid}` as one argument and adds `sid`+`uid` to `--meta`; the
instance appears as that UID's session row and the legacy console uses the
`guarded_v1` native claim. One managed instance per native identity: a running
one is reused (`action:"reused"`), an uncertain and not cancelled one is 409
`launch_conflict`. The index also verifies a Grok main session's
scope (`summary.json` `info.id`), so Grok resume, stop, binding and the
recycle bin's run state work like Codex. Additional codes: 400
`invalid_launch_request` / `launch_adapter` (no unique entry for the source, or
no unique resume-capable profile) / `invalid_launch` (invalid cwd) /
`terminal_size`; 404 unknown `resume_uid`; 409 `launch_source` /
`launch_cwd_unknown`. Ledger schema 3→4 migrates strictly (old records gain
`spec.launch={"kind":"fixed"}` and `session_id:null`; old files that already
carry the new fields fail closed).

Takeover and stop use the same native process discovery model. They
match SID/file identity and ancestry, never ports or fuzzy command-line text.

## Stopping a session

`POST /api/session/stop {uid, request_id?}` accepts unrelated dictionary
members; a valid optional request ID only adds managed-stop replay.
The UID is checked against the index's native catalog (404 `session_missing`),
then resolved through a fresh guarded runtime observation and native process
scan. Managed targets use `guarded_v1`; external targets use exact native
SID/file/process ancestry. Capability `session_stop` is true when terminal
and the lifecycle service are configured; otherwise the route is the generic
501 `not_implemented`.

Managed instances use guarded host escalation:

1. Fresh exact status; an instance that already exited answers
   `stage:"already_exited"` without sending anything.
2. `C-d` through the guarded input path (`guarded_v1` keys), then up to 1.2 s
   polling the exact instance for exit; repeated once (timeout=2.4) —
   sends exactly two EOFs, no Ctrl-C.
   Exit here is `stage:"graceful"`.
3. Otherwise the existing guarded stop: for a launch receipt this is the
   durable `term/kill` path (persist cancel → retire the launch-derived
   leases → one host-performed `kill` = SIGHUP to the foreground group →
   exact exit evidence within the 3 s cancel budget); a guarded instance
   without a receipt (a host started elsewhere with `--meta`) receives the
   same host `kill` through its bound guard. Exit is `stage:"stopped"`.
4. No exit within those bounds is `stage:"uncertain"` (`stopped:false`): the
   cancel flag stays on the receipt, nothing is retried.
   External sessions instead use the TERM/KILL sequence on exactly
   attributed native process IDs.

Reply: `200 {ok:true, stopped, tmux, external_detection:"proc_scan", ...}`.
The `{ok, stopped, tmux}` keys keep their meaning. Managed responses add
the guarded-stop stage and receipt identity. Refusals include 409
`run_state_unknown` when a
managed record names the session but its host is unreachable, duplicated or
its identity unverifiable (nothing is sent), and 400 `invalid_stop_request`.
Stop ignores unrelated request fields and observes current state
again on every call.
No operator flag is required — confirmation is the browser dialog.

Browser leases: stop neither needs nor fails on a browser
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
confirmed stop drops the UID from `S.live`.

Validation: `cargo test -p sessiondock --test session_stop --locked`
(temporary ptyhost + fake CLIs: graceful stop with `/api/live` exited,
`already_exited`, replay, request conflict, external stop, 400/404, and a
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
receipt/launch/instance tuple. Rust mode does not run the pending-to-native
resolution/automatic discard loop, and directory completion does not scan the
filesystem. Native binding uses the separate [one-time host protocol](host-native-binding.md)
and [durable service](lifecycle-binding.md). `POST /api/term/bind` names a
native UID; the server resolves its actual SID from one verified snapshot. It
rejects browser SID/source overrides, missing confirmation, conflicts and
subagents. Old immutable-metadata Grok consoles remain separate.

Receipts add nullable `binding` with source, actual SID/UID, state and method
`operator` or `process` (see above); `native_binding` is unbound, intent,
confirmed or uncertain. Neither association is a native CLI receipt. Existing
pending sockets stay attached without auto-upgrade. The page closes its own
pending socket when it follows a confirmed binding or opens the native console
of the same host; the native console then requests a new lease. Cross-kind
force is rejected.
Native claim and attach require fresh lifecycle authorization for launch-derived
targets, even after Web restart with a new in-memory registry. Cancelled launches
cannot regain native control simply because the host remains alive.

## Acceptance evidence

`tests/lifecycle_browser.py` builds a temporary corpus and private launcher
configuration for a fixed free shell. It uses the real legacy create dialog,
pending xterm keyboard, Web stop/restart, sidebar navigation and mobile
discard of an unpersisted launch. It checks same-request replay creates only one shell, conflicting specs
are rejected, pending status is explicit, no reliable-send composer is enabled,
cancellation does not reclaim ownership, an unpersisted pending header offers
delete (discard) rather than stop, and native fixture bytes are unchanged.
Host/client, store and coordinator tests cover the deeper identity, persistence,
lost response, shutdown, capacity and crash-recovery boundaries separately.

The `--native-binding` browser variant additionally binds through
`POST /api/term/bind`, checks that the page follows the binding by itself
(pending socket released, native console claimed through the native lease),
and cancels it from a temporary browser storage. Its free shell deliberately ignores HUP: a subsequent Web restart must
still deny native claim while guarded host status proves the child is alive.
