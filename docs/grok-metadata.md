# Grok native metadata compatibility

The read-only Grok source discovers exactly `root/*/*/summary.json`. Each session
is identified by its directory: `grok:` plus the first 16 hexadecimal SHA-1
characters of that absolute directory path. A
session with only a summary or an existing zero-byte `chat_history.jsonl` remains
visible. No CLI is launched and no native files are created by the reader.

| Metadata | Selection |
| --- | --- |
| `sid` | `info.id`, otherwise session directory name |
| `title` | `generated_title`, otherwise `session_summary`, otherwise `新建 Grok 会话` (the pending-session label). Python still falls back to the first eight directory-name characters (DELTA). Collapse whitespace, truncate after 110 Unicode characters and append `…` when needed |
| `cwd` | `info.cwd`, otherwise percent-decode the parent directory name as UTF-8 with replacement; `+` stays literal |
| `created` | Normalize `created_at`, otherwise fallback file mtime |
| `updated` | Normalize the first truthy value of `last_active_at` / `updated_at`, otherwise fallback file mtime |
| `model` / `branch` | `current_model_id` / `agent_name` |

Fallback mtime comes from an existing chat file, including a zero-byte one, or
from the summary when chat is absent. It is rounded down to seconds.
Response timestamps are UTC ISO timestamps with
millisecond precision; parity comparisons normalize the local timezone.
First and last transcript-message timestamps never replace summary metadata.
A nonempty whitespace-only generated title is selected before whitespace
collapse, yielding an empty displayed title.
The decoded cwd is display metadata; it grants no filesystem access.

## The session preamble

Grok CLI writes the instructions as the first `chat_history.jsonl` record:
`{"type":"system","content":"You are Grok …"}`. Real sessions use `type:
system` only for that preamble (always offset 0, no `synthetic_reason`).
The projection skips it, same as `synthetic_reason` users and
`timeline_protocol` envelopes; it is not a conversation turn and does not
count toward the message total. Python still emits the record as role
`system` (DELTA). Unit case:
`sessions::providers::tests::grok_system_preamble_is_not_a_conversation_message`;
`tests/advanced_parity.py` keeps a synthetic `Grok system notice` in the
fixture and asserts it is absent from the Rust view.

## The `user_query` envelope

Grok writes a user turn as optional `<image_files>…</image_files>` blocks
followed by `<user_query>…</user_query>`; the images arrive as structured
parts, so the projection shows the body alone. A message sent while Grok was
still working, or after the previous turn was interrupted, additionally
carries a protocol prefix before the tags (`The user sent a message while you
were working:` / `The user interrupted the previous turn:`, image blocks may
sit on either side of it) and the reminder `Make sure to complete any
unfinished tasks from previous turns.` after them; none of that is the
user's text and it is stripped with the tags (`GROK_USER_QUERY` in
`sessions/providers.rs`, the reference `_GROK_USER_QUERY`). The match is
whole-text and case-insensitive, one leading and one trailing newline of the
body are removed and the user's indentation is kept; any other text outside
the tags (or a tag quoted inside ordinary text) leaves the record verbatim.
Unit cases: `sessions::providers::tests::grok_in_flight_user_query_envelope_is_removed`
and `grok_user_query_prefix_suffix_and_image_blocks_are_independent`;
`tests/grok_parity.py` compares the `inflight` session with the Python adapter.

## Missing files, cursors and live updates

`Candidate` retains its existing fields. Grok stamps are `[summary]` when chat
is absent and `[chat, summary]` when present; `data_stamp()` explicitly returns
an optional chat stamp. Restamping checks both files and detects creation,
deletion and replacement. An absent chat yields `start: 0`, `end: 0` and
`messages: []` for an initial read, `meta.chat_exists: false`,
`version.exists: false`, `version.size: 0` and `version.mtime: null`.
An existing zero-byte chat instead has both existence fields set to true and
its real mtime. Summary mtime is never presented as a fictional chat version.
`version.exists` is an additive field on the common message response.

Summary changes update inventory, search metadata and SSE metadata while
leaving the native body cursor unchanged. Missing-to-empty transitions also
change the immutable view revision even though the byte cursor is still zero,
so an open browser EventSource receives the existence change. Appended complete
lines use the existing cursor protocol; an unfinished JSONL tail is uncommitted.
Deleting a previously consumed chat requires a cursor reset. UID, fork and
agent ownership semantics remain unchanged.

## Metadata sources and differences

- `size`: regular files under the session directory
  are included recursively. The additive `chat_exists` stays; `size_scope` is
  gone.
- Summary and chat inputs have no additional Rust-only size, depth or entry
  gates.
- A malformed summary contributes no metadata. Readable
  symlinked files are handled through their targets. A missing optional chat
  counts as absence.
- The existing Grok message parser preserves a record's `timestamp` as `ts`.
  This metadata change
  does not change that behavior. The new parity test excludes message `ts`
  explicitly; it compares `created` and `updated` without that exclusion.
- Timestamp normalization covers RFC3339, naive date/time, dates and numeric
  seconds/milliseconds. This is not a claim to support every alternative ISO
  spelling.

## Validation

Build the server, then run
`python3 tests/grok_parity.py --python-source PATH --browser`.
The Python checkout is adapter-only and read-only. Tests create all data in a
temporary directory and bind the server to loopback. They access no CLI homes,
paid CLIs, production hosts, or active sessions.

Grok `run_terminal_command` results start with an
`exit: N` header (one space before the number);
it is not an exit code, so such results carry `exit_code: null` and
`error: false`. The
synthetic adapter comparison covers metadata precedence, timezone/numeric
timestamps, Unicode title/cwd, summary-only and empty chats. Chromium checks
real EventSource updates for existence alone, summary titles and appended
body, corruption/recovery, chat deletion, visible console explanations and
desktop/mobile rendering. Validation runs on Linux; cross-compiling would not
constitute runtime validation on Windows or macOS.
