# Isolated delivery persistence boundary

This is a development-only durable ledger for the independent Claude and Codex
domains in [delivery.md](delivery.md). It does not implement a sender, native
acknowledgment adapter, scheduler, HTTP endpoint, CLI launch, or production queue
migration. In particular, these persistence tests do **not** complete M5.

## API and ownership

`delivery::store::DeliveryStore` has no default path and creates no directory.
Its explicit directory must already exist, be dedicated to this module, and have
Unix permissions `0700`. Do not reuse the metadata, native history, ptyhost,
frontend, or Python runtime directory. The eventual configuration layer must
also reject overlapping roots; this library does not discover those other roots.

- `initialize(directory, codex_epoch, claude_epoch)` requires an empty directory.
  It durably creates revision-zero snapshots. A preexisting lock, ledger, temp,
  or unrelated file is not overwritten, adopted, or removed.
- `open(directory)` requires both the stable lock and an existing valid ledger.
  Missing data is an error, never an empty idempotency database.
- `snapshots()` returns committed Claude/Codex snapshots after rechecking the
  disk fingerprint and filesystem ownership. It does not perform recovery.
- `commit_codex(expected_version, &effect)` accepts only `codex::Effect::Persist`
  and returns the matching `codex::Command::Persisted` after durable commit.
  `commit_claude` does the analogous operation with Claude's independent types.
  Neither method applies the returned command or executes any follow-up effect.

One store handle holds an exclusive OS file lock for its entire lifetime. Its
mutex serializes the shared on-disk document. The application must additionally
own exactly one live `Machine` per provider and execute an authorized effect at
most once. A disk acknowledgment is not an effect scheduler or a replacement for
domain operation tokens.

Capture a Machine's committed `snapshot().version` before applying its command:

```text
command
  -> Machine.apply -> Persist { version, snapshot }
  -> store.commit_provider(previous_version, Persist)
  -> matching Command::Persisted
  -> Machine.apply -> next authorized effect, if any
```

Persist/Inspect/Prepare/Enter are distinct boundaries. A caller must not invoke
an injection before the matching persistence acknowledgment, interpret a store
commit as native acceptance, or fabricate a `Persisted` command after an error.

## One envelope, existing provider schemas

The fixed `delivery-ledger.json` contains:

| Field | Meaning |
| --- | --- |
| `format` | Exactly `agenthub-delivery` |
| `schema` | Store envelope version `1` |
| `codex`, `claude` | The existing typed provider `Snapshot`, without a second receipt schema |
| `codex_previous`, `claude_previous` | Exact previous provider `Version`, or null at revision zero |

Every update atomically replaces this one file. Updating one provider retains the
other provider verbatim as typed data; there is no two-file transaction to tear.
`previous` is necessary to validate an identical proposal replay with its exact
original expected epoch/revision, including an epoch-changing recovery commit.

For a normal update, the expected version must equal the stored version and the
next revision must be exactly `previous + 1`, without overflow. The effect token
must equal its snapshot version. Replaying the last identical full snapshot with
its exact previous token returns the same acknowledgment after verifying disk
contents; changed payload, stale epoch/revision, or a changed same-token snapshot
is rejected. A later commit makes an earlier proposal stale.

All previous receipt IDs must remain, with the original complete request and
creation time. Claude also preserves original sequence identities. This prevents
updates from forgetting a tombstone or substituting another text, attachment
digest, session, child-agent scope, or terminal ownership target for a prior ID.
Both domain validators run on load and on commit. This is a persistence boundary
for trusted domain effects, not an alternative implementation of every state
transition or a verifier of native proof provenance.

## Restart and failure semantics

After `open`, each provider is independently gated until the caller restores its
loaded snapshot with a fresh process epoch using `Machine::restore`. The store
recomputes that pure recovery proposal and requires exact equality; changing an
epoch on an arbitrary ordinary snapshot is not accepted. Its durable recovery
commit releases that provider's gate. Recovery does not automatically start the
other provider, enqueue a CLI action, or replay an old pending callback.

The old process's same-epoch proposals and acknowledgments are not accepted on a
reopened handle. The application must generate unique epochs, discard stale
in-memory Machines, and retain the new Machine after recovery. Provider recovery
continues to map in-flight Prepare/Enter to `Uncertain`; native queued, accepted,
completed, and stopped distinctions remain the domain's responsibility.

Writes use a cryptographically random, same-directory, private `create_new`
temporary file, `write_all`, file `sync_all`, an additional old-file check,
atomic rename over the existing target, and Unix directory `sync_all`. The
installed bytes and directory/lock identities are checked before acknowledging.
The old ledger is never deleted to make replacement succeed.

| Failure boundary | Result |
| --- | --- |
| Validation, capacity, CAS, or before rename | No acknowledgment; old memory and ledger remain authoritative |
| Rename succeeded, later sync/verification failed | `Uncertain`; the entire handle refuses reads and commits for both providers |
| Restart after uncertainty | Explicit reopen + fresh-epoch domain recovery; no automatic reinjection |
| Missing/corrupt/unknown ledger on reopen | Fail closed and preserve files; no empty replacement |

If a before-rename failure is known, the pending **same Persist proposal** can be
retried; this is not permission to retry Prepare/Enter. Alternatively, the caller
can notify its Machine using that provider's `PersistenceFailed` command and
restart conservatively. The two domains use different command shapes and are not
normalized by this store. Any uncertain result requires dropping the old handle,
reopening, and restoring; retrying the same live frozen object is forbidden.

An interrupted first initialization may leave a lock and/or temporary file but no
ledger. Those are preserved and cannot be silently reinitialized. Manual recovery
must establish whether any old ledger or process exists before choosing a fresh
dedicated directory. There is no automatic ledger reset, tombstone expiry,
compaction, or recovery-from-temp heuristic.

## Filesystem and decoding protections

Only `.delivery.lock`, `delivery-ledger.json`, and exact module temp-name patterns
are permitted. Files must be regular, writable, `0600`, and singly linked. Every
read/write checks all directory ancestors for symlinks and reparse points, the
original directory identity, and the stable locked file identity. Opened ledger
file handles are checked against the pathname before and after bounded reads.
The lock is explicitly unlocked on drop, including when a duplicated file
description exists; its pathname is never unlinked or recreated by normal open.

Only the current invocation's exact successfully created temporary file is
cleaned on failure, with identity and ancestor checks. Abandoned temps are not
loaded as truth or cleaned automatically. These checks block persistent parent
directory/link replacement and accidental redirection. They are not a security
boundary against a malicious same-user process racing between filesystem calls;
directory-relative capability-based operations and native no-follow primitives
would be needed for that stronger guarantee. SHA-1 fingerprints here detect
accidental/external edits, not adversarial authenticity or access authorization.

The serialized file is capped at 32 MiB, with an additional 500,000-value and
64-level JSON budget. Provider limits remain in force: retained rows and exact
payload bytes cannot be evicted to satisfy a new request. Encoding uses a bounded
writer and checks the same structural budget as decoding before touching disk.
Reading first uses a recursive duplicate-key-rejecting visitor, then typed decode,
domain validation, and semantic JSON round-trip comparison. Consequently:

- Unknown nested fields, missing persisted nullable fields, duplicate receipt
  IDs/keys, invalid state combinations, and trailing documents fail closed.
- Legitimate JSON whitespace and field ordering are accepted on reopen; there
  is no requirement for byte-canonical formatting.
- An external rewrite while a handle is open, even whitespace-only, fails the
  expected fingerprint check instead of being overwritten.

These bounds are file/structure bounds, not a 32 MiB process-memory guarantee.
This initial correctness-oriented implementation clones whole typed snapshots and
uses transient JSON values during validation; peak allocation can be several
times the file size. Every commit rewrites and fsyncs the entire retained ledger
and verifies its bytes. It is deliberately not a high-throughput append journal;
benchmark before considering it production queue storage. Prompts and attachment
references in full-payload tombstones are private data requiring an explicit
retention/export policy, not material to expose through generic diagnostics.

## Platform and validation status

The durable disk backend is enabled only on Unix. Linux synthetic tests exercise
the local-file rename/file-fsync/directory-fsync contract and OS locking. They do
not simulate sudden power loss or certify network filesystems, storage hardware,
or macOS. The caller is responsible for establishing the dedicated directory's
own durable lifecycle; this module does not create or persist ancestor directories.

On Windows (WP-W) initialization/open work like the lifecycle store: the temp
file is synced and renamed through `MoveFileEx(REPLACE_EXISTING)`; the
standard library offers no directory handle to flush there, so the `Persisted`
acknowledgment rests on the file sync plus NTFS's journaled rename, and file
identity for the replacement checks is the creation time. Other non-Unix
targets return `DurabilityUnavailable` without touching files. The pure
Claude/Codex domains remain platform-independent.

Tests use only private synthetic temporary directories. They cover exact typed
acknowledgments, two-provider retention, CAS/replay, payload/tombstone protection,
pre/post-rename failures, recovery without reinjection, strict JSON, missing and
foreign data, lock exclusion and duplicated descriptors, permissions, links, and
persistent directory replacement. There is no HTTP/browser integration to claim
for this unexposed library, and no real CLI was started.
