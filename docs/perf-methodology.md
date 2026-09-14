# Performance measurement methodology

How this repository times native-history and native-media HTTP work, and how
results must be reported. These scripts emit **observations**, not pass/fail
thresholds; cursor/rewrite/image assertions are the only gates. Historical
static analysis is not a production conclusion; current measurements live in
[performance.md](performance.md). The smaller read smoke described there
(`tests/read_benchmark.py`) is a separate list/window/delta check and is not
the native-history comparison.

## Fresh-process samples

Each source/size/sample (ordinary history) or size/sample (native spans and
envelopes) starts one fresh, explicitly configured loopback server. The
ordinary-history comparison is three providers × 1000 and 10000 records × five
samples = 30 processes per binary. Native-span and envelope defaults are three
samples per image size (3 and 32 MiB).

A fresh process is **not** a cold OS page cache. The scripts do not flush it,
and the parent’s fixture writes already populate it. Do not describe these
runs as cold-disk.

## What timings include

Ordinary history (`tests/append_benchmark.py`) times loopback HTTP plus Python
JSON decode of `/api/messages/…` (cold inventory, deserialization, projection,
view construction and HTTP). Native GET timings include complete streamed body
consumption and SHA-256 verification; they do not decode the image in Python.

**Excluded** from every reported millisecond: fixture generation/writes,
process startup, `/proc` sampling, and post-phase token/native checks.

## p50, p95, and first-read

Each phase reports p50 and p95 as `sorted(ms)[ceil(n * fraction) - 1]`. With
the usual five (or three) samples, p95 is the maximum — a small-sample caveat,
not a tail-latency SLA. Ordinary-history tables lead with first window,
append, and rewrite; idle-delta p50 is recorded but is not the headline.

Always report first-read (first-window) cost **alongside** append
improvements. Establishing reuse has a cold-read cost; [append
cache](append-cache.md) and [native input](native-input.md) both show append
and first-read moving in opposite directions.

## Linux RSS and HWM

`--rss` and the native benchmarks read `VmRSS` / `VmHWM` from
`/proc/<pid>/status` of the **exact live server child**, outside HTTP timing
windows. Figures include all server allocations, not only the AST cache, and
exclude the Python fixture process. They are not a cross-platform or
whole-workload bound. `tests/rss_watch.py --pid PID` is the manual companion
(procfs only; it never signals the target).

| Field | Meaning |
| --- | --- |
| RSS | Current resident set at that phase. |
| HWM | Cumulative process high-water mark since start. It is not a per-phase allocation count or peak delta, and it does not fall when later RSS falls. |

Reporting only RSS after a later phase can hide a temporary copy: an
intermediate 32 MiB run was 48.297 MiB RSS vs 74.168 MiB HWM after cold GET.
Logical AST-cache weights, scanner resident statistics, and fixed buffer sizes
are not allocator/RSS measurements.

## Saved binaries

Compare the **same** script against saved pre-change and post-change release
new binaries under `target/sessiondock-beforeNN`
and `target/release/sessiondock`. Keep them local/ignored; do not publish
them or point a benchmark at production directories. Record both SHA-256
values (`kind=benchmark` JSONL rows). Run old then new sequentially, never in
parallel, on an otherwise idle machine (`perf_compare.py` warns when load
average exceeds CPUs/2).

## Commands

From the repository root, after a release build of the saved binaries:

```sh
python3 tests/append_benchmark.py --binary target/sessiondock-beforeNN \
  --sizes 1000 10000 --samples 5 --rss
python3 tests/append_benchmark.py --binary target/release/sessiondock \
  --sizes 1000 10000 --samples 5 --rss
python3 tests/native_spans_benchmark.py --binary target/release/sessiondock
python3 tests/native_envelopes_benchmark.py --binary target/release/sessiondock
python3 tests/native_envelopes_benchmark.py --binary target/release/sessiondock \
  --sizes-mib 3 --depth 8
python3 tests/perf_compare.py --old target/sessiondock-beforeNN \
  --new target/release/sessiondock --sizes 1000 10000 --samples 5 --rss
python3 tests/bench_summary.py compare OLD.jsonl NEW.jsonl
python3 tests/bench_summary.py single FILE.jsonl
python3 tests/rss_watch.py --pid PID
```

`perf_compare.py` runs `append_benchmark.py` against `--old` then `--new`,
writes JSONL under `target/perf/<UTC>/`, and renders `bench_summary.py`
tables. `--dry-run` prints the commands. `--envelopes` adds the envelope
benchmark on the new binary. `compare` prints first-window/append/rewrite
p50 (p95) and first-window RSS / final HWM medians; `single` prints
native/envelope tables. These benchmarks are opt-in and skipped by
`run_validation.py`.

## Small samples

Five (or three) sequential observations are smoke measurements. They do not
establish statistical significance, a confidence interval, or an overall
speedup. Adjacent runs of the same binary already move by several percent
(see the ordinary-history notes). Do not relabel an intermediate
binary’s numbers as the final release.

## Anti-patterns

- Never substitute logical weights for RSS.
- Never claim speedups from parser micro-benchmarks. The ignored
  `scanner_cpu_benchmark` is parse/output-drop only, not HTTP or native I/O;
  `read_benchmark.py` is also not a parser-only timing.
- Never report a non-200 response as a successful timing sample. A 413 is
  expected only when one decoded image exceeds Python's 32 MiB item limit.

No paid CLI, native homes, production services, or real history.

## Historical 10000-record p50 (batches 16–19)

Copied from [native input](native-input.md). Each cell is that batch’s saved
before/after pair, milliseconds, p50 (p95).

| Batch | Provider | First window p50 (p95) | Append p50 (p95) | Rewrite p50 (p95) |
| --- | --- | --- | --- | --- |
| [16](native-input.md#validation) | Claude | 175.992 (194.543) → 224.096 (228.696) | 144.471 (151.349) → 157.983 (178.487) | 129.008 (134.905) → 156.363 (165.840) |
| [16](native-input.md#validation) | Codex | 113.160 (138.681) → 141.774 (156.493) | 79.673 (89.002) → 88.260 (94.313) | 70.528 (76.365) → 99.981 (111.706) |
| [16](native-input.md#validation) | Grok | 86.359 (106.515) → 120.901 (126.756) | 69.624 (76.329) → 75.927 (85.739) | 50.113 (50.721) → 64.839 (68.506) |
| [17](native-input.md#validation) | Claude | 216.341 (220.233) → 194.745 (200.790) | 153.709 (162.934) → 148.882 (152.232) | 157.425 (167.482) → 148.642 (153.982) |
| [17](native-input.md#validation) | Codex | 152.439 (159.904) → 130.126 (149.163) | 87.454 (90.496) → 82.182 (85.781) | 100.207 (110.676) → 88.294 (92.504) |
| [17](native-input.md#validation) | Grok | 107.609 (114.264) → 104.505 (112.493) | 73.649 (76.041) → 73.376 (82.632) | 64.965 (66.927) → 60.574 (62.298) |
| [18](native-input.md#validation) | Claude | 199.602 (215.157) → 207.012 (224.286) | 149.273 (153.258) → 155.568 (178.932) | 150.848 (155.435) → 155.487 (159.173) |
| [18](native-input.md#validation) | Codex | 130.112 (131.634) → 137.247 (139.251) | 80.718 (87.385) → 85.310 (95.490) | 89.183 (95.818) → 95.520 (104.792) |
| [18](native-input.md#validation) | Grok | 99.751 (133.408) → 101.098 (118.646) | 72.772 (85.600) → 71.799 (74.052) | 62.820 (64.451) → 67.350 (68.605) |
| [19](native-input.md#validation) | Claude | 223.372 (240.961) → 216.777 (224.863) | 154.097 (162.379) → 157.408 (159.292) | 166.412 (171.977) → 166.443 (169.771) |
| [19](native-input.md#validation) | Codex | 141.161 (171.451) → 146.613 (179.956) | 84.405 (94.425) → 89.746 (119.956) | 105.090 (106.008) → 99.234 (105.763) |
| [19](native-input.md#validation) | Grok | 105.259 (120.568) → 102.955 (108.902) | 77.521 (80.297) → 76.384 (77.953) | 73.476 (74.142) → 72.889 (73.364) |
