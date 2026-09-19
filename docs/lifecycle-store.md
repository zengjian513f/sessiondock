# Private lifecycle creation receipts

`lifecycle::{model, store}` implements a synchronous, private creation-intent
ledger. It does not launch or discover a process, connect to ptyhost, bind native
UID/SID, expose HTTP/configuration, or complete the lifecycle migration. All test
inputs and directories are synthetic; no CLI or paid model is invoked.

## Explicit directory and specification

`LifecycleStore::initialize(directory)` requires an existing directory without a
ledger. `open(directory)` requires the existing valid ledger. Neither method creates
the directory, chooses a home/default path, or resets missing data.

The only specification is `LaunchSpec::new(Source, adapter_id, absolute_cwd)`,
where Source is Claude, Codex or Grok. It expands and resolves the requested
working directory. New create and begin-start calls revalidate the directory; serde
decoding cannot bypass those checks. Reading/replaying an existing receipt does
not require its historical cwd to still exist.

There are no arbitrary command strings, shell snippets, argv, environment
variables, credentials, model calls, or client-selected runtime-file paths.
Adapter IDs use 1–64 ASCII letters/digits/underscore/hyphen. An adapter ID is an
identity, **not an executable path or permission to invoke any program**. A future
trusted launcher must resolve it through its server-owned CLI catalog and
verify that it supports the declared source. It must revalidate cwd immediately
before launch or use an appropriately retained directory capability. These
point-in-time checks cannot prevent a malicious same-user rename race.

## Durable authority and state transitions

`create(request_id, &spec)` returns `CreateOutcome { record, prepared }`:

- A new request first persists its complete Prepared record, including
  library-generated record_id, launch_id, instance_id and host_name. Only after
  durable success does it return `Some(PreparedAuthority)`.
- The same request ID and complete canonical spec returns the existing record
  with `prepared: None`, in any state, without a write or fresh authority.
- The same request ID with any different source, adapter or cwd returns Conflict.

IDs/nonces use 128 bits of operating-system randomness. A host name has the fixed
`sessiondock-` prefix and a fresh random suffix; it is a routing label, not process
identity. Future host operations must also guard the immutable instance_id.

`begin_start(PreparedAuthority)` consumes that non-Clone token, verifies its
handle owner and exact record revision/state, and commits Starting **before**
returning a non-Clone `StartAuthority`. A future launcher must own that token
before spawning and preserve its launch/instance identity. The current library
does not execute the spawn or claim exactly-once external side effects.

```text
create --durable--> Prepared + PreparedAuthority
begin_start(authority) --durable--> Starting + StartAuthority
                            | mark_running(authority) --> Running
                            | mark_failed(authority, typed reason) --> Failed
                            | mark_uncertain(authority) --> Uncertain
Running --mark_exited(record_id)--> Exited
Prepared --cancel_prepared(record_id)--> Failed
```

`mark_running`, `mark_failed` and `mark_uncertain` consume StartAuthority and
check the exact Starting record and its creating handle. Successful cancellation
and exit can be replayed by record ID without another write. All other state
combinations fail. Failure reasons are a small typed enum; raw launcher errors,
paths and command output are not stored or echoed. Public lookup/mutation
identifiers never become filesystem paths.

Every handle has a new random private owner nonce. Tokens from a different
directory or a dropped/reopened handle cannot drive transitions in the new
handle. Neither authority implements Clone, serde or Debug. Record/LaunchSpec
also omit Debug because they contain private cwd data. Full records can be read
explicitly via `get(record_id)` or bounded `list(offset, limit)`; they are private
library values, not a ready-made public diagnostic response.

On open, all Starting rows are changed to Uncertain in one durable recovery
commit before the handle becomes usable. Recovery never reissues spawn authority.
A crash in Prepared leaves a retained Prepared receipt with no new authority;
an operator may cancel it and use a new request ID after resolving intent. A
Starting/Uncertain creation must not be resubmitted as a new ID merely to retry a
possibly successful spawn. Reconciliation with the exact launch/instance identity
is future work. Running, Failed and Exited rows remain distinct on reopen; Running
is a historical committed result, not a fresh liveness probe. No native UID/SID
or parent/child session identity is inferred.

The future launcher owns exactly one result per consumed StartAuthority. It must
correlate real spawn/exit outcomes to the immutable instance, preserve the token
until reporting the result, and treat any lost result as uncertainty. Record-ID
exit/cancellation methods are trusted internal APIs; HTTP authorization and
process-observation provenance must be established above this store.

## Persistence, failures and budgets

The dedicated directory contains `lifecycle-ledger.json`, `.lifecycle.lock`, and
only exact module temporary names. Envelope schema 3 contains format
`sessiondock-lifecycle`, a monotonic revision, and full records keyed by generated
record ID. Each record retains the original request/spec and its own revision.
There is no receipt deletion, compaction, TTL, tombstone expiry or capacity eviction.
Schema 5 adds three display fields to every record — `created_at`
(Unix seconds the intent was persisted), `finished_at` (set once by the
transition into Exited or Failed) and `discarded` (`discard(record_id)`,
allowed only for Exited/Failed receipts or ones with `cancel_requested`,
idempotent) — and `method`/`evidence`/`bound_at` to a binding. Older
schemas migrate with `created_at:null`, `finished_at:null`,
`discarded:false` and `method:"operator"`; a file of an older schema that
already carries any of these fields fails closed. `finished_at` on a
non-terminal record or `discarded` on a live one is invalid.

Schema 2 introduced required `cancel_requested` to every record and the CancelRequested
state. Strict schema-1 ledgers are durably migrated with that flag false; malformed
or mixed schemas are not silently normalized. Starting and CancelRequested recover
as Uncertain before open returns, retaining cancellation intent. Cancellation
authority is bound to the current handle and exact revision; subsequent typed
observations match record, launch, instance and revision and cannot reauthorize
spawn/kill. See [the coordinator contract](lifecycle-service.md) for implemented
launch, readiness, cancellation and recovery behavior above this synchronous store.
Schema 3 adds required nullable `binding` with an immutable private source/SID/UID
spec and Intent/Confirmed/Uncertain state; schema 5 adds its method, evidence
and confirmation time. Old schemas migrate with no binding;
open downgrades historical Intent to Uncertain and preserves Confirmed as a
durable association receipt; current host state is required separately for
control. Binding authority is
handle- and revision-bound, conflicting specs never overwrite intent, and every
confirmation requires typed exact-record observation. The
[binding contract](lifecycle-binding.md) documents explicit operator authority,
same-value retries, read-only recovery and post-restart native authorization.

The local disk boundary is adapted from the reviewed delivery store without
changing that existing module. It creates a unique temp,
writes and fsyncs it; atomically renames; fsyncs the directory; and verifies the
installed bytes before acknowledging. The lock path is never removed by normal
Drop. Only this invocation's own temporary file may be cleaned. Abandoned temps,
partial initialization and unrelated files are preserved.

An actual persistence error is returned and no optimistic state is acknowledged.
Each operation reloads the current ledger data; the list refresh's `refresh_many`
reloads once for the whole batch and applies each observation and binding
observation against that one load. Missing/corrupt ledgers fail closed,
never as an empty store.

Retained receipts and encoded ledger bytes have no fixed capacity quota. Existing
cwd paths follow the operating system's path limits; private creation request IDs
remain 8–128 ASCII identifier characters. Generated nonces have exact fixed sizes.
Parsing accepts JSON's ordinary last-key-wins and extra-field behavior while
rejecting missing required fields, invalid states, duplicate identities and invalid revisions. Snapshot
writes preserve every receipt and idempotency key; they neither expire entries
nor silently evict records when the ledger grows. Whole snapshots are still
cloned and rewritten, so this is not a high-throughput journal design.

The durable backend runs on Unix and on Windows. Unix syncs the
temp file, renames it and then flushes the directory handle; Windows syncs the
temp file and renames it through `MoveFileEx(REPLACE_EXISTING)` — std offers
no directory handle to flush there, NTFS journals the rename itself, and file
identity for the replacement checks behind the exclusive lock is the creation
time rather than an inode (as for the metadata store). Other targets return
DurabilityUnavailable and do not fake a directory-sync guarantee. Linux tests
do not certify macOS, network filesystems, storage hardware or power-loss behavior.
The caller owns the durable lifecycle of the preexisting directory itself. All
methods are synchronous; a future async service must perform filesystem calls,
fsync, serialization and lock release in bounded blocking work, independently of
HTTP cancellation.

## Validation

Default check is `python3 tests/lifecycle_http_suite.py` (and
`python3 tests/lifecycle_browser.py` when the change is user-visible). Do not
run crate unit tests unless the user asks.
