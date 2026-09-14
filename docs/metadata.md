# Development metadata store

This module persists SessionDock-owned preferences only. It never discovers CLI
homes, migrates Python state, writes native transcripts, sends terminal input,
or starts a CLI. Enabling metadata does not enable general session mutations.

## Directory and schema

The configured state directory is created when needed. Existing permissions and
symlink ancestors use ordinary OS access rules. The store updates its own files
and leaves unrelated directory entries untouched.

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

UIDs are nonempty opaque strings. The API separately verifies that a UID exists
and that a visibility target is a valid fork parent. Metadata stores all retained
rows and attachment records. Timestamp and persisted-schema validation catch
invalid state without silently replacing the file with an empty store.

The state directory is created when needed. Existing directory permissions and
symlink/hardlink aliases follow OS access rules. Writes replace the named metadata
entry atomically and preserve unrelated files in the directory.

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

## Timeline pins

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

## `spawned_by`

`spawned_by: {source, sid}` records which session started this one; the
server writes it itself from the process tree while both CLIs are alive
([liveness.md](liveness.md#spawned_by)) — every `/api/live` and a 10 s
background tick call `MetadataStore::record_spawn_parents`, which applies
`with_spawn_parents`: the first relation is kept for good, a later different
clue is ignored, and entries with an empty uid/source/sid are skipped. The value names the spawner's source
and native session id, not a UID, and the spawner row may no longer exist.
`enrich`/`enrich_one` put the object on the row as `spawned_by`; there is no
HTTP route to set or clear it, and stars/visibility/pins never touch it.
`tests/meta_import.py` carries Python's identical key over unchanged.

```json
"grok:example": {"spawned_by": {"source": "claude", "sid": "8accf618-…"}}
```

## Writer exclusion and durable publication

An in-process mutex serializes metadata updates. Each read and update reloads
external edits, retaining the current immutable snapshot when the file is unchanged.
Malformed or unsupported documents follow Python's empty-read behavior.

Writes use a unique temporary file in the same directory, flush it, and replace
the metadata entry atomically. Failure before replacement preserves the previous
file. A post-replacement synchronization failure is reported for that operation;
the next read reloads the file and later requests can proceed without a restart.
Captured snapshots remain immutable.

## Platform acceptance limits

Rust's [`rename` documentation](https://doc.rust-lang.org/std/fs/fn.rename.html)
describes its Unix and Windows replacement behavior. This implementation uses
the standard-library replacement operation on both; it never removes the old
file to accommodate a Windows sharing/ACL failure. File contents are flushed
before replacement. Unix directory synchronization is implemented; Rust does
not provide the same portable directory-fsync guarantee on Windows here.

The regression suite covers persistence, restart, sequential writer handles,
external edits, concurrent callers and injected pre/post-replacement failures.
Platform execution and deployment results are recorded in the batch ledger.
