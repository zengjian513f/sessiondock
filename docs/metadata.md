# Development metadata store

This module persists AgentHub-owned preferences only. It never discovers CLI
homes, migrates Python state, writes native transcripts, sends terminal input,
or starts a CLI. Enabling metadata does not enable general session mutations.

## Directory and schema

The caller must explicitly provide an existing, dedicated development directory.
The store does not create the directory. It accepts only its two fixed files,
unrecovered `.metadata-tmp-*` files and the debug-run registry the session
read model consults (`debug-runs.json`, plus its `debug-runs.json.tmp` replace
step, never read or written by this store — see
[read-model.md](read-model.md#debug_run-视图python-agenthubdebug_runspy));
other entries, including Python's `session-meta.json`, cause an error. Old temporary files are not read or cleaned.
Use a new local directory, not a production state directory or a native session
root. Directory ancestors cannot be symlinks; supply an actual physical path on
systems where a conventional temporary-directory prefix itself is a symlink.

Files:

- `session-metadata.json`: the committed document.
- `.metadata.lock`: a stable, exclusively locked writer file, retained on exit.
- `.metadata-tmp-<random>`: one operation's same-directory temporary file.
- `debug-runs.json` (optional, foreign): the debug-run registry, tolerated here
  and read by `sessions::debug_runs` only.

The independent Rust schema starts at version 1; this is not Python's version 1.

```json
{
  "schema_version": 1,
  "revision": 2,
  "sessions": {
    "claude:example": {
      "starred": true,
      "starred_at": 1789120800.0,
      "fork_parent_visible": true
    }
  }
}
```

UIDs are opaque ASCII keys, not paths: 1–256 bytes of letters, digits or `:._-`.
The API must separately verify that a UID exists and that a visibility target is
a valid fork parent. Bounds are 10,000 metadata records, 4 MiB per document,
1,000 UIDs per visibility transaction, 256-byte timeline IDs and 2,048-byte
activity reasons. Invalid timestamps, unknown fields, duplicate UID keys,
unknown schemas and corrupt JSON are rejected. No malformed document becomes
an empty store, and no existing invalid document is automatically overwritten.

On Unix, committed/lock files must be regular files with mode 0600 and one hard
link. Existing permissions are not silently repaired. The directory must be
owner-writable/searchable and not group/world-writable. On Windows, read-only
files and reparse points are rejected; ACL access failures remain errors.

## Snapshot and preference semantics

`MetadataStore::snapshot()` returns an immutable `Arc<MetadataSnapshot>`.
One snapshot provides a consistent `revision()`, `row(uid)`, `enrich_one` and
`enrich` view. Missing fields are absent; callers can supply explicit `false`
values for mutation responses. Enrichment replaces stale decoration fields,
never native message data, and needs the full topology even for a search subset.

`set_starred` preserves the first timestamp while already starred. Unstarring
removes only star fields. `set_fork_visibility` updates a validated batch as one
transaction; hiding removes the explicit preference. Empty rows are removed.
No-op updates retain the revision and existing `Arc`; a changed transaction
increments the durable revision once. The pure `with_*` methods apply the same
rules without file access.

Fork-parent detection is constrained by source and optional node identity.
Unsupported topology rows cannot invent an edge that hides another session.
Parents are hidden unless their explicit visibility preference is true.

Activity-stop and timeline structures are domain hooks for later verified
terminal integration, not authorization to perform native operations. An
activity stop can replace older working/waiting status but not a newer turn.
Only inferred stops can be cleared by inferred-stop cleanup. A pending rewind
survives restart without changing the confirmed display leaf; confirming it
requires a pending operation and must happen only after external native
confirmation. These methods themselves do not establish that confirmation.

## Timeline pins (batch 27)

`POST /api/session/rewind {uid, target, request_id?}` persists a display pin
for a Claude main session (`target` is the record node whose *preceding*
state should be shown — the pin's `tip` is its parent, matching Claude's own
"resume before this input" selector; `target: null` clears the pin).
The target is validated against the freshly opened main view (404 unknown node, 409
when it exists but is not on the current active timeline or nothing precedes
it); the store applies `with_timeline_pin` through the same atomic update
(a repeated identical pin is a no-op that keeps `pinned_at` and revision).
Without a state directory the route is 501 `metadata_disabled` and the
`timeline_pin` capability is false.

The read model applies a pin purely as parser options — `declared_tip = tip`,
`abandoned_after = stale_end` (the committed byte length frozen at pin time) —
so history, pages, SSE and search views equal the pure-option result already
covered by tests, and `meta.timeline_pin` is published on the session row.
The semantic anchor carries the pin stamp, so pinning, unpinning and
retirement produce a reset rather than a diff. A pin retires as soon as any
lineage signal appears after `stale_end` (a graph node or `last-prompt`, the
same rule Python's effective-tip computation uses): the native records become
authoritative again and the row reports `timeline_pin.retired: true` with
`retired_reason` ∈ `native_confirmed` (the signal is the tip itself),
`native_continued` (the first new node's parent is the tip — the CLI really
rewound), `native_advanced` (the CLI went past the pin — "CLI 未回滚"),
`native_diverged` (a new leaf that does not contain the tip), `tip_missing`.
Pins never write native files and never signal the CLI; every response says
`native_rewind: false`. Subagent views drop `timeline_pin`.

Legacy, under `timeline_pin: true`, offers a "回到此处" action under user
messages and a notice explaining that only the display is pinned and the CLI
was not rewound, plus the retirement reason when present; Python pages are
unchanged. Validation: metadata/provider/session unit tests,
`cargo test -p sessiondock --test rewind_http --locked` and
`python3 tests/rewind_browser.py` (pin → trimmed history + explanation,
SSE retirement `native_advanced`, reload keeps state, 390 px pin/unpin, Web
restart persists, native file only appended).

## `spawned_by` (batch 36)

`spawned_by: {source, sid}` records which session started this one; the
server writes it itself from the process tree while both CLIs are alive
([liveness.md](liveness.md#spawned_by)) — every `/api/live` and a 10 s
background tick call `MetadataStore::record_spawn_parents`, which applies
`with_spawn_parents`: the first relation is kept for good, a later different
clue is ignored, entries with an empty uid/source/sid are skipped, and a
malformed one (whitespace, control characters, `source` over 32 or `sid` over
256 bytes) fails the whole transaction. The value names the spawner's source
and native session id, not a UID, and the spawner row may no longer exist.
`enrich`/`enrich_one` put the object on the row as `spawned_by`; there is no
HTTP route to set or clear it, and stars/visibility/pins never touch it.
`tests/meta_import.py` carries Python's identical key over unchanged.

```json
"grok:example": {"spawned_by": {"source": "claude", "sid": "8accf618-…"}}
```

## Writer exclusion and durable publication

The store holds `std::fs::File::try_lock()` on `.metadata.lock` for its lifetime.
A second handle or process is refused, and the lock is never unlinked during
normal shutdown. The owner explicitly unlocks before closing its descriptor;
a concurrent fork's transient pre-exec duplicate cannot extend the declared
writer lifetime. A duplicate-handle regression verifies that dropping an old
duplicate does not release a newly acquired writer lock.
This is cooperative writer exclusion, not a sandbox against a
malicious process running as the same OS user. Before each transaction, the
store rechecks directory/lock identity and the exact prior document fingerprint;
external edits are conflicts, not permission to overwrite them. The OS mapping
and portability caveats are described in the [Rust File locking documentation](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock).

Publication order is:

1. Validate the full proposed snapshot and byte budget.
2. Revalidate the existing directory, lock and committed document.
3. Create a unique same-directory file with `create_new`, write and `sync_all`.
4. Revalidate the previous committed document, close the temporary handle, and
   rename over the destination. There is no delete-old-file fallback.
5. On Unix, synchronize the already-open directory handle.
6. Publish the new in-memory `Arc` and revision only after success.

Failures before replacement keep the old disk document and in-memory snapshot.
Cleanup targets only the exact temporary file created by this operation, with
an identity check; unrelated files and prior-run temporary files are preserved.
If replacement has happened but subsequent synchronization/validation fails,
the operation returns `metadata_commit_uncertain`. The store then rejects both
new snapshots and writes with HTTP-compatible 503 errors until restart. Existing
captured `Arc`s stay immutable, but are not a claim about the uncertain disk
state. The UI/health integration must expose this condition; blind retry is not
safe. Restart validates whichever complete document is actually present.

## Platform acceptance limits

Rust's [`rename` documentation](https://doc.rust-lang.org/std/fs/fn.rename.html)
describes its Unix and Windows replacement behavior. This implementation uses
the standard-library replacement operation on both; it never removes the old
file to accommodate a Windows sharing/ACL failure. File contents are flushed
before replacement. Unix directory synchronization is implemented; Rust does
not provide the same portable directory-fsync guarantee on Windows here.

Linux temporary-directory tests cover real persistence, restart, independent
writer processes, symlink/hardlink/permission rejection, external modification,
concurrent callers and injected pre/post-replacement failures. Linux success
does not establish Windows/macOS execution, Windows power-loss durability,
network-filesystem locking, or hardware/storage power-loss guarantees. Those
platform acceptance checks remain required before production use.
