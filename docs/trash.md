# Session recycle bin (batch 26)

`DELETE /api/session/{uid}`, `POST /api/sessions/delete`, `GET /api/trash`,
`POST /api/trash/restore` and `POST /api/trash/purge` migrate `trash.py` as an
isolated, fail-closed capability. Everything answers 501 unless
`SESSIONDOCK_TRASH_DIR` names an existing private directory (absolute, no `..`,
symlink-free ancestry, Unix 0700, disjoint in both directions from the web dir,
native roots, ptyhost, state, delivery, lifecycle, launcher config, Codex index,
audit dir and every file root); `capabilities.trash` reports it.

## What is moved

The file set of a session comes only from the published index row, never from
a client path: the Claude main JSONL plus the `agent-*.jsonl` sidecars named in
`agent_items` and the matching `.meta.json`; the Codex rollout plus its owned
subagent rollouts; for Grok (WP-E, Python parity) the **whole session
directory** named by the row's `path` (`events.jsonl`, `updates.jsonl`,
`prompt_context.json`, … travel with it) — its `summary.json` must still be
present, otherwise 409 `changed_since_inventory`. Each file's
size/mtime/dev:ino is captured when the plan is built and re-verified right
before the rename; a mismatch is 409 `changed_since_inventory` and already
moved files are rolled back. A directory entry (`role: "directory"`) is
verified by its dev:ino only — its size and mtime legitimately move while the
CLI writes into it — and reports the sum of its regular files as `bytes`.
Symlinks are never followed (a link inside a Grok directory moves as a link);
outside Grok, directories, attachments and files the inventory did not name
are never touched. Moves use same-filesystem `rename` only (EXDEV → 409
`cross_filesystem`, no copy fallback). Entries live under `<trash>/<entry
id>/` with a `manifest.json` recording original absolute paths, stamps, UID,
source, title and the run state at deletion.

## Protection and liveness

- Fork parents: rows with a `history_base` object depend on its `thread_id`
  (or `forked_from_id`), regardless of `supported`; rows without one use
  `forked_from_id` only when `supported != false`. A parent still referenced by
  a non-deleted session is 409 `fork_parent_protected`, also when a batch names
  the child before the parent.
- Liveness uses a fresh runtime observation over the same frozen catalog
  (never the `/api/live` cache): `running` is always refused (409
  `session_running`, `force` does not override); `exited` proceeds; `unknown`
  (including `no_instance`, and `no_runtime` when no host dir is configured)
  requires `force:true` (409 `run_state_unknown` with `needs_force:true`).
  Unknown is not evidence of exit; the UI asks before forcing. A session the
  page itself stopped (Claude declared identity, or a Codex/Grok launch bound
  by process evidence / the operator — WP-E) answers `exited` from the host
  exit, the identity memory or the durable exit receipt, so no force prompt
  appears; Grok sessions take part since their `summary.json` `info.id` is a
  verified scope.

## Routes

- `DELETE /api/session/{uid}[?force=1]` → 200 `{ok, uid, title, entry_id,
  trash, files, bytes, run_state, forced}`; 404 `not_found`; 409 as above;
  403 for paths outside roots or symlinks; 500 `move_failed`.
- `POST /api/sessions/delete {uids ≤ 200, force?}` → always 200
  `{ok, deleted[], skipped[], failed[], errors[]}` (partial success is a 200);
  400 for empty/invalid/oversized lists.
- `GET /api/trash?limit≤200&cursor=` → `{ok, items[{id, entry_id, uid, source,
  sid, title, cwd, updated, deleted_at, deleted_ts, size, bytes, files, kind,
  origin, state, forced, run_state, recorded, restorable, reason?,
  reason_code?}], count, size, dir, limit, next_cursor, truncated}`;
  corrupt manifests are listed as `corrupt` and can only be purged.
- `POST /api/trash/restore {id}` → 200 `{ok, path, uid, source, title, files,
  bytes}`; 404 `entry_not_found`; 409 `restore_conflict` (any original path
  exists — no overwrite), `entry_not_restorable`, `restore_outside_roots`,
  `changed_since_trashed`. Restore renames file by file and rolls back on
  failure.
- `POST /api/trash/purge {id | ids ≤ 200 | all:true | days:N}` → 200
  `{ok, removed, freed, errors, failed, remaining}`; a single unknown `id` is
  404 (Python parity); `all`/`days` remove at most 200 entries per call and
  report `remaining`. Purge never touches native directories.

The session list excludes trashed sessions immediately (inventory refresh
after delete/restore). Legacy gains `trashCapable()`, a force confirmation for
`run_state_unknown`, `?limit=200` on the trash view with a count/truncation
note; without the capability its behaviour is unchanged.

## Differences from Python

Python deletes "orphan agents directories" on purge; Rust never deletes from
native directories (a purge removes the entry's own `files/`, including a
trashed Grok directory tree). Grok directories move whole since WP-E, like
Python. Legacy entries without a manifest are not inferred. Metadata
(`session_meta.discard/restore`) needs no mirror because it is keyed by UID;
delivery ledgers are not touched. Name-prefix liveness (`term.has_session`) is
not used (names are not identity evidence). `/api/session/{uid}` with other
methods is now 405 instead of the previous blanket 501.

## Validation

`cargo test -p sessiondock trash --locked` (8 unit tests: protection set,
manifest round trip, stamps) and `--test trash_http` (6: unconfigured 501,
delete/list/restore/purge round trip across providers, fork parent refused,
unknown state needs force, restore conflict 409, stamp change 409, partial
batch results, list excludes trashed); `python3 tests/trash_browser.py`
(desktop + 390 px: delete → force confirmation → trash view → restore, visible
refusal reasons).
