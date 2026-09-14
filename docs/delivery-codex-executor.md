# Codex reliable send

This wires the [Codex delivery domain](delivery.md) and the
[Codex native acknowledgment adapter](delivery-codex-ack.md) into the same
executor, terminal driver and four HTTP routes built for Claude
([delivery-executor.md](delivery-executor.md)). A Codex session on a managed
instance is now a send target; Grok remains `501 delivery_source_unsupported`.
Nothing in this batch confirms from screen text: a receipt becomes acknowledged
only from a causal matching user record in the session's rollout. A turn ID is
used for completion when present, but is not required to confirm the input.

## One executor, two providers

`delivery/executor.rs` resolves the target's `Provider` (`Claude` | `Codex`)
from the opened view's native scope (`SessionStore::native_scope`, never a
display name) and shares everything that is not domain-specific:

| Shared | Provider-specific |
|---|---|
| admission (`workers`), per-UID `tokio::sync::Mutex` serialization, shutdown cancellation | the domain commands (`claude::Command` vs `codex::Command`) and receipts |
| `ManagedResolver` (unique guard-capable managed host whose verified native association names the UID; launch origin authorized through the lifecycle binding) | the composer model: `driver::inspect_for(ComposerKind::Claude | Codex, capture)` |
| lease borrow/claim (`sessiondock-delivery-executor` page or the request's own console lease) | the native adapter (`claude_adapter` JSONL inputs vs `codex_adapter::observe` over `ViewSnapshot::native_tail`) |
| `overwrite_draft` (consent before any durable write), `prepare` (paste + independent full-text observation), `enter` (frame recheck, `Enter`) | the outbox projection (`claude_outbox` / `codex_outbox`) and the messages |
| the tracker tick | the replay schedule: Claude's in-memory `confirm_timeout` clock vs Codex's `codex_adapter::ReplayClock` |

The Claude path, its unit tests (`delivery::executor::tests`) and the
`delivery_send` integration suite are unchanged in behaviour.

## Codex flow: persist before every write, two steps, no idle wait

```
POST /api/session/send (Codex uid)
  → replay lookup (same request_id + text = status read; different text = 400 request_conflict)
  → overwrite_draft: composer editing without the exact token → 409 {draft_conflict, draft_token}, no receipt
  → persist Submit (CheckingDraft)            → InspectComposer
  → capture screen + the frozen view's checkpoint (`native_checkpoint()`, before any paste)
  → persist DraftObserved:
       Unknown  → FailedBeforeWrite (Python "failed", attempts 0, 重试 = re-inspect; nothing pasted)
       Editing (unapproved) → DraftConflict (409 with the token)
       Empty / approved → PrepareInFlight, confirmation = watch = that checkpoint   → InjectPrepare
  → clear an approved draft (C-u C-k, verified empty), paste, wait until the composer shows the whole text
  → persist Prepared (EnterInFlight + enter_operation)                                → InjectEnter
  → recheck the frame, press Enter
  → persist EnterFinished → Uncertain (Python "failed", attempts 1, "发送结果待核对；禁止自动重试")
  → tracker: codex_adapter::observe after the fixed boundary → NativeAck (PossibleTextMatch) → Acknowledged
       → task_complete of that turn → Completed (hidden like Acknowledged)
```

The executor
keeps the domain's two persisted steps and the independent paste observation,
so a crash between paste and Enter restores as `Uncertain` and is never
re-injected, and a paste the composer does not visibly hold (for example a
paste Codex collapses into a `[Pasted Content …]` placeholder) stays
`Uncertain` **without** Enter. Codex owns follow-up queueing while a turn is
running (`docs/delivery.md`): the executor never waits for an idle footer,
a second send while the first is unconfirmed captures its own fence and is
pasted immediately (`codex_follow_up_is_pasted_immediately_and_both_confirm`).

`Submit` while another receipt of the same UID is still inside a live
prepare/Enter boundary is the domain's `WrongState` → `409 delivery_busy`; with
the per-UID lock this only happens across a crash, and recovery turns those
rows into `Uncertain` first.

## Composer inspection: `codex_bridge.composer_state`, ported

`driver::inspect_codex` reproduces the rules on the host screen
capture (`capture_screen`, styled, with cursor):

- ANSI is stripped and Codex 0.154's braille **particle glyphs**
  (U+2800–U+28FF, painted in RGB colour, never dim) are
  blanked before locating the block and ignored when deciding whether the
  composer holds text, so a particle padding row is never a draft block and an
  empty composer with particles in its blank cells is `empty`.
- The live block is the nonblank block immediately above a recognized status
  footer: `Ready` with a `Context N% used` line within three rows (the whole
  wrapped bar excluded), the final `model · cwd` line, or the dim
  `esc again to edit previous message` rewind hint (dim styling required).
  Without a footer the host cursor must sit inside the block (or one/two blank
  rows above a bottom-row composer at column marker+2).
- The block must start with `›` or `»`; content after the marker that is not
  whitespace, not a particle and **not dim** means `editing`, otherwise
  `empty` (the rotating placeholder is dim). Menus/approval prompts are
  `unknown`.
- `text` is the bright, particle-free text after the marker (rows joined with
  newlines; compared whitespace-insensitively like Claude's soft wraps).
- A lagging capture (`lag > 0`) is `unknown`, never idle; `dropped` moving
  between capture and write aborts the prepare/Enter, as for Claude.

Draft consent is identical to Claude: `draft-status` → `{draft_state}` plus
`{draft_conflict:true, draft_token}` while editing (token = the fingerprint
`sha256(x\0y\0screen)`); `send`/`retry` without the exact token → `409`
and no receipt; with it the approved draft is cleared (`C-u C-k`) and verified
empty before the paste.

## Native acknowledgment: causal text matching

The tracker polls every attempted Codex receipt that is `Uncertain` and not
dismissed. For each tick `codex_adapter::ReplayClock::plan(policy,
delivered_ms, now)` decides:

| Plan | Action |
|---|---|
| `Watch` | `InspectNative{replay:false}`; the read is skipped when the committed end has not moved past the watch cursor, otherwise `observe` from the **fixed** boundary |
| `Replay` (overdue ≥ 8 s, at most every 8 s) | `InspectNative{replay:true}`; always `observe` from the fixed boundary |
| `Expired` (one hour) | polling stops; the row keeps its state and issue and is not retryable |

`delivered_ms` is the receipt's durable `created_ms` (the Submit clock, never
later than the Enter clock). The physical fence is the real boundary; the
timestamp check only
skips records stamped before the request existed.

The boundary is `Boundary { confirmation: receipt.confirmation, sequence:
enter_operation.revision, delivered_ms }`, i.e. `Boundary::capture(&snapshot
.native_checkpoint(), …)` taken from the frozen view at inspection time,
before the paste and the Enter. `observe` runs on the bounded reader worker
against a fresh `SessionStore::snapshot(uid, "")`; no ad-hoc file read.

Outcome mapping:

| Adapter outcome | Executor |
|---|---|
| `Possible(m)` | `NativeAck` with `m.evidence` and `PossibleTextMatch` → `Acknowledged`; when the record has a turn ID and the adapter already saw that turn's `task_complete`/`turn_aborted`, `NativeCompletion` follows → `Completed`/`Stopped` |
| `Absent` | `AdvanceWatch` to the observation's current cursor |
| `Uncertain(CheckpointMismatch)` | one `NativeReset` annotation (the fence never moves); nothing retried |
| `Uncertain(ForeignScope | UnmatchableText | Unreadable)` | nothing; the receipt stays `Uncertain` |

The adapter produces `PossibleTextMatch`. The Machine accepts it only after it
matches the receipt's fixed confirmation cursor, source identity, real-user
record, exact media metadata and end-trimmed text.
The executor held the write lease for the instance, verified
the composer was empty (or cleared the one approved draft), captured the
physical fence from the frozen view before the paste, independently observed
the complete text in the composer, rechecked the frame, and pressed Enter
itself. The only human input record that can then appear after that fence
with exactly that text (end-trimmed) is this delivery. The Machine refuses a record whose start or record ID already
acknowledged another receipt. A nonempty turn ID also cannot be consumed twice;
it is never derived from a neighbouring `task_started`.

What stays uncertain: a swallowed line, an ambiguous transport result, a
rewritten/truncated rollout, and anything past the one-hour window.

## HTTP: the same four routes, the Codex bodies

Under `outbox:true` (ledger + terminal transport configured) the routes accept
a Codex UID with the Claude bodies; codes follow the Codex handlers where
they differ from Claude's:

| Route | Codex-specific behaviour |
|---|---|
| `send` | `item.state` is the Codex projection: `queued`/`failed` with `attempts 0` before a write, `failed` with `attempts 1` and `error "发送结果待核对；禁止自动重试"` once pasted (legacy `codexNeedsInspection`: "终端写入待核对", 检查终端/移除). Replay of a confirmed/hidden ID → `state:"confirmed"`, no text. `name` mismatch → `409 terminal_unlinked "Codex 终端会话未连接…"`. |
| `draft-status` | identical (`empty`/`editing`+token/`unknown`) |
| `outbox/retry` | only a `failed` row with `attempts 0` (`FailedBeforeWrite`, or a `DraftConflict` with its token) re-inspects; anything attempted → `409 "消息已经写入终端或仍在确认，禁止重复发送"`; unknown → `404 "待发送消息不存在"`. The console draft is probed before the retry. |
| `outbox/discard` | A pre-write row is discarded (tombstone kept); an attempted row, including an in-flight prepare/Enter, is **dismissed** (hidden, state, operation revision and dedup identity kept); authorized callbacks may settle once, and dismissal never resends; a missing/already removed ID → `200 {ok, uid, outbox…}` (idempotent, unlike Claude's 404). |

`GET /api/session/outbox` is unchanged. The legacy composer needs no change:
`sendToSession` already treats Claude and Codex the same way and the Codex
row labels/actions gate on the projected `failed`+`attempts`.

## Domain addition: `codex::Command::Dismiss`

`delivery/codex.rs` gains `Receipt.dismissed` (serde default, only valid on an
attempted row) and `Command::Dismiss`: a pre-write row behaves
like `Discard`; an attempted row, including in-flight prepare/Enter, is hidden
without changing its state or revision. Authorized one-shot callbacks keep
their tokens and may settle once; a dismissed/hidden row replays. `Discard` keeps
its batch-6 semantics and tests.

## Validation

- Unit: `cargo test -p sessiondock --lib delivery::driver` (six Codex
  composer tests: placeholder + particles, bright draft, wrapped rows,
  Ready/Context and rewind footers, footerless cursor rules, menus/lag/busy),
  `--lib delivery::codex` (`Dismiss`), `--lib delivery::executor::codex_tests`
  (persist → paste → Enter → causal `PossibleTextMatch` → Completed with the fence
  captured before the paste; request-ID replay/conflict; draft consent;
  ownership/unlinked/unknown session; swallowed line uncertain + retry
  refused + restart without re-injection + dismiss hides + tombstone replay +
  a fresh identical request confirms on its own record; no-turn-ID and
  duplicate native records confirm in row order; ambiguous Enter and unknown composer
  (pre-write failure, manual retry); lagging capture never idle; tracking
  window; immediate follow-up).
- Integration (real router + temporary ptyhost + launcher + ledger, fake Codex
  CLI only): `cargo test -p sessiondock --test delivery_send_codex` —
  resume through `resume_args ["resume","{sid}"]`; the driver reads the
  fake's particle/placeholder composer as empty; send → `failed`/`attempts 1`
  → confirmed and the persisted association is `PossibleTextMatch`;
  replay; uploaded media path delivery; wrong name/unknown session; console draft consent under
  a page lease; slow TUI (Working footer) confirms late; swallowed line stays
  uncertain, retry refused, Web restart does not re-inject, discard hides and
  a second discard is `200`, later resend confirms; `--no-turn-id` and
  `--duplicate` confirm by the first causal text record; the four routes are `501`
  without the ledger/transport.
- Browser: `python3 tests/send_codex_browser.py` (desktop + 390 px) — resume
  via the console button, compose through the real legacy composer against
  the fake Codex CLI, "终端写入待核对" row replaced by the native message over
  SSE, server ledger emptied by the tracker, a second page refused with
  `terminal_ownership`, a 390 px send through the server-claimed lease.
- Real CLI: `python3 tests/send_codex_real.py` runs only with `--include-real` or direct invocation, using
  the cheapest configuration only (`gpt-5.6-luna`,
  `-c model_reasoning_effort="low"`, temporary `CODEX_HOME` reusing
  `auth.json` read-only with its own `config.toml` trusting the throwaway
  cwd, proxy variables passed through, `codex exec` login probe creating the
  session the TUI resumes, one prompt, model asserted from
  `turn_context.payload.model`, instance killed). It skips with the CLI's own
  error text when `codex` is absent, cannot authenticate or the account is
  over its limit, and when the read model lists the real session
  `supported:false`. On 2026-09-12 a first run skipped on a temporary
  usage limit; the second run passed against the
  real Codex 0.154 TUI: the driver read its particle/placeholder composer as
  `empty`, the send was pasted and Entered, the receipt was confirmed from
  the real rollout's causal user record (`PossibleTextMatch`), the model was asserted
  from `turn_context`, and the batch-33 read model listed the session
  `supported:true` with `migration_warnings` for `world_state`,
  `item_completed` and `token_usage_record`.

The fake Codex CLI (`tests/fake_codex_cli.py`, `# run_validation: skip`)
renders the Codex layout (particle rows, `›` with a dim placeholder, `model ·
cwd` footer, `Working … esc to interrupt` while delayed) and appends the real
rollout shape (`turn_context`, `task_started`, user `response_item` with the
passthrough turn ID, `user_message`, optional assistant reply,
`task_complete`) to the resumed rollout under `SESSIONDOCK_TEST_CODEX_ROOT`;
knobs `--delay`, `--swallow N`, `--no-turn-id`, `--duplicate`, `--reply`,
`--busy-footer`.

## What stays uncertain or unsupported

- Long pastes that Codex collapses into a placeholder are never Entered by
  the executor (the domain requires the full text to be observed); the row
  stays `Uncertain` with the text left in the composer for the user.
- Identical prompts follow row order: the first causal native record
  confirms the first matching receipt and cannot confirm another.
- An unknown composer (approval prompt, menu, lagging capture) is a
  pre-write failure with a manual 重试, not a background re-dispatch.
- Expired tracking is silent for Codex (the domain has no timeout command).
- Attachment paths are submitted in the prompt and their preview metadata is
  retained in the outbox. Stop/interrupt, rename and compact acknowledgment
  remain out of scope; Grok is not a send target.
