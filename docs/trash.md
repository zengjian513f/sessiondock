# Session recycle bin

`DELETE /api/session/{uid}`, `POST /api/sessions/delete`, `GET /api/trash`,
`POST /api/trash/restore` and `POST /api/trash/purge` provide recoverable deletion.
`SESSIONDOCK_TRASH_DIR` selects storage; missing directories are created.

The file set comes from the published inventory row. Claude and Codex transcripts
move with their indexed agent files; Grok sessions move as whole directories.
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
