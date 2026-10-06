# Explicit local creation and pending consoles

The backend connects creation receipts, configured CLI profiles, guarded host
status, browser leases and the pending view. Configured services support creation,
resume, external CLI takeover, stop and native association; default startup
launches nothing. Conversation SEND has its own [contract](conversation.md),
and public authentication belongs to the reverse proxy.

[Agy](agy.md) uses `new_pending`. The service reads its explicit
native catalog/transcript into a private mirror and associates native rows by
CLI conversation DB fd evidence under the host child process, never cwd alone.
Composer and selected native menus use the same CHECK/SEND path; stopped-session
`--conversation <sid>` continuation is covered by the real CLI browser test.

New sessions open the conversation page. The configured CLI and ptyhost still
start on the backend; the browser does not claim or attach a console until the
user switches to it. Reopening a session may restore the user’s saved terminal
choice. Saved pending input remains editable even when its CLI has exited.
Reload restores a selected pending session from `term/list` after the native
catalog loads, even before its first native history record exists. A delayed
list must not override a newer selection or the mobile list view. Restoration
uses the existing pending opener, including its saved terminal choice and
confirmed native-binding transition.

A partial hub terminal list is authoritative only for nodes that answered.
For failed nodes the page retains its known receipts and console identities,
including a create receipt newer than the hub cache. Missing rows or saved
drafts on those nodes mean uncertain status, not exit or deletion. Confirmed
exit/failure remains final; a successful later list replaces retained rows.
`tests/hub_pending_state_browser.py` covers creation during a node outage,
draft preservation across reload, recovery, and a subsequent confirmed exit.

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
| POST `/api/term/create` | `source`, `cwd`; optional `request_id`, `create_cwd`, `model`, `effort`, `cols`, `rows` | Persist/replay a creation receipt, request confirmation before creating a missing directory, start at most once, verify guarded readiness |
| GET `/api/term/new-status` | `record_id`, `instance_id` | Refresh status of that exact recorded instance |
| POST `/api/term/kill` | `record_id`, `instance_id` | Persist cancellation, retire input authority, for a shell receipt EOF (`C-d`) then up to 1.2 s for the shell's own exit, guarded stop, verify exit |
| POST `/api/term/discard` | `record_id`, `instance_id` | Drop a finished (Exited/Failed) or durably cancelled receipt from `term/list.pending` together with the input the conversation service retained for it (a draft shared with a native session still in the catalog stays; an alias to a trashed UID does not keep it). A `new_assigned` launch also moves the matching empty native session into trash, so Grok's startup `summary.json` cannot rebuild the row; a shell receipt's recordings are deleted with it; 409 `launch_not_finished` while the instance may still run; the receipt stays queryable |
| POST `/api/session/stop` | `uid` | Stop a managed instance through guarded host control or terminate process IDs attributed to this exact external native session; an already-stopped session succeeds with `stopped:false` |
| POST `/api/term/bind` | `record_id`, `instance_id`, `uid`, `operator_confirmed: true` | Validate a real main-session scope, persist one immutable intent, bind and observe |
| GET `/api/term/list` | optional `force=1` | Native-bound sessions plus separate launch-only pending receipts, `sources` and `resume_sources` per source (false when the source has no unique CLI or its CLI is not installed, [below](#installed-clis)) and `backends`; served from a 2 s per-view response cache that every mutation above drops at once, `force=1` bypasses it ([liveness.md](liveness.md#response-caches)) |
| POST `/api/term/takeover` | `uid`; optional `force`, `cols`, `rows` | Reuse a managed console, start a stopped session, or return `needs_confirm` for a running external CLI; confirmed force terminates only exact native-session process matches before resume |
| GET `/api/term/complete-dir` | `path`, optional `limit` | Absolute/`~/` completion without a configured root gate; directory symlinks are followed, ≤50 (default 24) |
| GET `/api/term/models` | `source` | The source's one CLI profile's model catalog for the new-session picker: `{models:[{id,name,efforts,default_effort?}], efforts, default_model?}`; empty when the source has no unique profile or the CLI keeps no list ([below](#model-and-effort)) |
| POST `/api/term/backend` | `backend` | `{ok, backend:"ptyhost", backends}` for `ptyhost`/`host`, not persisted; `tmux` is 400 `backend_unsupported`, unknown 400 `backend_unknown` |

The limited legacy diagnostics `_build`, `_trace_id`, `_page_id` are accepted but
confer no authority and are not forwarded. `cols`/`rows` are validated display
hints; the terminal's attach/resize owns its actual size. They do not change the
immutable launch specification. Missing working directories return
`needs_create`; `create_cwd:true` creates the confirmed absolute path. Executable,
argv, environment, SID, and legacy adapter fields cannot choose the command.

The first sidebar/detail action is **启动会话 / 停止会话**. A stopped native
session starts through the same exact-UID `/api/term/takeover` resume flow as the
console, with a launch receipt and duplicate-click protection. Running sessions
show stop in the same position; deletion remains a separate action after tree
migration and is unavailable while an ordinary session is running. Controls
refresh from native liveness and managed terminal observations. Sources without
a resume-capable profile, and exited SSH/pending receipts without a native
session to resume, show a disabled start action with the reason.
Running AI launches without a native session record show a disabled stop action
in both the detail header and sidebar menu, explaining that they cannot resume
after stopping. Bulk stop excludes these launches. They can still be used or
explicitly discarded; once a native record appears, normal stop/resume applies.
SSH receipts retain their existing stop and final-screen behavior.

### Installed CLIs

A configured profile does not prove its CLI exists: nodes launch through a
shell wrapper (`with-zshrc grok`) whose command resolves only after the rc
file sets `PATH`. The service therefore runs every agent profile once with
`--version` at startup and again every 5 minutes, in parallel, each bounded to
10 s. Only positive evidence of absence counts: the executable cannot be
started, or it exits 127/126 (a shell's "command not found" / "not
executable"). A timeout, any other exit status or a probe not yet answered
leaves the source available. An absent CLI turns that source's `sources` and
`resume_sources` false, so the new-session picker, console takeover and the
bug-report dialog disable it with a reason instead of failing at launch.

Reading native history does not configure a CLI for activation. In
`BUG-20261003-103542-d86b9f`, a Windows node could read a Codex Desktop
rollout but its launcher contained only Claude and Grok profiles. The first
console state already lacked Codex creation/resume capability; no takeover
request or lifecycle receipt followed. The installed standalone Codex executable
was added as an explicit profile after a version and configuration check, with
the previous launcher backed up. Missing source capability now explains the
CLI/launcher prerequisite before the generic unlinked-terminal message.

### Model and effort

A new CLI session may carry the picker's `model` and `effort`; empty or absent
means the CLI's own default, and a resume or takeover keeps the session's own
setting. They are persisted in the receipt's spec (omitted when absent, so older
receipts and binaries are unaffected) and passed as single argument values:
Claude `--model X --effort Y`, Codex `-m X -c model_reasoning_effort="Y"`, Grok
`-m X --reasoning-effort Y`, Agy `--model X` with optional `--effort Y`. OpenCode's TUI has no model option, so the model
(`provider/model`, split at the first `/`) goes into the pre-created session's
`session.create` body; OpenCode takes no effort, and a shell neither. A value
that is empty, longer than 200 bytes, contains whitespace or control characters,
or starts with `-` is 400 `launch_model`; the model itself is not checked against
the catalog, the CLI decides.

The catalog comes from each CLI's own data, read as its child would see it
(profile environment over the service's): Codex `$CODEX_HOME/models_cache.json`
(when `client_version` matches the installed CLI; otherwise its read-only
`debug models --bundled` catalog, with a 3 s command timeout; older CLIs without
that command fall back to the cache). This avoids using an older CLI’s catalog
after an upgrade or when versions share a home. Only `visibility: list` models
are offered, with `supported_reasoning_levels`; `default_model` is
`config.toml`'s top-level `model`; the catalog's default effort uses the top-level
`model_reasoning_effort` before the cache's `default_reasoning_level`, including
when a model is explicitly selected. The new-session and bug-report pickers
remember effort per source and model in browser storage, shared across machines
and both dialogs. A model without a remembered supported effort starts at `high`;
if it does not support `high`, the picker uses its supported catalog default or
first supported level. Switching models restores that model's effort rather
than carrying the previous model's level. There is no effort placeholder option;
CLIs without effort support keep the control disabled and omit the override.
Agy is an exception to the effort default rule above: it offers an empty
“CLI 默认” option, omitting `--effort` unless the user selects a level. Its
catalog reports no per-model support/default effort; the CLI decides compatibility.
An unknown default model stays unselected and its override is omitted, letting
the CLI use its own setting. Model
menus have no separate default entry. Claude reads the profile's
`CLAUDE_CONFIG_DIR/settings.json` (or `HOME/.claude/settings.json`) for the saved
model and per-model effort; the profile's `ANTHROPIC_MODEL` and
`CLAUDE_CODE_EFFORT_LEVEL` take precedence. If the selected model is not one of
the fixed aliases, it appears as an additional row. A previously chosen picker
model value takes precedence over these defaults (model choice remains per
machine and source; old machine-specific effort choices are ignored).
Grok `$GROK_HOME/models_cache.json`
(non-hidden, `reasoning_efforts`), OpenCode `opencode models` (bounded to 15 s;
one `provider/model` per line), Claude its fixed aliases `fable`, `opus`,
`sonnet`, `haiku` with `low`…`max`. The page asks again every time the dialog
opens; the model menu gains a search box above ten models. Agy runs `agy models`
in the profile environment with a 15 s bound, requires successful exit and
parses stdout `id\tdisplay name` TSV. It reports no default model; supported
flag choices are low, medium, high, xhigh, max, except that a model whose ID
ends in an effort level (optionally followed by `-thinking`) is that variant
and lists no separate flag efforts in the raw API catalog. Both page pickers
group its variants into one base-model row, keep the effort control visible,
and disable/gray levels absent from the catalog. Their launch request maps the
model/effort selection back to the listed native ID and omits `effort`.
Groups with a listed base-model ID also offer “CLI 默认”; standalone models
without variant metadata retain the optional effort flag. Saved native variant
IDs restore the base model and its effort. Agy and OpenCode
catalogs are cached per profile in memory: the CLI probe warms absent or
10-minute-stale entries in the background, a stale entry is still served while
one background refresh runs, and an empty listing never replaces a cached one.
The first request joins an ongoing startup listing; failed listings also respect
the refresh interval.
The CLI may independently use
a different synthetic catalog model for its background title generator; that
request is separate from the interactive model selected by `--model`.

Source selects exactly one interactive configured CLI or the `shell` terminal.
The legacy source picker advertises only this unambiguous subset and labels
`shell` as SSH. `term/list.sources.shell` advertises shell creation; resume
sources are the AI CLIs (Claude, Codex, Grok, OpenCode, Agy). Shell receipts use fixed argv, stay in the
terminal list while running, and support the same guarded attach, reconnect,
kill and discard as other launch receipts without native binding.
Shell receipts stay in the sidebar after exit or launch failure until explicitly
discarded; the minimal lifecycle receipt stays queryable for idempotency. An
exited shell's console shows its [final screen](terminal-final-screen.md)
read-only, or explains that none exists. Discard also deletes it.
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
`bound_at`. `GET /api/term/list` excludes discarded receipts from `pending`.
It considers the complete receipt ledger before filtering; historical receipts
do not limit which new launches or bound terminals appear in this snapshot.
Shell receipts stay listed until discarded, including after Exited/Failed.
Finished AI receipts stay for at most 600 s after `finished_at`; a finished
AI receipt migrated from an older ledger without that time is archived at
once. Archived receipts still answer `term/new-status`.

## Automatic binding by process evidence

A new Codex/Grok pane is associated with its native record once the
first prompt is on disk (`_new_session_status`: same cwd, not in the
`before` set, and for Codex the rollout held by a process of the pane). The
Rust service keeps the process evidence and the `before` set — never cwd or
file name — in `lifecycle::autobind`, a background task that runs while a
`Running` receipt with launch kind `new_pending` has no binding:

1. one fresh guarded host observation names the exact instance and its
   child process (`summary.pid`);
2. the native process scan of [liveness.md](liveness.md)
   pairs every indexed session of the receipt's source with its owned CLI
   main processes; a session counts when one of them is, or descends from,
   that child with no other CLI main process in between;
   Codex keeps its rollout open, a Grok TUI
   keeps `events.jsonl` of its session directory open; Agy holds
   `<SESSIONDOCK_AGY_HOME>/conversations/<sid>.db`, and the scan keeps its
   identity under an `agy:` namespace;
3. a session whose native `created` is more than 2 s before the receipt's
   `created_at` existed before the launch and is never a candidate: a CLI
   may hold another session's record open briefly (Codex reads old rollouts
   for its resume picker) without owning it;
4. exactly one such session, whose native scope the index verifies
   (Claude `sessionId`, Codex `session_meta.payload.id`, Grok
   `summary.json` `info.id`, Agy mirror `summary.json` `session.id`), is bound through the ordinary durable bind
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
session and reclaims the console through the native lease.

HTTP response permits (`max(read_workers * 2, 8)`) cover queued work through serialization and retained
response bodies, including never-polled responses. Serialization runs off the
async reactor, and bodies stream in 32 KiB chunks while
retaining their permits.
The service has independent work admission. Full pools wait for permits; valid
responses are serialized and streamed without an additional size rejection.

## Launch identity kinds

`POST /api/term/create` accepts `resume_uid` and the receipt reports
`launch_kind` (`fixed`, `new_pending`, `new_assigned`, `resume`) with
`declared_sid`/`declared_uid`. A Claude new session is `new_assigned`: the
server mints one UUID v4 before the Prepared receipt is persisted (replays and
restarts reuse the same `--session-id`) and passes `--meta {source, launch_id,
instance_id, sid}` so the runtime catalog can match the native record as soon
as it appears. Grok new sessions are also `new_assigned`; Codex new sessions
are `new_pending` (`--meta` without an identity) and still use native binding;
Agy new sessions are also `new_pending`, with resume argv `--conversation <sid>`.
Its native binding and stopped-session continuation are covered by the real CLI
browser test ([Agy](agy.md));
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

The page confirms a stop (header action, sidebar menu, multi-select) unless
the session is known to be idle: its turn state
([read-model.md](read-model.md) `turn`, the open session's CLI screen) says the
latest turn is finished and no subagent runs. A turning, question-waiting or
unknown-state session (Grok, OpenCode, older nodes without `turn`) still asks;
a multi-select skips the question only when every target is idle.

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
launch-derived leases exactly as `term/kill` does. Different instances stop
concurrently within lifecycle admission capacity; the EOF and exit waits overlap.
Stops and cancellations of the same instance stay serialized. Other lifecycle
writes remain ordering barriers, and list refreshes wait for the active stops
to finish so they cannot invalidate in-flight cancellation evidence.

Legacy (`session_stop:true`): the header "停止会话"/"删除会话" action and the
sidebar menu treat a session as stoppable when `S.live` has it **or** a
managed instance with this UID is listed (`S.live` is not polled under
`live:false`); the request carries a `request_id`; the outcome or the
server's refusal (unmanaged/external explanation, unknown host state) is
shown inline in `#session-stop-notice` instead of a bare alert, and a
confirmed stop drops the UID from `S.live`.

The sidebar multi-select toolbar also offers “停止”, with the count of selected
stoppable sessions. One confirmation starts concurrent stop requests using the
browser's Settings → Features → stop concurrency preference (default six);
ended selections are skipped. The button updates after each result as
“已停止 3/44”; failures and uncertain outcomes do not increment the stopped count
and appear in expandable details beside the button, without a bulk-stop toast.
The completed progress stays until the selection changes. Selection controls
are disabled while the batch runs. Pending launches use their existing
`term/kill` receipt and instance identity (plus node routing on the Hub).
Stopping preserves records, drafts and selection for a later explicit delete.

Validation: `python3 tests/session_stop_browser.py` (desktop + 390 px).

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
pending console keyboard, Web stop/restart, sidebar navigation and mobile
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


## Freeze a diagnostic scene (Linux)

The main session's action menu offers **冻结现场** for a verified managed
Linux instance. It becomes **恢复运行** while paused. The separate report
button stays available: freeze first, then report the problem. The header
orders viewing controls before the paired freeze/report controls, then move/copy
and stop/delete. Freeze and report stay together when folded into the menu. A paused session
shows a pause glyph instead of the running dot at the top right of its sidebar
and header icon; unread counts remain visible. The frozen scene is a centered
overlay inside the selected session pane: one line, **会话已暂停**, followed by
a play button to resume. It uses the shared popup styling. A theme-specific shade
dims the pane in light and dark mode. The overlay persists while frozen, disappears when another session is
selected, and is never shown on the mobile session list. It does not use the
global notification stack.
Freezing does
not create a report or start an investigation automatically.

`POST /api/session/freeze` takes `{uid, instance_id, frozen: true|false}`.
It requires lifecycle configuration and a unique verified instance; a stale
instance is `409 freeze_instance_changed`. A failed OS operation is
`503 freeze_failed`. The response includes `ok`, `frozen`, `instance_id` and
`process_count`. `capabilities.session_freeze` is advertised only on Linux
with lifecycle and terminal transport enabled. Supported `term/list` rows
carry `frozen`, which also gates the Hub button per node. Unsupported nodes and
sessions without a verified running instance retain a gray pause button with a
hover explanation; it cannot submit a freeze request. The same state appears in
the folded action menu.

The server pins each process incarnation with a Linux pidfd, checks its start
time, stops the CLI with SIGSTOP and recursively stops its descendants.
Parents stop before their children are enumerated, including children created
by other threads. Resume sends SIGCONT to descendants before the CLI.
A failed freeze resumes only the processes newly stopped by that request.
The independent ptyhost stays responsive to capture and attach; existing host
protocols and sessions need no restart. Detached processes already reparented
outside the CLI tree are outside this operation.

`term/list` reports `frozen` from the verified CLI's actual OS state, so a page
refresh or Web service restart retains the recovery control. The pause retains
memory, terminal output and native files; it is not a durable snapshot across
machine restart. Network timeouts and external services continue to advance.
Stopping a frozen session first resumes the verified tree, then uses the
ordinary guarded EOF/stop sequence so descendants can exit normally.
Do not type into a frozen CLI: PTY input can queue and be consumed on resume.

Validation: `python3 tests/session_freeze_browser.py` uses a temporary host,
a fake CLI with a ticking child and headless Chromium; it exercises freeze,
idempotent freeze, wrong-instance refusal, report dialog access, refresh,
resume authenticated Hub/mobile controls and stopping directly while frozen, checking real
OS states and progress. Freeze state joins the browser report snapshot and
`session.freeze` audit events include the instance and process count.
