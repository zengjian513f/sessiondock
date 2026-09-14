# Codex native acknowledgment adapter (library only)

`crates/sessiondock/src/delivery/codex_adapter.rs` classifies the native
records the checked reader committed **after a fixed submission boundary** into
the Codex domain's evidence types. It is a pure library: no HTTP route, no
scheduler, no PTY access, no screen parsing, no CLI launch, and no file access
of its own. Nothing here makes a receipt `acknowledged`. It is wired
into the shared executor ([delivery-codex-executor.md](delivery-codex-executor.md)),
which submits the adapter's causal `PossibleTextMatch` evidence directly.

The audited Python baseline is `send_queue.observe` plus the `_poll_outbox`
loop in `server.py`; see [delivery.md](delivery.md) for that audit. This
adapter keeps Python's text-and-time rule behind the server-captured native
cursor and terminal ownership checks.

## Where the records come from

The adapter reads only `ViewSnapshot::native_tail(position, head, anchor)` in
`sessions/native_tail.rs`. That method performs no I/O: it walks the frozen
events of the immutable view that `SessionStore::snapshot(uid, "")` produced
through `CheckedNative` (identity/stamp-checked, no-follow, bounded), `RawIndex`
(LF checkpoints and prefix digests) and the strict Codex provider projection.
The caller refreshes the inventory on every poll the same way history reads
do; there is no ad-hoc `std::fs::read`, no second decoder, and no re-parse
beyond the incremental one the inventory already performs.

`native_tail` validates the boundary exactly like an `/api/messages` cursor
(`valid_checkpoint`): `position` must be a committed LF checkpoint of the
current file and `head`/`anchor` must still match. It then returns every event
with a physical end strictly after `position`, each with the exact `[start,
end)` byte range of its native JSONL line (`RawIndex::record_start`). Inherited
fork prefixes have physical end zero and are never part of a tail; an
unfinished trailing line is not committed and therefore not a record.

## Inputs and outputs

| Type | Meaning |
| --- | --- |
| `Boundary { confirmation: NativeCursor, sequence, delivered_ms }` | The receipt's fixed `confirmation` cursor captured from the frozen view before Enter (`Boundary::capture(&snapshot.native_checkpoint(), sequence, delivered_ms)`), a monotonic Enter sequence echoed for fencing, and the server clock of the durable Enter marker. It never moves. |
| `Delivered { uid, text, media }` | The exact receipt payload, never the composer echo. |
| `Observation { sequence, watch, outcome }` | `watch` is the current committed cursor (the next `AdvanceWatch` target), `None` when the boundary itself could not be validated. |

`Outcome` is the evidence taxonomy:

| Outcome | When | Mapping to `delivery/codex.rs` |
| --- | --- | --- |
| `Possible(PossibleMatch)` | Exactly one real `user` record after the boundary whose text equals the delivered text after trimming only both ends, with a timestamp not earlier than `delivered_ms` (or no parsable timestamp). | `evidence: AckEvidence` with `Correlation::PossibleTextMatch`, `real_user_input: true`, `validated_confirmation` = the boundary cursor, `record: NativeAcceptance { source_identity, record_id: "codex-line:<start>-<end>", start, end, turn_id }`. `Machine::acknowledge` retires the receipt after the same boundary, media and one-record-per-receipt checks used for stronger evidence. `completion: Option<CompletionEvidence>` carries a later `task_complete`/`turn_aborted` status of the record's own turn (`Succeeded`/`Failed`/`Stopped`). |
| `Absent(Absence { skipped_earlier })` | Nothing qualifying after the boundary. `skipped_earlier` counts identical user records whose timestamps precede delivery; they are skipped like Python's `_causal`. | Nothing to apply; the caller may `AdvanceWatch` to `watch`. |
| `Uncertain(CheckpointMismatch)` | The file was rewritten or truncated below the boundary, or the logical view changed. | `NativeReset` at most; the confirmation fence never moves. |
| `Uncertain(ForeignScope)` | The view is not the Codex main session named by the payload UID and the boundary's `source_identity`, or is a child agent view. | Nothing. |
| `Uncertain(UnmatchableText)` | Whitespace-only text can never match. | Nothing. Opaque media preview metadata does not block text confirmation. |
| `Uncertain(Unreadable { status })` | The frozen view refused the query (sanitized status only, no path or OS detail). | Nothing. |

No `NativeRequestId` or `OperationTurn` correlation is ever produced **by the
adapter**. The raw Codex TUI does not echo a SessionDock request ID and the
rollout does not bind a `task_started` to the Enter operation that caused it;
selecting the next `task_started` would manufacture an association. `turn_id` is copied from
the record itself (`payload.turn_id` or
`internal_chat_message_metadata_passthrough.turn_id`) when it declares one
and is otherwise empty. The Machine accepts the adapter's causal
`PossibleTextMatch`; a turn ID is only needed to associate later completion.

"Real user input" follows the history projection: `response_item` messages
with role `user` that are neither developer prompts, rebuilt instruction blocks
(`<user_instructions>`, `# AGENTS.md instructions`, ...) nor
`goal.internal_context` records, with any `<turn_aborted>` prefix stripped.
`event_msg` `user_message` telemetry, assistant output, tool calls, status
events and inferred `/rename` events are not user records.

## Comparison with Python `observe`

| Python | Rust | Reason |
| --- | --- | --- |
| Matching text retires the first causal row (`rows.pop`); the receipt is deleted. | `PossibleTextMatch` acknowledges the first matching receipt after its fixed pre-injection cursor; a native record cannot acknowledge two receipts. | Matches Python's trimmed-text and causal-boundary behavior while preserving one-record-per-receipt ordering. |
| Matches over the messages the current read returned, from the advancing watch cursor on ordinary ticks. | Always classifies the full range after the **fixed** confirmation boundary, and the boundary must revalidate as a committed checkpoint each time. | A watch-only read can miss an earlier duplicate; a moved or rewritten prefix must not be silently accepted. |
| First record whose text matches and whose timestamp passes wins; earlier-timestamped duplicates are skipped and later ones confirm. | Same, within the fixed validated confirmation range. | Parity. |
| Accepts `user` and `command` roles. | Accepts projected `user` records only. | The Rust projection emits no native `command` role for Codex; inferred rename events are not native records. |
| Absent/invalid timestamps pass `_causal`. | Same: `TimestampCheck::Absent` passes, but is reported. The projection normalizes valid stamps to RFC 3339 UTC milliseconds and nulls invalid ones. | Parity as documented; the physical boundary still applies. |
| Boundary is a wall-clock `after_ts` plus a browser-supplied cursor. | Boundary is the server-validated `NativeCursor` captured from the frozen view plus the server's Enter clock. | Browser cursors and clocks are not evidence. |
| Attachments are not compared. | Same: opaque media preview metadata is carried on the evidence while text remains the native confirmation key. | The composer already included uploaded paths in the submitted text, matching Python's delivery path. |

Equal parts: trimming is both ends only (internal spaces and newlines stay
significant), and the comparison is on the projected text, exactly as Python
compares `messages_for` output.

## Polling constants and the replay helper

`ReplayPolicy`/`ReplayClock` reproduce the Python schedule without a timer:

| Constant | Value | Python source |
| --- | --- | --- |
| `CONFIRM_TIMEOUT_MS` | 8 000 | `send_queue.CONFIRM_TIMEOUT = 8.0`: after this a write is overdue, never failed |
| `REPLAY_INTERVAL_MS` | 8 000 | `_poll_outbox` spaces fixed-cursor replays by `CONFIRM_TIMEOUT` (`_CODEX_CONFIRM_REPLAY_AT`) |
| `TRACK_WINDOW_MS` | 3 600 000 | `send_queue.TRACK_WINDOW = 3600.0`: automatic tracking stops; the row keeps its state and is not retryable |
| `POLL_INTERVAL_MS` | 500 | `_outbox_loop` wake interval; the Rust inventory TTL is also 500 ms |

`ReplayClock::plan(policy, delivered_ms, now_ms)` returns `Watch` (cheap
tick), `Replay` (overdue and at least `REPLAY_INTERVAL_MS` since the last
replay; the instant is recorded) or `Expired` (one hour after delivery). The
first overdue tick replays immediately, then every eighth second, matching
`test_codex_confirmation_replay_is_rate_limited_like_claude`. A clock that
runs backwards is treated as not overdue. `earliest_tracked` reproduces
`send_queue.tracked`: only the earliest still-tracked receipt of a UID is
polled, and an expired one drops out of polling without changing state.

In Rust the replay is cheap: the inventory parse is incremental and
`native_tail` is an in-memory walk of the events after the boundary, so the
rate limit bounds bookkeeping rather than a growing re-read. A `Watch` tick
may skip `observe` when the committed end has not advanced past the receipt's
watch cursor; a `Replay` tick observes even then.

## Validation

- `cargo test -p sessiondock --lib delivery::codex_adapter --locked`:
  twelve synthetic tests through the real `SessionStore` path in private
  temporary directories (exact match with physical range and completion,
  Machine acceptance of the causal possible match, end-trim only, UTF-8 multi-line,
  earlier timestamped identical input skipped before the causal match, records at/before the boundary and
  unfinished tails ignored, rewrite/truncation → checkpoint mismatch,
  absent/invalid/earlier/numeric timestamps, inherited fork prefix excluded,
  protocol injections/telemetry/status not user input, foreign scope/media/
  blank text uncertain, replay constants/window/rate limit).
- `cargo test -p sessiondock --test codex_ack --locked` (also selected by
  the name filter `cargo test -p sessiondock codex_ack --locked`): a
  private fake Codex CLI shell script reads stdin lines and appends
  `turn_context`, `task_started`, the `response_item` user message and the
  `user_message` event (plus `task_complete` on `finish`) to a real rollout
  file; the adapter classifies through the real reader path (possible match
  with exact line range, causal selection on a resubmitted identical prompt,
  a later boundary, and truncation to/below a boundary). Linux only: it runs `/bin/sh`
  and GNU `date`.

The Claude executor work in the same batch added a Claude-specific read
(`SessionStore::claude_native_inputs`) beside this provider-neutral
`ViewSnapshot::native_tail`; both validate the fence like a message checkpoint
and share `RawIndex::record_start`. Unifying them is a later cleanup.

No real CLI, model, native home, PTY or browser is involved. Passing these
tests alone does not complete M5; the executor wiring and its acceptance are
in [delivery-codex-executor.md](delivery-codex-executor.md).
