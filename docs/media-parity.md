# Synthetic Python/Rust media differential

`tests/media_parity.py` constructs isolated Claude, Codex and Grok histories and
compares Python adapter output with SessionDock output. The corpus never reads a
real native root or remote URL.

The comparison covers provider grouping, placeholder text, data URLs, file
references, Markdown discovery, malformed candidates and supported image MIME
types. Rust tokens are opaque, so equality compares whether a descriptor exists
and whether its GET returns the original bytes and MIME.

SessionDock follows Python media behavior:

- a decoded image may be at most 32 MiB (`media.MAX_ITEM`);
- cache retention defaults to 128 MiB and eviction does not reject valid input;
- concurrent media GETs may all complete;
- there is no per-message reference count, discovery text/work, frame-count,
  pixel-count or decoded-envelope-work admission quota;
- invalid or unsupported image candidates do not reject readable message text;
- ordinary paths, percent decoding, `file://` parsing, symlinks and hard links
  use the same path semantics as Python;
- remote HTTP(S) references match Python; the test and server do not fetch them.

File references remain lazy: history exposes an opaque descriptor and the media
GET reads the selected source. Native embedded bytes also stay out of public
message JSON. Literal user-written `[图片]` text is preserved.

Run the differential only after building the server:

```bash
python3 tests/media_parity.py --python-source ../agenthub
```
