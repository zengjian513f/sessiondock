# Delivery state and Python behavior

The Rust backend follows the frozen Python send queue and native confirmation
behavior. [delivery-engine.md](delivery-engine.md) describes orchestration and
[delivery-store.md](delivery-store.md) describes persistence.

## Audit of the Python baseline

The frozen Python `send_queue.py`, `send_protocol.py`, `send_audit.py` and native bridge modules define submission and confirmation behavior. Text matching, time boundaries, native queue records and completion observations follow those implementations.

## Interface and state transitions

`delivery::codex::Machine` and `delivery::claude::Machine` retain request identity and state. The engine persists each transition before releasing the related terminal action. A transport return leads to native observation; the receipt changes when that observation establishes queueing, acceptance or completion.

## Draft and injection contracts the future adapter must enforce

The executor inspects the current composer and uses the existing terminal lease. Native draft conflicts and transport errors remain visible. The prompt passes through the host input protocol, with its actual 1 MiB payload limit and 4 MiB wire framing.

## Native acknowledgment and fixed confirmation fences

Confirmation examines the selected native session after the send boundary. Matching prompt text uses Python’s end trimming and timing behavior. Text evidence accepted by Python can confirm the Rust receipt, including records without a turn ID. Task completion is tracked separately from acceptance.

## Persistence, ordering, privacy and bounded resources

The request ID identifies a submission and its full payload. An identical retry reuses that receipt; a changed payload under the same ID conflicts. Ledger persistence and response serialization have no added row, byte or JSON-depth quota. Media is opaque preview metadata; uploaded file references are already present in the prompt.

## Validation

Use the consolidated workspace, HTTP, differential and browser suites after implementation is complete. Ordinary suites use synthetic histories and fake CLIs. Real CLI tests use isolated configuration and the models required by AGENTS.md. Record current results in the release or change record; put only unfinished follow-up work in `TODO.md`.

## Claude: independent pure queue/turn domain

Claude has its own receipt states and native queue observations. It shares the persistence engine with Codex while keeping source-specific confirmation and interruption behavior.

### What the current Python Claude bridge actually proves

`AskUserQuestion` hooks describe a tool dialog. Prompt acceptance instead comes from the native queue or transcript using Python’s matching rules. A tool hook alone does not establish that a particular submitted prompt was accepted.

### Four different milestones must remain distinct

The receipt distinguishes persisted submission, terminal transport, native acceptance and turn completion. Native queueing and interruption are displayed using Claude’s corresponding states.

### Drafts, interruption and parent/child boundaries

Operations resolve the selected native view and its terminal association. Parent and child identities come from that view. Interruption follows the active native turn and preserves the composer behavior of the Python backend.

### Recovery, cancellation, dismissal and remaining integration

Restart reconciles pending terminal work with the native session. Retrying persistence does not repeat a paste or Enter. Cancellation and dismissal retain the receipt identity used to recognize duplicate requests. Storage errors remain separate from an uncertain CLI outcome.

## Live question cards and approvals (WP-G)

Python shows a *live* question card on the conversation page while the CLI's
dialog is open and its answer is not yet in the records: Claude's
`AskUserQuestion` through the hook bridge (`agenthub/claude_bridge.py`,
`server._claude_prompt`) and Codex's command approval, which exists only on
the TUI screen (`codex_bridge.approval_prompt`, `_codex_prompt`). The
frontend (`renderConversationTail`, `questionNode`) renders the `prompt`
field, answers through `/api/term/send` keys under the page's console lease
(`cli.js` decides the keys per CLI) and falls back to `pendingHistoryQuestion`
for Codex `request_user_input` records. Batch 44 WP-G implements the backend
half in `crates/sessiondock/src/bridge/`:

- **`sessiondock claude-hook [--state-dir DIR]`** (`bridge::claude`) is the
  hook command. It reads one hook payload from stdin, writes
  `<state dir>/claude-prompts/<session_id>.json` (directory 0700, file 0600,
  temp file + rename, unchanged content not rewritten) and always exits 0
  silently — a bridge failure must never block the CLI's tool call. Semantics
  are `claude_bridge.py`'s: `PreToolUse` of `AskUserQuestion` writes
  `{version:1, source:"claude", id:<tool_use_id>, created, state:"waiting",
  questions:[{header, question, options:[{label, description}], multiple}]}`;
  `PostToolUse` / `PostToolUseFailure` with the same `tool_use_id` set
  `state` `submitted` / `cancelled` plus `settled` but keep the file (Claude
  appends the `tool_use`/`tool_result` records seconds later; deleting here
  would show a false "Working"); `SessionStart` / `SessionEnd` delete it. The
  session id must match `^[A-Za-z0-9_-]{6,128}$`; the state directory comes
  from `--state-dir` or `SESSIONDOCK_STATE_DIR` (the settings file carries this path as an explicit argument).
- **`sessiondock --write-bridge-settings FILE`** writes the hooks-only
  Claude settings document (0600): `PreToolUse` / `PostToolUse` /
  `PostToolUseFailure` with matcher `AskUserQuestion` plus `SessionStart` /
  `SessionEnd`, each an exec-form command hook
  `{"type":"command","command":"<this binary>","args":["claude-hook",
  "--state-dir","<SESSIONDOCK_STATE_DIR>"]}` (no shell, no
  `permissionDecision`). The Claude launch profile passes it as
  `args: ["--settings", FILE]` ([lifecycle-launcher.md](lifecycle-launcher.md)).
- **`bridge::codex::approval_prompt(screen)`** is the strict port of
  `codex_bridge.approval_prompt`: the last `Would you like to …?` heading,
  the exact `Press enter to confirm or esc to cancel` footer with nothing
  printed after it, at least two numbered options each ending in `(y)`,
  `(p)` or `(esc)`, both a positive and the negative path; the result is
  `{id:"codex-approval:<sha256[:16]>", state:"waiting", kind:"approval",
  questions:[{header:"命令审批[ · env]", question, options:[{label, key,
  description?}], multiple:false}]}` with the same labels, keys and digest
  input as Python (`tests` pin the ids Python computes). Codex launch
  profiles carry Python's `--enable default_mode_request_user_input -c
  suppress_unstable_features_warning=true`.
- **`bridge::live::LivePrompts`** (in `AppState.prompts`) fills the field:
  `/api/messages` of every main view carries `prompt` (JSON `null` unless a
  card is live; agent views have no field, like Python's `if not agent`),
  `/api/watch` message packets carry it for Claude/Codex main sessions, and
  the watch loop polls every 0.5 s — the Claude file stamp
  `(mtime_ns, size)`, or a lease-free screen capture (last 80 joined
  scrollback rows, `codex_bridge` capture semantics) of the unique managed
  instance bound to the UID (looked up through the shared runtime
  observation, at most once a second while missing, dropped when a capture
  fails) — and emits `{"prompt_only": true, "prompt": …}` when only that
  changed. The clearing rule is Python's `_claude_prompt`: a card whose
  `tool_use_id` matches an `answer`/`tool_result` record in the packet's
  messages is deleted and reported `null`, even if the hook state is still
  `waiting` (Claude 2.1.226 does not always emit `PostToolUseFailure` on
  Esc, but the failed `tool_result` always lands).
- Answering stays the page's own keyboard input: `/api/term/send` under its
  console lease. For that the Codex mnemonics (`y`, `p`) and menu digits
  (`1`–`9`) are accepted as single literal keys
  ([terminal-input.md](terminal-input.md)); nothing in the bridge writes to a
  CLI, takes a lease or acknowledges a receipt.

Validation: `cargo test -p sessiondock --lib bridge:: terminal::input --locked`
(hook file semantics, settings shape, Python-derived approval ids, literal
keys), `python3 tests/claude_prompt_suite.py` (the subcommand as a process,
`--write-bridge-settings`, `/api/messages` and SSE `prompt`/`prompt_only`
packets, clearing by the native answer, restart with the directory present),
and the real-CLI suites `tests/prompt_claude_real.py` (haiku, `--settings`
bridge, `--tools AskUserQuestion`; `--browser` clicks the card on the real
page) and `tests/prompt_codex_real.py` (luna, `-a on-request` + read-only sandbox, 拒绝 through
`Escape`), both cheapest configuration per AGENTS.md.
