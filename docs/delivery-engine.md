# Isolated synchronous delivery engine

`delivery::engine::DeliveryEngine` exclusively owns a private `DeliveryStore`
and one Claude/Codex Machine each. This is a library boundary for the independent
Rust migration. It has no HTTP route, scheduler, timer, native-history reader,
PTY writer, CLI invocation, or production configuration. A future trusted driver
must validate terminal ownership and native evidence before providing callbacks.
The library does not establish native acknowledgment provenance itself.

## Lifecycle and persistence

`initialize(explicit_directory)` requires a preexisting empty private directory.
`open(explicit_directory)` requires an existing valid ledger. They inherit the
dedicated-path, permissions, locking, capacity and Unix durability requirements
of [the store](delivery-store.md). There is no default directory, directory
creation, automatic initialization, reset, or native-home discovery. Windows
opens the same store since WP-W; these Linux tests do not certify other systems.

Epochs use 256 bits from the operating-system random source. Opening restores
**both** providers with fresh epochs, commits each exact `Machine::restore`
proposal, checks the exact returned `Persisted` token, and applies it. A usable
engine is returned only after both barriers complete. A partial recovery returns
an error and releases the handle; the next explicit open performs fresh recovery
again. No recovery external action is allowed. In-flight Prepare/Enter becomes
uncertain under the provider's rules, never automatic reinjection.

Calls require exclusive `&mut self`; callers sharing the engine must serialize
access (for example, one mutex around the entire engine). The command sequence is:

```text
Machine.apply(command)
  -> retain the unique Persist proposal with its previous committed version
  -> DeliveryStore.commit_provider(previous_version, exact proposal)
  -> check and apply matching Persisted
  -> DispatchBatch with typed external actions
```

The engine does not expose its Machines, store, pending proposal, or full domain
snapshots. It rejects caller-supplied `Persisted` and `PersistenceFailed` commands.
Provider commands/evidence are internal trusted Rust types, not an HTTP input
format. Codex calls take an explicit inventory-resolved `agent_id` argument and
reject any child scope; Claude retains exact `{uid, session_id, agent_id}` scope.
The engine cannot validate inventory resolution for a future caller.

A known precommit error retains one exact proposal across both providers.
`retry_commit()` only retries that persistence proposal. Other state transitions
are blocked; an identical submit/enqueue can return a replay. For a first request
whose commit failed, that replay has `known: false, pending: true`. A previously
durable request with a pending change remains known. Conflicting payloads fail.
Neither replay nor reads auto-retry the pending proposal. Reads show committed
state only. Full disk identity/fingerprint validation also runs before normal
read/replay/command processing.

Store uncertainty, an invalid internal acknowledgment, or an abandoned action
batch freezes the entire engine. No effects or regular snapshots are released
while frozen. Drop the handle and explicitly reopen with fresh recovery; retrying
the live frozen object is forbidden. Store disk errors on read also freeze the
engine because it can no longer establish its committed view.

## External action ownership

`DispatchBatch` and its typed `Action` enums intentionally implement neither
`Clone` nor prompt-bearing `Debug`. There is no public persistence-action variant.
Only consuming `DispatchBatch::claim()` releases the external actions. The engine
blocks further processing while a nonempty batch is outstanding. Dropping it
unclaimed marks the shared lease abandoned; the engine freezes on its next
normal check. Dropping the engine also invalidates its outstanding batch, whose
later `claim()` returns an error even after the ledger has been reopened. An empty
batch can be dropped freely. Replays expose only bounded
receipt metadata, not submitted text or native proof.

After claim the trusted driver owns action execution and result reporting. It
must preserve operation tokens, ownership checks, conditional interrupt targets,
and composer verification. Duplicate/stale completion callbacks are rejected by
the domain without releasing another action. The driver must not clone payloads
and replay them as new work merely because they are ordinary owned Rust values.
The mechanism is a one-time handoff, **not exactly-once execution** across crashes.
A driver that claims and loses an action must report the appropriate failure or
perform explicit restart recovery. The engine supplies no action timeout or
automatic resend. Explicit native inspection commands may intentionally reread
native evidence; they are not terminal mutations.

## Legacy display projection and diagnostics

`codex_outbox(uid, agent_id)` and `claude_outbox(exact_scope)` return a complete
committed display snapshot of the existing wire shape:

```json
{
  "outbox": [{
    "id": "example-request", "uid": "codex:example", "text": "example",
    "media": [], "created": 1234, "state": "queued", "attempts": 0,
    "server": true
  }],
  "outbox_version": {"epoch": "process-epoch", "revision": 1}
}
```

`created` is milliseconds. `error` is optional and uses fixed display messages;
it does not echo an adapter's potentially sensitive reason. The optional `afterTs`
field is omitted because the current domain has no trustworthy display timestamp
for that purpose. It is never an acknowledgment or evidence of delivery.

| Provider domain state | Legacy state | Attempts |
| --- | --- | --- |
| Codex CheckingDraft | queued | 0 |
| Codex DraftConflict, FailedBeforeWrite | failed | 0 |
| Codex PrepareInFlight, EnterInFlight, Uncertain | failed, explicit verification message | 1 |
| Claude Queued, Inspecting | persisted | durable attempted flag |
| Claude DraftConflict, PrepareInFlight, EnterInFlight, Uncertain, NativeRemoved, NativeDequeued | ambiguous | durable attempted flag |
| Claude NativeQueued | native_queued | durable attempted flag |
| Claude Restored | restored | durable attempted flag |
| Claude Interrupted | aborted | durable attempted flag |

Accepted, completed, stopped/stop-requested and discarded/cancelled receipts
follow each provider's visibility rule; Claude dismissal also hides its row.
Their complete tombstones remain in the private store and duplicate requests do
not send again. No tombstone expiry or retention reduction is implemented.

There is no trusted legacy-media mapping yet. New submissions with media fail
with `UnsupportedMedia` before mutation; projection of a visible preexisting
media receipt also fails explicitly, and a Prepare action with media cannot be
released. Nothing is silently converted to an empty attachment list. Media-free
projections have `media: []`.

`receipts(provider, offset, limit)` returns committed ID/revision/visibility
metadata with a maximum of 128 entries per call. `logs(after_sequence, limit)`
returns at most 128 entries from a 128-entry event ring and remains available for
diagnosing a frozen engine. Logs contain only sequence, event kind and optional
provider; neither diagnostics API returns prompts, native proof, file paths,
ownership targets or adapter error strings. The full outbox is deliberately a
prompt-bearing display API and uses the existing provider ledger capacity bounds.

## Validation boundary

Synthetic engine tests cover commit-before-dispatch, stale callbacks for both
providers, duplicate/conflicting submissions, undurable replay, exact retry,
cross-provider gates, dropped versus claimed batches, acknowledgment loss after
a real successful store commit, reopening both epochs without reinjection,
media rejection, exact Claude scope, legacy fields/state mapping, retained hidden
tombstones, bounded prompt-free diagnostics, explicit initialization and locking,
and detection of external disk changes before a replay.

Engine failure hooks are test-only: precommit errors are injected before calling
the real store; the lost-ack test commits through the real store then suppresses
the acknowledgment with an uncertainty error. Actual before/after-rename failure
boundaries are separately covered by the store's disk failpoint tests. No test
opens a native CLI, PTY, browser, production queue, or paid model session. This
unexposed library does not claim browser validation or completion of migration M5.
