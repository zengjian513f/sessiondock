# Session list, views and finite history pages

The read model behind `/api/sessions`, `/api/messages/{uid}` and the history
pages is the lazy index plus on-demand views of [read-model.md](read-model.md).
This file is the wire contract of the three routes as the
facade `sessions::SessionStore` now serves them: what a list row carries, how a
session is opened, which per-session codes ask the client to retry, and the
finite history pages. Nothing here is a frontend framework, a
native-span parser or a large-image promise.

## Point title lookup

`GET /api/sessions/titles?ids=claude:<sid>,codex:<sid>` returns only requested
`session_uid/source/sid/title` rows and a `missing` list (also used for ambiguous
IDs). Through the hub, use `/api/nodes/<nid>/api/sessions/titles` to select the
owning node. No full session-list response or message projection is produced.

A cold process initializes its index once (`index_initialized: true`). Warm
queries use native-ID lookup, stat only selected files and necessary Codex
ancestors, and reread bounded summaries only when those files change
(`summary_reads`). Codex names use the existing names-file stamp cache; changing
that shared names file reloads that metadata file. Warm title queries do not
walk session directories or discover new sessions. Unknown IDs stay missing
until normal list/index discovery has seen them; callers retain their stored
title on a missing result or an unavailable node. The normal list and its
invalidation state remain independent of point reads.

Validation: `python3 tests/session_titles_browser.py` uses 500 synthetic sessions,
clicks refresh, appends a native rename, and checks that hot point requests do
not discover a newly created unrelated session.

## The list: index rows plus the physical cursor

`GET /api/sessions` (`?force=1` rescans; otherwise rows younger than 500 ms
are reused) is the index's rows — one bounded head/tail summary per native
file, never a full parse — with the persisted metadata applied and re-signed:

- `{sessions, sig, built_at}`; `?sig=<sig>` answers `{unchanged:true, sig}`.
  `sig` is the SHA-1 of the serialized rows **after** metadata enrichment
  (stars, fork visibility, timeline pins) and **before** any view-derived
  decoration; `built_at` and `sig` are kept while the rows are unchanged, so a
  forced rescan that finds nothing new republishes the same document.
- Row fields are the list-row fields (`uid/source/sid/title/cwd/
  created/updated/size/model/branch`, topology `agent_items/forked_from_id/
  history_base/root_sid/fork_depth`, Grok `chat_exists`, Codex
  `renamed_at/renamed_to`, Claude `continued_in`) plus `supported` as the
  head (96 KiB, ≤ 40/120 pieces) and tail (512 KiB) can tell it;
  `migration_warnings` (the fatal reason) only on an unsupported row, the
  non-fatal notes live in the detail `meta` alone.
- **Agent items.** Each `agent_items[]` entry carries `id/title/type/active/
  path/cwd/model/created/updated/size` (plus `supported`, `migration_warnings`
  when unsupported, and the cursor rule below). `active`: a Codex subagent whose
  last turn-boundary `event_msg` is `task_started`/`turn_started`; a Claude
  sidecar whose last user/assistant record is not an assistant `end_turn` and
  whose owner has no later stop notice for it (`<task-notification>` task-id
  or a foreground `Agent` tool_result; notice copies count once, at their
  first time; `async_launched` is not a stop) — the owner file is scanned
  incrementally for that, only when such a sidecar exists
  ([read-model.md](read-model.md)). `created`/`updated` are the sidecar's
  first/last record times (meta.json mtime, then `updated`, as fallbacks).
  The agent view's `meta` is the owner row with the item's fields applied;
  it keeps the owner's `agent_items`.
- **Continuations.** A Claude main row whose tail holds a `continued-in`
  record carries `continued_in` = the uid of the listed Claude session with
  that sid (same source, this index; a self-reference or an unlisted sid
  gives no field).
- **List cursor rule.** A supported row (and each supported `agent_items`
  entry) carries `cursor: {end, head}` — the offset after the last complete
  JSONL line and the `rs-m2-1` hash of the committed prefix's first 4 KiB,
  both read by the index. The semantic `anchor` needs the whole projection,
  so it is present only when the server holds a cached view of exactly that
  file version (the session was opened in this process and has not changed
  since); it is borrowed from the view at list time and never changes `sig`.
  The legacy sidebar (`syncSidebarUpdates`) therefore compares `anchor` only
  when both its cached cursor and the row have one, keeps the anchor it
  already has otherwise, and detects growth from `end`/`head`. Consequence:
  the first growth of a session nobody opened in this process is fetched as a
  reset (no unread increment for that one event); once opened, rows carry the
  anchor. A Grok session without a chat file
  publishes `{end: 0, head: <hash of nothing>}`.
- **Timeline pins.** A Claude main session with a persisted pin publishes
  `timeline_pin: {target, tip, stale_end, pinned_at, native_rewind:false}`;
  `retired:false` is added while the file has not grown past `stale_end`
  (physical fact), and the full `retired`/`retired_reason`/`retired_message`
  comes from the cached view when one is current. `/api/messages` `meta`
  always carries the projected pin state.

What a summary cannot know (deltas from the old full parse, all documented
in [read-model.md](read-model.md)): Claude lineage
notes (missing ancestor, cycle, missing declared leaf — non-fatal,
the timeline is the reachable part) and content-block notes surface in the
detail `meta.migration_warnings` when the session is opened; the row lists
carry none;
unknown-kind warning counts on files larger than 512 KiB come from the head
and tail only. There is no fixed file/record/checkpoint/event count quota on
open, nor a session count, total-byte or directory-entry cap on the list.

## Opening a session: on-demand views

`GET /api/messages/{uid}?agent=&start=&head=&anchor=&append=&window=` opens
the view of `(uid, agent)`: the index answers the owner's file, the agent's
own file (an `agent_items` id, never a client path) and the persisted pin;
`Views` re-`stat`s those files, extends from the last committed offset on an
append (full old-prefix digest verification, reused ASTs) and rebuilds on a
rewrite, truncation or pin change. The response keeps the batch-2 shape:
`meta` (the published row, `agent` views),
`version{size,mtime,head}`, `reset/start/end/anchor`, `messages`,
`message_total`, `partial`, `activity_changed/activity`; `meta.cursor` is the
view's `{end, head, anchor}`.

**Claude timeline rules** (`sessions/providers/claude.rs`):
the transcript is an append-only tree, and
the view is the chain from the current tip (the last graph record, or the
`last-prompt` leaf, or a persisted pin) to its root — a completed old branch
left behind by a double-Esc rewind is hidden. Three kinds of off-chain
records still render, so the page matches what tmux showed: (1) an
**unanswered sibling** of the current input (a fast Esc committed the row,
the next input went beside it); (2) the input whose turn an **Esc cut
short** after the assistant already wrote text/tools/thinking — found by
walking up from the native interrupt record (`interruptedMessageId`, or a
text that is exactly `[Request interrupted by user]` / `…for tool use]`) to
the first input beside the current chain — together with everything below
it (its tool calls and results, the interrupt record); (3) unanswered inputs
typed below such a branch (any depth). Each such input carries
`interrupted: true`, `interrupt_reason: "输入已中断，未进入当前 Claude 分支"`,
starts its own `turn_id` and emits no `working`; the branch's other records
keep that turn id without the flag. Its `aborted` status follows the input
directly when nothing native sits below it, otherwise it is the interrupt
record's own `aborted`, in file order after the branch. Inputs at or
before a confirmed rewind boundary (`abandoned_after`) stay hidden. A native
interrupt record never starts a turn. The batch-35 lineage notes (missing
ancestor, cycle, missing declared leaf) are unchanged and non-fatal.

Per-session codes (they concern this one session; the list is unaffected):

| Code | Meaning | Client action |
| --- | --- | --- |
| 404 | uid unknown, agent not an `agent_items` entry, or an agent uid opened directly (`子代理必须通过所属主会话访问`) | refresh the list |
| 409 | ambiguous native id among indexed files (`父线程 ID 在已配置索引中存在歧义`, duplicate Codex agent id), Claude sidecar `sessionId` ≠ owner's | show the reason; it clears when the duplicate goes away |
| 501 `unsupported_history` | the projection failed closed: corrupt line, scalar `content`, missing Claude ancestor/cycle, Codex parent unindexed / subagent file / bad `history_base` | show the reason |
| 503 | the file (or a parent prefix) changed while it was being read (`会话在读取期间变化，请重试`) | retry; the next open extends from the new stamp |
| 413 | one decoded image exceeds the 32 MiB media limit | inspect that image; ordinary history has no fixed file/record/page-size rejection |

An open within the index TTL that fails with 404/409/501/503 triggers one
forced rescan and retry, so SSE-only clients discover new parent/agent files
without polling the list. The watch stream (`/api/watch`) polls the view every
500 ms per logical view: a `stat` per file, an incremental extension only when
the file grew, and one event per changed view revision (row metadata changes
count as a revision, so an append can produce a message event followed by a
metadata-only event once the list row catches up).

History capacity follows the actual stamped input, without service file,
record, index or projected-message byte quotas. The default view LRU uses
16 entries / 128 MiB accounting and the AST cache uses 64 MiB; these affect
retention, not whether history can be read. See [read-model.md](read-model.md).

## Finite history pages

The Rust-only `history_pages: true` capability is declared. The legacy UI uses
it to fill its middle-history gap incrementally.
No frontend framework is introduced.

### Wire contract

`GET /api/messages/{uid}?window=1&agent=...` keeps the existing live checkpoint
fields. When history is omitted, `partial` contains `head`, `tail`, `omitted`
and an opaque `cursor`. Initial windows prioritize up to 500 latest events, then
up to 100 earliest events within the shared budgets. Heavy media can reduce
either segment. SSE resets also use a bounded window; ordinary append semantics
remain unchanged.

`GET /api/messages/{uid}/page?cursor=...&agent=...` returns:

```json
{
  "messages": [],
  "page": {
    "cursor": "opaque-request-token",
    "next": null,
    "start": 100,
    "end": 300,
    "stop": 300,
    "remaining": 0
  }
}
```

The example omits actual message contents: a successful page is nonempty and
`messages.length == end - start`. Indices count all non-status display events,
including `counted:false`, rather than native byte offsets or `message_total`.
Several events may share an offset; inherited events may have offset zero.
`next` is null exactly when the initial omitted range has been filled.

A page has no live `end`, `head`, `anchor`, `version` or session metadata fields.
Its messages are inserted before the existing tail. Loading a page must never
advance or roll back the live append checkpoint or interrupt the watch stream.

### Checkpoints, lifetime and errors

Each 128-bit random token binds canonical owner UID, exact agent, the original
byte/semantic checkpoint and an event range. Grants retain no native snapshot
or message payload. Ordinary append can preserve an old grant; prefix rewrite,
rewind, branch or inherited-prefix changes must pass the existing complete
checkpoint validation or fail with 409. The initial event count is also checked.

The in-memory store admits 1024 grants, evicts oldest grants and expires grants
after ten minutes. Reads do not consume or refresh a grant. Retrying a page
preserves its range/content, but a newly issued continuation token may differ.
Malformed tokens return 400, a wrong UID/agent 403, unknown/evicted tokens 404,
and an expired token still present in the store 410. Expired tokens already
purged return 404. Restart loses all grants.

The UI keeps its current history on failure and offers retry or an explicit
bounded reload. It does not silently clear history or request unbounded history
after a stale, evicted or expired grant. Responses from an old view/reset are
discarded. Explicit reload must also avoid overwriting concurrent live updates.

### Page grouping targets

- Each page selects at most `SESSIONDOCK_HISTORY_PAGE_EVENTS` events (default
  2000, 1–10000), 128 typed native image
  references and 24 MiB estimated embedded compressed-image bytes — so a
  page normally groups as much as fits in 8 MiB, and the 51 MB / 7,426-message real
  Claude session fills its gap in a handful of pages. Initial head/tail
  windows use the same media/byte budget and at most 600 events.
- The legacy gap button chains pages: one click keeps requesting the next
  grant until the gap is filled (progress on the button, a second click
  aborts and keeps the pages already read), then renders once. Every page
  still passes the same validation and the same view/cursor checks; the
  chain stops on any failure and reports it on the fresh gap. Browser tests
  set `HISTORY_PAGE_CHAIN=false` to drive one page per click.
- 8 MiB JSON is a soft page target. Selection reserves 64 KiB for metadata
  and a conservative per-image descriptor allowance. A later oversized event
  ends the current page without discarding progress; a page starting with it
  returns that one event even above the JSON or image-byte target. The next
  cursor advances normally, so large messages cannot strand the history gap.
  Final response serialization validates structure without restoring a size cap.
- Page requests/responses share an independent permit pool (`Pools::responses`,
  2× the read workers; queued within the bounded admission wait, then 503
  `history_page_busy`). A cancelled request keeps its permit until its
  blocking reader finishes; an unconsumed response or retained byte frame
  holds its permit until release. This is in addition to the existing shared
  blocking-reader admission.
- Complete current-view file-reference authority remains independent of the
  selected display page. Existing media authorization and blob-cache eviction
  apply.

Lazy media materialization applies to the selected page; see [media.md](media.md).
The same grant store serves per-message media continuation
(`media_more` / `GET /api/messages/{uid}/media-page`): a message initially
displays at most 16 typed images and continuation exposes the rest. Page byte,
image and event targets only group transport responses; they do not reject a
valid record or cap a message's underlying media. An explicit non-windowed
messages request retains the existing view behavior; the capability-gated
legacy gap button no longer uses it. See
[remaining media design](media-pagination-design.md).

## Fixture checks

Claude paste envelopes (`<pasted_content id="…">` with the same id on the
closing tag, observed in Claude Code 2.1.278) are unwrapped by the native
projection for user text, before history and search serialization. One framing
newline at each end is removed; body indentation, surrounding text, unknown
tags and incomplete/mismatched envelopes remain literal. Pasted protocol-looking
text remains user content. Assistant/tool text and native files are unchanged.
The list's fallback title uses the same unwrapping. This is an intentional
display DELTA from the frozen Python adapter's verbatim paste envelope, matching
the native terminal. `history_browser.py` covers wire output, title, live append
and actual page interaction using synthetic records.

```sh
cargo test -p sessiondock --test history_pages --locked
node --test tests/legacy_contract.mjs tests/history_pages_contract.mjs
cargo build -p sessiondock --locked
python3 tests/sessions_list_suite.py
python3 tests/inventory_scale_suite.py --quick
python3 tests/inventory_live_append_suite.py
python3 tests/list_rows_parity.py --python-source PATH
python3 tests/history_pages_browser.py
```

Fixtures cover multi-page reconstruction, shared offsets, inherited end-zero
events, 257 images, checkpoint changes, token scope/lifetime/eviction, byte
budgets and retained response ownership. Browser checks use temporary synthetic
histories and the actual legacy renderer, not a production session or model CLI.
The browser pause hook is tied to the actual renderer's Promise/timer
call and additionally checks unpublished DOM plus the render generation. A loose
whole-stack match could pause a nested helper instead; earlier passing results
alone did not establish that the renderer was genuinely suspended. Page insertion
and explicit-reset SSE races must pass the calibrated scenario.
