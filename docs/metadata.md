# Development metadata store

This module persists SessionDock-owned preferences only. It never discovers CLI
homes, migrates predecessor state, writes native transcripts, sends terminal input,
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

The independent Rust schema starts at version 1.

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
When shown, sidebar metadata identifies them as parent sessions and includes
the native fork depth (or the original root). Fork leaves also show their depth,
so inherited titles do not make distinct generations look like duplicate rows.
The fork-chain menu still opens, shows and hides each ancestor independently;
labels never change saved visibility or native transcripts.

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
same rule the effective-tip computation uses): the native records become
authoritative again and the row reports `timeline_pin.retired: true` with
`retired_reason` ∈ `native_confirmed` (the signal is the tip itself),
`native_continued` (the first new node's parent is the tip — the CLI really
rewound), `native_advanced` (the CLI went past the pin — "CLI 未回滚"),
`native_diverged` (a new leaf that does not contain the tip), `tip_missing`.
Pins never write native files and never signal the CLI; every response says
`native_rewind: false`. Subagent views drop `timeline_pin`.

Legacy, under `timeline_pin: true`, offers a "回到此处" action under user
messages and a notice explaining that only the display is pinned and the CLI
was not rewound, plus the retirement reason when present.
A pin written because the CLI rewound on its own screen carries `cli: true`
(persisted and published on the row and view meta; see
[CLI state](cli-state.md)): its notice says the terminal rewind was followed,
offers no unpin, and disappears once the pin retires.
Validation: metadata/provider/session unit tests,
`cargo test -p sessiondock --test rewind_http --locked` and
`python3 tests/rewind_browser.py` (pin → trimmed history + explanation,
SSE retirement `native_advanced`, reload keeps state, 390 px pin/unpin, Web
restart persists, native file only appended).

## Sidebar parent

`POST /api/session/nest` stores one display-only parent, `nest_parent: {source, sid, node_id?}`.
Absent `node_id` means the child's own node. `{uid, parent_uid}` attaches to a listed
session; `{uid, parent_uid: null}` clears the relation. There is no independent flag,
startup-source fallback or restore action. Native subagent relationships remain in the CLI history.
Process launch evidence remains diagnostic and never changes sidebar attachment.

### `spawned_by`

Older metadata is read with a one-way migration: keep an existing manual parent;
otherwise move a previously displayed `spawned_by` into `nest_parent`. A legacy
`nest_independent: true` clears the relation. Legacy startup and independent fields
are absent from API responses and from the next metadata write. Clearing the parent
cannot be undone by process scans, polling or restart.

The handler rejects attaching to self (`400 nest_parent_self`), a missing target
(`404 nest_parent_missing`) or a descendant (`409 nest_parent_cycle`). Without a state
directory the route is `501 metadata_disabled`.

The Hub also accepts a parent on another registered machine. It resolves both
scoped UIDs against the fleet list, checks the complete displayed parent chain,
and forwards a trusted `remote_parent: {node_id, source, sid}` descriptor to the
child's node. The child's metadata owns the durable relation; Hub restart and
browser reload preserve it. The descriptor is accepted only on the authenticated
node listener, never from a local browser. Browser-supplied descriptors are removed
by the Hub before resolution. Hub nest writes are serialized through validation
and forwarding; a cyclic fleet edge returns HTTP 400. Direct node writes can only
validate their local inventory. Missing/filtered/offline parent rows do not retarget
the relation to a same-SID session on a different machine.

The legacy sidebar offers these from the session context menu: 解除附属 and 附属到… (click the parent, with 取消).

When rewind or continuation hides a recorded parent, the sidebar resolves its
children under the visible successor on the same node and source. Fork selection
uses the existing live-first, then newest-created order, and follows hidden
intermediate parents. Showing the original parent restores its own subtree.
This applies to the stored `nest_parent` relation.
Missing or filtered parents without a visible successor leave their children as
roots. These display decisions never rewrite the stored relationship.

Validation: `python3 tests/metadata_suite.py`,
`python3 tests/nest_tree_browser.py`, `python3 tests/hub_nest_browser.py`
(two actual nodes and Hub, cross-machine click attach, same-SID isolation, cycle
checks, node/Hub restart, detach and explicit local reattachment).

## Writer exclusion and durable publication

An in-process mutex serializes metadata updates. Each read and update reloads
external edits, retaining the current immutable snapshot when the file is unchanged.
Malformed or unsupported documents follow empty-read behavior.

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

## Node-owned session groups

`group_catalog: {groups: [name, ...]}` is optional in `session-metadata.json`.
Each session has at most one `group`. Names are trimmed, nonempty and
case-sensitive; identical names identify the same entry across nodes. Catalogs
retain unused entries and support creation, assignment and deletion.

- `GET /api/groups` reads the catalog, including imported session assignments.
- `POST /api/groups {create_groups?, delete_groups?}` creates or deletes names.
  Deletion clears that group's session assignments atomically on each node.
  It deletes only the organization preference, never sessions or native history.
- `POST /api/groups {groups?, changes?}` merges a catalog from synchronization.
- `POST /api/session/group {uid, set_group?, group?}` changes a listed session.
  Omitted or false `set_group` preserves its group; true with null clears it.
  Group, catalog additions and revision commit together. Other preferences and
  native CLI records remain intact. Without metadata these routes return
  `501 metadata_disabled`.

The Hub merges all registered enabled nodes' catalogs and distributes the result
back to reachable nodes every ten seconds, independent of browser filters.
`hub-cache/groups.json` is a rebuildable cache; nodes own the durable catalogs.
Hub creation requires at least one node to confirm the write. Responses expose
`synced_nodes` and `sync_errors`; offline nodes catch up after rejoining.
A node remains independently usable after restarting without a Hub.

The Group view shows named groups, including empty ones, with a delete button
on each heading; ungrouped sessions are omitted from this view. The final row is
New group. Clicking it edits the name inline; Enter or Create commits it, and
Escape cancels. Existing session tree/time views still show ungrouped sessions.

The session right-click/hold menu has a second-level group menu. Its first item
is Ungrouped, followed by named groups, and a click assigns immediately. Desktop
hover and ArrowRight open the submenu; ArrowLeft or Escape returns to the parent.
Multi-select uses the same immediate assignment menu. A single group filter
organizes sessions across machines; only the filter is browser state. Clone
preserves the group and catalog reads include imported names.

Catalog `changes` records the latest create/delete operation for each name,
including deletion markers. Stamps advance beyond observed operations and include
a random tie breaker. Merges keep the later operation, so offline catalogs and
cached session rows cannot resurrect a deleted group. Rejoining nodes remove
its assignments, while a later explicit creation can reuse the name. Deletion
markers persist even when all groups are deleted. Simultaneous independent
operations converge by stamp; they do not establish a global causal order until
nodes communicate.

Session tags have been removed from the UI, API and metadata model. Old
`label_catalog` documents are read only to preserve their groups; obsolete tag
fields are ignored and disappear on the next metadata write. The Hub likewise
reads the groups from its previous `labels.json` cache if the new cache is absent.
Tags are never converted into groups. Old tag API routes are no longer served.

Validation: `python3 tests/groups_browser.py --binary target/release/sessiondock` exercises two independent nodes
and a real Hub, inline creation/cancellation, immediate assignment and batches,
empty groups, removal without a dialog, hover/keyboard/mobile submenus, offline
deletion/rejoin, same-name recreation, restarts, and unchanged native records.
