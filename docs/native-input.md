# Native input and structural scanner foundations

Batch 16 wired checked chunked input, raw JSONL checkpoints and a strict resident
record adapter into history reads. Batch 17 removes retained raw-file bodies and
uses direct resident JSON construction. Batch 18 adds provider-authorized native
image spans for structured content and structured tool results; nested stringified
tool output and per-message media continuation remain unfinished.

## Physical input and checkpoints

`CheckedNative` retains the opened file and checked directory handles, refuses
symlink/reparse-point ancestors and non-ordinary or multiply-linked files, and
checks identity and stamps before/after reading. Reads are at most 64 KiB;
completion requires consuming the expected length and passing final checks.
Failure cannot publish a successful partial snapshot. This is defensive checked
I/O, not a claim of a race-free filesystem sandbox on every operating system.
Runtime file/link/race evidence in this batch is Linux-only. Unix stamps include
device/inode/ctime; the current non-Unix stamp fallback uses creation time,
mtime and size, in addition to capability-layer identity checks. Windows cross
compilation does not validate NTFS/SMB/junction behavior or timestamp attacks.

`RawIndex` retains the first 4096 physical bytes, full raw SHA-1 and a compact
offset/digest checkpoint for every LF, including blank lines. Offset zero has
the empty-prefix digest; an unfinished final line is not committed. Cursor
schema `rs-m2-1`, physical byte offsets and prefix hashes remain unchanged.
An internal scan can cover at most 4 GiB and 2,000,000 LF checkpoints per file
(`sessions::budgets`, the table in [read-model.md](read-model.md)); the scan
runs only when a session is opened: there is no inventory-wide scan budget.
Non-Grok summaries retain 256 MiB and Grok summaries 16 MiB limits.
These are physical work budgets, not resident allocations. The LF limit is independent
of the 1,000,000 parsed-record limit per view.

All stores share an independent process-wide 1 GiB index capacity budget.
RAII charges cover the structure, head/digest buffers and checkpoint vector
capacities. Growth reserves old plus replacement allocations before allocating;
old snapshots and in-progress scans retain their charges until the actual index
is dropped. Exhaustion returns 413 for the view being built; the session list is
never affected (it does not build views).
This budget is separate from the raw-file, AST-cache and media budgets.
It is not shared between processes and is not an RSS/cgroup limit.

Batch 16 temporarily teed the stream into `Parsed.bytes`. Batch 17 removes that
field entirely: a bounded current-line buffer feeds strict decoding during the
same raw-index pass. A partial oversized line is scanned but not materialized;
it becomes an explicit history error only after LF commits it. The ordinary
line buffer is capped at 64 MiB (`budgets::RECORD_BYTES`); output ASTs, events
and media still retain their separate memory costs. Old immutable snapshots can coexist with new snapshots.

AST reuse now verifies SHA-256 over every byte of the old committed prefix, not
just a header, inode, length or timestamp. The raw index retains the committed
SHA-256 digest and at most one requested old-boundary digest; SHA-1 wire
checkpoints are unchanged. The speculative suffix and reused records are not
published before the prefix matches. A mismatch discards them and performs one
cold second pass through a newly checked handle bound to the same candidate
stamp. Read/change errors never restore a stale cache entry as a fallback.
This replaces direct comparison against retained old raw bytes with full-prefix
cryptographic verification; it is not append-only reuse of old Event vectors.

Fixed parent cuts use `CheckedNative::open_prefix`: only `[0, cut)` is readable,
but identity/stamp checks still cover the entire original file. The prefix goes
through the same decoder/index, then verifies its physical cut and recorded
prefix digest. Unit graph fixtures now use real private temporary input files,
not a test-only raw-byte fallback. Parent-tail changes can require a fresh view;
they never authorize a stale handle or change the leaf's byte cursor.

## Structural JSON

The private scanner consumes one complete bounded record, validates grammar,
UTF-8, surrogate pairs, escaped keys, nesting and trailing JSON whitespace.
Numbers use the existing serde number representation. Object insertion order
must match the workspace's `serde_json/preserve_order`: tool summaries select
parameters in that order. Batch 17's `scan_value` constructs the final `Value`
containers directly through the same generic grammar/checks as the private
`Node` scanner, without building and transferring a second object tree. Both
paths use the same conservative logical charges; this optimization does not
relax node, duplicate-key, ordering or resident admission. Ordinary records have
a 64 MiB physical line limit and a 2 MiB span threshold, with an independent
256 MiB conservative AST weight budget (4× the record, as 8 MiB was for 2 MiB
records). Limits also cover nodes (3,200,000), keys (1,600,000), individual keys
(16 KiB), number tokens (1024 bytes), and depth (128).
Logical weights and fixed buffer bounds are not allocator/RSS measurements.

Intentional safety differences from direct serde parsing:

- Duplicate keys, including differently escaped spellings of the same key,
  fail closed rather than using the final value.
- A small physical record can exceed node/key/resident budgets and be rejected
  before building an enormous disposable AST. The previous cache-overweight
  fixture with hundreds of thousands of nulls now explicitly asserts rejection.
- Native hardlinks and excessive physical LF checkpoints are rejected explicitly.

Records rejected by the adapter use the existing unsupported-history path; they
are not silently skipped. Repairing the input restores readable history with
normal reset semantics. Summary metadata and known stringified tool JSON remain
on their existing bounded parsers pending the next integration step.

## Large strings are not image authority

The scanner can replace an oversized string with a **private**
`TextSpan`: physical range inside the quotes, decoded UTF-8 length, full decoded
SHA-1 and whether JSON escaping occurred. The default threshold is 64 KiB;
production uses 2 MiB. A bounded decoded prefix and data-URL suffix digest are
lexical evidence only, not permission to read an image.
`into_value` fails explicitly on an unmaterialized span. No public JSON marker,
empty string, remote URL or synthetic media reference substitutes for its data.

Batch 18's pull decoder observes all physical bytes through `RawIndexBuilder`,
including read-ahead, skipped AST prefixes and partial tails. The first underlying
I/O/index error remains sticky even if the structural scanner catches it. Records
up to 64 KiB keep the direct Value path; longer records use the private tree.
Only an entirely valid, provider-recognized structured image can discharge a span
as media. Every other top-level span (tutorial extensions, tool arguments, a
pasted giant user text, a plain tool output without an envelope) is ordinary
text: after the image walk it is read back from the same stamped record range
through the checked range reader and the JSON-unescape reader, verified against
the span's decoded length and SHA-1, and only then becomes part of the row
(batch 34; `Node::materialize_spans`). It never becomes an image, a marker or an
empty string; a source that changed under the read is a per-session 503 retry.
Giant text inside a decoded envelope layer has no file range and stays an
explicit failure. Ordinary records (no image span) stay bound by the 64 MiB
physical line limit, and after removing authorized payload fields the remaining
ordinary JSON must also serialize within 64 MiB, counted through a bounded
writer without a body copy. One image cannot exempt unrelated large ordinary
content from its own budget.

Classification validates all payload aliases and normalized MIME, removes the
encoded fields and keeps typed sidecars at reviewed JSON paths. Provider lookup
uses addresses of the final borrowed Values; JSON cannot forge those addresses,
and clones or newly parsed stringified JSON do not inherit the authority. Sidecar
paths/string capacities are charged separately, with at most 256 sidecars and
8 MiB sidecar weight per record. Event-level 16-image limits still apply.

Each span retains its trusted native root/path, stable file identity, real source
record/range, decoded string length/SHA-1, data-URL offset and payload digest.
GET checks membership in the full current selected branch, including inherited
origins, then opens only that range using the candidate stamp which produced the
view. It never uses external file roots or accepts a fresh replacement stamp.
Bounded JSON unescaping and streaming base64 decoding are followed by mandatory
EOF/digest and retained-handle verification before HTTP publication. Warm hits
also reauthorize and read/hash the current source; they avoid decoded allocation,
not validation I/O. Ordinary append preserves unchanged source spans.

Structured native spans permit decoded images up to 32 MiB (AVIF keeps its
separate 1.5 MiB parser limit). Inline sources retain their old limits. History and
pages charge span metadata; GET owns the independent 32 MiB decoded budget.
Escapes count toward the 4 GiB per-file physical scan limit
(`budgets::FILE_BYTES`), so not every spelling of a 32 MiB image is supported. Batch 19 adds bounded replay of giant **stringified**
Codex tool envelopes and batch 36 decodes every part of a multi-part output
(below); a single envelope split across several strings is not a shape Codex
writes and stays unsupported.

## Nested stringified tool envelopes (batch 19)

Codex `function_call_output`/`custom_tool_call_output`/`local_shell_call_output`
payloads, MCP `{content:[...],isError}` wrappers and single text blocks whose
`output`/`text` string exceeds the inline threshold become a **private
`TextSpan`** first. Only reviewed tool-output positions are candidates: ordinary
user/assistant messages, tool arguments, unknown giant objects and MCP content
arrays holding more than one giant string fail explicitly (HTTP 501), so a
tutorial or argument that merely looks like a tool envelope gains no authority.

`tool_envelopes` streams the candidate string through the same JSON-unescape
reader and locates candidates exactly like the small-string parser: the text after
the last `Output:\n` marker, the first non-whitespace character and at most 32
object-start lines; whitespace trimming is Unicode-aware. Each candidate is
rescanned with the structural scanner (duplicate keys, 128 depth, 2 MiB inline
strings, 8 MiB resident) and accepted only when the object carries `output`,
`wall_time_seconds` and one of `exit_code`/`session_id`/`chunk_id`. Syntax
misses continue to the next candidate; source, budget and structural errors are
fatal. Every candidate open drains and finishes the whole checked source before
the next open, so a miss cannot leave an unverified partial read.

Recognised envelopes replace the string with the validated private tree and the
walk continues into `output` with the accumulated `DecodePlan`: one physical
outer range plus up to eight inner `StringRange`s whose offsets address the
preceding **decoded** stream, never file bytes. Images found inside carry that
plan in their `NativeSpan`; the sidecar charges the plan's heap bytes. `isError`
from an MCP wrapper is kept as a separate sidecar exactly as the small parser
does (an array containing an MCP wrapper still does not inherit the flag). The
ninth stringified layer is rejected, and the residual ordinary JSON after
removing image payloads must still serialize within 64 MiB.

All candidate scans and reopenings while projecting one record share a single
`WorkBudget` (512 MiB, charging physical bytes read plus every decoded layer's
output); GET replays the plan under its own 512 MiB budget. Depth does not
replenish either: a two-layer 32 MiB image makes the history request fail with
413 even though each layer alone would fit. The failed flag is sticky, and
exhaustion is classified only after the failing read or finish, including the
parent-tail drain performed by `ReplayReader::finish`.

GET opens the outer physical range with the candidate stamp that produced the
current branch, rebuilds the layered reader from the immutable plan, streams the
innermost string through base64 decoding, then finishes every parent: unread
tails are drained, each layer's decoded length/SHA-1 and the outer EOF are
verified, and the retained checked handle completes before HTTP publication.
The image bytes being identical is not enough: rewriting a non-image field in
the outer tail with unchanged file size and mtime revokes cold and warm tokens
(409) because the plan's parent digest no longer matches. Warm hits reauthorize
the full current branch and re-read the source like direct spans.

Fixed parent cuts and Codex subagents keep the real outer record range and
leaf-only end; scope tokens stay per selected view. `tests/native_envelopes.py
--browser`, `tests/native_envelopes_authority.py` and
`tests/native_envelopes_benchmark.py` cover 1/2/4/8 layers, `Output:` prefixes,
escaped inner base64, Unicode trimming, mixed structured/stringified envelopes,
MCP `isError`, exact 32 MiB and one-byte-over, shared-budget rejection,
append stability, same-size restored-mtime outer-tail rewrites, fixed-cut and
agent authority, the multi-chunk output below, and Chromium rendering including
the mobile console entry.

### Multi-part outputs: every chunk (batch 36)

Codex records an `exec` script result as an `output` **array**: part 0 is the
header `Script completed\nWall time … seconds\nOutput:\n`, then one text part
per streamed chunk holding one stringified envelope
(`{"chunk_id","wall_time_seconds","exit_code"?,"session_id"?,
"original_token_count","output"}`), sometimes followed by an empty part or an
`input_image` part, and the script's own prints (`--- 1 ---`, `{}`, `[]`,
`{"i":0,"status":"fulfilled"}`, a web result) can sit anywhere between. A part
is a **chunk** when its Unicode-trimmed text is exactly one envelope object, or
when it already is a structural envelope (a giant part decoded by the replay
below). When at least one part is a chunk the result is built from the chunks
only, in part order: the text is the concatenation of every chunk's `output`
with no separator added (chunk outputs carry their own newlines), `exit_code`
is the last chunk that has one, `duration_s` is the sum of every chunk's
`wall_time_seconds`, and an MCP `isError` inside any chunk marks the result.
The header, prints, an empty part and a `{"i","status","value":{…}}` wrapper
are never chunks and never enter the text (the wrapper form shows the raw
joined text like the reference adapter); a prefixed envelope inside one part
is still found by the single-envelope search above. One chunk therefore gives
the same result as the batch-19 search; two or more give the full output.
Image parts next to the chunks and images inside a chunk's output register as
media in part order exactly as before (`tools::sanitize_output_with_media`
looks structural chunks and image parts up at their original addresses).

The reference `_tool_output` joins the parts before decoding and therefore
shows one chunk: the last one when the joined text ends with a chunk (its
reversed object-line search parses that suffix), otherwise the first envelope
part (a trailing `[图片]`, `{}` or print breaks every suffix). This is a
deliberate superset **DELTA** (`docs/migration.md`): `tests/advanced_parity.py`
(`codex-envelope-chunks`) declares it per field and `tests/shadow_compare.py`
classifies a tool result whose Python text is one whole piece of Rust's longer
text as documented, printing both lengths.

On the native path a multi-part array at the reviewed output position is not one
candidate: the walk visits each part as its own reviewed position under the
record's single shared `WorkBudget`. A giant chunk part is decoded in place
through `tool_envelopes::parse` (span candidate, never read back as text and
parsed a second time); its inner `output` beyond 2 MiB still has no file range
and fails as before, while images inside it carry the accumulated plan. A giant
ordinary part stays a top-level span and is read back verbatim after the walk;
it is not a chunk. `tests/native_envelopes.py` covers a giant chunk between
small ones with a giant ordinary part and a trailing image part;
`sessions::records::native_records::tests` and `sessions::providers::tools`
unit tests cover 2/3/5 chunks, `exit_code` only on the last, an empty trailing
part, prints between chunks and the structural chunk.

Image semantic keys now use `image-semantic-v2`, normalized MIME and the full
base64 payload SHA-1, uniformly for inline/span/data-URL forms. This intentionally
resets old image-bearing cursors once across versions; wire cursor schema and
physical SHA-1 checkpoints remain unchanged.

## Validation scope

Scanner tests use chunked and generated readers. Contract tests compare exact
serde values, not normalized shapes: repository synthetic provider records,
known tool envelopes, seeded values, numeric/Unicode boundaries, truncated
prefixes and 1 MiB ordinary text. Value equality alone ignores map ordering:
the initial contract suite missed a key-reordering regression which the actual
tool adapter/browser check exposed (`z=first ...` became `a=False ...`). The
contract suite now also requires identical serialization, with deliberately
unsorted nested tool arguments; Chromium checks the displayed summary order.
Native-reader tests use private temporary
files and cover identity changes, link rejection, read completion, physical
limits and exact checkpoint hashes. Production history, append/rewrites,
parent cuts, finite pages, media and browser regressions remain required.

Performance uses the same `tests/append_benchmark.py` and separate saved release
binaries, with fresh isolated server processes. HTTP plus JSON decode is timed;
fixture writes/startup are excluded and the OS page cache is not cold. Always
report first-read costs as well as append costs. No paid CLI or real history is
needed for these checks.

## Batch 16 release comparison

Same Linux loopback benchmark, 1000/10000 records, each provider and size with
five fresh-process samples per binary (30 samples each). Saved pre-16 SHA-256:
`c159ee2cb2ee97955065a8f887a20e28f00414b384ea03168394e0480692f7b9`.
Final ordered-scanner release:
`72ad3bd228bc4c17453b9aeadf4a3ff52f80b21e5a6284b3b55bfa6e0eac4f54`.
All append/idle/same-length rewrite checks passed, including changes past the
4096-byte head with restored mtime. This is a small sequential observation,
not an isolated CPU benchmark, statistical confidence interval or RSS test.

At 10000 records, milliseconds, old → new:

| Provider | First window p50 (p95) | Append one p50 (p95) | Rewrite p50 (p95) |
| --- | --- | --- | --- |
| Claude | 175.992 (194.543) → 224.096 (228.696) | 144.471 (151.349) → 157.983 (178.487) | 129.008 (134.905) → 156.363 (165.840) |
| Codex | 113.160 (138.681) → 141.774 (156.493) | 79.673 (89.002) → 88.260 (94.313) | 70.528 (76.365) → 99.981 (111.706) |
| Grok | 86.359 (106.515) → 120.901 (126.756) | 69.624 (76.329) → 75.927 (85.739) | 50.113 (50.721) → 64.839 (68.506) |

At 1000 records, first-window/append p50 old → new respectively:
Claude 26.629/13.888 → 29.437/12.718 ms;
Codex 17.272/6.861 → 25.396/7.901 ms;
Grok 14.263/6.801 → 16.277/7.292 ms.
At 10000 records idle deltas remained about 0.76–1.30 ms p50.

There is **no overall speedup** in this batch: the observed 10000-record first
read costs increased about 25–40%, and append costs about 9–11%. Strict structural
validation, the extra AST representation/transfer, checked path traversal and
all-LF digest indexing add work; this comparison does not isolate their separate
shares. The source-only span hashing optimization avoids needless hashing of
small text, but does not eliminate those other costs. The next integration must
profile/reduce redundant representation and scanning, alongside replacing the
retained whole-file vector. Do not advertise these foundations as a performance
improvement or treat their acceptance as permission to ignore this regression.

## Batch 17 direct construction and streaming comparison

The explicit ignored **free CPU-only** `scanner_cpu_benchmark` compared the old
Node-then-Value path with direct Value construction in one release test process,
alternating their order over five samples. p50 milliseconds: repository native
records repeated 1000 times 50.735 → 35.930; small objects 67.705 → 49.778;
64 KiB text 12.932 → 12.971; stringified tool envelopes 23.434 → 22.986. These
measure parse/output-drop only, not HTTP or native I/O; big-text throughput did
not materially improve. The ordinary workspace run leaves this test ignored.

Whole-server comparison reruns the same updated `append_benchmark.py --rss`
against saved batch 16 (`72ad3bd…4f54`, full hash above) and final batch 17:
`c5e3d3b73d51f1606e4dae25a83fb456b1cd9a887a25a808bebc28e3a3a9e7c0`.
Each binary had 30 fresh-process samples: three providers × 1000/10000 records
× five samples. All cursor/rewrite checks passed. RSS/HWM are Linux `/proc`
figures for the exact live server child, read outside HTTP timing windows.
They include all server allocations and are not a cross-platform memory bound.

10000-record milliseconds, batch 16 → 17:

| Provider | First window p50 (p95) | Append p50 (p95) | Rewrite p50 (p95) |
| --- | --- | --- | --- |
| Claude | 216.341 (220.233) → 194.745 (200.790) | 153.709 (162.934) → 148.882 (152.232) | 157.425 (167.482) → 148.642 (153.982) |
| Codex | 152.439 (159.904) → 130.126 (149.163) | 87.454 (90.496) → 82.182 (85.781) | 100.207 (110.676) → 88.294 (92.504) |
| Grok | 107.609 (114.264) → 104.505 (112.493) | 73.649 (76.041) → 73.376 (82.632) | 64.965 (66.927) → 60.574 (62.298) |

10000-record process memory medians, MiB, batch 16 → 17:

| Provider | RSS after first window | HWM after complete scenario |
| --- | --- | --- |
| Claude | 88.91 → 85.75 | 124.16 → 118.08 |
| Codex | 70.04 → 67.60 | 90.23 → 86.09 |
| Grok | 57.32 → 55.58 | 80.14 → 76.11 |

After the final phase, current RSS did not consistently fall (Claude and Codex
were slightly higher); peak reduction is not a claim that every phase uses less
memory. At 1000 records, first/append p50 old → new: Claude 38.372/13.999 →
28.584/12.791 ms, Codex 25.095/8.198 → 18.423/7.748 ms, Grok 20.125/9.083 →
18.581/6.828 ms. Small samples are noisy, especially the tiny Grok append change.

Thus this batch observes roughly 3–15% lower cold-read p50 and 0–6% lower append
p50 at 10000 records, with 4–6 MiB lower peak process RSS. It does **not** fully
recover the earlier batch-15 costs, isolate each optimization's contribution,
or imply huge-image support. Full AST retention/projection/event construction
remain significant costs. Source span classification, nested unescape readers
and current-branch media authority remain the next implementation boundary.

## Batch 18 measurement method and intermediate finding

Ordinary-history comparisons continue to use the identical
`append_benchmark.py --sizes 1000 10000 --samples 5 --rss` against saved batch 17
(`c5e3d3b73d51f1606e4dae25a83fb456b1cd9a887a25a808bebc28e3a3a9e7c0`)
and batch 18. These are 30 fresh-process samples per binary with real HTTP
append/rewrite assertions, not a claim of cold disk or statistical significance.

`native_spans_benchmark.py` additionally uses synthetic valid ancillary-padded
3/32 MiB PNGs, three fresh-process samples per size. GET consumes the full body
in 128 KiB chunks and verifies SHA-256; those costs are included in timing.
Fixture construction, process startup and Linux `/proc` samples are excluded.
RSS/HWM belong to the exact live server child, not Python's large fixture buffers.
HWM is a cumulative process peak, not a per-operation allocation count.

The first batch-18 candidate
(`3eccaedf3660f64ed697c796b00bb9b9e169beec33218cbf04a6a76012e5acc3`)
passed these checks but did **not** improve overall ordinary-history speed:
10000-record first-window p50 increased roughly 2–13% versus the adjacent batch-17
run, while append and HWM were approximately flat. Its 32 MiB image first-window,
cold-GET and warm-GET p50 were 215.917, 527.735 and 170.093 ms. After first window,
server RSS/HWM were 16.203/16.203 MiB; after cold GET, 48.297/74.168 MiB.
The peak exposed the existing PNG/APNG validator's full-image temporary copy;
reporting only the retained 48 MiB would have hidden it. These are intermediate
measurements, not the final post-removal binary results below.

### Final batch 18 ordinary-history comparison

Final release SHA-256:
`33d790a541af2a7be63219606979dc07b96c3d3e1c2a82e9a2c1783132d2e920`.
All 30 final-release and 30 saved-batch-17 correctness samples passed. At 10000
records, milliseconds (saved batch 17 → final batch 18):

| Provider | First window p50 (p95) | Append p50 (p95) | Rewrite p50 (p95) |
| --- | --- | --- | --- |
| Claude | 199.602 (215.157) → 207.012 (224.286) | 149.273 (153.258) → 155.568 (178.932) | 150.848 (155.435) → 155.487 (159.173) |
| Codex | 130.112 (131.634) → 137.247 (139.251) | 80.718 (87.385) → 85.310 (95.490) | 89.183 (95.818) → 95.520 (104.792) |
| Grok | 99.751 (133.408) → 101.098 (118.646) | 72.772 (85.600) → 71.799 (74.052) | 62.820 (64.451) → 67.350 (68.605) |

First-window RSS / complete-scenario HWM medians, MiB:
Claude 85.82/118.17 → 86.05/118.11;
Codex 67.59/86.16 → 67.68/86.21;
Grok 55.52/76.17 → 55.79/76.09. Thus ordinary-history peak memory is approximately
unchanged, not a whole-workload improvement. First-window p50 rose about 1–6%;
append changed roughly -1–6% and rewrite rose 3–7%.

At 1000 records, first/append p50 old → final: Claude 24.668/11.892 →
32.713/12.658 ms; Codex 30.698/13.121 → 20.322/7.783 ms;
Grok 16.389/6.838 → 17.475/7.458 ms. These small samples vary substantially.
The intermediate and final ordinary-history measurements also differ despite
the intervening change targeting PNG inspection, not text-only records. Do not
attribute that timing variation to PNG removal or claim statistical significance.
Large-media support adds capability; ordinary parsing/projection still needs work.

### Final batch 18 large-image measurement

The same six-sample native benchmark passed on final release `33d790a…e920`:

| PNG size | First window p50 (p95), ms | Cold GET p50 (p95), ms | Warm GET p50 (p95), ms |
| --- | --- | --- | --- |
| 3 MiB | 30.375 (34.547) | 50.568 (53.651) | 19.539 (21.071) |
| 32 MiB | 242.132 (247.295) | 514.907 (520.581) | 175.165 (175.546) |

Exact-server RSS/HWM medians, MiB:

| PNG size | Startup | First window | Cold GET | Warm GET |
| --- | --- | --- | --- | --- |
| 3 MiB | 15.473/15.473 | 16.043/16.043 | 19.141/19.141 | 19.176/19.176 |
| 32 MiB | 15.426/15.426 | 15.977/15.977 | 48.113/48.113 | 48.148/48.148 |

The 32 MiB sample's observed cumulative peak fell from about 74 to 48 MiB after
removing the PNG filtered-body copy, while first-window RSS stays about 16 MiB.
This supports removal of that concrete temporary buffer, not a universal RSS
ceiling or general speedup: first-window/warm timings varied and PNG still checks
CRC twice. These are valid ancillary-padded 2×3 PNGs, not a varied photographic
or worst-case compressed-pixel workload. Chromium separately verified actual
decoding; the benchmark itself performs no Python image decode. Warm GET saves
decoded allocation but still reads, unescapes and hashes the currently authorized
native source. Bytes delivered to clients cannot be retroactively revoked.

## Batch 19 nested-envelope measurement

Ordinary-history comparison reuses the identical
`append_benchmark.py --sizes 1000 10000 --samples 5 --rss` against the saved
batch-18 release (`33d790a5…e920`, the batch-19 baseline) and the final batch-19
release `6be953858fe195e746411fe97a4d5c0ad02dd78bcd47c8928fa2c2e0b5242238`.
All 30 + 30 correctness samples passed. At 10000 records, milliseconds
(saved batch 18 → final batch 19):

| Provider | First window p50 (p95) | Append p50 (p95) | Rewrite p50 (p95) |
| --- | --- | --- | --- |
| Claude | 223.372 (240.961) → 216.777 (224.863) | 154.097 (162.379) → 157.408 (159.292) | 166.412 (171.977) → 166.443 (169.771) |
| Codex | 141.161 (171.451) → 146.613 (179.956) | 84.405 (94.425) → 89.746 (119.956) | 105.090 (106.008) → 99.234 (105.763) |
| Grok | 105.259 (120.568) → 102.955 (108.902) | 77.521 (80.297) → 76.384 (77.953) | 73.476 (74.142) → 72.889 (73.364) |

First-window RSS / complete-scenario HWM medians, MiB:
Claude 85.95/118.11 → 86.11/118.16; Codex 67.71/86.16 → 67.85/86.33;
Grok 55.70/76.20 → 55.83/76.14. First-window p50 moved -3%…+4%, append
-1%…+6%, rewrite -6%…0%; peak memory is unchanged. The nested path is only
entered for giant tool strings, so ordinary history neither gains nor loses
measurably; the differences are within the run-to-run variation seen in batch 18.

`native_envelopes_benchmark.py` (three fresh-process samples per row, synthetic
ancillary-padded PNGs inside one Codex `function_call_output` string; GET
consumes the full body and verifies SHA-256):

| Envelope | First window p50 (p95), ms | Cold GET p50 (p95), ms | Warm GET p50 (p95), ms |
| --- | --- | --- | --- |
| 3 MiB, 1 layer | 67.434 (82.746) | 63.630 (64.379) | 26.081 (26.412) |
| 32 MiB, 1 layer | 680.429 (681.010) | 636.782 (654.384) | 228.243 (234.101) |
| 3 MiB, 8 layers | 682.741 (688.691) | 101.874 (107.183) | 66.185 (66.299) |

Exact-server RSS/HWM medians, MiB:

| Envelope | Startup | First window | Cold GET | Warm GET |
| --- | --- | --- | --- | --- |
| 3 MiB, 1 layer | 15.480/15.480 | 17.891/17.891 | 21.027/21.027 | 21.066/21.066 |
| 32 MiB, 1 layer | 15.566/15.566 | 18.066/18.066 | 50.141/50.141 | 50.184/50.184 |
| 3 MiB, 8 layers | 15.594/15.594 | 18.102/18.102 | 21.238/21.238 | 21.277/21.277 |

Compared with batch 18's structured 32 MiB span (first window/cold/warm about
242/515/175 ms, cold-GET RSS about 48 MiB), one stringified layer costs roughly
2.8× on the first window and 1.2–1.3× on GET: the projection streams the outer
string once for candidate discovery and again for the structural rescan, and GET
replays one more unescape layer. Eight layers multiply the first-window cost
about tenfold for a 3 MiB image while memory stays at the same ~18 MiB, which
is the intended trade: replay work grows with depth, resident bytes do not.
These are synthetic padded PNGs and small samples, not a photographic workload
or a throughput claim; the 32 MiB two-layer case is a deliberate 413, not a
timing. Warm GET still re-reads, unescapes and hashes every layer.
