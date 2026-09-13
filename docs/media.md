# Native media

`media::{NativeImage, MediaStore, MediaBlob}` projects provider-recognized images
through private opaque URLs. Native history bytes are never returned inside
message JSON and are never modified.

## Recognition and grouping

Recognized embedded forms include Claude base64 sources, Codex/Grok image and
file blocks, and `data:image/...;base64,...` URLs. Supported MIME types are PNG,
JPEG, GIF, WebP, AVIF and BMP. Invalid or unsupported candidates are ignored so
readable surrounding text still matches Python.

Claude preserves separate content-block events. Codex and Grok keep their
content array in one event and add `[图片]` only when no text remains. Typed media
is retained independently of this display grouping.

Tool result strings and nested stringified envelopes are searched recursively
without SessionDock-specific depth, candidate-count or replay-work quotas.
Ordinary user text is not image authority.

## File references

File references are registered lazily through the selected session's file
service. Registration passes the original reference so Python-compatible path
handling performs percent decoding exactly once. Relative paths resolve as in
Python, `file://` references use their parsed path, and symlinks and hard links
are followed. Local reads do not require a configured file root. HTTP(S) URLs
are returned as external image references for the browser to load directly;
the server does not fetch or proxy them.

History captures the file version used for a descriptor. Replacement before a
GET yields a retryable conflict instead of serving a different version.

## Size and cache behavior

Python's `media.MAX_ITEM` is retained exactly: one decoded image may be at most
32 MiB. Base64 input is bounded by the corresponding encoded length before
decode. There are no additional reference-count, batch-byte, dimension,
pixel-count, animation-frame or container-parser size admission rules.

The materialized blob cache defaults to Python's 128 MiB and 512 entries. Those
values govern eviction only. Response ownership may temporarily keep more bytes
alive. Concurrent GETs are allowed to finish even when cached entries are
borrowed or retention targets are exceeded.

Format inspection is best effort metadata. A browser or client remains the
decoder; inability to derive width and height does not turn otherwise accepted
bytes into a session or media admission error.

Image fields use Python's first matching source and alias priority. Base64
decoding accepts Python's trailing-bit and complete-group padding forms.

## Continuation and errors

The initial history display includes a bounded transport slice and issues an
opaque continuation cursor when more typed images exist for that event.
Continuation eventually exposes every image and is bound to the same view
version. Page targets group the response and do not cap the message's media.

Unknown, expired or wrong-view tokens fail without exposing source paths.
Changed sources produce a conflict/retry response. The public response sets the
stored MIME, `nosniff` and private no-store caching headers.

Synthetic coverage lives in `tests/media_*.py`; unit coverage lives under
`src/media` and `sessions::media_projection`.
