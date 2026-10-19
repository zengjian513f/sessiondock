# Codex SDK mode

The SDK view is the default view for Codex sessions and their subagents. 对话 and 终端
remain available during their retirement transition. It drives the session through the Codex app-server JSON-RPC protocol
(the interface used by rich Codex clients such as the IDE extension) instead of
typing into a terminal. Persisted history still comes from the ordinary read
model, so its bubbles are identical to the message view; only the in-flight
turn, approvals and questions come from the app-server.

References: [Codex SDK](https://learn.chatgpt.com/docs/codex-sdk),
[Codex app-server](https://learn.chatgpt.com/docs/app-server). Verified
against Codex CLI 0.163.0.

## Runtime topology

```text
browser ─HTTP/SSE─▶ SessionDock node ─WebSocket over Unix socket─▶ codex app-server daemon
                                                            ▲
                                       default `codex` TUI ─┘ (same daemon, same thread)
```

- The node keeps **one** connection to the shared daemon
  (`codex app-server --listen unix:// --managed-daemon`), the same server a
  default `codex` TUI starts or reuses. The daemon owns model context, turns,
  approvals and rollout writes; it outlives SessionDock restarts.
- Endpoint: unset `SESSIONDOCK_CODEX_APP_SERVER` uses
  `<parent of SESSIONDOCK_CODEX_ROOT>/app-server-control/app-server-control.sock`.
  If that socket is not reachable, the node runs
  `codex app-server daemon start` with that `CODEX_HOME` once per connect
  attempt. An explicit socket path is never started; `off` disables the mode.
  Non-Unix nodes report `codex_sdk: false`.
- Handshake: `initialize` with `clientInfo.name = "sessiondock"` and
  `capabilities.experimentalApi = true`, then `initialized`.
- A thread is resumed (`thread/resume`, `excludeTurns: true`) at most once per
  connection; later pages reuse it. If it is already active without a local
  turn ID, the bridge reads the newest turn (`thread/turns/list`, `limit: 1`,
  descending, full items) before publishing its snapshot. This restores
  explicit steer, interrupt and all replies/tools for work started in a TUI
  before subscription. Summary items omit replies/tools and cannot replace
  the active turn's native history (BUG-20261018-091356-69d2ff). Notifications
  received during recovery keep their newer bodies in the native item order.
  A turn completed during recovery is never resurrected. A thread with no attached page, no turn in
  flight and no open request for 10 minutes is unsubscribed (except an
  unpersisted new thread, whose live instance is its only copy)
  (`thread/unsubscribe`). When the connection closes every attached page gets
  `bridge/reset` and reconnects; the next attach opens a new connection.
  A permanently closed browser EventSource also schedules a reconnect; closing
  or switching the view cancels that timer. Reconnection retains the unsent draft
  and recovers the server snapshot without submitting another turn.
  A transient network/server failure during reconnect keeps retrying; a browser
  regression injects a 503 before allowing the real snapshot stream to recover.
  BUG-20261018-040613-165ffa follow-up: browser audit selected another session
  from 04:20:53 to 04:31:18 UTC, while this task's native record contains 73 tool
  calls in that interval. This case did not kill execution on view detach.
  The browser regression closes the SDK view during a live turn, generates
  progress while detached and restores it without resume/unsubscribe/interrupt.

### Terminal coexistence

DELTA (SDK → PTY): when this bridge has the exact thread loaded on a live
connection, and the configured Codex CLI profile joins that same Unix socket,
opening 终端 starts or reuses a TUI client of the shared daemon. It does not
ask to replace an external process or signal that daemon, even if process
discovery finds it holding the rollout. An independent writer and a terminal
lease held by another browser still follow their existing confirmation flow.
DELTA (BUG-20261018-030737-e115f2, BUG-20261018-153253-e86c26): a thread loaded
on the configured shared daemon is a managed session and shows the existing blue marker
in the sidebar and header, including idle and newly created threads. Merely
selecting SDK or failing to resume does not establish ownership. Disconnect
or unsubscribe removes the bridge's subscription evidence; a read-only
`thread/loaded/list` observation still identifies threads retained by the daemon,
including before the first click and after a service restart. Observation never
starts the daemon, resumes threads or grants terminal coexistence authority;
unload or failed observation withdraws discovered evidence. An external process
alone stays green.
`/api/live.sdk_uids` carries this evidence through the Hub and live events,
separately from `tmux_uids`, which continues to mean console ownership. SDK
connection snapshots also reconcile live state, so opening/reconnecting between
global observer samples cannot leave a stale green marker. An attached main-thread
SDK snapshot is also direct display evidence, independent of delayed/partial fleet
reads; close, stream error or bridge reset withdraws that local evidence. A live
CLI observation with an exact managed terminal binding likewise stays blue when
the fleet response is missing it. These display fallbacks do not alter process
control or claim that a failed SDK open succeeded (follow-up screenshot to the
same report). Browser coverage:
[codex_sdk_status_browser.py](../tests/codex_sdk_status_browser.py),
[codex_sdk_create_browser.py](../tests/codex_sdk_create_browser.py),
[codex_sdk_terminal_browser.py](../tests/codex_sdk_terminal_browser.py).

The separate SDK-stall observation reported with BUG-20261018-122932-0e7a3c
does not establish execution waiting for PTY: the delivery receipt was sent,
native input was recorded at 12:29:40.997 UTC, assistant output at
12:29:52.909 and a tool call at 12:29:53.898. The browser clicked terminal
at 12:29:53.936; ptyhost recorded its first attach at 12:29:54.269. Browser
snapshots already showed SDK and working before the click. The audit did not
record SDK packets, so exact SDK rendering latency remains undetermined.
The terminal browser regression now requires SDK output and an approval card
before any private PTY exists, then completes that same turn after switching
views, with no second submission.

Two clients share one live thread only when both use the same app-server, and
Codex 0.163 gives each of these a different runtime: a plain `codex` /
`codex resume` joins the shared daemon, but any `-c` override makes it run an
in-process runtime that nobody else can reach (and that holds the rollout's
writer lock, so the daemon answers `already has an active writer`).

SessionDock-launched Codex terminals therefore join the daemon: the launcher
appends `--remote unix://` (`unix://<path>` for an explicit endpoint) and
`-C <launch directory>` after its existing arguments, for new, resume and
report-worker launches ([lifecycle-launcher.md](lifecycle-launcher.md)). Verified
on 0.163.0: `-m` and `-c model_reasoning_effort` still apply under `--remote`;
without `-C` the daemon starts the thread in *its own* directory, so `-C` is
required; a TUI told to `--remote` never starts a daemon and exits with a
connection error when none runs, so before each Codex launch the launcher
connects, else runs `codex app-server daemon start` (bounded 30 s) and, if the
daemon still is not reachable, launches the old way without `--remote`.

The default endpoint applies only when the configured Codex sessions root's
parent is the `CODEX_HOME` the CLI itself resolves (`$CODEX_HOME`, else
`~/.codex`); an isolated home (tests, other roots) is never joined. Only
terminals started after this change share; an already running terminal keeps
its own runtime until it is restarted, and the SDK view reports
`409 codex_sdk_thread_busy` ("正被另一个独立运行的 Codex 进程占用") if such a
process holds the thread. Consequences of a shared daemon: commands the agent
runs inherit the *daemon's* environment (the one SessionDock or the first TUI
had when it started the daemon), not each terminal's; and restarting or
updating the daemon ends every attached terminal.

An SDK view rejected with `codex_sdk_thread_busy` retries only its open request
with backoff from 1 second up to 30 seconds, so an independent writer's exit
allows the same page to connect. The error stays visible until open succeeds.
Clicking the selected SDK button or reselecting the session retries a failed
open immediately; a healthy connection is reused. Closing or switching the
view cancels pending retries and open reads. Recovery preserves history and
the unsent draft and never replays SEND. BUG-20261018-030340-bc6f60 showed a
successful host exit at `03:02:59.251Z` (code 0), an Exited lifecycle receipt
and browser `running: false`, followed by an SDK click and same-session
selection that both returned without retrying the failed open. The audit did
not capture the original SDK RPC error; the attachment shows the writer-lock
notice. [Recovery browser coverage](../tests/codex_sdk_recovery_browser.py)
exercises native lock rejection, a real private PTY stop and subsequent SDK SEND.

## HTTP

Existing-session routes take the page's opaque session `uid` (query or body), so a Hub
forwards them to the owning node. Create takes `_node` on a Hub; model discovery
also accepts `node` without an existing session. Both main and subagent `codex` views resolve through `native_scope(uid, agent)`;
the optional `agent` query/body field selects an exact owned child and its own
native thread ID. Other sources answer `400 codex_sdk_unsupported`. The
mode is declared by `capabilities.codex_sdk` (node: endpoint configured; Hub:
always, per-node failures degrade per request with `501 codex_sdk_disabled`).

| Route | Effect |
| --- | --- |
| POST `/api/codex-sdk/create` `{request_id, cwd, create_cwd?, model?, effort?}` | `thread/start`; returns `{uid, session, cwd, thread_id, raw}`; a missing directory returns `needs_create` before starting anything |
| POST `/api/codex-sdk/discard` `{uid}` | Unsubscribe and remove an unpersisted create receipt; native history is preserved if it appeared meanwhile |
| POST `/api/codex-sdk/open` `{uid}` | Connect/resume; returns `{thread_id, raw: summary}` |
| GET `/api/codex-sdk/events?uid=` | SSE: first a `snapshot`, then every thread event |
| POST `/api/codex-sdk/turn` `{uid, request_id?, text?, attachments?, quotes?, model?, effort?}` | Server-owned FIFO input; starts immediately when idle, otherwise waits; repeated `request_id` returns the same submission; staged attachments are resolved once |
| POST `/api/codex-sdk/queue` `{uid, agent?, id, action}` | `cancel`, `retry`, or explicit `steer` of one queued input (`expectedTurnId`); no implicit steering |
| POST `/api/codex-sdk/interrupt` `{uid}` | `turn/interrupt` for the in-flight turn (`409 codex_sdk_idle` without one) |
| POST `/api/codex-sdk/respond` `{uid, key, result}` | JSON-RPC result for one open server request (`409 codex_sdk_request_gone` once resolved or from an older connection) |
| GET `/api/codex-sdk/models?uid=` | `model/list` (cached 5 minutes per node) |
| GET `/api/codex-sdk/limits?uid=` | `account/rateLimits/read`: windows, credits and earned reset cards (account-wide) |
| POST `/api/codex-sdk/reset-credit` `{uid, credit_id?, idempotency_key}` | `account/rateLimitResetCredit/consume` for the chosen card, then a fresh read; returns `{outcome, limits}` |
| POST `/api/codex-sdk/turns` `{uid, cursor?}` | `thread/turns/list`, newest first, with summary items and native pagination |
| POST `/api/codex-sdk/rename` `{uid, agent?, name}` | `thread/name/set`; native name validation and persistence, then refresh the inventory and pending creation title |
| POST `/api/codex-sdk/fork` `{uid, last_turn_id?, before_turn_id?}` | `thread/fork`; optional native child revert before a selected turn; returns the new identity and navigable session |
| POST `/api/codex-sdk/revert` `{uid, before_turn_id}` | `thread/revert`; removes the selected turn and later conversation, without reverting workspace files |
| POST `/api/codex-sdk/compact` `{uid}` | `thread/compact/start`; progress uses ordinary turn/item events |
| POST `/api/codex-sdk/review` `{uid, target}` | `review/start` with inline delivery; uncommitted changes, base branch, commit or custom instructions |
| POST `/api/codex-sdk/controls/settings` `{uid, mode?, approval_policy?, sandbox_policy?, service_tier?}` | `thread/settings/update`; only explicit changes are sent, effective settings arrive through notifications |
| POST `/api/codex-sdk/controls/shell` `{uid, command}` | `thread/shellCommand`, the CLI's direct `!command` semantics; output uses command execution items |

Errors: `503 codex_sdk_unavailable` (socket unreachable / start failed),
`409 codex_sdk_thread_busy` (the rollout's writer lock is held by an independent
runtime), `502 codex_sdk_disconnected` (connection lost or 120 s timeout; the call may
have run), `502 codex_sdk_rejected` (JSON-RPC error from the app-server, its
message forwarded).

### Event stream

Each SSE `sdk` event is `{"seq", "method", "raw"}`; `raw` carries the native
app-server params unchanged (it is an opaque key for the Hub's namespace
rewrite). The first event is `{"type": "snapshot", "raw": {summary, turn,
items, requests}}`: the turn in flight with its items as accumulated so far
(deltas applied) and the open server requests. Snapshot and subscription are
taken under one lock, so no event is missed or duplicated. `heartbeat` events
keep proxies alive; a lagging page receives `bridge/reset` and reloads.

Server requests are forwarded as their own method with
`raw = {key, params}`, where `key` is the JSON spelling of the request id.
Handled: `item/commandExecution/requestApproval`,
`item/fileChange/requestApproval`, `item/tool/requestUserInput`,
`item/permissions/requestApproval`, `mcpServer/elicitation/request`. Any
other request, or one whose thread no page has opened, is refused at once with
JSON-RPC `-32601`, so the daemon never waits on an answer nobody can give.
`serverRequest/resolved` removes the card for every page (and for any other
client of the daemon).

MCP form answers retain their entered JSON on a parse error and show the error
inside the request card; invalid JSON never becomes an accepted empty object.
Native `collabAgentToolCall` items use `agentsStates` for child results and
statuses, through the existing expandable tool cards.

DELTA (BUG-20261018-050342-5d4448): `request_user_input_async` emits an
`agentMessage` with `delivery: "async"` and `questions: [{title, options?}]`,
not a server request. SDK renders these as the existing radio/free-text cards;
continuing output preserves selections. Explicit 提交回答 sends the question
and chosen/free-text answer through the durable SDK queue, then explicitly
steers if the turn is still active; idle answers start a turn. A failed steer
leaves the saved answer visible in the queue. No JSON-RPC request result is
invented, and the composer draft is preserved. Reload recovers live questions
from the snapshot and historical async tool arguments after the latest user
message, deduplicated by native call/item ID; ordinary user replies supersede
those earlier cards. `accepted: true` is the tool acknowledgment, not an answer.
The report's native event at 05:02:55 UTC contained the structured questions,
while the SDK item-to-message conversion discarded them; its terminal capture
showed one queued question. Browser audit had no SDK reply attempt. Coverage:
[async question browser suite](../tests/codex_sdk_questions_browser.py).

## Creation

DELTA (SDK default and lazy creation): choosing Codex in 新建会话 uses
`thread/start` directly, without a PTY or `api/term/create`, on an SDK-enabled
node. Model discovery uses `model/list`; the selected effort rides in
`config.model_reasoning_effort`. Other sources and SDK-disabled nodes retain
their existing launcher flow. Continuation launches retain their existing
message/attachment contract during the transition.

Like the CLI, a new thread initially has no native rollout. Its private
create receipt supplies an empty SDK page and a sidebar row through
`api/term/list.sdk_sessions`, including after a browser reload. The node keeps
that thread loaded. First SEND calls `turn/start`; Codex owns the first native
write. The UID derives from the daemon's rollout path and stays identical when
the normal inventory replaces the receipt. No placeholder file, forced name
or synthetic history is written. DELTA (BUG-20261019-032629-1ba6cf): with a
configured state directory, creation receipts and completed same-`request_id`
results are saved before success in
`codex-sdk-sessions.json`. A node update/restart before first SEND preserves
the UID/SID and empty page; reattachment resumes that exact thread on the
surviving daemon. Retries, including a lost HTTP response followed by a restart,
reuse the completed attempt. Without a state directory these receipts remain
memory-only. Native handoff and explicit discard durably retire the pending row,
retaining request identity so neither a restart nor a retry resurrects it.
No restart creates another thread or replays SEND. This does not restore an
empty thread lost by the daemon itself or recover old, already-lost memory-only
receipts (BUG-20261019-032629-1ba6cf).
Browser regression: [creation retention](../tests/sdk_creation_retention_browser.py).

## View

- `#a-sdk` (chat bubble with brackets) sits first in the session view
  group and is shown for Codex main sessions and subagent views when the capability is
  declared (per owning node on the Hub). Opening or reloading a Codex main
  session or subagent selects SDK by default. An explicit switch to 对话/终端 lasts for
  the current page; background refreshes do not switch it back. It opens through `SessionContentView` like 信息/资源; the header
  stays, the message list, composer and terminal pane are hidden.
- Cold entry selects SDK immediately, before the native history response. The
  SDK connects while the existing history request is pending; its loading state
  stays inside the SDK surface instead of showing the console placeholder.
  The history consumer waits for that request's cache admission, so complete
  history is not fetched twice. A manual switch away while it loads is preserved.
  BUG-20261018-040613-165ffa established a 5.9-second history request; this fixes
  the delayed SDK entry. The collector contention fix and its measured limits
  are recorded in [history opening phases](logging.md#history-opening-phases).
- History is displayed independently of app-server open and the first SSE snapshot.
  The native JSONL head/tail window already read for `(uid, agent)` is reused on
  entry, including its omitted middle. SDK entry never silently fills this gap.
  The existing “加载中间 N 条消息” control reads finite native-history pages only
  on an explicit click; a second click stops a chain. Failures, explicit reload
  and view-generation guards share the conversation view's page implementation.
  Fallback catch-up also requests a window if the native checkpoint resets.
  The snapshot reconciles live items
  against that history without a second read. Connection errors remain visible
  alongside readable history. BUG-20261018-014447-60f001 recorded a 2,262 ms
  initial history read followed by another identical 1,068,259-byte response;
  SDK entry additionally awaited open and the snapshot. The audit did not time
  SDK RPCs, so it does not establish their share of the reported delay.
  `codex_sdk_browser.py` holds open and SSE independently, checks readable history
  on failure, and asserts a single history read on complete-cache entry.
- History: `fetchMessages(uid, {agent})` rendered by the same `planTurns` / `buildPlan`, so
  persisted turns look exactly like the message view. While a turn is in
  flight, history is cut at the matching native turn identity (falling back to
  the last matching user text for older records), and the turn is shown from
  SDK items instead. If recovery omits the user item after compaction, its
  persisted prompt stays visible. After
  `turn/completed` native history catches up incrementally without filling the
  middle, and SDK items that are now on disk
  are removed, so each turn appears once. Completed processes use the shared
  fold/expand cards and header toggle; prompts and final answers stay visible.
  In-flight SDK items remain visible. History refresh preserves individual
  process folds, and the global preference also applies after page reload.
  The active SDK turn is still recovered in full separately from the native
  history window; partial history does not discard current-turn progress.
  [SDK page coverage](../tests/codex_sdk_history_pages_browser.py) checks window-only
  entry, page/live races, retries, switching views, completion and explicit chains.
- DELTA (bubble types, BUG-20261018-040836-e49b26 follow-up): assistant prose,
  including streaming commentary, plans and final answers, renders in full.
  User input and reasoning retain height/length-based previews; tools and diffs
  keep their own previews. SDK history and live bubbles share the layout observer
  that exposes 展开全文/收起 for content below 4,000 characters but above 340 px.
  Streaming text updates reuse the bubble and disclosure button, preserving a
  click in progress and the user's expanded/collapsed choice across deltas.
  Live tool rerenders likewise preserve output expansion, raw-argument disclosure
  and wrapping choices across output deltas and item completion.
  The first fix installed the missing observer but missed replacement on each
  delta. Coverage: [bubble expansion](../tests/codex_sdk_bubbles_browser.py).
- Live items map to message objects for `msgNode`: user/assistant/thinking
  bubbles, command/file/MCP/web-search tool cards (in-progress cards show
  `运行中…`).
- Server requests render as question cards with the native choices
  (`availableDecisions` for commands, accept/acceptForSession/decline/cancel
  for files, grant turn/session or deny for permissions, radio/text answers for
  user input, accept/decline/cancel for MCP elicitations).
- DELTA (effective model/effort): the pickers select the thread's actual model
  and effort from `thread/start`/`thread/resume`, then follow
  `thread/settings/updated`, including changes made by another client. A current
  value absent from `model/list` is still displayed verbatim. Catalog defaults
  are not evidence of the current thread's settings. Unsent user choices survive
  incoming settings events; after SEND the pickers follow the effective settings again.
- DELTA (service tier, BUG-20261018-122932-0e7a3c): the composer toolbar also
  shows the current thread's service tier. Choices use the effective model's
  `model/list.serviceTiers` names, descriptions and native IDs (Fast may use
  `priority`; Ultrafast uses the advertised ID). Normal sends the native
  `default` value, explicitly disabling a faster catalog default. Selecting a
  tier immediately calls `thread/settings/update.serviceTier`, without sending
  a turn or clearing the draft. The control follows confirmed settings on open,
  notifications, reload and reconnect; rejected changes retain the actual value.
  A returned null tier displays Normal, an absent field stays unknown, and an
  unlisted current value is displayed verbatim. Catalog defaults never replace
  the current setting. Tier stays alongside model and effort where space permits;
  at the narrowest pane/scale it moves into the existing toolbar overflow menu.
  Coverage: [controls](../tests/codex_sdk_controls_browser.py) and
  [layout](../tests/codex_sdk_layout_browser.py).
- Composer: same look as the message composer. Enter sends (Shift+Enter
  newline, not on mobile); the button reads 发送 or, during a turn, 排队.
  Inputs are queued by default. Each pending bubble has 撤销 and 立即引导
  (explicit steer); failed deliveries have 重试. 停止 interrupts the current turn. Model and effort selectors come from `model/list`;
  displaying the current setting does not send an override; only a user change applies to the
  started turn and, per app-server semantics, to later turns.
- The status line shows the last turn error, the rate-limit-reached notice and the
  waiting state. Recoverable errors (`willRetry: true`) show a temporary retry
  status, cleared by resumed item activity or turn completion. Successful
  completion clears the prior error; failed completion retains its native
  `turn.error`, including after a snapshot reload. DELTA
  (BUG-20261018-233652-c23d56): when the app-server reports only
  `systemError`, the SDK status uses the latest native `task_complete.error.message`
  so a model capacity failure shows its actual explanation after reload.
  Status, model/effort/mode selectors, action icons and usage all
  share the toolbar above the input, with status left and usage/context immediately before the model picker when space permits;
  routine labels are 就绪 / 连接中 / 重连中 / 处理中 / 待批准 / 待回答 / 用量已满 /
  系统错误. Status uses at most six characters and reserves its space before optional controls,
  including on narrow panes at 125% interface scale.
  DELTA (BUG-20261019-095333-0cd031): an idle daemon does not imply that its
  preceding turn completed successfully. The bridge retains the last observed
  terminal turn's ID/status as `summary.last_turn`, including `interrupted`,
  across page reloads; starting a turn or reverting clears it. An interrupted
  turn displays 已中断 with the existing warning color. When no live outcome
  is retained, native `aborted` history and the shared CLI observation rule in
  [CLI state](cli-state.md) provide the same fallback as the conversation view,
  including a missing native ending followed by a newer ready/idle terminal.
  CLI-only changes refresh this status even when history is unchanged.
  New turns and explicit successful completions supersede older interruptions;
  waiting, compaction and errors retain their precedence. The label does not
  attribute an interruption to a user. Coverage:
  [interrupted SDK browser suite](../tests/codex_sdk_interrupted_browser.py).
  Otherwise it displays 就绪 when there is no active turn, waiting state or error.
  Only statuses with additional detail or clipped text can be clicked, tapped or
  activated with Enter to open their full detail, including the original error.
  Complete short statuses are disabled labels; returning to one closes any open
  detail. Escape or an outside click also closes it.
  DELTA: status text has a matching six-pixel dot using the shared
  [SDK status palette](native-sdk.md#配置与页面). Working, compaction,
  retry and connection dots breathe; reduced-motion preferences disable animation.
- DELTA (BUG-20261019-065056-5a1004): the toolbar always occupies one line.
  Status comes first, then usage. Model, effort, tier, mode and action controls
  move into ⋯ before taking space from usage. Remaining wide quota sets scroll
  horizontally within the usage area; they never wrap above status. Model width
  stays bounded. Compaction/fork/revert/review use header-style icon buttons with
  tooltips and accessible names; overflow entries retain their text in the ⋯ menu.
  Model, effort, tier, mode, approval and sandbox all use native system selects.
  Approval and sandbox selectors follow mode in the same overflow order when
  space runs out; their native option lists retain the full labels. Selecting
  an option applies that policy immediately; selections follow server-confirmed
  settings, including changes from another client. Failed changes retain the
  actual selection and display the error. No Apply button is needed.
  Controls move into overflow according to measured space for the entire row,
  including status and usage. Sidebar resizing and interface scaling recalculate
  available space; model and effort remain usable in ⋯. Goal controls and `/goal` are removed.
  Model/effort/mode selectors show actual values with hidden placeholders when
  not loaded. Modes are 执行模式 / 计划模式.
  Popovers use existing menu styling, stay inside the viewport and close on
  resize, Escape or outside click. Long model names truncate within their selector.
  DELTA (BUG-20261019-112331-45e344): hide ⋯ when no controls or messages remain
  in its menu. Empty groups and hidden reset cards do not count as content;
  the permissions separator appears only between two populated menu groups.
- Input queues are owned by the node, shared in SSE snapshots and `bridge/queue`
  events, and continue after a page closes. With `SESSIONDOCK_STATE_DIR` they
  persist in private `codex-sdk-queue.json`; without it they are memory-only.
  Each thread serializes submit/dispatch/cancel/steer; FIFO starts require no
  active turn or outstanding request. A completed failed turn may leave the
  daemon in `systemError`; this state also permits a new turn, as in the TUI.
  An active thread is never implicitly
  steered. Queued inputs capture model/effort and published attachment paths at
  submission. Native rejection stays visible and blocks later queued inputs until
  explicit retry or cancellation. An uncertain/disconnected native RPC is never
  automatically replayed; an in-flight entry recovered on restart is marked for
  manual verification. Submission IDs survive draft reload and lost HTTP replies.
  A cancelled receipt never consumes the composer draft. A retry that discovers
  that state preserves text, attachments and quotes, clears only that submission
  ID, and waits for another explicit SEND with a new ID. Unchanged queue entries
  retain their controls during streamed progress, including a pointer click.
  RPC acceptance is not input consumption: keep the input in `sending` until a
  matching native `item/completed` userMessage confirms it in the returned turn.
  Pre-input compaction can end or be interrupted without consuming that input.
  End/reset before confirmation retains its text and attachments as a failed entry
  for explicit verification/retry, never automatic replay. BUG-20261018-015243-e40d02
  showed sent receipts but four started/interrupted turns without native user
  messages; the browser audit did not capture SDK RPCs or compaction events.
  [Compaction regression](../tests/codex_sdk_compaction_browser.py) covers both
  successful consumption and interruption/reload/restart before consumption.
- User bubbles with a native turn identity have a ⋯ button on their left,
  vertically centered on the bubble, including expanded long messages and live items
  (BUG-20261018-043107-c69063). 回退
  reverts before that turn; 分支 forks through that turn and reverts only the new
  child before it. Both restore the selected message text into the composer,
  preserving an existing draft by appending it, and never send it automatically.
  Revert uses the existing confirmation; cancelling it preserves history and draft.
  Fork preserves the source and opens the new branch. Missing native identities
  are never guessed from display text or context records.
- The model picker fits the longest available model name, capped at 160px and
  shrinking further when the toolbar is narrow. Switching between listed models
  keeps its width stable; the selected full name remains in the tooltip.
- Usage alongside the model/effort pickers, one `.sdk-bar` progress bar per
  gauge showing the **used** share (`100%` = full, red at ≥ 80 %): `5h` and `7d`
  from the account bucket (`rateLimitsByLimitId.codex`, else `rateLimits`),
  labelled by `windowDurationMins` (300 → 5h, 10080 → 7d; an account with a
  single window shows only that one), and `Ctx` (`last.totalTokens /
  modelContextWindow`). Tooltips give used/remaining percent, reset time and
  token counts. `🪙 N` is `credits.balance` (`无限` when unlimited). The
  row is hidden while nothing is known. On resume, the node reads the latest
  native `token_count.info` from the rollout returned by app-server, so context
  appears before the next message. It uses the last request's token usage,
  never cumulative session tokens, and does not reuse pre-compaction counts.
  Live usage notifications take precedence over this initial measurement.
  DELTA (BUG-20261018-094314-a4fd77): the same tail read also recovers recorded
  Codex quota windows and their timestamp. Open's summary paints context and
  these windows immediately, before the first SSE snapshot; recorded windows
  show their capture time in the tooltip. The independent account read starts
  alongside open, replaces recorded values on success and retains them on failure.
  Fresh reads/notifications are never replaced by the recorded fallback. Missing
  native fields stay unknown; compaction still invalidates earlier context counts.
  The report had native weekly usage of 9% while the server logged a 2.6-second
  account read and a later 3.2-second rejected read. Previously the page discarded
  open's summary and waited for SSE plus the separate account round trip. Browser
  audit did not record SDK RPCs, so it cannot establish the exact first paint time.
  [SDK browser coverage](../tests/codex_sdk_browser.py) holds both the live read and
  SSE, verifies recorded gauges, then exercises failed refresh and recovery.
- 重置卡: shown when `rateLimitResetCredits.availableCount > 0`; a selector lists
  the available cards (title, expiry) and 兑换 asks for confirmation, then posts the
  chosen `credit_id` with an idempotency key that is reused for retries of the
  same card until it succeeds. The result (`reset`, `alreadyRedeemed`,
  `nothingToReset`, `noCredit`) is shown for a few seconds and the bars are
  replaced from the fresh read. Limits load on open, after every turn and on
  `account/rateLimits/updated`, which the node forwards to every attached page
  (that notification carries one bucket only; reset cards refresh on the reads).
  Failed reads retain the last known values and retry automatically with backoff
  from 1 second up to 30 seconds, without starting a model turn. Only one read is
  in flight per view; closing or switching the view cancels its read and retry.
  Rolling notifications merge available values into the current bucket; missing
  or null fields do not erase known windows, credits or account metadata. A full
  account read replaces the snapshot and can remove a window no longer present.

## Composer and conversation controls

DELTA (SDK composer): on nodes with conversation storage, SDK uses the same
server-owned draft, CAS saves, quotes, resumable uploads and attachment cards
as the transitional conversation view. Selecting, pasting or dropping files
stages them privately; only SEND publishes them into the recorded workspace.
DELTA: the app-server receives every attachment, including images, through
the same textual path references as the ordinary SessionDock composer. Native
image inputs are removed; queued inputs from older releases retain their text
references and drop image blocks before dispatch. Empty-text sends with attachments or quotes are
supported. Failed sends preserve the draft, and edits made during a successful
send are retained. Unpersisted SDK creation receipts provide identity for draft
and upload operations without creating a native rollout. Nodes without
conversation storage retain text-only SDK input.
The Hub checks the owning node's `term/list.capabilities.conversation_send`,
so a node without draft storage remains usable for text even when other nodes
support attachments. Child SDK views currently use text-only input and never
read, publish or clear their owner's draft or staged attachments.

DELTA (native controls): the operation menu starts fork, paginated turn
selection for revert, compaction and inline review. Native APIs own every
history mutation. Explicit SDK forks of main sessions persist the source's
`fork_parent_visible` preference, keeping both branches in the sidebar after
reload; the fork-chain menu can still hide the parent manually.
A `thread/reverted` notification clears obsolete live items
and reloads history for all attached pages. Compaction items use the native
`item/started` / `item/completed` phase, retained in reconnect snapshots:
DELTA: progress appears only in the status line with a purple breathing dot,
and completion turns green. SDK live/history areas do not add duplicate
compaction bubbles. Native compaction supplies no percentage, so the UI does
not invent one. The mode
selector changes 执行模式 /
计划模式; approval/sandbox choices apply directly from checked ⋯ submenus. Goal controls and `/goal` are removed. Direct shell
commands are entered in the composer only. Actual settings notifications from
another client update the controls and checked selections.
Direct shell commands follow CLI `!` behavior and run with full permissions,
outside the conversation sandbox.

DELTA (BUG-20261018-035503-dd075f): the SDK composer immediately uses the
existing tool color for `!` and `/shell` input, including restored drafts.
Tab uses the shared [conversation path completion](conversation.md#路径补全)
in ordinary text and shell input. Static command suggestions are removed.


DELTA (BUG-20261019-105205-4cfaa3): `/rename 名称` calls native
`thread/name/set`; bare `/rename` opens the shared name form. Native failure
or cancellation retains the command. Success refreshes list/detail names,
including creation receipts before the first rollout, without starting,
steering or interrupting a model turn. `thread/name/updated.threadName`
updates retained live snapshots and attached views. Covered by
[codex_sdk_rename_browser.py](../tests/codex_sdk_rename_browser.py).

The composer recognizes `/rename`, `/fork`, `/revert`, `/compact`, `/review`, `/plan`,
`/default`, `/permissions`, `/approval`, `/sandbox`, `/shell` and
`!command`; `/model`, `/effort`, `/status`, `/stop`, `/new` and `/help` use their
corresponding SDK controls. Unrecognized slash commands stay in the draft and
show an explanation rather than becoming a model prompt. These are explicit
SDK mappings, not a PTY command parser. CLI account/plugin management commands
remain outside this view.

Validation:
SDK 实时流启用时暂停额外的历史 watch，落盘历史仍由增量轮询追上；
离开 SDK 后恢复当前会话 watch。此行为避免 HTTP/1 两个标签页占满连接池后
撤销/停止请求无法发送，见[双标签页回归](../tests/sdk_multitab_browser.py)。

[codex_sdk_browser.py](../tests/codex_sdk_browser.py) with
[codex_sdk_fixtures.py](../tests/codex_sdk_fixtures.py), and
[codex_sdk_create_browser.py](../tests/codex_sdk_create_browser.py) for node/Hub
creation, lazy persistence, reload and default view selection.
Additional browser paths: [drafts and attachments](../tests/codex_sdk_compose_browser.py),
[native conversation actions](../tests/codex_sdk_actions_browser.py), and
[modes, permissions and commands](../tests/codex_sdk_controls_browser.py),
[FIFO input queues](../tests/codex_sdk_queue_browser.py), and
[user bubble actions](../tests/codex_sdk_message_actions_browser.py).
Additional recovery coverage: [cancelled submissions](../tests/codex_sdk_cancel_recovery_browser.py),
[turn errors](../tests/codex_sdk_errors_browser.py), and
[native tool, MCP form and sparse quota events](../tests/codex_sdk_protocol_browser.py).

DELTA (subagent SDK views): SDK selection, persisted history, live events, sending,
steering, interrupt and request replies follow the selected child thread. Switching
views never substitutes the owner thread. Explicit chat/terminal selection is kept
per `(uid, agent)`. Browser coverage: [codex_sdk_subagent_browser.py](../tests/codex_sdk_subagent_browser.py).
