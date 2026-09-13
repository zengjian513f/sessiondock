# Synthetic Python/Rust media differential

`tests/media_parity.py` compares the explicitly selected Python checkout's real
`agenthub/adapters.py` and `agenthub/media.py` with an isolated Rust HTTP server.
It never starts the Python service or imports its index/server modules. All
histories and images are synthetic and live in one temporary directory.

```sh
cargo build -p sessiondock --locked
python3 tests/media_parity.py --python-source ../agenthub
# Optional: real Chromium assertions for the message-grouping regression.
python3 tests/media_parity.py --python-source ../agenthub --browser
```

The browser option uses Playwright's installed Chromium or the explicitly set
`PLAYWRIGHT_CHROMIUM_EXECUTABLE`. No Pillow or image CLI is needed at runtime.
The harness reuses fixed, independently browser-decoded PNG/JPEG/GIF/WebP/AVIF/BMP
fixtures, including two-frame GIF and WebP animations.

## Scope and safeguards

- Only the two Python modules are loaded under a private module namespace.
  `Path.home()` returns a temporary placeholder during import; all adapter roots
  are replaced before any adapter call. Session listing, scanning, inherited-SID
  discovery and native name-index access are disabled.
- Python adapter/media calls have explicit network and subprocess guards.
  Local image registration first proves that both the spelling and resolved
  target remain inside the synthetic directory. The original Python resolver
  then runs unchanged, including its different symlink and token semantics.
- Plain-body enrichment calls the real `media.enrich_message` directly, matching
  the Python index's enrichment stage without importing or running that index.
- Rust runs only on loopback, with explicit native roots and an optional,
  disjoint synthetic file root. Remote-image metadata is examined but never
  fetched. Original native fixture bytes are checked after the run.
- The replacement test changes one owned synthetic image intentionally. It
  neither rewrites native history nor changes files in the Python checkout.

## What equality means

The common contract compares message count and order, text, roles, turn/phase,
call identity, error/counting metadata, EOF cursor, image count/order, exact
decoded-byte SHA-256 and MIME. Inline Markdown references and gallery semantics
remain part of the comparison. It does not compare random media token values.

Display-only `alt` and derived dimensions are not equality keys: Python usually
omits dimensions and can copy dimensions supplied by a native block, whereas
Rust does not publish dimensions in lazy descriptors; it inspects the container
on GET and the browser obtains actual image dimensions. Rust uses a generic localized
caption where Python may preserve an image label or basename. These omissions
do not allow the harness to merge messages, remove placeholder text, drop
images, change bytes, reorder media, or forgive an unexpected HTTP status.
The existing three media browser suites independently check actual dimensions
and decoding, navigation, isolation, and native-byte immutability.

## Explicit differences, not compatibility failures

Every named difference asserts the behavior on both sides; disappearance or a
different result fails and requires review. On Unix the measured **49 cases**
comprise **28 exact semantic matches and 21 explicitly asserted differences**:
47 fixtures (28 matching, 19 different), plus file replacement and missing-root
authority scenarios. This is not a claim that all 49 outputs are identical.
The symlink fixture is omitted on Windows; this is not a Windows runtime claim.

| Case | Python reference | Rust behavior |
| --- | --- | --- |
| Native base64/data URI, six formats | Registers typed media | Same MIME and exact bytes |
| Claude/Grok direct tool-result and MCP image arrays | Preserves typed images | Same text, count, order and bytes |
| Codex tool-result image arrays (e.g. `view_image` output) | Stringifies image placeholders without media | Preserves private typed media and real text — intentional DELTA: real root codex:2c316cee4d039a0d shows 18 images where Python shows 1; every one is an embedded native image (always loadable, never a "图片不可用" placeholder) |
| MCP `{content:[...], isError:...}` tool envelope | Not extracted by the reference adapter | Explicitly recognized structural envelope; image bytes preserved |
| Native `source.path` | Reference `from_block` does not inspect this field | Scoped path reference supported |
| Unknown file extension with a genuine image container | Extension-based registration declines it | Checks the authorized file and sniffs supported container bytes |
| Raw-path gallery metadata | Gallery entry omits `ref` | Adds the exact recognized `ref`; bytes and gallery behavior match |
| No configured file roots | Python ambient local-path registration succeeds | HTTP 200 retains text; `media.error.status=501`, no image `src` |
| Path outside the allowed file root, still inside the fixture | Ambient registration succeeds | Per-image 403, no token |
| Synthetic symlink | Follows the link | Per-image 403, no token |
| Multiply hard-linked file inside the root (Python bug-report attachments) | Registers | Registers (read side accepts hard links since WP-D; only writes refuse them) |
| Text reference that does not resolve to an image file (missing path, cwd-less relative path, directory, over 32 MiB) | Registers nothing; text keeps `[图片: ref]` | Same: no descriptor, no placeholder (`media::silent_failure`); real root codex:e68eb048e365afb7 (13 Markdown images in a `cat` output naming files that do not exist) and claude:179009468904dece (`附件1: ./agenthub_attachments/1/image.png` after the cwd was renamed) project no media |
| Session whose branch exceeds the media reference budget | No limit | Index degrades (later references unknown, no placeholder); the budgets sit an order of magnitude above the largest real sessions ([files.md](files.md#representation-and-resource-policy)) |
| Remote native image | Returns external URL metadata | Explicit 501; no network fetch |
| Invalid container with an allowed MIME | Registers supplied bytes | History remains readable; image GET rejection 422 |
| 129-frame GIF | Accepts the bytes within its larger size limit | History remains readable; image GET rejection 413 |
| File replaced after token issuance | Old token reads the replacement path's bytes | Old token 409; explicit reprojection gives a new token and the replacement bytes |

Ordinary raw image-looking tool output and fenced paths are *not* automatically
loaded by either implementation. Explicit Markdown images in tool output are
eligible, subject to the respective file-authorization boundary.

Rust's smaller byte/cache budgets, 128-frame and aggregate-pixel limits,
response permits, no-follow handles, and current-view/version reauthorization
are intentional safety differences. This differential is not a full decoder,
resource-exhaustion proof, or verification of the deployed Python service.

## Compatibility defects found and fixed

The initial differential did **not** pass by stripping placeholders or joining
records. It exposed two genuine Rust adapter defects:

1. Claude adjacent native text/image blocks had been merged into one event.
   An eight-image native record counted as one message instead of eight;
   mixed text plus image counted as one instead of two.
2. Native image nodes became `[图片]` text lines before normal text aggregation.
   Codex/Grok multi-image-only messages repeated the placeholder for every
   image, and mixed/tool-result content gained extra placeholder lines.

Real Chromium reproduced the incorrect counts and grouping. Eight image
placeholders even caused a short Grok tool result to acquire an unnecessary
“expand full text” control. The provider fix preserves Claude's individual
block events and their original turn/phase/branch metadata. Other providers
keep their native single message; one `[图片]` fallback appears only when that
message has no actual text. Recognized MCP wrappers and the existing 16-image
budget remain enforced; literal user-written `[图片]` text is never stripped.

The optional browser regression checks the same four synthetic sessions:
Claude mixed content (2 messages), Claude eight images (8), Codex eight images
(1, one placeholder), and Grok mixed tool output (1, no invented placeholder
lines or unnecessary expansion control). Provider tests also cover call/error
metadata, image-only tools, nested wrappers, privacy, and unchanged plain text.
