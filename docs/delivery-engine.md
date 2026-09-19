# Delivery engine

`DeliveryEngine` owns the delivery store and one Claude/Codex machine each.
`DeliveryService` serializes operations; executors perform terminal writes and
native confirmation after the corresponding state has been persisted.

## Lifecycle and persistence

`initialize(directory)` creates the initial state. `open(directory)` restores
an existing ledger with fresh process epochs, or initializes a missing ledger. Both use ordinary filesystem
paths as described in [delivery-store.md](delivery-store.md).

The command sequence is:

```text
command → Machine.apply → Persist proposal → store commit
        → matching Persisted → DispatchBatch
```

The engine retains a failed Persist proposal for `retry_commit()`. A retry uses
the same proposal and operation identity. Committed reads expose the durable
state. Native reconciliation decides what happened to terminal work interrupted
by a restart or transport failure.

## External action ownership

Consuming `DispatchBatch::claim()` transfers terminal actions to the executor.
Operation identities associate later callbacks with their original actions.
A replayed HTTP request can return its existing receipt without performing
another paste or Enter. A storage error must not be reported as native acceptance.

## Legacy display projection and diagnostics

`codex_outbox(uid, agent_id)` and `claude_outbox(scope)` return
`{outbox, outbox_version}`. Rows preserve request ID, text, opaque media metadata,
creation time, state and attempts. File paths are already embedded in the prompt;
media preview metadata does not add another terminal write.

Confirmed, completed and dismissed rows follow the provider's visibility rules.
Retained receipts preserve retry identity after a row disappears from the visible
outbox. `receipts(provider, offset, limit)` returns receipt metadata;
`logs(after_sequence, limit)` reads a 128-entry diagnostic ring. Diagnostic
retention does not reject submissions or discard their receipts.

## Validation boundary

Executor, HTTP and browser suites cover the full send path with temporary fake
CLIs. Do not run crate unit tests unless the user asks.
