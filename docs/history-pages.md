# Session list, views and finite history pages

The read model behind `/api/sessions`, `/api/messages/{uid}` and the history
pages is the lazy index plus on-demand views of [read-model.md](read-model.md)
(batch 34). This file is the wire contract of the three routes as the
facade `sessions::SessionStore` now serves them: what a list row carries, how a
session is opened, which per-session codes ask the client to retry, and the
finite history pages of batch 14. Nothing here is a frontend framework, a
native-span parser or a large-image promise.

## The list: index rows plus the physical cursor

`GET /api/sessions` (`?force=1` rescans; otherwise rows younger than 500 ms
are reused) is the index's rows — one bounded head/tail summary per native
file, never a full parse — with the persisted metadata applied and re-signed:

- `{sessions, sig, built_at}`; `?sig=<sig>` answers `{unchanged:true, sig}`.
  `sig` is the SHA-1 of the serialized rows **after** metadata enrichment
  (stars, fork visibility, timeline pins) and **before** any view-derived
  decoration; `built_at` and `sig` are kept while the rows are unchanged, so a
  forced rescan that finds nothing new republishes the same document.
- Row fields are the Python `list_sessions` fields (`uid/source/sid/title/cwd/
  created/updated/size/model/branch`, topology `agent_items/forked_from_id/
  history_base/root_sid/fork_depth`, Grok `chat_exists`, Codex
  `renamed_at/renamed_to`, Claude `continued_in`) plus `supported` as the
  head (96 KiB, ≤ 40/120 pieces) and tail (512 KiB) can tell it;
  `migration_warnings` (the fatal reason) only on an unsupported row, the
  non-fatal notes live in the detail `meta` alone (batch 44 WP-C).
- **Agent items.** Each `agent_items[]` entry carries `id/title/type/active/
  path/cwd/model/created/updated/size` (plus `supported`, `migration_warnings`
  when unsupported, and the cursor rule below). `active` follows Python: a Codex subagent whose
  last turn-boundary `event_msg` is `task_started`/`turn_started`; a Claude
  sidecar whose last user/assistant record is not an assistant `end_turn` and
  whose owner has no later stop notice for it (`<task-notification>` task-id
  or a foreground `Agent` tool_result; notice copies count once, at their
  first time; `async_launched` is not a stop) — the owner file is scanned
  incrementally for that, only when such a sidecar exists
  ([read-model.md](read-model.md)). `created`/`updated` are the sidecar's
  first/last record times (meta.json mtime, then `updated`, as fallbacks).
  The agent view's `meta` is the owner row with the item's fields applied
  (Python `session_view`); it keeps the owner's `agent_items`.
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
  anchor and behaviour equals Python's. A Grok session without a chat file
  publishes `{end: 0, head: <hash of nothing>}`.
- **Timeline pins.** A Claude main session with a persisted pin publishes
  `timeline_pin: {target, tip, stale_end, pinned_at, native_rewind:false}`;
  `retired:false` is added while the file has not grown past `stale_end`
  (physical fact), and the full `retired`/`retired_reason`/`retired_message`
  comes from the cached view when one is current. `/api/messages` `meta`
  always carries the projected pin state.

What a summary cannot know (deltas from the old full parse, all documented
in [read-model.md](read-model.md) and the batch-34 ledger): Claude lineage
notes (missing ancestor, cycle, missing declared leaf — batch 35: non-fatal,
the timeline is the reachable part) and content-block notes surface in the
detail `meta.migration_warnings` when the session is opened, the row lists
like Python's;
unknown-kind warning counts on files larger than 512 KiB come from the head
and tail only; per-file budgets (64 MiB record, 4 GiB file, 2,000,000 LF,
1,000,000 records) are enforced on open, never by the list; there is no
session count, total-byte or directory-entry cap.

## Opening a session: on-demand views

`GET /api/messages/{uid}?agent=&start=&head=&anchor=&append=&window=` opens
the view of `(uid, agent)`: the index answers the owner's file, the agent's
own file (an `agent_items` id, never a client path) and the persisted pin;
`Views` re-`stat`s those files, extends from the last committed offset on an
append (full old-prefix digest verification, reused ASTs) and rebuilds on a
rewrite, truncation or pin change. The response keeps the batch-2 shape:
`meta` (the published row, `agent` views per Python `session_view`),
`version{size,mtime,head}`, `reset/start/end/anchor`, `messages`,
`message_total`, `partial`, `activity_changed/activity`; `meta.cursor` is the
view's `{end, head, anchor}`.

**Claude timeline rules** (Python `_active_lineage`/`_read_one` at e5b023a,
`sessions/providers/claude.rs`): the transcript is an append-only tree, and
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
| 501 `unsupported_history` | the projection failed closed: corrupt line, scalar `content`, missing Claude ancestor/cycle, Codex parent unindexed / subagent file / bad `history_base`, record-count budget | show the reason |
| 503 | the file (or a parent prefix) changed while it was being read (`会话在读取期间变化，请重试`) | retry; the next open extends from the new stamp |
| 413 | this file exceeds a physical work budget (4 GiB file, 64 MiB line, 2,000,000 LF, 1 GiB view, 8 MiB page) | not retryable; the session is pathological |

An open within the index TTL that fails with 404/409/501/503 triggers one
forced rescan and retry, so SSE-only clients discover new parent/agent files
without polling the list. The watch stream (`/api/watch`) polls the view every
500 ms per logical view: a `stat` per file, an incremental extension only when
the file grew, and one event per changed view revision (row metadata changes
count as a revision, so an append can produce a message event followed by a
metadata-only event once the list row catches up).

Budgets (the table in [read-model.md](read-model.md#物理工作预算防病态文件不是功能上限)
and the `budgets` block at the top of `sessions/mod.rs`): 64 MiB per record,
4 GiB per file and per inherited prefix chain, 2,000,000 LF checkpoints,
1,000,000 records / 2,000,000 events per view, 1 GiB serialized messages per
view, a 64-view / 2 GiB LRU, 256 MiB / 16 MiB sidecar summaries.

## Finite history pages (batch 14)

Batch 14 adds the Rust-only `history_pages: true` capability. The legacy UI uses
it to fill its middle-history gap incrementally; Python without the capability
retains its existing full-history behavior. No frontend framework is introduced.

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

### Page budgets and remaining limits

- Each page selects at most `SESSIONDOCK_HISTORY_PAGE_EVENTS` events (default
  2000, 1–10000; batch 44 WP-A, was a fixed 200), 128 typed native image
  references and 24 MiB estimated embedded compressed-image bytes — so a
  page is “as much as fits in 8 MiB”, and the 51 MB / 7,426-message real
  Claude session fills its gap in a handful of pages. Initial head/tail
  windows use the same media/byte budget and at most 600 events.
- The legacy gap button chains pages: one click keeps requesting the next
  grant until the gap is filled (progress on the button, a second click
  aborts and keeps the pages already read), then renders once. Every page
  still passes the same validation and the same view/cursor checks; the
  chain stops on any failure and reports it on the fresh gap. Browser tests
  set `HISTORY_PAGE_CHAIN=false` to drive one page per click.
- Serialized JSON responses are at most 8 MiB. Selection reserves 64 KiB for
  metadata/envelopes and a conservative per-image descriptor allowance; the
  fully projected response is checked again. A later oversized event does not
  invalidate already selected progress. A page starting with an unsplittable
  oversized event explicitly returns 413, never an empty continuation loop.
- Page requests/responses share an independent permit pool (`Pools::responses`,
  2× the read workers; queued within the bounded admission wait, then 503
  `history_page_busy`). A cancelled request keeps its permit until its
  blocking reader finishes; an unconsumed response or retained byte frame
  holds its permit until release. This is in addition to the existing shared
  blocking-reader admission.
- Complete current-view file-reference authority remains independent of the
  selected display page. Existing media authorization and blob budgets apply.

Batch 15 adds lazy media materialization to the selected page; see [media.md](media.md).
Batch 20 shares this grant store with per-message media continuation
(`media_more` / `GET /api/messages/{uid}/media-page`): a message inlines at most
16 typed images and the window/page budgets count only that prefix.
Pagination still does **not** make parsing incremental by native span,
split a single oversized text message, or enable 32 MiB inline images. The
per-record/file/view and per-message image limits remain. An explicit
non-windowed messages request retains the old bounded-by-view behavior; the
capability-gated legacy gap button no longer uses it. See
[remaining media design](media-pagination-design.md).

## Isolated checks

```sh
cargo test -p sessiondock sessions:: --locked
cargo test -p sessiondock --test history_pages --locked
node --test tests/legacy_contract.mjs tests/history_pages_contract.mjs
cargo build -p sessiondock --locked
python3 tests/sessions_list_suite.py
python3 tests/inventory_scale_suite.py --quick
python3 tests/inventory_live_append_suite.py
python3 tests/list_rows_parity.py --python-source ../agenthub
python3 tests/history_pages_browser.py
```

Fixtures cover multi-page reconstruction, shared offsets, inherited end-zero
events, 257 images, checkpoint changes, token scope/lifetime/eviction, byte
budgets and retained response ownership. Browser checks use temporary synthetic
histories and the actual legacy renderer, not a production session or model CLI.
Batch 15 tightened the browser pause hook to the actual renderer's Promise/timer
call and additionally checks unpublished DOM plus the render generation. A loose
whole-stack match could pause a nested helper instead; earlier passing results
alone did not establish that the renderer was genuinely suspended. Page insertion
and explicit-reset SSE races must pass the calibrated scenario.
