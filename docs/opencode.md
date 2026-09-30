# OpenCode sessions

OpenCode 2 keeps every session in one SQLite database (`opencode.db`:
`project`, `session_v2`, and ordered `session_message` rows). SessionDock
reads it without writing to it and projects each session into files the
session index already understands; everything downstream (list, history
pages, search, media, rows' live state) treats OpenCode as a fourth AI CLI
source with native UID `opencode:<hash>` and native SID `ses_…`.

Only the OpenCode 2 store (`session_v2`) is supported. A database without
that table mirrors nothing.

A pre-created launch is unused only while neither its catalog cursor nor the
accepted conversation window contains native records. An older empty catalog
snapshot cannot turn a populated, running session's stop action into deletion.
The action updates when the first native records arrive, even without a title
change.

## Configuration

| Variable | Meaning |
| --- | --- |
| `SESSIONDOCK_OPENCODE_DB` | OpenCode's database, e.g. `~/.local/share/opencode/opencode.db`. Opened read-only. |
| `SESSIONDOCK_OPENCODE_ROOT` | SessionDock's private mirror directory (created `0700`); the index's OpenCode root. |

Both are set together or not at all. A node that launches OpenCode also names
exactly one `opencode` CLI profile in its launcher configuration
([launcher](lifecycle-launcher.md)); that profile is also how SessionDock
calls `opencode api` to create and delete sessions.

## Mirror

`sessions::opencode::Mirror` polls the database once a second on its own
thread. Each session becomes `<root>/<project id>/<session id>/`:

- `summary.json`: `{"format": "sessiondock-opencode-mirror", "version": 1,
  "session": <session_v2 row>, "project": {"id", "worktree"}}`, rewritten
  atomically when the row changes;
- `messages.jsonl`: one line per message row in `seq` order,
  `{"id","type","seq","time_created","time_updated","data"}`. A user row
  also carries `content`: its text and each inline image as a base64 image
  block (the image bytes move out of `data.files`), the shape the media
  pipeline renders. An assistant row still streaming (no `time.completed`,
  no `finish`, nothing after it) is written once it settles. Lines are
  appended; when an exported row changes or disappears (revert, compaction)
  the file is rewritten atomically and the index reads it as a new file.

Identifiers are checked before they become path components. Sessions that
disappear from the database lose their directories; a restart re-derives
identical files without touching their stamps.

## History

The row comes from `summary.json` (`index::summary::opencode`): title (the
first prompt while OpenCode has none), directory, created/updated, model id
and agent. The view parser (`providers::opencode`) renders user prompts with
their images, `reasoning` as thinking, `text` as assistant output (`final`
on the last text of a `stop` turn), and each `tool` item as a call with its
result or error. A provider failure (`finish: error`, except a user
interruption) and a turn that failed without any reply (`idle` with
`outcome: failed`) show one `[OpenCode …]` notice. A turn the user
interrupted (`error.type: aborted`) keeps its partial reply as the turn's
last progress, marked interrupted, like a Codex `turn_aborted`.
`synthetic`, `model-switched`, `agent-switched`, `location-switched` and
`compaction` rows are bookkeeping.

A subagent runs in its own child session (`parent_id`), mirrored and listed
as a row of its own; the parent shows the `subagent` tool call with the
child's session id and answer.

## Launch, resume, send

A new OpenCode launch is `new_assigned`: the lifecycle store mints an id in
OpenCode's format, and before the host starts the launcher runs
`<profile> api session.create --data {"id", "location": {"directory"}}`
(bounded, 20 s; an existing id counts as created), then starts the TUI with
`--session <id>`. OpenCode 2 opens only existing sessions and otherwise
creates one with the first prompt, so this is what makes the identity known
at launch; the mirrored row appears right away and the page moves to it.
Resume uses `--session <sid>`. The composer recognizes OpenCode's prompt
([composer input](composer-input.md)) and SEND waits for the native user
record like the other CLIs. Its question form and permission prompt block
SEND as CLI questions; they are answered in the terminal.

## Delete

OpenCode has no recycle bin. Deleting an OpenCode row calls
`<profile> api session.remove` (which also removes its child sessions) and
drops the mirror directory at once. There is no trash entry and no undo; the
confirmation says so. A running session is refused like any other.
