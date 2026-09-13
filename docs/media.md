# Bounded local native media

`media::{NativeImage, MediaStore, MediaBlob}` implements an ephemeral, private
image cache. Batch 15 separates source descriptors from lazily materialized blobs.
Batch 18 adds structured native spans with current-branch authorization; see
[native-input.md](native-input.md#large-strings-are-not-image-authority).
Embedded parsing and text discovery perform no I/O. File-backed projection
captures a checked version in the separately authorized FileService selected-view
scope but does not read image contents; it cannot expand home directories, fetch URLs, invoke a CLI
or write native histories. A reference appearing in native text is not filesystem
authority. Malformed data URLs never fall back to filesystem reads.

## Recognized input and public projection

`NativeImage::from_block(&Value)` returns `Result<Option<NativeImage>, String>`.
Non-image values return None. Explicit `image`, `input_image`, `image_url`, an
`image_url` field, or a `file` block with a file object are recognized candidates.

Provider grouping follows native/Python semantics: Claude emits separate visible
content-block events, including individual image events, so counts and same-record
byte offsets must both be preserved. Codex/Grok retain one message for their
content array, use one `[图片]` only when no text remains, and do not add an image
placeholder to mixed text or tool-output text. Images stay in private typed event
media regardless of display grouping. This was corrected through batch 13's
strict adapter differential, not by merging counts or stripping text in tests.

Supported embedded representations include:

- Claude `source: {type:"base64", media_type:"image/png", data:...}`;
- `file: {base64:..., mimeType:"image/png"}` inside an explicit image/file block;
- top-level `data`/`base64` with `media_type`, `mime_type` or `mimeType`;
- `image_url` as a string or `{url:...}`, using
  `data:image/png;base64,...` or `data:image/jpeg;base64,...`.

Duplicate source aliases must agree; conflicting payload/MIME/URL declarations
fail. Missing data, malformed declared sources and excessive encoded length fail
without echoing any input. PNG (including APNG), JPEG, GIF, WebP, static AVIF and
BMP are supported. `image/apng` normalizes to `image/png`, `image/jpg` to `image/jpeg`, and `image/x-ms-bmp`
normalizes to `image/bmp`. SVG and unsupported codec subtypes fail explicitly;
they are not silently dropped or returned as empty images. External URLs are not
fetched, and the embedded constructor does not authorize reading local paths.
Data URLs must use base64; percent-encoded or non-base64 data URLs are not decoded.

NativeImage is Clone, with a private raw source and no Debug/Serialize.
Clones share one random 128-bit token, rendered as 32 lowercase hex digits. A new
parse gets a new token. `semantic_key()` is a deterministic SHA-1 digest of the
normalized MIME and full encoded-payload digest (`image-semantic-v2`), independent
of the token and inline/span/data-URL representation. Upgrading intentionally
resets pre-v2 image-bearing cursors once. It is a non-security
history/cursor key, not authorization or a secret token. Message digests must use
this semantic key so reparsing does not cause token-driven resets or omit content
changes. `encoded_len()` lets the history/view layer account for private image
payloads even though they are omitted from public message JSON.

## Text discovery versus file authority

`media::discover` examines only the projected message's `role` and string `text`,
not native JSON blocks, tool metadata or arbitrary nested objects. Markdown image
destinations are candidates across roles; raw paths are candidates only in user,
assistant and command messages, including their `·subagent` forms, and only
when spelled like a path — `/…`, `~/…`, `./…`, `../…` or a Windows drive —
exactly Python's `_RAW_PATH` rule, so a bare `shot.png` in prose stays text
(before WP-D Rust probed `cwd/shot.png` and could show a placeholder Python
never shows). Supported extensions match the formats below. Angle-bracket Markdown destinations support
spaces; ordinary path tokens may be quoted/backticked. Discovery preserves the
original reference for inline UI replacement and marks Markdown references
`gallery:false`, raw references `gallery:true`.

It skips line-delimited backtick/tilde fenced code, deduplicates exact references
and emits at most 16 candidates per message, with at most 256 discovered text
candidates per selected HTTP window (additional text remains unchanged). Remote/data URLs and network-authority
paths are excluded. Local `file:///` references use the file service's pure
normalization validator, but the original spelling is retained. A `~/` candidate
may be discovered; this does not permit home-directory expansion. Input is bounded
to 2 MiB and Markdown parsing has a 4 MiB byte-step budget to prevent repeated
unclosed image syntax from causing quadratic scanning.

Discovery neither opens files nor grants token access. The application resolves
candidates against the complete selected branch/agent reference index, then
registers only the selected message window using configured file roots and checked handles.
File responses require current-view authorization; historical discovery or an
extension alone is never a grant. Raw provider image blocks remain a separate
typed parsing path, not inputs to the text scanner.

The media reference index (`files/references.rs`, media mode) scans the whole
selected view once per immutable view revision and is then reused from a
small cache in `FileService` (8 entries, at most 1,000,000 retained
references; `ViewSnapshot::revision()` is the key), so a history window with
images and every file-backed media GET that reauthorizes against the same
view no longer rescans the branch (a 155 MB fork chain went from ~0.9 s per
GET to milliseconds). Its budgets are 512 MiB of scanned text and 250,000
references — an order of magnitude above the largest real sessions
(`files.md`). Beyond them the media index *degrades*: what was indexed stays
resolvable, later references are unknown, and no scope-wide error is
projected (the earlier 20,000-reference cap turned a 155 MB session's images
into "文件引用过多" placeholders; Python has no such limit). Typed native
references are always inserted. A bare image name resolves against the
session cwd (and explicitly mentioned same-basename paths, which stay
ambiguous rather than guessed), exactly like Python's `media.register_path`;
the "direct child of a mentioned directory" rule belongs to `resolve-files`
and is not applied to media, so a bare name never triggers a probe sweep.

Python's `media.register_path` registers nothing when a discovered reference
does not resolve to an image file: the text keeps its `[图片: ref]`/path
rendering and no placeholder appears. Rust projects the same for a
text-discovered reference whose failure Python would also swallow
(`media::silent_failure`): missing file (404), relative path without a cwd,
directory instead of file, device/socket, over the shared 32 MiB limit,
unparsable path, and a reference the (possibly degraded) index does not
contain. Refusals that Python would *not* have — no configured roots (501
`media_files_disabled`), outside the configured roots, symlink, permission
denied, ambiguous basename — remain visible `error` descriptors so the
operator sees why an image Python shows is withheld. Typed native file
references keep their error slot in either case. Multiply hard-linked files
are ordinary readable files ([files.md](files.md#filesystem-checks-and-platform-behavior)).

After the caller has selected a validated message window, it passes the complete
batch to `MediaStore::register_prepared`. This registers private source descriptors
without decoding or container inspection, returning:

```json
{"src":"/api/media/<32-lowercase-hex>","alt":"会话图片","lazy":true}
```

Dimensions and validated MIME are not available until GET inspects the bytes;
the descriptor does not invent them. Untrusted name/dimensions fields do not
become raw paths, attributes or diagnostics; alt is a fixed label. Byte-identical
images parsed separately need not share a token/cache item. Repeated clones of one
NativeImage share the same cached blob and return the same src. Only selected
windows should be registered: scanning native inventory must not decode/register
every historical image. Do not put base64 or private media descriptors into
message JSON, search output, SSE events or logs.

## Format and size checks

The inline embedded decoded per-image maximum is **1.5 MiB**, with at most **2 MiB** of base64.
Inline images above that go through native spans (below); the 64 MiB native
JSONL record budget bounds the ordinary body around them. This is not the Python media module's
32 MiB per-image promise. Strict standard base64 decoding rejects bad characters,
padding and noncanonical trailing bits. Allocation is bounded before decoding;
the decoder cannot allocate an unbounded intermediate buffer.

Structured native strings larger than the 2 MiB inline threshold now use private
spans and allow up to **32 MiB** decoded bytes, except AVIF's separate limit.
They retain source/range/hash metadata, not the encoded body. GET uses a bounded
JSON-unescape reader and one reserved decoded Vec; 32 MiB succeeds and an extra
byte fails even when base64 lengths are equal. The full file scan work is
capped at 4 GiB per opened session, including physical escapes. Giant stringified Codex tool
envelopes (up to eight nested layers) replay through a bounded decode plan under
a separate shared 512 MiB work budget; see
[native-input.md](native-input.md#nested-stringified-tool-envelopes-batch-19).
Ordinary oversized records remain unsupported; multi-part tool outputs are
concatenated chunk by chunk since batch 36
([native-input.md](native-input.md#multi-part-outputs-every-chunk-batch-36)).

## Per-message media continuation (batch 20)

A logical message may carry up to **256** typed images (`MAX_MEDIA`); more is an
explicit provider failure. Every public projection (initial window, SSE delta,
history page) inlines at most **16** typed image descriptors per message
(`media_projection::DISPLAY_LIMIT`) followed by discovered text/file images as
before. When a message has more, it also carries
`media_more: {remaining, total, cursor}`; `cursor` is a 32-hex grant token, or
`null` for callers without a page store (search and input history never see
`media`/`media_more`). Window and page budgets charge only the displayed prefix.

`GET /api/messages/{uid}/media-page?cursor=…&agent=…` returns
`{"media":[…≤16 descriptors…],"page":{cursor,next,start,end,total,remaining}}`.
Descriptors are registered through the same lazy path as inline projection
(native span, embedded, file references with the selected-view scope, error
descriptors); GET-time materialization, descriptor and blob budgets are
unchanged. Grants live in the history `PageStore` (1024 entries, ten minutes,
oldest eviction) and bind canonical UID, exact agent, the live checkpoint of
the producing view, the event's non-status index, a content identity (SHA-1 of
the projected message JSON, which never contains media fields), the image
offset and total. A page read never touches the live cursor; rereading a page
returns the same `next` token while it is still valid. Errors: 400 malformed,
403 wrong UID/agent, 404 unknown/evicted/cross-kind token, 410 expired, 409
checkpoint/event identity/image-count change, 413 page cannot advance or
response over 8 MiB, 503 when the eight shared page permits are busy.

Legacy renders `<button class="media-more">` inside the gallery only under the
Rust `media_continuation` capability; a click appends the next batch in place,
updates the cached message (`media` concatenated, `media_more` advanced or
removed) and never changes `end/anchor/version`. Failures keep every loaded
image, show `.media-page-error` with the server reason and offer a retry;
a stale view (selection change, reset, mismatched cursor) discards the result.
Python never emits `media_more`, so its page is unchanged. Native tokens bind canonical UID/exact agent;
cold and warm GET require current full-branch membership and checked native input,
without external file roots. A shared inherited image gets distinct scope tokens.

Explicit-root disk images allow up to **32 MiB** of compressed image bytes,
except AVIF's separate 1.5 MiB parser-input limit below. They share the same
32 MiB resident cache with embedded images; this is not 32 MiB per concurrent
image. Signature sniffing supports explicitly typed files with unknown or wrong
extensions. Extensions are hints, never authorization or sufficient validation.

The declared MIME must match the actual container signature. PNG validation checks
the signature, first/unique IHDR, supported header fields, chunk bounds and CRCs,
required IDAT/IEND, selected palette/order constraints and no trailing bytes or
unknown critical chunks. APNG additionally checks acTL/fcTL/fdAT ordering, CRCs,
contiguous sequence numbers, declared versus actual frame count, frame rectangles,
dispose/blend values and nonempty frame payloads. Both default-image-as-first-frame
and separate default-image fallback are supported. Batch 18 removes the old
full-image filtered PNG copy: animation and base checks borrow the same immutable
bytes, retaining both CRC passes. A deliberately tighter edge rejects animation
chunks after IEND; the old filtered path could hide such invalid trailing chunks.
JPEG validation checks SOI,
bounded markers/segments, an 8-bit baseline or progressive SOF, dimensions,
quantization/Huffman-table presence, SOS/scan boundaries and final EOI. Other JPEG
coding modes are explicitly unsupported. All formats require nonzero dimensions, no edge
above 8192 pixels, and at most 16,777,216 declared pixels.

`media/formats` adds these container checks:

- GIF87a/89a: logical canvas, palettes, bounded extension/data subblocks, graphic
  control shape, image rectangles and exact trailer. Plain-text rendering extensions
  are unsupported rather than excluded from the frame budget.
- WebP: exact RIFF size and zero padding, VP8/VP8L headers, extended canvas,
  declared metadata chunks, ANIM/ANMF frame structure and nested image dimensions.
  Frame rectangles must fit the canvas. Unknown RIFF chunks are skipped within
  their declared bounds; duplicate/invalid ordering of recognized chunks fails.
- BMP: exact file size, file/pixel offsets, core and Windows 40/52/56/108/124-byte
  DIBs, planes, palette bounds, uncompressed/bitfield modes, nonoverlapping contiguous
  channel masks, row stride and available pixel bytes. Windows top-down images are
  accepted. RLE, embedded JPEG/PNG, OS/2 extended DIBs and V5 profile pointers are
  explicitly unsupported, without rejecting ordinary BMP files.

GIF, WebP and APNG allow at most **128 frames** and **67,108,864 cumulative canvas
pixels**. Each frame charges the entire canvas, not merely its changed rectangle.
An APNG fallback outside its animation charges an additional canvas and frame.
Loop counts do not multiply this budget; continuous playback is browser-owned.
These checks follow the [GIF89a specification](https://www.w3.org/Graphics/GIF/spec-gif89a.txt),
[WebP container specification](https://developers.google.com/speed/webp/docs/riff_container),
[PNG Third Edition](https://www.w3.org/TR/png-3/) and
[Microsoft DIB documentation](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-bitmapinfoheader).

AVIF uses the pure Rust [avif-parse library](https://docs.rs/avif-parse/2.1.0/avif_parse/):
the library resolves the `pitm` primary item and its extents, then parses that
item's AV1 sequence header. Dimensions come from the selected bitstream, **not the
first ispe box**. All encountered ispe declarations are independently size-bounded.
An auxiliary alpha item must have matching dimensions and a still-picture header.
Static 8/10/12-bit AV1 is accepted; animated `avis`/movie tracks, grid-derived images,
rotation/mirroring and clean-aperture transformations remain explicit unsupported
subtypes. There is no libdav1d, native image decoder or helper-process dependency.

Before calling the AVIF parser, preflight limits input to 1.5 MiB, metadata to
64 KiB, box nesting to 8, total inspected boxes to 256, item declarations to 128,
total extents and property associations to 256 each, and each auxiliary-type
property to 4 KiB. The sum of declared extent lengths is capped at 3 MiB, counting
repeated extents repeatedly; zero-length/to-end extents conservatively charge the
whole file. Out-of-file and overflowing ranges are rejected before parser
allocation. These metadata/parser working buffers and the PNG/APNG validation
copy (at most 32 MiB for a disk image) are
bounded temporary workspace, separate from the 32 MiB resident-blob budget. The
two-media-worker limit also bounds concurrent parser work; the cache budget is not a
claim about total process RSS.

These are **container/header and declared-pixel bounds, not full pixel decoding**.
PNG zlib, JPEG entropy/Huffman, GIF LZW, WebP entropy and AV1 pixel contents are not
decoded. Passing the checks
does not prove that every compressed stream will render, that every codec semantic
is valid, or that browser decoder memory is bounded by the cache byte count. This
module does not resize, strip metadata, create thumbnails or certify browser codec
safety. Returned original bytes can still contain image metadata.

## Cache, response ownership and failure

Each MediaStore has independent descriptor and blob stores. The descriptor table
has at most 1024 entries and a **32 MiB retained encoded-source budget**, charged
by String capacity. Source Arcs retain no View; held tickets stay charged after
table eviction. File descriptors retain bounded scope/reference/version metadata,
not file handles or image contents. Registration accepts at most 256 references
and protects its entire batch from self-eviction; duplicate clones charge once.
The caller still enforces per-message and history-page budgets.

The blob cache admits at most 256 cached/in-progress images and has a separate
**32 MiB decoded-byte budget**. A GET reserves only its own image, not every image
on the page. Concurrent materialization of the same token returns explicit Busy.
Descriptor and blob locks are released before file IO, base64 decode or format
inspection, so registration does not wait for a decoder holding the cache lock.

The budget includes staged decode buffers and all live MediaBlob objects,
including blobs removed from the cache but still held by an HTTP response. The
last Arc's Drop, not cache eviction or GET completion, returns those bytes. A
retained Body frame therefore must retain its MediaBlob owner. `MediaBlob::bytes()`
and `mime()` expose only validated cached bytes/MIME, not original input, paths or
tokens. Dropping the store does not invalidate outstanding Arc bytes or prematurely
release their accounting.

Materialization reserves the missing image's decoded size before allocating.
Invalid bytes release the reservation and flight slot without publishing a blob;
the already readable history remains intact. Capacity occupied by held bytes
returns Busy, never an uncharged allocation. Eviction is nondurable: an evicted
descriptor returns 404 even if its old decoded blob is still cached. Explicit
history reload re-registers sources; ordinary append does not by itself revoke
the old strong embedded-source descriptor. No source token is a durable URL.

`ticket(token)` accepts only exact 32-character lowercase hex. Missing, malformed
or evicted tokens have no path fallback. `materialize(ticket, scope)` verifies
the ticket belongs to this MediaStore; file-backed tickets additionally require
fresh current selected-view authorization and the original checked version even
when a decoded blob is already cached. Embedded tickets retain immutable
in-memory snapshot bytes until eviction; they do not implement native-span
revocation on a later history rewrite. The planned native-span layer needs that
separate membership/content check. This is an opaque cache capability,
not a substitute for the application's authentication/session authorization.
The cache itself is not persistent and does not promise revocation of bytes already
returned to a caller when a source file or selected history later changes.

## File grants and error descriptors

Native image parsing can retain a normalized local path or authority-free
`file:///` URL privately, without opening it. Only file URLs percent-decode,
exactly once; ordinary percent signs are filename bytes, and home expansion,
network authority and URL query/fragment syntax are rejected. The file service
uses a complete selected-view index, including typed native references, before
any window-selected read. Basename ambiguity is not narrowed by pagination.

Prepared disk images retain a checked reader only through descriptor registration,
then release it without reading image contents. Each file projection creates a fresh random token holding the canonical
owner UID, optional exact agent identity, normalized reference and FileVersion.
GET/HEAD first acquire one of eight response slots, wait at most two seconds for
one of two media-worker slots (without occupying a shared Reader), then
resolve the current native view and recheck reference membership, explicit roots,
file/ancestor identity and version. On cache miss the newly checked handle is
retained through reading and verification, never reopened by display path. A
replaced file yields 409 for its old token, including before its very first GET;
a removed reference yields an unavailable-file error. File tokens do not silently
change content under an existing URL. Embedded tokens retain immutable in-memory
semantics. There is no filesystem watcher: explicitly reload the conversation to
project the latest disk image; already delivered bytes cannot be revoked.

Unconfigured, missing or outside-root disk images project as
`{"alt":"会话图片","error":{"status":501,"code":"media_files_disabled","message":"…"}}`
(with the actual status/code for other errors), without src or private paths.
Legacy renders an escaped visible explanation in place of that image. A failure
during a checked read currently maps to a generic 422 GET error, whereas
GET's explicit version mismatch is 409. This distinction is deliberate and does
not permit replacement bytes to be published. Container, MIME-mismatch, base64-content
and frame errors now fail at GET while text remains readable. Malformed native
source structure/aliases and encoded-length bounds still fail during parsing;
this step does not support oversized native records or per-message continuation.
Rust `media_lazy:true` enables visible image-load errors and explicit retry/reload
in the legacy UI. Python pages without that capability retain their existing behavior.

MediaError classifications are Unsupported (501), Invalid (422), Limit (413),
Busy (503) and Unavailable (503), with fixed input-free display strings. Native
parsing keeps its existing String-error interface; public projection exposes the
typed error and status. No raw image, URL or filename is attached to errors.

## Async integration responsibilities

Registration and materialization are synchronous and run in bounded blocking
read work. Registration never acquires the decoded-blob lock; materialization
holds it only for admission, lookup and publication, not decoding. The old eager
`project`/`get` helpers are compiled only for legacy low-level unit checks, not
the production HTTP path. The application owns four shared Readers, two media
workers and eight media response slots. Waiting for a media worker already owns
a response slot and has a two-second deadline. Cancelling an HTTP future must
not release a running blocking job's permits or reserved bytes early.

GET `/api/media/{token}` preserves the MediaBlob and HTTP-response permit through
`Bytes::from_owner`, including consumer-retained frames after the Body is dropped.
It sets the exact image content type, `nosniff`, `private, no-store`, content length
and a fixed inline filename. HEAD returns the same metadata without a body; other
methods are not implemented. Missing/evicted tokens return 404 and exhausted
response admission returns 503. A worker permit ending after lookup is not
sufficient accounting for slow consumers.

No server-side remote proxy is enabled. Rust declares `media_remote:false`, so
the legacy renderer keeps a text placeholder instead of automatically requesting
an external Markdown image. Explicit normal web-link navigation is unchanged.
Pages without this capability declaration retain the Python renderer's remote
image behavior; that compatibility branch is not described as purely local.

## Validation

Module tests use synthetic valid PNG/JPEG containers and cover provider wrappers,
random-token versus semantic identity, unsupported paths/URLs/MIMEs, conflicting
aliases, strict base64, MIME spoofing, CRC/marker/truncation checks, dimensions,
the actual 1.5 MiB boundary, whole-batch residency, re-registration, held Arc
accounting, failed-batch rollback, malformed tokens, store Drop, and empty projection
while the cache lock is held. These format/cache checks perform no filesystem
or network I/O; scoped-file checks use private temporary fixtures. None invokes
a CLI. HTTP/gallery rendering and browser decode remain separate integration
checks using isolated fixtures; header tests alone are not browser evidence.

The integrated `media_http` suite passes five tests, including eight held responses
and retained-frame ownership. `tests/media_browser.py` checks actual PNG/JPEG
decoding through all three native providers, structured tool outputs and a Grok
MCP wrapper; branch/agent separation, reload/SSE/mobile, no external image request,
and unchanged fixture bytes are verified. These use a newly built local server,
not mocked message responses or real CLI data. The separate `media_formats` HTTP
suite and `tests/media_formats_browser.py` cover new-format real decoder fixtures;
their results must be reported separately from these earlier PNG/JPEG checks.
Format module tests additionally cover APNG fallback/sequence/CRC errors, animation
budgets, BMP missing rows, cross-MIME spoofing, truncated containers, AVIF extent
amplification and oversized parser declarations. Filesystem-source authorization
is separate from format validation and must not be inferred from an image MIME.

Batch 12 adds five `media_files` HTTP scenarios and
`tests/media_files_browser.py`: actual three-provider disk-image rendering,
no-root errors without sources, unknown-extension sniffing, current-view
reauthorization, replacement 409 followed by explicit refresh, branch/agent
separation, complete-view basename ambiguity, denied roots and remote isolation.
On Linux an inotify IN_OPEN negative check plus full-view positive control proves
that omitted-window images are not opened. The file-cache unit tests separately
exercise retained handles, replacement during preparation and shared held-byte
budgeting. These are synthetic local acceptance checks, not live CLI or
Windows/macOS runtime verification.

Batch 15 adds twelve descriptor tests, two lazy HTTP scenarios and two file
version/revocation scenarios (seven file HTTP cases total). They cover zero
decoded allocation at registration, retained encoded tickets, source lifetime
after ordinary append, single-flight concurrency, descriptor/blob eviction,
first-GET file replacement and warm-cache authorization. API cancellation tests
use actual worker barriers; saturated media work leaves a history Reader free.
Linux IN_ACCESS additionally proves selected history registration does not read
the file's bytes, with the actual first GET as a positive read control.
`tests/media_lazy_browser.py` checks offscreen zero GET, visible errors, controlled
retry/reload, stale view isolation, no-gap recovery and render/SSE interaction.
Error-response injection is explicit; real supported images still use the actual
Rust GET and Chromium decoder. No performance multiplier or RSS result is claimed.
