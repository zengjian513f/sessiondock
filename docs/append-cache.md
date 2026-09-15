# Reusing native JSON records on append

The session store now has a disposable JSON-record cache. It avoids reparsing
unchanged JSON strings; it is **not an incremental timeline parser** and does not
turn history reads into constant-time operations.

## Reuse boundary

Every changed candidate still goes through the existing full bounded file read,
trusted-path/open-handle checks and before/after restamping. A cache entry can be
reused only when it corresponds to the exact previous published Parsed candidate,
has no incomplete/error checkpoint, and the entire previously committed byte
prefix is verified against the freshly read prefix. Verification is the content
fingerprint (`crate::fingerprint`, a 128-bit non-cryptographic mix that runs at
several GB/s; it replaced SHA-256 on 2026-09-15 because hashing cost more than
reading the file on gigabyte sessions) over every byte through that complete-line
boundary, replacing retained raw-byte comparison; it never substitutes a sampled
header hash. The fingerprint detects our own files changing under us — an actor
who can rewrite the native root already owns the data, so a MAC would add
nothing. Source/root/data/summary paths and stable file identity must also agree.
The 4 KiB public head, file size and mtime alone are never evidence of
append-only contents.

Stable file identity is distinct from the full change stamp: on Unix dev/inode
identify an incarnation, while ctime remains part of the ordinary change stamp.
Using ctime as immutable identity would miss every real append. On other platforms
the existing creation-time fallback remains; the full byte prefix is independently hashed.
This is not a new claim of Windows/macOS runtime validation.

Only the old committed prefix is reused. A partial final JSONL row is reparsed
from its start when completed; it is never treated as an already valid row. All
new rows obey the existing 2 MiB line and cumulative 50,000-record limits. Blank
rows still advance byte offsets. Malformed complete rows and early budget exits
are not retained as reusable checkpoints. Rewrite, truncate, replacement,
eviction or any mismatch safely falls back to a complete decode.
The streaming path verifies while tentatively decoding only the suffix. A prefix
mismatch discards that work and the old AST, then reopens the same stamped input
for one cold pass. Failure of either checked read returns an error, not stale
cache contents. Parsed no longer retains a whole native byte vector.

The cache owns its Vec of JSON records exclusively. The Vec moves out of the
cache while parsing and back afterwards: no deep cloning of the prefix and no
additional AST ownership in old ViewSnapshot Arcs. If later dependency validation
fails, a retained uncommitted cache candidate will not match the still-published
previous Parsed object on the next attempt and is discarded.

## Semantics deliberately recalculated

Provider metadata, native identity provenance, Claude lineage and the complete
provider projection still run against all validated records. Claude last-prompt
may remove an old visible branch; Codex turn_aborted may change an old assistant
message. Summary changes can alter metadata without adding a JSONL row. Appending
old Event vectors would be incorrect for these cases.

Inherited fixed prefixes, semantic digests, byte/head/anchor cursor validation,
window selection, search and file/media selected-view authority remain unchanged.
Full native reads, timeline projection, event clones, hashing, inherited-prefix
parsing and HTTP serialization can still dominate large-history latency. Further
optimization must measure these costs and preserve their separate invariants.

## Resource tradeoff and measurement

At most 16 AST entries are retained under a **32 MiB logical-weight budget**.
Each nested Value, object entry, key/string capacity and array capacity contributes
to that weight; a tiny-encoded array of many scalars is not free. This is an
additional disposable cache, not part of the existing 64 MiB raw-input, 16 MiB
view or 32 MiB media budgets. It is not an allocator/RSS upper bound. Oversized
multi-record batches simply are not cached; eviction must not change history
output or become an HTTP error. An individual record must also
pass the scanner's separate structural/resident limits; physically small but
enormous node trees are explicitly rejected before cache admission.

`tests/append_benchmark.py` creates fresh synthetic Claude/Codex/Grok servers for
1k/5k/10k records, measures first window/idle/single append/same-length rewrite,
and verifies cursor behavior and reloaded text. Rewrites are beyond 4 KiB and
preserve mtime. Timings include loopback HTTP and Python JSON decoding, exclude
fixture generation and process startup, and are observations rather than test
thresholds. A fresh process does not imply a cold operating-system file cache.
Optional `--rss` reads Linux VmRSS/VmHWM for the exact spawned server PID outside
the HTTP timing windows. These process figures include all server allocations,
not only the record cache; they cannot prove a cross-platform or whole-workload
memory bound. Further timing and memory observations are in [native input](native-input.md).

Run the same script against saved pre-change and post-change release binaries:

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
against production directories. Unit-test decode/reuse counters establish which
records avoided deserialization; wall-clock timing alone cannot prove a cache hit.
