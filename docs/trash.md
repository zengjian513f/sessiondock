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

Agy single and batch deletion uses the native CLI's resume picker, with an
explicit permanent-deletion confirmation and no recycle-bin entry or undo.
The worker selects the complete SID and verifies the native current-session
marker and confirmation before submitting. It verifies native removal before
refreshing the mirror and retiring launch receipts. Running sessions remain
protected; failures remain visible. See [Agy](agy.md) for the verified interface
and its evidence boundary. Whole-tree deletion remains unsupported for Agy.

DELTA: deleting a fork parent marks it **待删除**, hides it from the ordinary
list and search, and creates a recoverable trash entry. Its native histories stay
in place while any descendant exists, including descendants in the recycle bin.
Deleting a child to the recycle bin does not release that dependency. Permanently
removing the final descendant also permanently removes pending ancestors. A
restore cancels pending deletion without moving the histories.

Permanently deleting a pending parent explicitly includes all descendants. The
confirmation names those sessions, including descendants still in the ordinary
list. The server checks that exact preview again and refuses changed membership
or running members before moving any native file. It first stages surviving
native histories into recoverable trash, then purges the entries. A shared native
thread's immutable rollout generations are one logical deletion, so older
versions cannot reappear afterward. Unrelated sessions are excluded.

Pending manifests are authoritative. The list projects their visibility directly,
including after service restart, without another state cache or native-file writes
on ordinary reads.
Currently running sessions remain protected. Liveness uses fresh managed-host and native CLI observations over the
same inventory. An absent managed receipt does not prevent deleting an inactive
session. Hub requests preserve the caller's `force` argument.

Restore uses the original paths recorded in the manifest and recreates missing
parent directories. Existing destinations produce `409 restore_conflict`. A failed
multi-file move attempts rollback and retains the recovery manifest when some
files could not be returned. Purge removes the complete selected trash entry.

The default listing includes all entries; optional `limit` and `cursor` provide
pagination. Batch deletion returns individual `deleted`, `skipped`, `failed` and
`errors` results. Purge accepts `id`, `ids`, `all:true`, or `days:N`; a pending-parent purge also
requires `uids` matching the `sessions` and `cascade_sessions` preview from
`GET /api/trash`. Hub requests retain each node's confirmed UID set. It reports
`removed`, `freed`, `errors`, `failed`, and `remaining`.

Regression coverage includes deferred parent restore/restart/cascade and final-child collection (`parent_trash_browser.py`), native process protection, changed-content and
cross-filesystem round trips, symlink moves, restore conflicts, and batch results.

## Whole session trees

`session_delete_tree` adds a separate **删除会话树** action to the session menu
and detail actions. Its scope is the same connected component as whole-group
copy: ancestors, sibling branches, descendants, subagents, manually nested
sessions and every physical history generation. Claude, Codex and Grok may
coexist in one tree. Ordinary single-session and batch deletion use the deferred-parent behavior
above. The tree-delete entry is disabled while the selected
session is running, with the same unavailable styling and input guard as tree
migration. Both the detail action and an open sidebar menu update when liveness
changes; execution still checks every member on the server.

Detail deletion actions stay in the **更多会话操作** menu at every screen width,
with visible scope labels: **删除当前会话** uses the trash icon and
**删除会话树** uses the existing tree icon. They never flatten into adjacent
unlabelled trash buttons. The global **回收站** entry opens deleted items;
the deletion confirmations and tree member preview keep their existing scope.

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

Agy 删除成功后在私有镜像根保存持久删除回执，阻止原生旧索引重新导入已删除的空壳。
显式删除仅剩索引且数据库和 brain 均不存在的会话，只登记回执，不恢复或改写原生数据。
若原生载体实际重现，则撤销回执并重新显示。详见 [Agy 合同](agy.md)。
