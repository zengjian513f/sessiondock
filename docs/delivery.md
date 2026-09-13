# Codex delivery domain: pure receipt state machine and Python audit

`crates/sessiondock/src/delivery/codex.rs` is the pure receipt state
machine for Codex sends: no disk or network access, no terminal capture, no CLI,
no executed effect. Claude has its own domain module (`delivery/claude.rs`, see
the last section) because its native queue, draft restoration and
interrupted-input behavior are not Codex's protocol.

Status (batches 30–32): the machine is wired and in use. The Codex native
acknowledgment adapter ([delivery-codex-ack.md](delivery-codex-ack.md)) classifies
records after a fixed confirmation boundary; the shared executor
([delivery-executor.md](delivery-executor.md), Codex path in
[delivery-codex-executor.md](delivery-codex-executor.md)) drives the machine
through the HTTP send routes with persist-before-inject, exclusive leases and
composer verification. The rule below still holds for the adapter alone: a
text match by itself (`PossibleTextMatch`) never acknowledges a receipt. The
executor turns a unique matching record that carries its own `turn_id` into
`OperationTurn` only because it held the lease, verified the composer and
pressed Enter after the boundary itself — the same reasoning as Claude's
`VerifiedEnter`; without that operation, or without a `turn_id`, the receipt
stays `uncertain` even if identical text appears in the rollout.

## Audit of the Python baseline

The source review covered `agenthub/send_queue.py`, `send_protocol.py`,
`create_requests.py`, their callers in `server.py`, all of
`tests/test_send_queue.py`, and creation receipt cases in `tests/test_hub.py`.
This was code inspection; no production ledger, CLI home or paid CLI was used.

- Codex sends a follow-up immediately to the live TUI, which owns queuing while
  working. `mark_delivering` persists before `term.submit_text`; exceptions
  afterward become non-retryable `confirming`. Eight-second timeouts do not
  imply failure. Esc/idle must not retire a later TUI-owned follow-up.
- `watch_*` advances; `confirm_*` snapshots the actual submission boundary.
  The server periodically rereads that fixed boundary rather than trusting
  only the advancing tail. Expensive replay is rate-limited, and the one-hour
  tracking window stops automatic polling without making the receipt retryable.
- A request replay is checked before touching a subsequent draft. Draft consent
  names a fingerprint of the screen and cursor, which is checked again before
  clearing. Clearing must actually yield an empty native editor.
- `observe` acknowledges matching `user`/`command` text with only end trimming
  and a timestamp check; absent/invalid timestamps pass that check. The fixed
  cursor improves rereading, but `observe` does not bind the matched record to
  the originating terminal operation or unique native turn. An arbitrary later
  identical human input can therefore be mistaken for this delivery.
- Acknowledgment deletes the Python receipt. A later replay of the same request
  ID no longer finds a durable deduplication record. Payload checks compare UID
  and text, not all attachment/target attributes.
- The Python queue uses temporary-file replacement but not the explicit file
  and parent-directory durability sequence used by `create_requests`. Its
  read helper also treats malformed/unreadable state as an empty queue. Neither
  behavior is used as an acceptable fail-open recovery strategy here.
- Creation receipts illustrate the stronger boundary: persist `started`, then
  execute, then persist the response. A crash with only `started` is uncertain,
  not an invitation to repeat a launch. Creation is **not implemented** by this
  Codex module; only its durable-boundary principle is reused.

## Interface and state transitions

`Machine::new(epoch)` creates an empty in-memory domain. `apply(Command)` returns
effects or a typed error. `snapshot()` exposes only the last committed snapshot.
`Machine::restore(snapshot, fresh_epoch)` validates persisted state and proposes
a recovery commit; it never emits a terminal mutation.

```text
Submit
  → persist checking_draft → inspect composer
      → draft_conflict → explicit token consent → inspect again
      → failed_before_write → explicit Retry → inspect again
      → persist prepare_in_flight → conditional clear / safe paste
          → independently observed exact prepared content
              → persist enter_in_flight → conditional Enter
                  → uncertain (even when the transport call succeeded)
                      → strong native proof → persist acknowledged
                          → matching native completion → completed / stopped
```

Every arrow that changes a receipt first emits **only**
`Effect::Persist { version, snapshot }`. Only the matching `Persisted(version)`
commits it and releases the subsequent effect. Repeating `Persisted` cannot
release the effect twice. A stale epoch/revision is rejected. A failed commit
whose outcome is not known freezes the machine until the application explicitly
reloads its durable store; it does not revert to an earlier apparently safe row.

The application must serialize commands around pending commits. A pending
proposal admits an identical Submit only as a read-only replay, and rejects a
conflicting payload. One physical terminal or UID cannot have multiple draft,
prepare or Enter critical sections in this machine. Once submission is
`uncertain`, a different follow-up may proceed; there is no inferred idle gate.

| State | Terminal mutation authorized by this domain | Recovery / retry |
|---|---|---|
| `checking_draft` | Inspection only | Restore as pre-write failure; explicit retry reinspects |
| `draft_conflict` | None until matching consent and fresh inspection | Preserve conflict; never clear automatically |
| `prepare_in_flight` | One conditional preparation after durable commit | Restore `uncertain`; never replay paste or clear |
| `enter_in_flight` | One conditional Enter after durable commit | Restore `uncertain`; never replay Enter |
| `uncertain` | Native inspection only for this request | No timeout/activity/transport result enables retry |
| `acknowledged` | Optional turn-specific stop, separately persisted | Keep durable acknowledgment/tombstone |
| `stop_requested` | At most the original conditional stop effect | Restore without repeating stop; wait for native outcome |
| `completed` / `stopped` | None | Terminal outcome, including native failure, is not retryable delivery failure |
| `failed_before_write` | None | Retry only while `attempts == 0` |
| `discarded` | None | Keep tombstone; same request ID remains a replay |

Discard is limited to pre-write rows. Removing an uncertain receipt is not
treated as canceling input owned by the TUI. There is deliberately no automatic
deletion of accepted/completed/discarded deduplication records.

## Draft and injection contracts the future adapter must enforce

`Target` binds a host instance, native terminal ID and ownership generation.
It is an identity, not a bearer credential. A display name alone is insufficient.
The application must verify that this Codex UID really belongs to that target.

The composer observation contains a nonreversible frame/cursor token and a
server-validated native cursor. On consent, the composer is inspected again;
a different editing token returns a new conflict. The ensuing Prepare effect
still requires the driver to compare the exact frame **under a valid writer
lease immediately before mutation**. A boolean check in this domain cannot
provide that terminal-side compare-and-act guarantee.

Prepare may clear only the explicitly approved draft, verify empty and paste
the exact payload without submitting it. A successful transport write alone is
not `PreparedEvidence`: the driver must independently verify the complete
prepared composer and all attachment references. If the TUI displays only a
placeholder or otherwise cannot prove this, preparation remains uncertain and
the domain will not authorize Enter. Before Enter, the driver must recheck the
prepared frame and target ownership. A monolithic paste-plus-Enter helper cannot
be connected across this two-phase boundary unchanged.

Effects are one-shot obligations, not a durable executable retry queue. The
application must never replay a saved Prepare/Enter/stop effect on restart or
transport reconnection. Durable pre-dispatch markers deliberately mean that a
crash even *before* the corresponding terminal write can leave an uncertain
receipt. This favors avoiding duplicate user submissions over automatic retry.

`InterruptTurn` is not a generic Escape command. The driver must prove the
accepted turn is still the target's current turn, atomically with its write
authority; otherwise it must not send Esc that could abort a newer task. Merely
requesting stop or returning from its transport call cannot mark a receipt
`stopped`. Only a native completion for the exact accepted turn can do so.

## Native acknowledgment and fixed confirmation fences

An `AckEvidence` is accepted only when all of these hold:

1. It names the same UID, source identity and exact original confirmation
   cursor. The trusted reader has revalidated that prefix against storage.
2. The whole native record begins at or after that boundary and ends after its
   start. It is a real user input, not a developer/rebuilt prompt, telemetry,
   command or inferred event, and has stable record and turn identities.
3. Its text equals the submitted text after **only** trimming the ends; internal
   spaces and newlines remain significant. Observed attachment identities and
   content digests also match.
4. It carries either a genuine native request ID, or an independently proven
   native turn association to the exact persisted Enter operation.
5. That native record, byte start or turn has not already acknowledged another
   receipt. The proof and native acceptance are retained in the snapshot.

The types do not manufacture cryptographic/native proof. `OperationTurn` is a
claim the trusted integration must establish independently; deriving it by
selecting the first later `task_started` or matching prompt would defeat the
contract. Today's raw Codex TUI does not echo AgentHub request IDs or establish
that reliable association. No production adapter currently supplies either
strong proof here. Synthetic tests explicitly manufacture such evidence to
test the state transitions, **not to demonstrate real native confirmation**.

`AdvanceWatch` can advance only a validated same-source checkpoint; it cannot
replace the original `confirmation`. Same-offset rewrites and backward/reset
cursors are rejected. `InspectNative(replay=true)` always emits the fixed
confirmation cursor, while normal inspection uses the advancing watch cursor.
A native reset can annotate `uncertain` but cannot move the confirmation fence,
forget an attempt, or make a receipt retryable. Native history rollback after an
already accepted receipt remains a future adapter policy; historical delivery
must not be mistaken for an unsubmitted message and sent again.

The module has no clock or background task. The future application scheduler
must limit polling and expensive fixed-prefix replay, preserve the Python
eight-second replay spacing or a justified equivalent, and stop automatic
tracking after a bounded window without changing retry eligibility. HTTP/SSE
activity, observation cancellation and watcher backpressure remain unconnected.

## Persistence, ordering, privacy and bounded resources

Snapshots are serde-serializable schema 1 data. A process epoch must be newly
generated for every recovery; revision monotonically increases across commits.
Receipt revisions fence late callbacks independently of unrelated receipts.
Only a committed snapshot should be published as an authoritative outbox.
Epochs are compared for equality, not lexical/numeric order.

The future store must enforce a single-writer/CAS discipline, bounded decoding,
private permissions, atomic replacement or a transactional journal, and an
explicit durability guarantee before reporting `Persisted`. Filesystem write
success or rename success alone is not a universal power-loss guarantee.
POSIX file/directory sync and Windows replacement/durability behavior need
separate implementation and fault-injection tests; this pure module has not
validated either platform's persistence behavior. Missing, malformed or
unsupported ledgers must never silently become an empty deduplication store.

Request IDs are required, 8–128 ASCII letters/digits/underscore/hyphen; unlike
the old send queue, they are not truncated or generated implicitly. The exact
payload includes UID, target identity, text and ordered attachment references.
Same ID with changed payload is a conflict even after acknowledgment/discard.
Attachment references are modeled only for identity checks; this does not
implement upload resolution, native attachment preparation or media sending.

Limits are 4,096 retained receipts, 256 KiB per payload and 8 MiB of retained
payloads. Capacity failure is explicit; there is no silent tombstone expiry.
Tombstones currently retain text and must be stored privately. A future compact
deduplication index with a collision-resistant canonical payload digest and an
explicit retention policy can reduce storage. It must not turn an expired ID
into permission to submit again accidentally. Full-snapshot cloning is bounded
but is not intended as a high-throughput production journal.

## Validation

Run `cargo test -p sessiondock --lib delivery`. The pure tests cover durable
gates, duplicate/conflicting requests, draft consent races, prepare/Enter crashes,
unknown commit outcomes, recovery epochs, immutable confirmation cursors,
weak/foreign/old/internal native evidence, attachment and association retention,
one-native-record consumption, scoped stop/completion, retry and tombstones.
All test evidence is synthetic and in memory; serde roundtrips perform no I/O.

The durable store, terminal-side preparation/Enter under ownership fencing,
the acknowledgment adapter, the polling policy, the HTTP routes and the browser
recovery behaviour are validated by the executor's own suites
(`delivery_send`, `delivery_send_codex`, `send_http_suite.py`,
`send_browser.py`, `send_codex_browser.py`) and by the real-CLI suites
(`send_claude_real.py`, `send_codex_real.py`, cheapest configuration per
AGENTS.md). The domain tests here never start a CLI.

## Claude: independent pure queue/turn domain

`delivery/claude.rs` is a separate state machine, not a wrapper around the Codex
implementation. It has its own schema, scope, queue order, command/effect types
and recovery rules. It is also **only an M5 prerequisite**: no durable store,
screen driver, host sender or HTTP route is connected to this domain. The
Claude question hook does exist since batch 44 WP-G (`sessiondock claude-hook`,
[Live question cards and approvals](#live-question-cards-and-approvals-wp-g)),
but it feeds the conversation page's `prompt` field only; it is not connected
to this domain as delivery evidence.

### What the current Python Claude bridge actually proves

The additional source review covered all of `claude_queue.py`,
`claude_bridge.py`, `tests/test_claude_queue.py`, relevant bridge tests, native
queue fixtures in `tests/test_adapters.py`, and the server's enqueue/poll callers.

The existing bridge installs `PreToolUse`, `PostToolUse` and
`PostToolUseFailure` for **AskUserQuestion only**, plus `SessionStart` and
`SessionEnd`. Its stable ID is `tool_use_id`, identifying a question tool.
The code does **not** install `UserPromptSubmit`, inject a delivery tag, or echo
an AgentHub `request_id` into a prompt-acceptance hook. Request IDs in
`send_audit.py` are AgentHub's own audit records, not Claude acknowledgments.
Question hooks are valuable for displaying a pending dialog while its native
tool result has not yet reached JSONL — that display bridge is implemented
(WP-G, below) — but they cannot confirm a web-submitted prompt or its
completion.

The Python Claude queue improves on Codex's receipt deletion by retaining
seven-day text-hash tombstones. It persists before terminal injection and
retains ambiguous submissions. However, acknowledgment still uses text plus a
time check, including a narrow two-second exact-suffix repair for text appended
to a restored draft. `/rename` can be matched by a later custom-title event and
`/compact` by compact completion. Queue remove and UI dismissal both use the
same `_confirm` tombstone operation, despite not proving a committed user turn.
These are audited baseline behaviors, not reliable new causal proof.

The new `Association::QuestionToolHook` and `PossibleTextMatch` are explicitly
rejected as prompt/queue acknowledgments. `NativeRequestId`, `VerifiedEnter`
and `QueueEntry` describe evidence a **future trusted adapter** would need to
establish independently. The current source does not provide that adapter or a
reliable web-request correlation. Synthetic tests intentionally construct strong
associations to exercise the domain; they are not evidence that real Claude
TUI sending already has such a contract.

### Four different milestones must remain distinct

```text
persisted local Queued
  → explicit FIFO DispatchNext → inspect/prepare/Enter durability barriers
      → Uncertain after the transport call
          → strongly associated native enqueue → NativeQueued
              → identified dequeue → NativeDequeued
                  → strongly associated real user record → Accepted
                      → exact turn's selected-lineage completion → Completed
```

The native queue stages are optional: a strongly associated real user record
can directly accept an uncertain submission. The following distinctions hold:

- Local `Queued` means only that AgentHub durably retained the request. Persisting
  that row releases no terminal effect. `DispatchNext` is a separate scheduling
  decision and still requires fresh composer/ownership proof.
- `NativeQueued` proves an identified native enqueue, not a committed user turn
  or work completion. A native dequeue advances to `NativeDequeued`, never to
  `Accepted`. Native removal advances to `NativeRemoved`, not success or safe
  retry. A later strong user record may still correct that observation.
- `Accepted` stores the actual user UUID, contextual parent turn, native record
  and association. Neither assistant activity nor queue persistence can create
  that proof. Transport return still leaves `Uncertain`.
- `Completed` stores a native outcome only for the exact accepted turn, in the
  selected lineage, with that user's UUID as the proven ancestor. A parent turn's
  completion, a child's tool result or an unrelated final answer is insufficient.

The local queue uses a persisted sequence, not client time or lexicographic
request ID. A pending draft conflict blocks later local requests in that scope
until the user confirms or cancels it. A currently accepted/working turn does
not prohibit another request from being enqueued or submitted to Claude's native
queue. Physical terminal write critical sections remain mutually exclusive.

Native dequeue evidence must reference the identified enqueue, and tracked
native entries are advanced in native byte order. The original fixtures contain
bare `dequeue`/`popAll` records without per-request IDs. It would be unsafe to
invent that association merely from AgentHub's local queue: untracked human or
internal native entries may exist. Unidentified dequeue and bulk `popAll`
reconciliation remain an adapter gap; this module does not guess or expose a
bulk-clear effect. A future complete native-queue observer must prove the mapping
and distinguish dequeue from internal notification migration/removal.

### Drafts, interruption and parent/child boundaries

The native Claude composer uses its own borders, dim suggestions and cursor
rules; those are not replaced with Codex screen parsing. The pure domain accepts
only a trusted result: unknown composer state returns to local `Queued` without
authorizing a write. Editing needs a consent token and another fresh inspection.
Prepare requires exact payload/attachment observation before a separately
persisted Enter boundary. The future driver must conditionally compare frames
and ownership at the moment it clears/pastes/presses Enter, not just before an
asynchronous persistence operation.

`Scope` explicitly contains UID, native session and optional agent ID. Turn
identity contains the accepted user UUID and a separate contextual parent UUID.
The parent cannot stand in for the new user's turn; agent/main scopes do not
implicitly target each other. This does not authorize sending into any child
agent: the future application must first prove target ownership and whether
such a child has an independently writable terminal at all.

A specifically associated return/interrupt observation can mark an uncommitted
submission `Restored` or `Interrupted`. Restoration additionally requires the
exact payload to be visible in the identified editor frame. Its witness is
retained. This is not permission to retry: delayed native user evidence can still
arrive and supersede that earlier observation. The module does not use a global
Esc, unrelated `aborted` status, or merely matching editor text as association.

`RequestStop` is only available after real user acceptance and emits a conditional
stop for that exact scope and turn, after its durable marker. It does not cancel
pending follow-ups or immediately mark the turn completed. The future driver
must refuse to issue a generic Esc if the current turn no longer matches. A
native interrupted outcome for the accepted turn is distinct from an input
restored before native commitment.

### Recovery, cancellation, dismissal and remaining integration

Every state change uses `Persist` followed by the matching epoch/revision
`Persisted` before a subsequent effect is released. Unknown commit outcomes
freeze the machine. Recovery uses a fresh epoch and validates identities,
sequence, native evidence and capacity before admitting work.

- Local queued rows retain their order. Interrupted inspection returns to
  `Queued`; a later explicit dispatch must recheck the current target/draft.
- Prepare/Enter in flight restores as `Uncertain`; no clearing, paste or Enter
  is automatically replayed. Native queue/accepted/stop-requested observations
  retain their states, and recovery never repeats the original stop effect.
- Only pre-write local waiters may be canceled. There is no post-injection Retry
  command. Timeouts annotate uncertainty or queued-native state but never permit
  duplicate injection or infer native failure.
- `Dismiss` hides a receipt without falsifying its delivery state or removing its
  deduplication identity. Dismissing a local waiter cancels that local dispatch;
  dismissing an already submitted row does not cancel Claude. Display-only
  dismissal does not invalidate an in-flight write callback's authority revision.
- Accepted, completed, canceled and dismissed requests retain full-payload
  tombstones. There is no automatic seven-day expiry. This deliberately avoids
  treating an old ID as a new send, but stores more private text than Python's
  compact hash tombstone; a bounded canonical digest policy remains future work.

The fixed server confirmation cursor is captured at preparation and never moves
with the advancing watch cursor. Reset/backward/same-offset rewritten checkpoints
cannot replace it. Polling schedules, replay throttling, live activity integration,
storage durability and browser UX are still application responsibilities.

Limits are 4,096 retained receipts, 64 visible pending receipts per scope,
256 KiB per payload and 8 MiB total retained payloads. Rejection is explicit and
never silently drops a request. Unknown/corrupted snapshots do not become an
empty queue. These are pure memory limits, not a production journal benchmark.

This initial Claude domain covers ordinary prompt delivery evidence only.
Automatic `/rename`, `/compact`, local-shell-command acknowledgment and raw
`popAll` mapping are not implemented. Question cards and approvals are
*displayed* through the hook/screen bridge below and *answered* as plain
keyboard input under the page's terminal lease; a hook-backed answer is never
promoted to delivery evidence. The old suffix/time heuristics are intentionally
not promoted to strong acknowledgment. Attachment identities are compared, but
attachment resolution/sending remains unconnected. No native file or hook state
is read or written by this module (the bridge module reads its own files).

Run `cargo test -p sessiondock --lib delivery::claude` for synthetic in-memory
tests of FIFO, exact commit barriers, queue-vs-user-vs-completion milestones,
weak/late/cross-request evidence, parent/child scope, draft races, timeout/cancel,
restoration, stop recovery, capacity and immutable confirmation cursors. Real
M5 readiness still requires the durable store and trustworthy execution/evidence
adapters plus authorized integration/browser tests; these pure tests do not
complete that milestone.

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
  from `--state-dir` or `SESSIONDOCK_STATE_DIR` (the launcher clears the
  CLI's environment, so the settings file carries it as an argument).
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
