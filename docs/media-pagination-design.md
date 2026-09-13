# Remaining large-media and history pagination work

This is the remaining large-media implementation design. Batch 14 implements
finite event pages and legacy gap insertion; its contract and limitations are
in [history-pages.md](history-pages.md). Batch 15 implements descriptor registration
and GET-time materialization; [media.md](media.md) records its budgets and scope.
Batch 16 added a private structural scanner; batch 17 removed retained raw bodies
and streamed ordinary records/parent prefixes. Batch 18 implements structured
provider-authorized image spans and current-branch cold/warm GETs. Batch 19
implements bounded replay of nested stringified Codex tool envelopes; batch 20
implements per-message media continuation (see [media.md](media.md#per-message-media-continuation-batch-20)).
Multi-string concatenated giant tool JSON is **not implemented**.
See [native-input.md](native-input.md) for current limits and remaining gaps;
the original motivation and acceptance requirements below remain useful history.

## Why raising constants is insufficient

A 32 MiB compressed image encoded as base64 needs approximately 42.7 MiB before
its JSON envelope. Before batch 18, the 16 MiB native file, 2 MiB native record and 16 MiB
logical-view/private-media budgets reject it before message-window selection.
The media decoder's 1.5 MiB embedded limit is only one of those boundaries.
Retaining that string in raw bytes, a serde_json Value and private image payload
would multiply memory even if every constant were enlarged.

The original legacy 100-head/500-tail window was not general pagination: its
middle-history button fetched the entire history. Batch 14 replaces that action
when the Rust capability is present, and can retrieve 257 images across finite
pages. A single 257-image response still exceeds native media batch admission.
Event.end cannot be used as a page
index: inherited events have end zero and one native record may emit several
events. message_total excludes counted:false events and is not an event index.

## Implementation sequence

1. **Batch 14 implemented event pagination; per-message media continuation remains.**
   Add finite history-page cursors bound to canonical UID, exact agent and a
   validated logical-view version. Keep them separate from live append byte
   checkpoints. Bound each page by event count, serialized JSON bytes and media
   references; preserve tool-result/event semantics. Return an explicit remaining
   range and per-message media continuation when required, never silent loss.
   Legacy should fill the middle gap page by page, retain the gap count and
   viewport position, and never advance the append cursor from an older page.
2. **Batch 15 implemented descriptor/GET separation; large embedded limits remain.**
   Separate private source-descriptor caching from decoded-blob caching. Message
   projection registers only authorized descriptors; GET decodes on demand under
   the existing four-worker/eight-response admission and held-Arc byte accounting.
   A single supported 32 MiB image may occupy the whole decoded budget; concurrent
   requests must get explicit Busy rather than evicting bytes still in flight.
   Lazy image loading needs visible load-failure/retry behavior in legacy.
3. Add a bounded native JSONL scanner and private native-span image sources so
   large base64 fields are not retained in Value or Parsed.bytes. It must parse
   JSON structure, duplicate keys, aliases, field ordering and escaped strings;
   regex matching a data key is not sufficient. Preserve native identity,
   record/span, normalized MIME, encoded length and a full-content semantic
   digest. Oversized ordinary strings must fail explicitly, not be replaced by
   empty placeholders. Known nested tool envelopes require equivalent handling.
4. Authorize native-span GETs through current SessionStore branch membership and
   trusted native roots, not external file_roots. Keep checked handles and verify
   identity/content. Ordinary append must not revoke unchanged historical images;
   replacement, truncation, image changes and branch exclusion must fail closed.
   Random tokens/offsets never substitute for content-aware semantic cursors.

Search must still consume text without decoding images, including sessions whose
images exceed the old limit. Complete selected-view file-reference authority must
remain separate from a finite display page. AVIF's independent parser/extent
limits remain explicit until separately implemented and validated.

## Batch 15: descriptor/GET split, before native spans

The implemented split follows these boundaries:

- Use a separately bounded private descriptor store. Embedded descriptors retain
  a charged strong image-source reference, not a whole View. Weak references alone
  lose not-yet-loaded images when ordinary append reprojects providers and old
  views are evicted. Encoded-source retention is a separate budget from blobs.
- Projection still chooses the display window first. For file descriptors it
  authorizes the selected scope and captures `FileVersion` immediately, without
  reading image contents. Deferring initial version binding until GET would let
  an old token silently bind a replacement file.
- GET reauthorizes every file access, including warm cache hits. A miss uses the
  newly checked, version-matched handle through read and final verification;
  it never reopens a display path or upgrades a token to replacement bytes.
- Preserve independent 256-item/32 MiB blob accounting and held-body/frame
  ownership. Do not hold the descriptor lock during decoding/file IO. Add bounded
  decoding admission so image work cannot occupy all shared history readers.
- Missing width/height means not yet inspected, not invented dimensions. Invalid
  containers now fail at GET rather than discarding readable history; legacy needs
  a visible per-image failure and controlled retry, not just a broken-image icon.

Acceptance must prove zero decode/blob allocation on initial history, pages and
SSE; first-GET decode and repeated-GET hits; bounded descriptor/encoded/blob
lifetime through append, eviction and cancellation; stale file rejection before
first GET and on warm hits; and real-browser image-load errors plus scroll
stability. This step alone still does not support large embedded native records.

## Required acceptance cases

The implementation needs four coordinated boundaries: a trusted streaming
native reader/index, structural JSON scanner with private span sidecars, shared
provider extraction context, and a native-span GET authority. The current
pre-batch-18 inventory and main reader rejected physical files over 16 MiB before provider
parsing; `Parsed.bytes`, record caching, parent-cut parsing and checkpoint slicing
depended on resident original bytes before batch 17. That batch replaces the raw
body and parent-cut byte slices with checked streaming reads and full-prefix
SHA-256 cache verification; batch 18 additionally carries structured native spans.
Fixing only `NativeImage` still cannot bypass provider/extraction/GET authority.

Keep physical offsets and full committed-prefix verification while replacing
resident raw bytes with bounded records/indexes. Stream prefix hashing once per
scan, not once per event. Parent cuts use the same reader/scanner, and inherited
end-zero events retain their real source record/span separately. Split resident
ordinary JSON cost from source encoded length; do not make `encoded_len()` zero
to dodge view/page limits. Pages of lazy spans charge descriptors, not a 24 MiB
decoded-image allowance; GET owns decoded-byte admission.

Known Codex tool-output JSON strings, including `Output:` prefixes and nested
envelopes, need bounded JSON-unescape views over physical spans. Keeping the
existing full String plus `from_str<Value>` path would retain/copy giant payloads.
Do not substitute forgeable public JSON markers for private extraction context,
or recognize image-looking metadata/tutorial strings as authorized media. Large
strings become spans only after their complete object/context validates type,
MIME, aliases and duplicate-key handling; other giant strings fail explicitly.
Escaping can enlarge physical input beyond ordinary base64 expansion, so raw scan
and nesting work need independent documented budgets.

- Exactly 32 MiB and one-byte-over images; large-record JSON escaping and partial
  final lines; malformed aliases/duplicate keys and non-image giant strings.
- 257 images spanning pages, over 16 images in one logical message, multiple
  events sharing an offset and inherited end-zero events.
- Concurrent append/rewind during pagination; Claude/Codex agents and fixed
  parent prefixes; native image content rewritten without changing its offset.
- Text-only search, lazy GET admission/cancellation, retained response frames,
  descriptor/blob eviction and explicit retry after Busy or stale authorization.
- Actual legacy desktop/mobile gap insertion, scroll position, media continuation
  and load errors; append cursor and full-history counts remain correct.

All tests must use isolated synthetic histories. This work does not authorize
production/native writes, model CLI invocations or a frontend framework rewrite.

## Batch 19: nested stringified tool output (implemented)

The proposal below is retained as the design record; batch 19 implements it as
`native_replay` (layered `ReplayReader`/`DecodePlan`/`WorkBudget`),
`records::tool_envelopes` (candidate/envelope classification) and
`native_records::replay_source` (stamped current-record reopening). Deviations
from the proposal are documented in [native-input.md](native-input.md#nested-stringified-tool-envelopes-batch-19):
GET uses its own 512 MiB budget rather than the projection's, and MCP `isError`
is kept as a sidecar rather than a marker. Multi-string concatenated giant tool
JSON still fails explicitly; per-message continuation remains the next entry.

Prefer one real outer physical source plus a bounded immutable decode plan over
per-character/segment physical maps. Inner ranges are relative to the preceding
decoded stream, never raw file offsets. GET opens one checked outer range and
replays bounded JSON-unescape/skip/take stages. Finishing the image is insufficient:
every parent stream's unread tail must also be drained and its full digest/EOF
validated before finishing the original checked handle.

Start only from reviewed Codex tool-output positions. Preserve known envelope
shape, `Output:\n` candidate priority, candidate counts and eight-level envelope
budget. Replays and parsing must share one cumulative work budget; every nested
layer cannot receive a fresh 256 MiB allowance. Move validated private Node trees
into the final stable provider tree, never serialized markers or giant copied
Strings. Multi-string concatenated giant tool JSON needs separate bounded-source
design; the existing small-string behavior does not prove it is implemented.

Independent implementation boundaries are the replay reader/work budget,
candidate/envelope classification, bounded native source-plan authorization, and
synthetic HTTP/browser acceptance. Cover parent-tail mutation, candidate order,
escaped keys, depth/total-work rejection, branch/agent/fixed-cut scope, ordinary
append stability and cancellation. Batch 19's `tests/native_envelopes.py`,
`tests/native_envelopes_authority.py` and Rust unit tests cover these cases.

## Batch 20: per-message media continuation (implemented)

A single logical message now carries up to 256 typed images; projections inline
the first 16 and issue a bounded media grant for the rest. The grant keeps the
first page's images and event identity stable, charges descriptors rather than
decoded bytes, and legacy appends each batch in place without moving the live
cursor. Contract, budgets and error codes are in [media.md](media.md#per-message-media-continuation-batch-20).
Remaining: a message with more than 256 typed images, which still fails
explicitly rather than dropping images.
