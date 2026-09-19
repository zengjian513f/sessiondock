# Terminal session recordings

A recording is the raw PTY output of one host session plus the resize and
exit events that occurred, with a checkpoint at the start of every on-disk
segment so the remaining oldest file can still rebuild the screen after
eviction. The host writes; the Web service only reads, including after the
host process has exited. Nothing here is the host's attach replay — that
path reconstructs the current terminal state for a live client, not a
complete historical byte log ([host attachment output](host-output.md)) —
and nothing here is a transcript: input is not recorded, and native JSONL
is not involved.

There is no separate index page: the session list is the index, and the
launch receipt is the SSH session (the recording is its archive, not its
identity). A managed session (`/api/term/list` session and pending rows)
carries `recording: {id, live, bytes, ended_ms, created_ms}` when a
recording of its host name exists. An exited shell session stays listed
until 删除 whether or not a recording exists, exactly like an agent
session's row (`pending_listed`): with a recording, opening its console
replays it read-only in the console pane itself (`term.js`
`attachRecordingReplay`: no claim, no input; the byte stream into
xterm.js, or `mode=grid` into the grid view when the renderer setting is
the server grid); without one (an old host, `--no-record`), the console
says 会话已结束，没有留下录制. A running shell session offers 停止
(`term/kill`: EOF first, the guarded stop only if the shell is still
there after 1.2 s, like `session/stop` for a CLI) and an exited one offers
删除 (`term/discard`, which also deletes the session's recordings). The
page follows the exit it observes itself (`T.ended`, `pendingPhase`):
the header action, the sidebar subtitle and the console notice change at
once, and a later list poll that still says running cannot flip them back.
A replay shows a timeline under the terminal
(`#term-timeline`: slider, play/pause, speed 1–16×, and 最新 while the
recording is live): a full-screen program's history is a sequence of
screens, not scrollback, so the slider seeks to any instant and play
replays the recorded pacing. `records.html` and `grid.html?record=` remain
as unlinked engineering pages used by the suites. Listing and replay take
no ownership lease, send no input, and do not talk to the host process
([terminal ownership](terminal-ownership.md),
[raw terminal input](terminal-input.md)). The files under the configured
ptyhost directory are the only source.

## On-disk layout

Each session is one directory under the host's `--dir`:

```text
<dir>/records/<created_ms>-<host_pid>-<name>/
  meta.json
  00000001.seg
  00000002.seg
  ...
```

`<name>` keeps only `[A-Za-z0-9._-]`; every other character becomes `_`,
at most 64 bytes. An empty result or a name that would start with `.`
gets a leading `_`. Segment files are `{index:08}.seg`. An empty directory
starts at index 1; the next index is always one past the highest
`NNNNNNNN.seg` name seen (including a short or unreadable file of that
name, which is skipped but still occupies the number).

A segment file is a 16-byte header plus frames. The first frame of every
new segment is a checkpoint, so dropping the oldest segment still leaves
an independently rebuildable screen.

Segment header (big-endian):
`magic "SDRC"(4) | version u16(2) | flags u16(2) | base_unix_ms u64(8)`.
Current version is 1; the writer stores flags `0`.

Frame (big-endian):
`kind u8(1) | len u32(4) | at_ms u32(4) | payload[len] | crc32 u32(4)`.
`at_ms` is milliseconds relative to that segment's `base_unix_ms`. crc32
(IEEE, the same polynomial as zlib) covers the contiguous bytes
`kind..payload`.

| `kind` | Frame | Payload |
| --- | --- | --- |
| 1 | Output | raw PTY bytes (the host has already stripped DSR queries; everything else is stored as-is) |
| 2 | Resize | `cols u16 \| rows u16` (big-endian) |
| 3 | Checkpoint | `cols u16 \| rows u16 \| state[..]` — full terminal state (history + current screen + modes + cursor), isomorphic to attach-replay bytes |
| 4 | Mark | UTF-8 JSON (format-legal; the host does not currently write this kind) |
| 5 | Exit | UTF-8 JSON, isomorphic to the host's attach exit payload |

A payload larger than `MAX_PAYLOAD` is corrupt. A checkpoint state's
`len` must also fit in `MAX_PAYLOAD - 4` (the two size words). Positions
are `(segment, offset)` with `offset` counted from after the 16-byte
header.

## Writer rules

Only the screen thread and session finish write. The store is
unbuffered: one `write_all` per append, no writer thread. Opening a
directory treats every existing segment as closed; `begin_segment` must
run before `append`.

The first frame of every segment is a checkpoint taken from the host's
terminal model (alacritty_terminal) **before** the next queued piece is
applied. Otherwise that piece
would appear both inside the snapshot and as a later output frame. The
checkpoint's `at_ms` is 0; the segment's `base_unix_ms` is the wall
clock at rotation.

Resize frames follow queue order. `Session::resize` updates the model
immediately, then enqueues a `Piece::Resize` so recording sees the same
order as other queued PTY bytes. A no-op resize (same cols/rows) is not
queued.

The exit frame is written after the model has drained already-published
bytes (`wait_applied`), then the open segment is closed and `meta.json`
is updated. Finish holds the record lock only after pending work is
gone, so Exit is last.

Dirty data is `sync_data`'d on a 2-second interval (`SYNC_INTERVAL`),
not per frame. `close` syncs then drops the open file. `Drop` closes.

Write failure stops recording and leaves the session running. The host
prints one `ptyhost: 录制停止（…）` line (or `ptyhost: 无法打开录制目录，本会话不录制`
if `Recorder::open` itself fails) and sets `active` false. Subsequent
checkpoint/output/resize/sync calls are no-ops. Exit still tries to
patch `meta.json`.

If `now_unix_ms - base_unix_ms` exceeds `u32::MAX`, that append is not
written. The writer opens a new segment with an empty checkpoint
(`cols=0`, `rows=0`, empty state) and retries; the reader will see a
gap.

`meta.json` is written through a `*.json.<pid>.tmp` file, mode `0600` on
Unix, then renamed. Fields at open:

| Field | Meaning |
| --- | --- |
| `name` | session name (updated in place on rename) |
| `host_pid` | host process id |
| `created_ms` | Unix milliseconds at recorder open |
| `argv` | argv passed to the child |
| `cwd` | working directory string, or `""` |
| `meta` | the host `run --meta` JSON object |
| `cols`, `rows` | size at open |
| `format` | `{"magic":"SDRC","version":1}` |

On session end the writer adds `exit` (the same JSON object as the Exit
frame: `code`, `output_complete`, and `reason` when incomplete) and
`ended_ms`.

## Limits

| Boundary | Limit | Policy |
| --- | --- | --- |
| Segment file (`segment_bytes`, including the 16-byte header) | 8 MiB (`8 << 20`); values below 4096 are raised to 4096 | Rotate: close the open file and `begin_segment` with a checkpoint |
| Directory total (`total_bytes`, sum of segment `len`) | 256 MiB (`256 << 20`); values below 4096 are raised to 4096 | While at least two segments exist and the sum exceeds the quota, delete the oldest file and drop it from the list. The open segment is the last entry and is never removed |
| Frame payload (`MAX_PAYLOAD`) | 16 MiB (`16 << 20`) | Refuse the write (`InvalidInput` / `PayloadTooLarge`); a failed writer stops recording |
| Follow interval | 150 ms | While the latest page is `at_end` and the host still looks live, wait this long (or until the browser closes / shutdown) and read again |
| Reader page (`PAGE_BYTES`) | 2 MiB (`2 * 1024 * 1024`) of Output / Checkpoint payload | The event that crosses the budget is included. Resize, Mark and Exit count 0. If any event remains, at least one is returned even when the budget is 0 |
| Browser binary chunk (`CHUNK`) | 32 KiB (`32 * 1024`) | Sanitized bytes are split into WebSocket binary frames of this size |

## Host flags and `info.record`

```text
ptyhost [--dir DIR] run --name N […]
        [--no-record] [--record-segment-bytes N] [--record-total-bytes N] -- CMD...
```

Recording is on by default, using the limits above. `--no-record` skips
the recorder entirely: no `records/` directory, and the `info` object
has no `record` key. `--record-segment-bytes` and
`--record-total-bytes` replace those two fields when recording is still
enabled (parse failure becomes 0, then the 4096 floor applies). Always
pass an explicit `--dir`; the imported host's implicit directory is not
this project's ([session host](session-host.md)).

When recording is active, `op: info` includes:

```json
"record": { "dir": "<absolute record directory>", "segments": <count>, "bytes": <sum of segment len>, "active": true }
```

`active` is false after a write failure. The object is absent only when
this session was started with `--no-record` or `Recorder::open` failed.

## Reader rules

The reader is read-only. It loads each bounded segment whole, ignores
names that are not `NNNNNNNN.seg` with a parseable 16-byte header, and
never panics on a corrupt byte.

`latest_checkpoint` walks segments from newest to oldest and returns the
**last** valid checkpoint in the first segment that has one. A newer
segment that contains only output does not hide an older checkpoint.
None means the directory has no usable snapshot.

`replay` takes that checkpoint and then `read_from` starting **after**
it, so the snapshot is not also emitted as an event.

`read_from(dir, from, max_payload_bytes)`:

- Missing `from.segment` with a later index present: start at the
  smallest greater index, offset 0, `gap = true`.
- Missing `from.segment` with nothing later: empty page, `next = from`,
  `at_end = true`, `gap = false`.
- `from.offset` past that segment's valid frames: treat the segment as
  finished; do not error.
- A leading (offset 0) checkpoint is emitted only when entering the
  segment through a gap, or when `from` points at it. Clean rotation
  skips it: the byte stream is continuous across the cut. Checkpoints
  at a nonzero offset are never emitted as events.
- Discontiguous segment indexes set `gap`.
- Exit is an ordinary event; it does not stop the page.

Incomplete tail versus crash tail: an unfinished frame on the **latest**
listed segment stops the page at that frame's start (`at_end = true`, no
gap) so the follower can poll. The same truncated or corrupt frame with
a later segment still on disk is treated as crash residue: skip to the
next segment and set `gap`. A decode error (`BadCrc`, unknown kind, bad
payload) follows the same rule.

## Browser wire

`GET /api/term/records` returns `{"records":[…]}` newest-first
(`created_ms` descending, then `id`). Each row:

`id`, `name`, `host_pid`, `created_ms`, `ended_ms` (optional), `exit`
(optional), `cols`, `rows`, `cwd`, `argv`, `meta`, `bytes`, `segments`,
`live`.

`id` is `<created_ms>-<host_pid>-<name>` with at most 20 digit
characters, 10 digit characters, and a 64-byte safe name. `live` is true
when `ended_ms` is absent and the host pid still exists (`/proc/<pid>`
on Linux; non-Linux treats the pid as alive). A missing `records/`
directory is an empty list. Without a configured ptyhost directory the
route is 501 `terminal_disabled`. An unreadable directory is 503
`records_unreadable`.

`GET /api/term/records/attach?id=…` is a WebSocket. Missing `id` is 400
`invalid_record`; unknown or unsafe id is 404 `record_not_found`; a
non-upgrade GET is 400 `websocket_required`. Replay needs no lease
([route ledger](route-ledger.md)).

All text frames are JSON with a `t` field. The browser controls only the
timeline (seek / play / pause / live); it never reaches the host.

| Direction | Frame | Meaning |
| --- | --- | --- |
| → browser | `{"t":"timeline","start_ms","end_ms","live"}` | first frame; bounds of the recording (first segment base, last frame time) |
| → browser | `{"t":"record","cols","rows","unix_ms","live","id"}` | a full state follows: on open, after every `seek` / `live`, and after a gap; reset the terminal |
| → browser | binary | sanitized terminal bytes (checkpoint or seek state, then output) |
| → browser | `{"t":"resize","cols","rows"}` | apply before the following bytes |
| → browser | `{"t":"gap"}` | data was lost; a fresh resize + checkpoint follows, reset the terminal |
| → browser | `{"t":"clock","unix_ms","end_ms"}` | playback position after each batch (and the grown `end_ms` while live) |
| → browser | `{"t":"exit","exit":{…}}` | the recorded host exit payload |
| → browser | `{"t":"end"}` | the tail of the recording was reached; preceded by the viewer reset bytes; the socket stays open for seeking |
| browser → | `{"t":"seek","unix_ms"}` | rebuild the screen as of that instant (clamped to the bounds) and pause there |
| browser → | `{"t":"play","speed"}` | play from the current position with the recorded gaps divided by `speed` (default 1); at the end, restart from the start |
| browser → | `{"t":"pause"}` | stop paced playback |
| browser → | `{"t":"live"}` | jump to the latest checkpoint and follow the tail again (fast mode) |
| browser → | anything else | ignored |

Playback has three modes. *Fast* (on open and after `live`): every page
is applied as fast as it reads, then the tail is followed every 150 ms
while the host is alive; when the tail is reached and the host is gone,
`end` is sent and the mode becomes *paused*. *Paused* (after `seek`,
`pause` or `end`): nothing is sent until the next control. *Playing*
(after `play`): events are applied in batches of at most 20 ms of
recorded time, each batch waiting the recorded gap (capped at 2 s)
divided by `speed`; a live recording that is caught up switches back to
fast mode.

A `seek` finds the newest checkpoint at or before the instant
(`reader::checkpoint_before`), runs the terminal model silently over the
events up to it, and sends `record` plus that screen: on the grid wire a
`reset:true` snapshot, on the byte wire the model's re-rendered state
(`Screen::replay_bytes`), so a byte viewer sees exactly what the grid
viewer would. Seeking inside a segment is therefore bounded by one
segment (≤ 8 MiB) of model work.

A directory with no checkpoint closes with 1011 `record has no checkpoint`.
An unreadable page closes with 1011 `record unreadable`. Shutdown is
1001. Mark events are dropped on this wire.

## Grid replay

`GET /api/term/records/attach?id=…&mode=grid` replays the same recording
through the terminal model inside the Web service and streams the grid
protocol instead of sanitized bytes; see
[server-side terminal grid](terminal-grid.md#recordings-and-the-shared-model).
No sanitizing is needed on that wire because the model consumes every
query and its answers are dropped.

## Sanitizer

Read-only replay feeds recorded bytes to the browser's xterm.js. Query
sequences that would make xterm.js answer the "host" would write DSR/DA
replies into a **live** session, and OSC 52 can touch the clipboard. The
reader strips those queries before the socket; the on-disk record still
contains them (except the DSR set the host already removed).

Only 7-bit ESC (`0x1B`) introduces a sequence. A C1 CSI byte (`0x9B`)
and other C1 controls are ordinary bytes. Sequences not listed below —
cursor motion, SGR, DECSET/DECRST, OSC title/hyperlink, sixel, and the
rest — pass through unchanged. Splitting a stream across chunks must
not change the output. An unfinished sequence at end-of-stream is
**not** a query: `Sanitizer::finish` emits the held bytes as-is. OSC/DCS
content is allowed to grow to 65536 bytes without a terminator; past
that the held bytes are flushed as ordinary output so a runaway string
cannot stall the filter.

Stripped set:

- CSI DSR / DECXCPR: `ESC[5n`, `ESC[6n`, `ESC[?6n`
- CSI DA1 / DA2 / DA3: `ESC[c`, `ESC[0c`, `ESC[>c`, `ESC[>0c`, `ESC[=c`, `ESC[=0c`
- CSI XTVERSION: `ESC[>q`, `ESC[>0q`
- CSI DECRQM: parameters starting with `?` or a digit, ending with `$`,
  final `p` (for example `ESC[?2026$p`, `ESC[4$p`)
- CSI window/title reports: final `t`, first parameter 11, 13, 14, 16, 18, 19, or 21
- OSC 52: clipboard read/write, any form
- OSC 4, 5, 10–19: color queries (last non-terminator content byte is `?`)
- DCS XTGETTCAP (content starts with `+q`) and DECRQSS (content starts with `$q`)

After the last event the viewer appends `VIEWER_RESET` and then `end`.
The sequence turns off mouse tracking, focus reporting, bracketed paste
and DECCKM, restores the numeric keypad and a visible cursor, turns off
synchronized output, and resets SGR. The viewer must not write these
bytes into a session that is still being recorded.

```text
ESC[?1000l ESC[?1002l ESC[?1003l ESC[?1005l ESC[?1006l ESC[?1015l
ESC[?1004l ESC[?2004l ESC[?1l ESC> ESC[?25h ESC[?2026l ESC[0m
```

Exact bytes:
`\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1004l\x1b[?2004l\x1b[?1l\x1b>\x1b[?25h\x1b[?2026l\x1b[0m`.

## Hub

`GET /api/term/records` is per machine: it is not one of the hub
aggregate reads. On a hub page, `records.html?node=<nid>` prefixes every
call with `/api/nodes/{nid}/api/` so the list and the attach socket hit
that node ([hub](hub.md)).

`/api/term/records/attach` is proxied as a WebSocket the same way as
`/api/term/attach`: after the 101, the hub copies the browser connection
and the node's TCP/TLS stream bidirectionally and does not re-frame.

## Privacy

A recording contains everything the terminal showed: secrets in CLI
output, OSC clipboard payloads until the sanitizer strips them on
**view**, titles, and any other bytes the child wrote. It does not
contain keystrokes or paste bodies; those never enter the record.

On Unix the `records/` directory and each session directory are `0700`,
and each `*.seg` and `meta.json` is `0600`. `--no-record` is per host
process.

## Known limitations

The model may resize before queued bytes are applied, so a resize can
land a few bytes early in the record: `resize` updates the model
immediately and only then enqueues the Resize piece.

The checkpoint `state` is the host model's own re-rendering of its
screen (alacritty_terminal cells serialized back to escape sequences,
the same reconstruction attach uses); the viewer's xterm.js can disagree
with it in edge cases (soft-wrap flags are not preserved). Later output
frames are the raw PTY tail and are not affected.

There is no compression. Segment `flags` is 0.

## Validation

`cargo test -p ptyhost --test host_record --locked` runs the Unix
`/bin/sh` fixture in `crates/ptyhost/tests/host_record.rs` with an
explicit private `--dir`: output, resize and exit order, `info.record`,
`--no-record` writing nothing, and small-segment rotation that keeps a
leading checkpoint.

`python3 tests/term_records_http_suite.py --binary target/release/sessiondock`
is the HTTP/WebSocket contract (no Chromium): 501 when the terminal
transport is off, bad ids, upgrade required, `timeline` then `record`,
live follow, resize, ignored inbound frames, exit/end with the socket
kept open, replay of an ended recording, and the timeline (seek to the
start shows nothing later, play at 16x reaches the end, seek to the end
shows the final screen, pause accepted). Needs POSIX, a built
`sessiondock`, and a built `ptyhost`.

`python3 tests/terminal_timeline_browser.py` is the console timeline
acceptance for both console renderers (Playwright Chromium, temporary
fixtures): an exited SSH row opens its recording read-only with the
timeline shown, seek to the start, play at 16x to the end, seek to the
end, keyboard ignored, timeline hidden again when the pane closes.

`python3 tests/terminal_records_browser.py` is the legacy `records.html`
acceptance (Playwright Chromium, temporary fixtures): list, live follow,
resize, read-only keyboard, exit status, `?id=` reload, fit toggle, no
page errors. Needs the debug `sessiondock` and `ptyhost` binaries
already built.
