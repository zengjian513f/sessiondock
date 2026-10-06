# Server-side terminal grid

The host keeps one VT emulator (alacritty_terminal behind `Screen`). A grid
client receives cells, not escape sequences, and the browser only paints.
Byte-console clients still get the raw PTY stream and parse it in xterm.js.
An exited SSH session's [final screen](terminal-final-screen.md) is saved as
one snapshot of this protocol.

The client is the main console (`term.js` through `legacy-web/grid/facade.js`,
see [Main console](#main-console)); the former standalone `grid.html` page was
removed. It uses the same claim and attach lease as the byte console
([terminal ownership](terminal-ownership.md),
[raw terminal input](terminal-input.md)). On a hub page, calls for a node's
session are prefixed with `/api/nodes/{nid}/api/` ([hub](hub.md)).

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
`cursor`, and the final screen saved at exit. Live byte traffic does not pass
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
| → browser | `{"t":"heartbeat_ready"}`, `{"t":"pong","id":N}` | only with `heartbeat=1`; transport controls, never grid/byte output ([liveness contract](terminal-ownership.md#explicit-directory-http--websocket-bridge)) |
| → browser | close 1000 `host exited` | host exit frame with complete output |
| browser → | binary | raw PTY bytes (unchanged from the byte console) |
| browser → | text `{"t":"resize","cols","rows"}` | PTY resize (unchanged). Other text is literal PTY input except `{"t":"ping","id":N}` on an attachment opted into `heartbeat=1`; that u32 probe is answered without host input |
| browser → | Close / disconnect | ends the attachment |

A `seq` gap is counted (`lostMessages`) and is **not** requested. A
reconnect gets a fresh `reset: true` snapshot.

### Row

```text
{"s":[[text, fg, bg, flags] | [text, fg, bg, flags, {"link": url, "ul": color}], …], "w": wrapped}
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

Light mode is a browser presentation transform for both the standalone grid
and the embedded console: explicit RGB/indexed foregrounds and backgrounds
receive hue-preserving lightness reflection. A final contrast adjustment uses
the actual foreground/background pair, including inverse and dim text; dim
opacity is 0.7 in light mode (0.5 in dark mode). Theme changes repaint existing
cells without changing the stored grid, PTY bytes, or CLI settings. The host
continues answering colour queries with its fixed dark palette in either mode.
`tests/terminal_grid_theme_browser.py` exercises theme switching in the main
console, code/diff/prompt colours, and the CLI's OSC background query.

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
| `history` | recent scrollback rows. For `reset: true`, the last `SNAPSHOT_HISTORY_ROWS` (2000) rows, oldest first (a final screen sends all of its history instead). For a live resize snapshot, `[]` |
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
| `scrolled` | rows that left the **primary** screen since the previous capture, oldest first. Absent when empty. Not collected while the next state is on the alt screen, right after leaving it, and not collected across a resize snapshot (the browser recovers those rows, see [resize seam](#resize-seam)) |
| `history_total` | present only with `scrolled`: absolute history length in the model after this diff. Once the host history is full it stays at `--history` while rows keep scrolling |
| `rows` | `[y, row]` pairs; `y` is a viewport row |
| `cursor` | `{x, y, visible}` when it moved or visibility changed |
| `modes` | when any mode field changed |
| `title` | OSC title string when it changed (including a reset to `""`) |

The browser appends `scrolled` only when its **current** `modes.alt` is
false (the value before this diff is applied). When the diff carries
`history_total` and it is lower than the browser's host total estimate, the
host has dropped that many of its oldest rows: the estimate and the host index
of the oldest loaded row move down by the same amount (older hosts omit the
field; `acceptHistory` then corrects the estimate on the next page).

`scrolled` counts rows pushed into the model's history, not growth of the
history length, so it stays exact when the history is at `--history` and every
new row evicts the oldest one. alacritty does not expose a scroll counter;
`Screen::take_scrolled` uses the primary grid's `display_offset`, which grows by
the scrolled count whenever it is non-zero. Each capture reads it and puts it
back to 1 (the host reads the grid only by absolute line, never by display
offset). The counter saturates at `--history`; a capture that saw at least
`--history - 1` new rows sends the whole retained history, except that exactly
`--history - 1` rows is recognised by the previous newest row now being the
oldest. A history clear (ED 3, RIS) or model rebuild zeroes the offset; the rows
then in history are all new. Resize captures consume the counter and discard it.

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
| `SNAPSHOT_HISTORY_ROWS` | 2000 | cap on rows copied into a `reset: true` snapshot and on one `grid_rows` page. Older history is paged through `GET /api/term/grid/history` |

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

Stripping DSR on the read path means those queries never reach a client and
never enter the model's saved state.

## Browser modules

The four modules under `legacy-web/grid/` have no DOM except
`input.js`'s hidden-textarea helper; `grid/facade.js` assembles them into
the console's `GridTerm`.

| Module | Owns |
| --- | --- |
| `grid/wire.js` | `LineDecoder` (newline-delimited JSON, UTF-8-safe), `encodeResize`, grapheme `segmentText` (cache 256) |
| `grid/model.js` | viewport, scrollback, cursor, modes, title, `seq`. Applies `snapshot`/`diff`. Materializes cells lazily. Default `scrollbackLimit` 100_000. **Does not reflow the viewport** (the host resends it) |
| `grid/render.js` | Canvas 2D. Metrics: `"W"` advance (CJK `"中"` / 2 as fallback), `cellHeight = round(fontSize * lineHeight * dpr) / dpr` (a whole device pixel, like the width) with defaults 14 px and 1.2, baseline from `'M'.actualBoundingBoxAscent` plus vertical centering. Backing store is CSS × `devicePixelRatio` × effective ancestor CSS zoom; the context and cell alignment use that combined density. Zoom changes rebuild the backing store and repaint even when rows/columns stay unchanged or a keyboard prevents PTY resizing. Every row is painted inside a clip of its own box: glyphs taller than the em box (block elements, ❯, emoji, accented capitals, CJK fallbacks) cannot spill into the neighbouring rows, which only repaint when dirty. Fit floor: 2 columns, 1 row |
| `grid/input.js` | `InputEncoder`: keys, paste (newlines → CR; bracketed `\x1b[200~…\x1b[201~` when that mode is on), focus (`\x1b[I` / `\x1b[O`), mouse (SGR / UTF-8 / X10, default X10 clamped at 223), alt-screen wheel-as-arrows (3 lines). `KeyCapture` holds a hidden textarea |

Scrollback reflow happens only in the model, on `reset: false` when
`cols` changes and on an explicit `reflow`/`resize`. Wrapped runs are
concatenated and rewrapped; a wide cell is never split. The viewport is
replaced by the next snapshot/diff from the host.

### Lazy reflow

The main console creates its model with `lazyReflow: true`, so a width
change does not block the main thread in proportion to the history. A width
change stamps every scrollback row that has no width yet with the width it
was wrapped at (`row.wcols`) and marks the model `reflowPending`. Reading a
row (`cellsOf`, `readCells`, `textOf`, selection text, search) always
materializes it at its own width, so text and copies are correct while rows
of two widths coexist.

`GridTerm` then reflows, right away, the logical lines covering the viewport
and two screens above and below it (`reflowRange`); every paint repeats this
for wherever the viewport is, so scrolling into a part not yet reflowed
reflows it before drawing. The rest is reflowed from the newest end toward
the oldest in `setTimeout` slices of at most 8 ms (`reflowStep(deadline)`).
A newer width change only changes the target: logical lines already at the
target are skipped and lines at an intermediate width are rewrapped from
their own width, so an in-flight batch is effectively cancelled without
losing or redoing finished lines. A single unwrapped row that certainly fits
(UTF-16 length × cell width ≤ target) is only restamped, without
materializing cells. Search (`term-menu.js`) finishes any pending reflow
before it scans, so its row numbers stay stable; history paging waits until
the reflow is done (`historyRequest` returns null meanwhile) and resumes when
the last slice emits a scroll event.

Each replacement of `[start, end)` by new rows calls
`model.onLinesReplaced(start, end, count, map)`. `map` converts a position
inside the logical line by its column offset in the joined line; positions
after it shift by the row-count change. `GridTerm` maps the viewport top (when
not following output), both selection ends and the drag anchor through it,
so the logical line on top stays on top and a selection, even one held
during the resize, still covers the same text and copies it on release.

Measured by `tests/terminal_reflow_browser.py` in headless Chromium with
9155 scrollback rows (9000 numbered rows, every 90th a 300-column line) and
five width changes: the old whole-history reflow ran synchronously inside
`term.resize` for up to 88 ms (longest main-thread task 92 ms); now the
synchronous call takes at most about 20 ms (canvas fit and paint
included), no main-thread task reaches 50 ms, and the whole scrollback is at
the new width 30–46 ms after the resize. The suite asserts a 120 ms longest
task and a 60 ms synchronous resize with slack.

### Resize seam

When the host resizes its model, alacritty moves rows between history and
screen: a shorter screen scrolls the rows above the cursor that no longer
fit into history (`shrink_lines`), a taller one pulls history rows back to
the top of the screen (`grow_lines`), and a width change reflows the whole
buffer and keeps the last rows as the screen. Those moves are not in any
`diff.scrolled` (the screen thread resets its capture across a resize), so
before this was handled the browser lost rows at the seam after a shorter
window or a narrower one and could show them twice after a taller or wider
one. The fix is entirely in the browser; the host protocol is unchanged.

- Every `reset: false` snapshot carries `history_total`. When the width did
  not change, the model compares it with its own estimate
  (`historyBase + historyAppended`): a growth of `n` rows moves the top `n`
  rows of the old viewport into the scrollback, a shrink of `n` rows drops
  the last `n` scrollback rows (they are in the new screen). Absolute line
  numbers do not change, so the viewport and selection stay put. After a
  snapshot taken on the alternate screen the estimate is not trusted.
- After a width change, or whenever `history_total` moved, the model records
  `tailSync`. On a live connection `term.js` (`syncTermHistoryTail`) fetches
  the host's newest 400 history rows ending at that snapshot's
  `history_total` through `GET /api/term/grid/history` under the page's
  lease; rows scrolled in after the snapshot are excluded from the
  comparison. `acceptHistoryTail` reflows the comparison window, finds the
  oldest 8 fetched rows in the local scrollback (the match nearest the
  expected position; rows compare after merging equal spans and trimming
  trailing default spaces), and replaces the local rows from there with the
  host's. Without a match nothing changes. A failed request is dropped
  quietly (`terminal.history_tail_failed` audit event); the local height
  correction stands. A final screen has no host and never fetches.

The grid renderer keeps its Canvas 2D path at every interface scale.

On narrow screens, the overlaid terminal rounds its top toward the opaque
header. Fractional interface zoom must not expose a strip of message glyphs
between the header and terminal; the keyboard browser suite checks this at
85–125% scale.

On narrow screens, a soft keyboard reduces the visible pane without resizing
the PTY. Both console renderers move the screen only enough to show its last
nonblank row and cursor; blank trailing rows do not push short menus off the
top. The cursor takes priority over a lower footer. Closing the keyboard clears
the offset. Rows and columns are always measured at keyboard-closed height, so
interface zoom with the keyboard up still resizes the PTY.
[terminal_keyboard_browser.py](../tests/terminal_keyboard_browser.py)
covers short menus, bottom editors, cursor visibility, zoom with the keyboard up
and both viewport resize paths.

Selection is a half-open column range on model lines (scrollback +
viewport). Drag on the canvas; Shift-drag still selects when mouse
reporting is on. Double-click selects a word (ASCII alnum, `_`, or
code point > 127). Copy uses `selectionText` (no newline inside a
soft-wrapped logical line) automatically on mouse release after a local drag,
then clears the selection. Ctrl+C is CLI input, not a copy shortcut. CLI mouse
gestures and empty selections leave the clipboard unchanged. The main console
uses the same copy-on-release behavior with either renderer.
On touchscreens, a stationary single-finger hold (450 ms) starts selection;
dragging extends it and release copies it. A second finger transfers ownership
to interface zoom: cancel the hold timer or active selection without copying,
close the menu, and wait for every finger to lift before accepting a new hold.
The mobile key bar also provides a latched **Shift** selection button. While
on, a single-finger drag starts local selection immediately, bypassing CLI mouse
capture without sending Shift or mouse bytes and without focusing its keyboard.
A pinch cancels the selection but keeps Shift latched; tap Shift again to return
to ordinary touch behavior. Closing the terminal clears the latch.
Touch long press does not open the terminal menu; use the visible `⋯` button.
A real mouse right-click opens the menu immediately, including after a touch
selection or pinch. Compatibility mouse events from touch do not become remote
CLI mouse input or accidentally reopen a menu.
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
| Snapshot history | 2000 rows (`SNAPSHOT_HISTORY_ROWS`) | Older rows come from `GET /api/term/grid/history` in pages of at most 2000 (`grid_rows`) |
| Browser scrollback | 100_000 rows | Drop from the oldest; history paging stops filling at the cap |
| Width-change reflow | viewport ± 2 screens at once, rest in ≤ 8 ms slices | A newer width change retargets the pending slices ([lazy reflow](#lazy-reflow)) |
| Resize seam alignment | newest 400 host history rows, 8-row anchor | No anchor found: local rows stay unchanged ([resize seam](#resize-seam)) |
| Scrolled-row flood | none | Every row that left the primary screen between captures is sent in `diff.scrolled` |
| WebSocket host payload | 32 KiB chunks | Split only; lines are reassembled in the decoder |
| Attach query size default | 120×32 | Browser fit replaces this on open |
| Host `--history` | 10_000 default | Model scrollback, not the snapshot window |
| Attachment reservations | 32 per session | Shared with byte clients |

Known gaps:

- Underline *style* (double, curly, dotted, dashed) collapses to the
  single underline bit; hyperlinks and underline colour travel in the
  span's fifth element (see below).
- Alt-screen scrollback is not recorded: `scrolled` is empty while the
  next state is alt, and the browser ignores `scrolled` while it is
  already on alt.
- A flood of output sends **all** newly scrolled rows, not a sampled
  tail.
- `title` travels only on diffs, so a client that never sees a title
  change keeps an empty title until one arrives.
- The host does not emit a snapshot because of a `seq` gap; only
  reconnect (or a live resize) produces one.
- A height shrink while the host history is full moves rows from the screen
  into the host history without changing `history_total`, so the browser
  cannot see them in the `reset: false` snapshot; those rows are missing
  locally until a reconnect snapshot.
- Hosts already running keep their old binary until restarted; an old host
  stops sending `scrolled` once its history is full.

## Fifth span element, clipboard, history paging

A span whose cells carry an OSC 8 hyperlink or an SGR 58 underline colour
gets a fifth element `{"link": url, "ul": color}` (either key may be
absent; `ul` uses the same colour encoding as `fg`). The browser model
keeps `link`/`ul` per cell and preserves them through reflow; the
renderer draws the underline in `ul` when set and a dotted underline
under a link that has no underline attribute; Ctrl/⌘+click on a linked
cell opens it in a new tab (`noopener`).

OSC 52 clipboard writes are consumed by the host model (alacritty
`ClipboardStore`) and forwarded to grid clients as their own message,
`{"t":"clipboard","text":"…"}` (decoded text, one message per write),
sent right after the flush that produced them. Byte clients keep the raw
sequence and let xterm.js handle it. Clipboard *reads* (OSC 52 `?`) are
never answered.

`GET /api/term/grid/history?name&page&token[&uid&instance_id|&record_id&launch_id&instance_id]&from&to`
returns `{"rows":[row…],"from","to","total"}` for absolute history rows
`[from, to)` (0 = oldest) under the page's own console lease (same
`ExpectedTarget` rules as `/api/term/send`; a stale token is 409). The
host op is `grid_rows {from, to}` (guard whitelist: exactly those fields,
both unsigned); it waits for the model to catch up like `capture`, clamps
`to` to `total` and to `from + 2000`. The main console uses it; see
[history paging in the main console](#history-paging-in-the-main-console).

## Final screen and the shared model

`ptyhost-screen` is the crate that holds `Screen` (the alacritty_terminal
wrapper) and `grid` (span extraction, diffing, `FlushPolicy`). ptyhost
uses it for live sessions and, at exit, to save an exited session's
[final screen](terminal-final-screen.md) as one `reset:true` snapshot with all
retained history; `GET /api/term/final?id=…` serves it as saved and the main
console shows it read-only (no claim, no input, no pty resize). The attach
replay serializer positions and erases each row (`ESC[r;1H ESC[2K`) instead of
`ESC[2J`, because alacritty's ED 2 would push the cleared rows into scrollback.

## Main console

The legacy console (`term.js`) renders only through the grid; the page ships
no xterm.js. A running host started before the grid protocol existed reports
`grid:false`: its console stays closed with the explanation
「此会话的终端宿主不支持网格显示，重新启动会话后即可打开控制台。」 An exited
row has no host and shows its final screen or explains itself. The hub
registry has no per-machine renderer attribute ([hub](hub.md)).
`legacy-web/grid/facade.js` exports `GridTerm`, an xterm.js-compatible
object (`write` of JSON-line text, `resize`, `buffer.active`, selection,
`onData`, `onSelectionChange`, `onClipboard`, `proposeDimensions`, …)
built on the grid modules; `index.html` publishes it as
`globalThis.GridTerm` from a module script placed before `term.js`.
`ensureTerm` creates it and adds `mode=grid` to the attach URL;
`writeTermOutput` hands each JSON-line chunk straight to it. Claim, lease,
resize, revoke, exit and the Codex side-thread scan through `buffer.active`
work on that object. A final screen goes through the same facade at the size
the host had at exit.

The main console's grid renderer reserves a 12-pixel right gutter for a vertical
history scrollbar. Dragging the thumb or clicking the track changes the same
local viewport as the wheel, without sending PTY input. When focused, the bar
accepts arrows, PageUp/PageDown and Home/End. New output preserves a history
position; returning to the bottom resumes following output. The bar is hidden
on the alternate screen, where the application owns scrolling.
`tests/terminal_scrollback_browser.py`
checks dragging, keyboard navigation, output anchoring and the resize gutter.

### History paging in the main console

A `reset: true` snapshot carries only the newest 2000 history rows, while the
host keeps `--history` (10_000 by default). `GridModel` records
`historyOlder = history_total - history.length`: the host index of the oldest
loaded row. When a user scroll (wheel, scrollbar drag or click, scrollbar
arrows/PageUp/Home) leaves the viewport top within 40 lines of the oldest
loaded row, `term.js` (`loadOlderTermHistory`) requests up to 500 older rows
with the page's own lease tuple (`name`, `page`, `token` and the
`uid`+`instance_id` or `record_id`+`launch_id`+`instance_id` binding captured
at claim). One request is in flight per view; nothing is fetched while idle.

The request range ends 4 rows past the oldest loaded row plus 64 rows of
slack. `GridModel.acceptHistory` finds those 4 local rows in the result (the
match nearest the expected position) and prepends only the rows before them,
so the seam has no gap or duplicate even when the host index estimate is off.
If they are not found, paging stops for that connection; misaligned rows are
never inserted. `GridTerm.applyHistoryPage` shifts the viewport and selection
by the inserted count, so the visible rows do not move. A page that leaves the
top still within 40 lines immediately checks the next one.

The host index estimate follows the host. Rows scrolled in after the
snapshot are counted; when a response's `total` is lower than that count, the
host has dropped its oldest rows and the estimate moves down before retrying.
A local scrollback reflow (width change) marks the model for a resync: the
next request first probes `[0, 1)` for the host total, assumes the local
scrollback is the host's newest rows, and the seam search corrects the rest.
No page is requested while a [lazy reflow](#lazy-reflow) is still pending.
Paging never grows the scrollback past `scrollbackLimit` (100_000).

Requests are made only for a live connection: the view is still current,
its socket and lease are the ones that issued the request, and it is not a
final screen, ended, revoked or retired. A response for a replaced
model (reconnect calls `term.reset()`, and every `reset: true` snapshot
bumps the model's history epoch) is ignored. Any failure, including a 409
stale token, stops paging for the connection quietly
(`terminal.history_page_failed` audit event); the next reconnect snapshot
starts again. A final screen sends all model history in its snapshot and
never pages.

[`tests/terminal_history_paging_browser.py`](../tests/terminal_history_paging_browser.py)
prints 5000 numbered rows before the console opens, then pages with a real
scrollbar drag, wheel input and scrollbar Home until row 1, checking order,
seams, viewport stability, live input afterwards, no idle requests, a reload,
a width change and that a final screen sends no history requests. It also
starts a host with `--history 60`, prints 600 numbered rows while the console is
attached, and checks that rows 1..600 all arrive in order on the same
connection and that the host total estimate stays at 60.

## Validation

`python3 tests/terminal_grid_browser.py` is the main-console grid
acceptance (Playwright Chromium, temporary fixtures): the console button
claims and attaches `mode=grid`, typing, the shell's `stty size` equals
the grid view before and after a viewport resize, clipboard text paste
(Ctrl+V), takeover by a second page after confirmation with the first page
notified, no page errors. Selection/copy, scrollback, file paste and host
exit are in `terminal_selection_browser`, `terminal_scrollback_browser`
and `terminal_input_browser`; paging older host history is in
`terminal_history_paging_browser`; width/height changes over fully paged
history (long-task budget, contiguous rows, selection and viewport anchor,
resize seam with and without the tail request) are in
`terminal_reflow_browser`. Needs POSIX and the debug `sessiondock` and
`ptyhost` binaries already built; it does not build them.

The browser file is the `terminal_grid_browser` suite in
[validation.md](validation.md).
