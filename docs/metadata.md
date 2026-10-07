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
- `.metadata-tmp-<random>`: one operation's same-directory temporary file.

Unrelated files, including an old `debug-runs.json`, are ignored and left untouched.

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
rows and attachment records. Missing, damaged or unsupported documents are read
as empty state; a later update can replace them. There is no lifetime file lock.
An in-process mutex serializes updates, which reload the current document first.

The state directory is created when needed. Existing directory permissions and
symlink/hardlink aliases follow OS access rules. Writes replace the named metadata
entry atomically and preserve unrelated files in the directory.

## Snapshot and preference semantics

`MetadataStore::snapshot()` returns an immutable `Arc<MetadataSnapshot>`.
One snapshot provides a consistent `revision()`, `row(uid)` and `enrich`
view. Missing fields are absent; callers can supply explicit `false`
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

DELTA: the Python baseline stored activity stops (`网页发送 Escape`, inferred
terminal-exit or idle stops) and pending rewinds per session. SessionDock never
applied them; their structures were removed on 2026-10-07 and such fields in an
existing metadata file are ignored and dropped by the next metadata write.

## Timeline pins

A timeline pin follows a rewind Claude made on its own screen
([CLI state](cli-state.md)): the watcher validates the target against the
freshly opened main view (`target` is the record node whose *preceding* state
is shown; the pin's `tip` is its parent, matching Claude's own "resume before
this input" selector) and stores it with `cli: true` through
`with_timeline_pin` (a repeated identical pin is a no-op that keeps
`pinned_at` and revision). There is no HTTP route that sets or clears a pin.
DELTA: the operator's display-only "回到此处" pin and `POST
/api/session/rewind` were removed on 2026-10-07; pins stored without `cli`
by that action stay in the metadata file but are no longer applied.

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
Pins never write native files and never signal the CLI; the published pin says
`native_rewind: false`. Subagent views drop `timeline_pin`.

Legacy shows a notice that the terminal rewind was followed while the pin is
active; it offers no unpin and disappears once the pin retires.
Validation: `python3 tests/rewind_cli_browser.py`.

## Sidebar parent

`POST /api/session/nest` stores one display-only parent, `nest_parent: {source, sid, node_id?}`.
Absent `node_id` means the child's own node. `{uid, parent_uid}` attaches to a listed
session; `{uid, parent_uid: null}` clears the relation. There is no independent flag,
startup-source fallback or restore action. Native subagent relationships remain in the CLI history.
Local process evidence initializes `nest_parent` once for a newly created CLI
session: its owned process identifies the child, and inherited session identity
or an owning CLI ancestor identifies the parent. A detached dispatcher retains
this evidence. For OpenCode, which exposes no per-session file handle, a new
top-level row is paired with the OpenCode processes already running in its
directory at birth; every matching process must identify the same initiator.
An ambiguous directory, a native OpenCode subagent or a session older than
the process is not inferred this way. Resuming a session created before the
process does not attach it.
Before `/api/sessions` first publishes a new native identity, it performs local
process discovery against that exact inventory, bypassing the older live-scan
cache, then enriches and signs the rows with the recorded parent. Internal list
reads do not consume this gate. Unchanged identities keep the ordinary list byte
cache; background discovery still records evidence that arrives later.
Verified SSH initiators also initialize the same parent, with the remote node ID.
A private `nest_initialized` decision marker preserves automatic initialization
and explicit attach/detach across scans, exits and restarts; it is not a second
relationship or a public row field. Existing manual parents always win. Legacy
`nest_independent: true` migrates to a decided empty parent. Older versions that
erased a cleared row left no durable detach decision to recover.

### `spawned_by`

Older metadata is read with a one-way migration: keep an existing manual parent;
otherwise move a previously displayed `spawned_by` into `nest_parent`. A legacy
`nest_independent: true` clears the relation. Legacy startup and independent fields
are absent from API responses and from the next metadata write. Legacy inferred
parents carry `nest_initialized: false` for native validation: a parent newer than the child
or the child’s own continuation is removed during sampling. Explicit parents are
never rejected on creation time. Import, clone and move preserve a decided empty
parent, including the private marker. Clearing the parent
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

Validation: `python3 tests/nest_tree_browser.py`, `python3 tests/codex_exec_nest_browser.py`,
`python3 tests/hub_nest_browser.py`
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
is Ungrouped, followed by named groups, and a click assigns immediately. The final
New group item opens an inline name input; Enter creates the trimmed name and
assigns the current session or multi-selection to it. Escape cancels editing and
returns focus to New group; closing the menu discards the draft. Empty input
stays in the editor, and a failed creation preserves the name for retry. Desktop
hover and ArrowRight open the submenu; ArrowLeft or Escape returns to the parent.
Multi-select uses the same immediate assignment menu. Groups are browsed through
the top-right Group view; there is no group filter dropdown. Previous browser
group-filter preferences do not restrict any view or the catalog. Clone preserves
the group and catalog reads include imported names.

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
and a real Hub, sidebar and menu inline creation/cancellation, Enter creation and
assignment for single sessions, cross-node batches and touch menus, creation
failure/retry, draft focus, immediate assignment and batches,
empty groups, removal without a dialog, hover/keyboard/mobile submenus, offline
deletion/rejoin, same-name recreation, restarts, and unchanged native records.
