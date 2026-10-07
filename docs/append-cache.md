# Reusing native JSON records on append

The session store now has a disposable JSON-record cache. It avoids reparsing
unchanged JSON strings; it is **not an incremental timeline parser** and does not
turn history reads into constant-time operations.

## Reuse boundary

Every changed candidate still goes through the existing complete stamped-file
read, trusted-path/open-handle checks and before/after restamping. The read
spans the stamped source length (`CheckedNative` on `[0, stamp.size)`). It does
not stop at a line-length, record-count or structural quota. A cache entry can
be reused only when it is the exact previous published `Parsed` candidate
(`entry.candidate == previous.candidate`, including that parse's stamps), that
parse has no raw record error (`Parsed.raw_error` is `None`), the cached
committed offset equals the published one, and the new file is still at least
that long. The entire previously committed byte prefix is then verified against
the freshly read prefix. Verification is the content fingerprint
(`crate::fingerprint`, a 128-bit non-cryptographic mix). The fingerprint
module describes that mix as ~6 GB/s on one core; that figure is the module's
own statement, not a measurement taken for this page. The fingerprint replaced
SHA-256 on 2026-09-15 because hashing cost more than reading the file on
gigabyte sessions. It covers every byte
through that complete-line boundary. It replaced retained raw-byte comparison
and never substitutes a sampled header hash. The fingerprint detects our own
files changing under us — an actor who can rewrite the native root already
owns the data, so a MAC would add nothing. Source/root/data/summary paths and
stable file identity (`file_identity`) must also agree. The 4 KiB public head,
file size and mtime alone are never evidence of append-only contents.

Stable file identity is distinct from the full change stamp. On Unix
`file_identity` is `dev:ino`, while the ordinary stamp `identity` also includes
`ctime` and `ctime_nsec`. Using ctime as immutable identity would miss every
real append, so reuse compares `file_identity`. On Windows this session stamp
sets both `identity` and `file_identity` to `dev:ino` from the `cap_std`
metadata; it does not use a creation-time fallback. The full byte prefix is
fingerprinted on either platform. This is not a new claim of Windows/macOS
runtime validation.

Only the old committed prefix is reused. A partial final JSONL row is reparsed
from its start when completed; it is never treated as an already valid row.
Blank rows still advance byte offsets and are not records. A complete line that
is not a JSON object, or that is not valid JSON, is skipped and counted
(`invalid`). It is not stored as an AST record. The committed prefix that
contains those bytes can still be reused. A small line is offered to
`serde_json` first. What that parser rejects — nesting past its own recursion
limit, or a real syntax error — is decided by the structural scanner, which
has no depth quota. Syntax and UTF-8 failures on a complete line are skipped
lines, not a cache-budget exit.

The former 2 MiB JSONL line limit, the cumulative 50,000-record limit, and the
scanner quotas that rejected a record for nesting, node count, key count, key
length, number length or resident-node weight are not applied. A well-formed
record is decoded. `scanner::Limits` only carries `inline_string_bytes`. Native
records use `budgets::INLINE_STRING_BYTES` (64 KiB; this span threshold was
2 MiB). A longer string becomes a source span. The nested tool-envelope scan
still passes 2 MiB as its own `inline_string_bytes`
(`records/tool_envelopes.rs`); that is the span threshold for one envelope
candidate, not a JSONL line rejection and not a record-count cap. Resident
accounting grows with the bytes actually parsed and is not a quota. A scanner
error on one complete line, including an allocation error while building that
record, is counted as an invalid line and skipped. The rest of the file
continues. The checked input itself still fails the read on a short read, a
changed stamp, or an allocation failure while building the raw-line index.
Reading a span back from its stamped range uses the same checked reader. If
that read fails those checks, `prepare` returns the `SessionError` and the
scan fails. Any other materialization error is a hard record error.
See [read-model.md](read-model.md#历史容量与读取边界) and
[native-input.md](native-input.md#structural-json).

A hard record error is not admitted to the cache. The AST retention budget
does not stop the scan, does not discard a valid suffix, and does not turn
the parse into an HTTP error. Rewrite, truncate, replacement, eviction or any
fingerprint mismatch drops the speculative suffix and the old AST, then reads
the same stamped input once from the start. Failure of either checked read
returns an error. The speculative AST is not put back, and the failed read
does not replace the published view. `Parsed` does not retain a native byte
vector.

The cache owns its `Vec` of JSON records exclusively. The `Vec` moves out of
the cache for the parse and returns only when `retain` admits the batch: no
deep cloning of the prefix and no additional AST ownership in old
`ViewSnapshot` Arcs. The next reuse still requires the retained candidate and
committed offset to equal the published `Parsed`. An entry that does not
match is dropped.

## Semantics deliberately recalculated

Provider metadata, native identity provenance, Claude lineage and the complete
provider projection still run against all validated records. Claude last-prompt
may remove an old visible branch; Codex turn_aborted may change an old assistant
message. Summary changes can alter metadata without adding a JSONL row. Appending
old Event vectors would be incorrect for these cases.

Inherited fixed prefixes, semantic digests, byte/head/anchor cursor validation,
window selection, search and file/media selected-view authority remain unchanged.
Full native reads, timeline projection and inherited-prefix parsing can still
dominate large-history latency. Further optimization must measure these costs
and preserve their separate invariants. This page does not add a current timing.

## Serialized bytes on append (2026-09-15)

The projection is still recomputed from every validated record, but its
serialized form is not: `Parsed.encoded` (docs/read-model.md "视图字节缓存")
keeps the exact `serde_json::to_vec` bytes of every projected message, and the
encoder of the new parse walks the old and new event lists side by side. Each
index is compared on its own. Status events are stored separately and are
serialized again; they are not copied from the message-byte run. A non-status
message is copied from the previous bytes only when that index has the same
physical `end`, empty typed media, a previous entry that is not `special`,
and a structurally identical tree (`same_value`: key-order and float-sign
sensitive, so equality implies identical bytes). `special` means typed media
or a text-discovered file reference. Anything else — the appended tail, a
Codex `turn_aborted` amendment, a Claude branch that disappeared or moved, a
special message — is serialized afresh. A later index that still matches can
still copy; a mismatch does not poison the rest of the list. A message that
no longer matches cannot keep a stale encoding. Special messages still store
their serialized bytes for the page budget and the
semantic digest. A hot read re-projects them per request (descriptor
registration, `media_more`, file tokens) instead of splicing those bytes. A
view whose bytes were not retained (a transient search projection) lends
nothing. The hot-read responses (full, `window=1`, increments, history pages)
then splice the retained non-special bytes instead of cloning and
re-serializing. The committed semantic digest and the LRU accounting come from
the same single pass.

## Resource tradeoff and measurement

The decoded-AST cache and the view LRU decide which parsed results stay
resident. They evict, or they decline to keep a batch. They do not reject a
valid history read. A file whose AST is not kept is still returned from the
parse that just finished.

The process installs one `budgets::Caches` value, from the first
`budgets::configure(Pools::caches())`. A later call with different numbers does
not replace it. Before `configure`, `caches()` is `Caches::default`, and that
default matches `Pools::default`. The numbers below are those defaults
(`crates/sessiondock/src/sessions/mod.rs` `budgets`, and
`crates/sessiondock/src/config.rs` `Pools`).

| Cache | Default entries | Default bytes | Environment |
| --- | ---: | ---: | --- |
| View LRU | 16 | 128 MiB | `SESSIONDOCK_CACHE_ENTRIES`, `SESSIONDOCK_VIEW_CACHE_MB` |
| Decoded AST | 8 | 64 MiB | entries are `cache_entries / 2` (`AST_CACHE_ENTRIES`); bytes are `SESSIONDOCK_AST_CACHE_MB` (`AST_CACHE_BYTES`) |

`read-model.md` records the same defaults and describes `0` as retaining
nothing ([常驻内存预算](read-model.md#常驻内存预算)). On the AST cache that is
what the code does: `ast_entries == 0` inserts nothing, and `ast_bytes == 0`
does not insert a batch whose logical weight is greater than zero, so an
append of a file that actually contains records decodes from scratch.

AST weight is `value_weight` plus a per-record charge and any native-image
sidecar weight. `value_weight` charges a base per `Value`, string and key
capacity, array capacity, and each object entry. A tiny-encoded array of many
scalars is not free. The figure is a logical estimate, not an allocator or RSS
ceiling. `retain` skips the batch when it has a hard record error, when its
weight exceeds the AST byte budget, or when the entry limit is zero. Otherwise
it drops least-recently-used entries until the new batch fits. That drop does
not change the history just parsed and is not an HTTP error. An enormous but
well-formed tree is decoded; if its weight exceeds the budget, it is simply
not cached.

The view LRU is a separate retention budget: serialized message bytes plus
resident embedded-image bytes (`Parsed::encoded_bytes`). It is not a 16 MiB
budget. The former fixed read rejections, including a 64 MiB record ceiling,
are not applied. The 32 MiB decoded-image limit is still a media-item
rejection (HTTP 413 for that image). It is not this cache. The materialized
image blob cache (128 MiB, 512 entries) only evicts; see
[media.md](media.md#size-and-cache-behavior).

### Historical measurement (2026-09-15)

The numbers in this section are the saved run described below. They are not a
current performance result, and the removed tools are not a current check.
The 32 MiB cache named with the second binary is that binary's budget, not
the 64 MiB default above.

The former `tests/append_benchmark.py` (removed on 2026-10-06 with the
non-browser scripts) created fresh synthetic Claude/Codex/Grok servers for
1k/5k/10k records, measures first window/idle/single append/same-length rewrite,
and verifies cursor behavior and reloaded text. Rewrites are beyond 4 KiB and
preserve mtime. Timings include loopback HTTP and Python JSON decoding, exclude
fixture generation and process startup, and are observations rather than test
thresholds. A fresh process does not imply a cold operating-system file cache.
Optional `--rss` reads Linux VmRSS/VmHWM for the exact spawned server PID outside
the HTTP timing windows. These process figures include all server allocations,
not only the record cache; they cannot prove a cross-platform or whole-workload
memory bound. Further timing and memory observations are in [native input](native-input.md).

It was run against saved pre-change and post-change release binaries:

```sh
python3 tests/append_benchmark.py --binary target/sessiondock-before13 --samples 3
python3 tests/append_benchmark.py --binary target/release/sessiondock --samples 3
```

### Release comparison

Adjacent serial runs, three samples per provider/size, passed all 27 cursor cases
for both binaries. The 10k-record p50 observations in milliseconds were:

| Provider | First window, old → new | Append, old → new | Rewrite, old → new |
| --- | ---: | ---: | ---: |
| Claude | 179.343 → 187.400 | 180.549 → 147.089 | 134.029 → 131.996 |
| Codex | 107.021 → 110.588 | 100.380 → 78.936 | 69.234 → 70.295 |
| Grok | 75.477 → 89.895 | 76.009 → 70.613 | 50.985 → 49.319 |

Append improved by approximately 18.5%, 21.4% and 7.1%, respectively; first reads
regressed by approximately 4.5%, 3.3% and 19.1%. Rewrite was within approximately
3%. At 1k records Grok append was slightly slower (8.159 → 8.361 ms). Idle reads
were approximately 0.7–1.5 ms. These small-sample smoke measurements do not establish
statistical significance or an overall speedup. Establishing and weighting the
cache has a cold-read cost; not every workload benefits.

Measured binary SHA-256: baseline
`49fd8b8b7ebbdb4cfb40f7edeafe672070dd8a5070c6a107518dea7e8cf1f522`,
new `2eef6f836142e6b37e96f91b157db9eff6f29ca37d684271f13dcd27e7f74f48`.
The latter is the 32 MiB cache implementation before the later over-budget
weight-calculation short-circuit and media text-parity fixes. Those changes were
validated separately; these numbers are not relabeled as final-binary measurements.

Keep baseline executables local/ignored; do not publish them or run a benchmark
against production directories. Decode/reuse counters that showed which records
avoided deserialization lived in the unit tests removed on 2026-10-06 with the
other non-browser tests. They are not a current check. Wall-clock timing alone
cannot prove a cache hit.
