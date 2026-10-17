# Codex SDK mode

The SDK view is the default view for top-level Codex sessions. 对话 and 终端
remain available during their retirement transition. It drives the session through the Codex app-server JSON-RPC protocol
(the interface behind the official Codex SDKs and IDE clients) instead of
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
  connection; later pages reuse it. A thread with no attached page, no turn in
  flight and no open request for 10 minutes is unsubscribed (except an
  unpersisted new thread, whose live instance is its only copy)
  (`thread/unsubscribe`). When the connection closes every attached page gets
  `bridge/reset` and reconnects; the next attach opens a new connection.

### Terminal coexistence

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

## HTTP

Existing-session routes take the page's opaque session `uid` (query or body), so a Hub
forwards them to the owning node. Create takes `_node` on a Hub; model discovery
also accepts `node` without an existing session. Only a top-level `codex` session resolves;
other sources and subagent views answer `400 codex_sdk_unsupported`. The
mode is declared by `capabilities.codex_sdk` (node: endpoint configured; Hub:
always, per-node failures degrade per request with `501 codex_sdk_disabled`).

| Route | Effect |
| --- | --- |
| POST `/api/codex-sdk/create` `{request_id, cwd, create_cwd?, model?, effort?}` | `thread/start`; returns `{uid, session, cwd, thread_id, raw}`; a missing directory returns `needs_create` before starting anything |
| POST `/api/codex-sdk/discard` `{uid}` | Unsubscribe and remove an unpersisted create receipt; native history is preserved if it appeared meanwhile |
| POST `/api/codex-sdk/open` `{uid}` | Connect/resume; returns `{thread_id, raw: summary}` |
| GET `/api/codex-sdk/events?uid=` | SSE: first a `snapshot`, then every thread event |
| POST `/api/codex-sdk/turn` `{uid, text, model?, effort?}` | `turn/start`; while a turn is in flight, `turn/steer` with `expectedTurnId` |
| POST `/api/codex-sdk/interrupt` `{uid}` | `turn/interrupt` for the in-flight turn (`409 codex_sdk_idle` without one) |
| POST `/api/codex-sdk/respond` `{uid, key, result}` | JSON-RPC result for one open server request (`409 codex_sdk_request_gone` once resolved or from an older connection) |
| GET `/api/codex-sdk/models?uid=` | `model/list` (cached 5 minutes per node) |
| GET `/api/codex-sdk/limits?uid=` | `account/rateLimits/read`: windows, credits and earned reset cards (account-wide) |
| POST `/api/codex-sdk/reset-credit` `{uid, credit_id?, idempotency_key}` | `account/rateLimitResetCredit/consume` for the chosen card, then a fresh read; returns `{outcome, limits}` |

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

## Creation

DELTA (SDK default and lazy creation): choosing Codex in 新建会话 uses
`thread/start` directly, without a PTY or `api/term/create`, on an SDK-enabled
node. Model discovery uses `model/list`; the selected effort rides in
`config.model_reasoning_effort`. Other sources and SDK-disabled nodes retain
their existing launcher flow. Continuation launches retain their existing
message/attachment contract during the transition.

Like the CLI, a new thread initially has no native rollout. Its in-memory
create receipt supplies an empty SDK page and a sidebar row through
`api/term/list.sdk_sessions`, including after a browser reload. The node keeps
that thread loaded. First SEND calls `turn/start`; Codex owns the first native
write. The UID derives from the daemon's rollout path and stays identical when
the normal inventory replaces the receipt. No placeholder file, forced name
or synthetic history is written. The receipt and same-`request_id` retry cache
live for this SessionDock process; retries, including a lost HTTP response,
reuse the completed attempt rather than starting another thread. A node restart
before first SEND does not retain these unpersisted receipts.

## View

- `#a-sdk` (chat bubble with brackets) sits first in the session view
  group and is shown only for top-level Codex sessions when the capability is
  declared (per owning node on the Hub). Opening or reloading a Codex main
  session selects SDK by default. An explicit switch to 对话/终端 lasts for
  the current page; background refreshes do not switch it back. It opens through `SessionContentView` like 信息/资源; the header
  stays, the message list, composer and terminal pane are hidden.
- History: `fetchMessages(uid)` rendered by the same `appendMessages`, so
  persisted turns look exactly like the message view. While a turn is in
  flight, history is cut before the last user message equal to the in-flight
  turn's user message, and the turn is shown from SDK items instead. After
  `turn/completed` the history is reloaded and SDK items that are now on disk
  are removed, so each turn appears once.
- Live items map to message objects for `msgNode`: user/assistant/thinking
  bubbles, command/file/MCP/web-search tool cards (in-progress cards show
  `运行中…`).
- Server requests render as question cards with the native choices
  (`availableDecisions` for commands, accept/acceptForSession/decline/cancel
  for files, grant turn/session or deny for permissions, radio/text answers for
  user input, accept/decline/cancel for MCP elicitations).
- Composer: same look as the message composer. Enter sends (Shift+Enter
  newline, not on mobile); the button reads 发送 or, during a turn, 追加
  (steer). 停止 interrupts. Model and effort selectors come from `model/list`;
  an empty choice keeps the thread's current setting, a choice applies to the
  started turn and, per app-server semantics, to later turns.
- The status line shows the last turn error, the rate-limit-reached notice and the
  waiting state.
- Usage row alongside the model/effort pickers, one `.sdk-bar` progress bar per
  gauge showing the **used** share (`100%` = full, red at ≥ 80 %): `5h` and `7d`
  from the account bucket (`rateLimitsByLimitId.codex`, else `rateLimits`),
  labelled by `windowDurationMins` (300 → 5h, 10080 → 7d; an account with a
  single window shows only that one), and 上下文 (`last.totalTokens /
  modelContextWindow`). Tooltips give used/remaining percent, reset time and
  token counts. `Credits N` is `credits.balance` (`无限` when unlimited). The
  row is hidden while nothing is known.
- 重置卡: shown when `rateLimitResetCredits.availableCount > 0`; a selector lists
  the available cards (title, expiry) and 兑换 asks for confirmation, then posts the
  chosen `credit_id` with an idempotency key that is reused for retries of the
  same card until it succeeds. The result (`reset`, `alreadyRedeemed`,
  `nothingToReset`, `noCredit`) is shown for a few seconds and the bars are
  replaced from the fresh read. Limits load on open, after every turn and on
  `account/rateLimits/updated`, which the node forwards to every attached page
  (that notification carries one bucket only; reset cards refresh on the reads).

## Not yet covered

Image/file attachments
(`localImage`), review, compaction, fork/revert, goals and the other
app-server management APIs are tracked in [TODO](../TODO.md). Validation:
[codex_sdk_browser.py](../tests/codex_sdk_browser.py) with
[codex_sdk_fixtures.py](../tests/codex_sdk_fixtures.py), and
[codex_sdk_create_browser.py](../tests/codex_sdk_create_browser.py) for node/Hub
creation, lazy persistence, reload and default view selection.
