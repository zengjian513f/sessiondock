# Kiro sessions

Kiro CLI (`kiro-cli`) supports two native session storage formats under `~/.kiro/sessions` (configurable via `SESSIONDOCK_KIRO_ROOT`):

1. **V3 KAS format (`--agent-engine=v3`, primary)**:
   - `<root>/<workspace_hash>/<session_id>/session.json` + `messages.jsonl` (or direct `<root>/<session_id>/session.json` + `messages.jsonl`).
   - `session.json` records `id` / `sessionId`, `cwd` / `workspaceDirectory`, `title`, `createdAt`, `updatedAt`, `modelId`, `effortLevel`, and optional `parentSessionId` / `createdReason` for subagents.
   - `messages.jsonl` records append-only turn items (`user`, `assistant`, `tool_call`, `tool_result`, `session_start`, `checkpoint`, `compact`).
2. **V2 legacy CLI format (`cli/<uuid>.json`, backward-compatible read model)**:
   - `<root>/cli/<uuid>.json` (+ optional `<uuid>.jsonl` and `<uuid>.lock`).
   - Contains `session_id`, `cwd`, `history` (embedded user/assistant/tool turns), and `session_state` (`rts_model_state`, `conversation_metadata.user_turn_metadatas`).

SessionDock indexes both formats under `source: "kiro"` with native UID `kiro:<hash>` and native SID `<session_id>`.

## Dual-Mode Only Architecture

Kiro sessions provide **two modes only**:

1. **ACP SDK mode (`#a-sdk`)**: Native JSON-RPC 2.0 conversation view hosted by `sdkhost` ([Kiro 原生 SDK 桥接](kiro-sdk.md)).
2. **CLI mode (`#a-console` / `#a-term`)**: Interactive xterm.js PTY terminal hosted by `ptyhost` running `kiro-cli chat --agent-engine=v3` (`--resume-id <sid>` on resume).

The screen-parsed PTY chat mode (`#a-chat`) is intentionally omitted and hidden for Kiro sessions.

## Mutual Exclusion

CLI mode and ACP SDK mode operate on the same native Kiro session (`SID`, `UID`, `cwd`, history) and cannot run simultaneously:

- When a Kiro CLI PTY session is active (via `ptyhost` binding, `/proc` scan, or a live `<sid>.lock` whose PID is verified alive in `/proc`), SDK `open` and `turn` reject with `kiro_sdk_session_busy`. Stale `.lock` files left behind by a terminated process are cleaned up automatically before `session/load`.
- When a Kiro ACP SDK session is active, `/api/sessions/spawn` and `/api/sessions/send` reject with `409 Conflict` (`sdk_session_busy`).
- Stopping the active mode allows immediate switching to the other mode on the same session.
