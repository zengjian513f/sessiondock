# Codex SDK mode

The SDK view is a third session view beside 对话 and 终端 for top-level Codex
sessions. It drives the session through the Codex app-server JSON-RPC protocol
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
  flight and no open request for 10 minutes is unsubscribed
  (`thread/unsubscribe`). When the connection closes every attached page gets
  `bridge/reset` and reconnects; the next attach opens a new connection.

### Terminal coexistence

Two clients share one live thread only when both use the same app-server.
SessionDock-managed Codex terminals are launched with `-c` overrides, which
makes Codex 0.163 run an in-process runtime instead of the shared daemon. While
such a CLI process is alive (`S.live`), the SDK view shows a persistent notice
and asks for confirmation before the first new turn; it does not block the
send. A TUI started as plain `codex` / `codex resume` (or with
`--remote unix://`) shares the daemon and sees SDK turns live.

## HTTP

All routes take the page's opaque session `uid` (query or body), so a Hub
forwards them to the owning node. Only a top-level `codex` session resolves;
other sources and subagent views answer `400 codex_sdk_unsupported`. The
mode is declared by `capabilities.codex_sdk` (node: endpoint configured; Hub:
always, per-node failures degrade per request with `501 codex_sdk_disabled`).

| Route | Effect |
| --- | --- |
| POST `/api/codex-sdk/open` `{uid}` | Connect/resume; returns `{thread_id, raw: summary}` |
| GET `/api/codex-sdk/events?uid=` | SSE: first a `snapshot`, then every thread event |
| POST `/api/codex-sdk/turn` `{uid, text, model?, effort?}` | `turn/start`; while a turn is in flight, `turn/steer` with `expectedTurnId` |
| POST `/api/codex-sdk/interrupt` `{uid}` | `turn/interrupt` for the in-flight turn (`409 codex_sdk_idle` without one) |
| POST `/api/codex-sdk/respond` `{uid, key, result}` | JSON-RPC result for one open server request (`409 codex_sdk_request_gone` once resolved or from an older connection) |
| GET `/api/codex-sdk/models?uid=` | `model/list` (cached 5 minutes per node) |

Errors: `503 codex_sdk_unavailable` (socket unreachable / start failed),
`502 codex_sdk_disconnected` (connection lost or 120 s timeout; the call may
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

## View

- `#a-sdk` (chat bubble with brackets) sits after 终端 in the session view
  group and is shown only for top-level Codex sessions when the capability is
  declared. It opens through `SessionContentView` like 信息/资源; the header
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
- The status line shows the live-terminal notice, the last turn error, the
  waiting state and context usage (`last.totalTokens / modelContextWindow`).

## Not yet covered

Starting a new thread from the SDK view, image/file attachments
(`localImage`), review, compaction, fork/revert, goals and the other
app-server management APIs are tracked in [TODO](../TODO.md). Validation:
[codex_sdk_browser.py](../tests/codex_sdk_browser.py) with
[codex_sdk_fixtures.py](../tests/codex_sdk_fixtures.py).
