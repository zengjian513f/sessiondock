# Server-side terminal grid

The host keeps one VT emulator (alacritty_terminal behind `Screen`). A grid
client receives cells, not escape sequences, and the browser only paints.
Byte-console clients still get the raw PTY stream and parse it in xterm.js.
Recordings stay a byte log of that same PTY
([terminal session recordings](terminal-records.md)): they are not this
protocol, and this protocol is not a recording.

The page is `grid.html`. It uses the same claim and attach lease as the
byte console ([terminal ownership](terminal-ownership.md),
[raw terminal input](terminal-input.md)). Hub pages pass `?node=` and
prefix calls with `/api/nodes/{nid}/api/` the same way as
`records.html` ([hub](hub.md)).

## Purpose

PTY bytes enter the host read thread. That thread forwards the stripped
stream to every **byte** attachment and queues the same pieces for the
screen thread. The screen thread is the only caller that feeds the model
and the only caller that emits grid JSON.

A grid client therefore never sees CSI/OSC/DCS. The host already turned
the screen into rows of spans. The browser draws those spans, reflows
its own scrollback on resize, and sends keystrokes as raw PTY bytes —
the same inbound path the byte console uses.

The model is shared with attach-replay reconstruction, `capture` /
`cursor`, and recording checkpoints. Live byte traffic does not pass
through it ([host attachment output](host-output.md)).

## Attach

`GET /api/term/attach` is the same WebSocket as the byte console. Adding
`mode=grid` selects this protocol.

| Piece | Contract |
| --- | --- |
| Query | `AttachQuery.mode`: the string `"grid"` becomes `AttachMode::Grid`; any other value, including the empty default, is `AttachMode::Bytes`. Default size when the query omits them is 120×32 |
| Claim / lease | Identical to the byte console: `POST /api/term/claim`, then attach with `name`, `page`, `token`, and the same identity fields (`uid`+`instance_id`, or `record_id`+`launch_id`+`instance_id`, or raw name) |
| Host `op: attach` | Fields `op`, `cols`, `rows`, optional `replay`, optional `mode`. Guard whitelist: `mode` absent, `"bytes"`, or `"grid"`; anything else is rejected before dispatch (`crates/ptyhost/src/guard.rs`) |
| Client library | `ptyhost_client::AttachMode::{Bytes, Grid}`. `attach` / `attach_bound` / `attach_launch` default to `Bytes` and omit the field. `attach_mode` (and the bound/launch variants) set `"mode":"grid"` only for `Grid` |
| Replay | The Web service still passes `replay: true`. The host ignores it for `mode=grid`: there is no byte reconstruction. The first snapshot is produced on the screen thread so it is contiguous with later diffs |
| Hub | Unchanged. After the 101, `/api/term/attach` is a bidirectional TCP/TLS copy and is not re-framed; `mode=grid` rides in the query string |

Byte and grid attachments may share one host process. The read thread
still publishes raw frames only to byte clients; the screen thread
publishes JSON lines only to grid clients. `info.attached` is true when
either kind is live. Per-session attachment reservations remain 32
([host attachment output](host-output.md)).

A new grid client is parked in `grid_pending` until the screen thread
sends a `reset: true` snapshot, then moves to `grid_clients`.

## Wire

Host frames are the existing attach binary protocol: a JSON-line
acknowledgement (`{"ok":true,"cols","rows"}`, plus `instance_guard` /
`launch_guard` when those envelopes applied), then `FRAME_DATA` payloads
and a `FRAME_EXIT` payload. Each grid `FRAME_DATA` is UTF-8 JSON **one
object per `\n` line**. The Web service forwards those payloads as
WebSocket **binary** frames, split at 32 KiB (`OUTPUT_CHUNK`). A line
may straddle frames; the browser buffers across them (`LineDecoder`
splits on byte `0x0A` so a UTF-8 character split across frames stays in
the byte tail).

Text frames are control notices from the Web service, not the host.
Unknown text frames are ignored.

| Direction | Frame | Meaning |
| --- | --- | --- |
| → browser | binary | UTF-8 JSON lines (`snapshot` / `diff`) |
| → browser | `{"t":"revoked","ip","by"}` | ownership replaced; then close 4001 `revoked:<ip>` (empty `ip` when the new claimant is at the same display address). Launch retirement is close 4002 `launch retired` with no notice |
| → browser | close 1000 `host exited` | host exit frame with complete output |
| browser → | binary | raw PTY bytes (unchanged from the byte console) |
| browser → | text `{"t":"resize","cols","rows"}` | PTY resize (unchanged). Any other text, including JSON that is not a valid resize, is treated as literal PTY input |
| browser → | Close / disconnect | ends the attachment |

A `seq` gap is counted (`lostMessages`) and is **not** requested. A
reconnect gets a fresh `reset: true` snapshot.

### Row

```text
{"s":[[text, fg, bg, flags], …], "w": wrapped}
```

`s` is an array of spans. A span is consecutive cells with the same
foreground, background, flags, and cell width. `WIDE_CHAR_SPACER` cells
are skipped. Empty cells are the string `" "`. Trailing spans that are
default-attribute spaces are omitted; a trailing default-attribute span
has its trailing spaces trimmed. The browser pads to `cols`. `w` is the
soft-wrap flag on the last column (`WRAPLINE`).

Grapheme splits of `text` use `Intl.Segmenter` when present, else
`Array.from`. A wide span (`flags & 32`) occupies two columns per
grapheme and stores no spacer cell.

### Colors

| Encoding | Meaning |
| --- | --- |
| `-1` | default (browser theme foreground / background) |
| `0..=255` | indexed. Named colours below 16 stay as that index; other named colours become `-1` |
| `0x1000000 \| (r<<16 \| g<<8 \| b)` | truecolour (`Color::Spec`) |

The renderer maps indexes 0–15 through the xterm/VS Code 16-colour
palette, 16–231 through the 6³ cube (`55 + n*40`, zero stays 0), and
232–255 through the gray ramp (`8 + (i-232)*10`). Bold plus a
foreground in 0–7 uses the bright pair 8–15.

### Flags

| Bit | Name | Render |
| --- | --- | --- |
| 1 | bold | weight 600 |
| 2 | dim | canvas `globalAlpha` 0.5 |
| 4 | italic | italic font |
| 8 | underline | 1 CSS-pixel fill at baseline+2 (all underline styles collapse to this bit) |
| 16 | inverse | swap fg/bg before paint |
| 32 | wide | each grapheme in the span occupies two columns |
| 64 | strikeout | 1 CSS-pixel fill at mid-cell |
| 128 | hidden | skip glyph, underline, and strike |

There is no hyperlink attribute and no underline colour.

### `snapshot`

Sent to a pending client with `reset: true`, and to live clients after
a resize (or when the screen thread has no previous grid state) with
`reset: false`.

| Field | Meaning |
| --- | --- |
| `t` | `"snapshot"` |
| `seq` | monotonically increasing `u64` per session grid stream |
| `reset` | `true`: replace the browser scrollback with `history`. `false`: replace the viewport only; if `cols` changed, the browser reflows its existing scrollback |
| `cols`, `rows` | viewport size |
| `grid` | `rows` row objects, index 0 = top of the visible screen |
| `history` | recent scrollback rows. For `reset: true`, the last `SNAPSHOT_HISTORY_ROWS` (2000) rows, oldest first. For a live resize snapshot, `[]` |
| `history_total` | absolute history length in the model (may exceed `history.length`) |
| `cursor` | `{x, y, visible}` in the viewport, 0-based |
| `modes` | see below |

`title` is not on this message.

### `diff`

Omitted entirely when nothing changed (and `seq` is not consumed).
Present fields are only the ones that changed.

| Field | Meaning |
| --- | --- |
| `t` | `"diff"` |
| `seq` | next sequence number |
| `scrolled` | rows that left the **primary** screen since the previous capture, oldest first. Absent when empty. Not collected while the next state is on the alt screen, and not collected across a resize snapshot |
| `rows` | `[y, row]` pairs; `y` is a viewport row |
| `cursor` | `{x, y, visible}` when it moved or visibility changed |
| `modes` | when any mode field changed |
| `title` | OSC title string when it changed (including a reset to `""`) |

The browser appends `scrolled` only when its **current** `modes.alt` is
false (the value before this diff is applied).

### `modes`

```text
{
  "alt": bool,
  "app_cursor": bool,
  "app_keypad": bool,
  "bracketed_paste": bool,
  "focus_events": bool,
  "mouse": "none" | "press_release" | "button_motion" | "any_motion",
  "mouse_encoding": "sgr" | "utf8" | "default"
}
```

`mouse` prefers motion, then drag, then press/release. `mouse_encoding`
prefers SGR, then UTF-8, else X10/default.

## Flush policy

The screen thread does not use a fixed timer. `FlushPolicy` records the
first dirty instant and the last change.

| Constant | Value | Rule |
| --- | --- | --- |
| `QUIET` | 1 ms | after the queue is empty **and** this long has passed since the last change, flush |
| `CAP` | 8 ms | after this long since the first unsent change, flush even if the queue is still busy |
| DEC 2026 | held in the VTE parser | `ESC[?2026h` buffers in the model; grid flush is skipped while `sync_deadline()` is `Some`. `ESC[?2026l` applies the buffer. If the end marker never arrives, `expire_sync` applies it when the parser timeout fires, then the grid is marked dirty |
| resize | snapshot | `Piece::Resize` sets `need_snapshot`; live clients get `reset: false` with empty `history` |
| first snapshot | screen thread | pending clients are not snapshotted under the attach lock. The screen thread captures once and sends that snapshot, then diffs from the same `GridState`, so the stream has no hole |
| `SNAPSHOT_HISTORY_ROWS` | 2000 | cap on rows copied into a `reset: true` snapshot. Older history is not paged on this wire |

A pending client with no dirty state flushes immediately. Otherwise the
1 ms / 8 ms rule applies. The idle wait is at most 200 ms, shortened to
the next policy deadline (floor 200 µs) or the DEC 2026 deadline.

Host `--history` defaults to 10_000 rows. That is the model's
scrollback, independent of the 2000-row snapshot window.

## Query answering

The model answers terminal queries (DA, DECRQM, XTGETTCAP, colour
queries, …) into `Screen::take_responses`. Colour replies use a fixed
dark palette in the host (the browser theme is not available there).

| Query | Who answers |
| --- | --- |
| DA, DECRQM, XTGETTCAP, OSC colour queries, other `Event::PtyWrite` | **Byte client present:** left to xterm.js (the host does not write the model's responses). **No byte client** (grid-only or nobody attached): the screen thread writes `take_responses()` to the PTY |
| DSR / DECXCPR (`ESC[5n`, `ESC[6n`, `ESC[?6n`) | **Always the host.** The read thread strips them before any client sees the bytes and the screen thread replies from the model cursor. Behaviour does not depend on who is attached |

Stripping DSR on the read path is the same rule recordings use: those
queries never reach a client and are not stored.

## Browser modules

`legacy-web/grid.js` is the page. The four modules under
`legacy-web/grid/` have no DOM except `input.js`'s hidden-textarea
helper.

| Module | Owns |
| --- | --- |
| `grid/wire.js` | `LineDecoder` (newline-delimited JSON, UTF-8-safe), `encodeResize`, grapheme `segmentText` (cache 256) |
| `grid/model.js` | viewport, scrollback, cursor, modes, title, `seq`. Applies `snapshot`/`diff`. Materializes cells lazily. Default `scrollbackLimit` 100_000. **Does not reflow the viewport** (the host resends it) |
| `grid/render.js` | Canvas 2D. Metrics: `"W"` advance (CJK `"中"` / 2 as fallback), `cellHeight = round(fontSize * lineHeight)` with defaults 14 px and 1.2, baseline from `'M'.actualBoundingBoxAscent` plus vertical centering. Backing store is CSS × `devicePixelRatio`; the context is scaled by `dpr`. Fit floor: 2 columns, 1 row |
| `grid/input.js` | `InputEncoder`: keys, paste (newlines → CR; bracketed `\x1b[200~…\x1b[201~` when that mode is on), focus (`\x1b[I` / `\x1b[O`), mouse (SGR / UTF-8 / X10, default X10 clamped at 223), alt-screen wheel-as-arrows (3 lines). `KeyCapture` holds a hidden textarea |
| `grid.js` | claim, attach `mode=grid`, list, fit/resize, follow/scroll, selection, copy/paste, mouse reporting vs selection, IME, title, reconnect |

Scrollback reflow happens only in the model, on `reset: false` when
`cols` changes and on an explicit `reflow`/`resize`. Wrapped runs are
concatenated and rewrapped; a wide cell is never split. The viewport is
replaced by the next snapshot/diff from the host.

Selection is a half-open column range on model lines (scrollback +
viewport). Drag on the canvas; Shift-drag still selects when mouse
reporting is on. Double-click selects a word (ASCII alnum, `_`, or
code point > 127). Copy uses `selectionText` (no newline inside a
soft-wrapped logical line) via the Copy button or Ctrl/Meta+C.
Paste uses the clipboard button, the textarea `paste` event, or
`navigator.clipboard.readText`; the encoder applies bracketed paste
when the host mode is on.

Mouse reporting: when `modes.mouse !== "none"` and Shift is not held,
pointer events become encoded mouse sequences in viewport coordinates.
Alt screen with `mouse === "none"` turns the wheel into arrow keys (3
per notch). Otherwise the wheel moves the local viewport by 3 lines
and leaves follow mode.

IME: `keydown` is ignored while `isComposing`. `compositionend` and
non-composing `input` emit the composed text as PTY bytes. The
encoder also returns `null` for `isComposing` key events.

Inbound PTY input is `WebSocket.send(Uint8Array)` of those bytes.
Resize is the text frame `{"t":"resize","cols","rows"}`. Both match
the byte console.

## Limits and known gaps

| Boundary | Limit | Policy |
| --- | --- | --- |
| Snapshot history | 2000 rows (`SNAPSHOT_HISTORY_ROWS`) | Older model history is not on this wire; there is no history-page request |
| Browser scrollback | 100_000 rows | Drop from the oldest |
| Scrolled-row flood | none | Every row that left the primary screen between captures is sent in `diff.scrolled` |
| WebSocket host payload | 32 KiB chunks | Split only; lines are reassembled in the decoder |
| Attach query size default | 120×32 | Browser fit replaces this on open |
| Host `--history` | 10_000 default | Model scrollback, not the snapshot window |
| Attachment reservations | 32 per session | Shared with byte clients |

Known gaps:

- No OSC 8 hyperlinks and no underline colour or underline style beyond
  the single underline bit.
- No history paging past the 2000 snapshot rows. `history_total` tells
  the client that more exists; nothing fetches it.
- Alt-screen scrollback is not recorded: `scrolled` is empty while the
  next state is alt, and the browser ignores `scrolled` while it is
  already on alt.
- A flood of output sends **all** newly scrolled rows, not a sampled
  tail.
- `title` travels only on diffs, so a client that never sees a title
  change keeps an empty title until one arrives.
- The host does not emit a snapshot because of a `seq` gap; only
  reconnect (or a live resize) produces one.

## Validation

`cargo test -p ptyhost --test host_grid --locked` runs the Unix `/bin/sh`
fixture in `crates/ptyhost/tests/host_grid.rs` with an explicit private
`--dir`: first snapshot then row diffs, scrolled rows plus a second
client's history snapshot, resize `reset: false` without history, alt
screen / title / exit, and a byte client coexisting with a grid client
(the byte stream is not JSON).

`node --test tests/grid_model_contract.mjs` covers `LineDecoder`
(split/merge, UTF-8 across pushes, dropped bad lines), `encodeResize`,
grapheme splits, snapshot `reset: true` padding, wide cells, diff
scrolled/row/cursor/modes/title, `seq` gaps, scrollback reflow, and
selection text across wrapped lines.

`node --test tests/grid_input_contract.mjs` covers `InputEncoder`
keys, paste, focus, mouse encodings, and alt-screen wheel-as-arrows
(≥ 40 cases).

`python3 tests/terminal_grid_browser.py` is the legacy `grid.html`
acceptance (Playwright Chromium, temporary fixtures): claim and
connect, typing, PTY resize follow, scrollback wheel/follow, selection
copy, paste, host exit, no page errors. Needs POSIX and the debug
`sessiondock` and `ptyhost` binaries already built; it does not build
them.

The two Node files are also in the `node_contracts` group of
[validation.md](validation.md). The browser file is the
`terminal_grid_browser` suite.
