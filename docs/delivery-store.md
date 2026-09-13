# Delivery persistence

`delivery::store::DeliveryStore` persists the Claude and Codex receipt state
machines. The executor performs terminal writes and observes native confirmation.
A storage acknowledgment records a state transition; it does not confirm that a
CLI accepted a prompt.

## API and ownership

`initialize(directory, codex_epoch, claude_epoch)` creates the initial ledger.
`open(directory)` reads an existing ledger. Directories are created as needed;
ordinary filesystem paths, aliases, permissions and unrelated neighboring files
are accepted. Valid existing ledger data is preserved. A missing ledger is
initialized; malformed or unsupported ledger JSON is reset to empty queues,
matching Python's reader. Operations use the store mutex and atomic file replacement.

`snapshots()` reloads the committed provider snapshots from disk. External
provider-state changes restore both machines with fresh epochs; formatting-only
edits preserve their current state. `commit_codex` and
`commit_claude` accept a provider's Persist proposal and return its matching
Persisted command. The engine owns the provider machines and schedules the
resulting effects. A retried persistence proposal does not resend terminal input.

## One envelope, existing provider schemas

`delivery-ledger.json` contains `format: "agenthub-delivery"`, `schema: 1`,
`codex`, `claude`, `codex_previous`, and `claude_previous`. The format identifier
preserves compatibility with deployed ledgers. One atomic replacement updates
both provider snapshots, retaining the unchanged provider.

Versions associate a proposal with its previous committed state. Replaying an
identical last proposal returns its acknowledgment. A changed payload under the
same request ID is a conflict. Receipts retain their original request and scope
so retries can be recognized after restart.

## Restart and failure semantics

Opening restores the provider machines using fresh process epochs. Pending
terminal effects are reconciled with native state; opening the ledger does not
repeat an old paste or Enter operation.

Writes serialize to a same-directory temporary file, flush its contents and
atomically replace the ledger. Unix directory synchronization is best effort,
matching Python. Failure before replacement leaves the previous file intact.
If the caller loses a persistence acknowledgment, it can read the installed file
and retry the identical Persist proposal. Storage acknowledgment is separate
from CLI acceptance.

## Filesystem and decoding protections

The writer cleans only the temporary file it created. Filesystem errors are
returned by the operation that encountered them. JSON duplicate keys use the
last value, unknown fields are ignored, and typed receipt state is validated.
There is no application quota on ledger bytes, JSON depth or retained receipts.

## Platform and validation status

Unix and Windows use file flush followed by atomic replacement. Unix directory
synchronization is implemented; this code does not claim a portable Windows
directory-fsync guarantee. Synthetic tests cover persistence, restart, exact
proposal replay, independent provider retention, aliases, external edits and
failures around replacement. Current batch results belong in the migration
ledger.
