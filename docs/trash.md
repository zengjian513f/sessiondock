# Session recycle bin

`DELETE /api/session/{uid}`, `POST /api/sessions/delete`, `GET /api/trash`,
`POST /api/trash/restore` and `POST /api/trash/purge` provide recoverable deletion.
`SESSIONDOCK_TRASH_DIR` selects storage; missing directories are created.

The file set comes from the published inventory row. Claude and Codex transcripts
move with their indexed agent files; Grok sessions move as whole directories.
A finished launch receipt that declared or bound that native session is discarded
with its retained conversation draft, so the pending row does not return after
the native files leave the catalog. The page drops those receipts when it removes
the deleted rows, before its next terminal list arrives.
The entry manifest records original paths, session metadata and observed run state.
Moves support different filesystems by publishing a complete copy before removing
the source. Named symlinks move as links. Later file changes remain part of the
selected entry.

Fork parents referenced by another session and currently running sessions remain
protected. Liveness uses fresh managed-host and native CLI observations over the
same inventory. An absent managed receipt does not prevent deleting an inactive
session. Hub requests preserve the caller's `force` argument.

Restore uses the original paths recorded in the manifest and recreates missing
parent directories. Existing destinations produce `409 restore_conflict`. A failed
multi-file move attempts rollback and retains the recovery manifest when some
files could not be returned. Purge removes the complete selected trash entry.

The default listing includes all entries; optional `limit` and `cursor` provide
pagination. Batch deletion returns individual `deleted`, `skipped`, `failed` and
`errors` results. Purge accepts `id`, `ids`, `all:true`, or `days:N` and reports
`removed`, `freed`, `errors`, `failed`, and `remaining`.

Regression coverage includes native process protection, changed-content and
cross-filesystem round trips, symlink moves, restore conflicts, and batch results.

## Whole session trees

`session_delete_tree` adds a separate **删除会话树** action to the session menu
and detail actions. Its scope is the same connected component as whole-group
copy: ancestors, sibling branches, descendants, subagents, manually nested
sessions and every physical history generation. Claude, Codex and Grok may
coexist in one tree. Ordinary single-session and batch deletion keep their
existing parent protections. The tree-delete entry is disabled while the selected
session is running, with the same unavailable styling and input guard as tree
migration. Both the detail action and an open sidebar menu update when liveness
changes; execution still checks every member on the server.

- `POST /api/session/tree/plan` with `{uid}` previews the exact member list,
  logical session count and owned file sizes. It reuses the transfer relationship
  inventory; it does not stage a copy, rewrite identities or snapshot native DBs.
- `POST /api/session/tree/delete` with `{uid, request_id, uids}` confirms that
  exact physical member set. The server acquires the transfer member locks,
  refreshes the component and running-state observations, and refuses the entire
  request if membership changed, a member is running or a migration holds it.
- `POST /api/session/tree/progress` with `{uid, request_id}` returns the durable
  outcome plus active counters. Deletion continues if the browser disconnects.
  The page shows monotonic overall progress, individual file counts and byte
  counts during cross-filesystem copies, with a fixed-height detail line.

All owned native histories go into one manifest-backed trash entry. Claude
session directories, agent sidecars and file-history directories are included;
Grok session directories are moved whole. Shared native databases, name indexes
and SessionDock metadata remain in place, just as with ordinary trash, so a
restore retains custom titles, stars and nesting. The trash row identifies the
session count and offers **恢复整棵树**. Existing restore conflict checks and
rollback apply to the entire entry.

Small private `.tree-delete-*.json` receipt files live alongside the existing
trash entries; no database or background synchronization is introduced. A
repeated request ID returns its recorded result, including after restoration,
purge or service restart. It never deletes restored files again. Each moved file
updates the recovery manifest. After a service interruption, a completed manifest
can recover the result; an incomplete operation asks the user to inspect the
list/trash and create a fresh preview rather than blindly repeating deletion.
