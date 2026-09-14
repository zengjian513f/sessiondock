#!/usr/bin/env python3
"""Free synthetic read smoke benchmark; no original data or CLI is accessed.

Times include loopback transport and Python JSON decoding. This is not a
production benchmark, nor an RSS/CPU measurement. Run a release
binary on an otherwise idle machine for comparable samples; no timing threshold
is treated as a correctness assertion.
"""

import argparse
import json
import math
from pathlib import Path
import tempfile
import time

from history_parity import BINARY, Corpus, codex_message, codex_row, cursor_query, encoded, isolated_server


def fetch(opener, base, route):
    with opener.open(base + route, timeout=30) as response:
        raw = response.read(16 * 1024 * 1024 + 1)
        assert len(raw) <= 16 * 1024 * 1024
        return json.loads(raw)


def measured(work, count=1):
    samples = []
    result = None
    for _ in range(count):
        begin = time.perf_counter()
        result = work()
        samples.append((time.perf_counter() - begin) * 1000)
    samples.sort()
    return result, {"samples": count, **{
        key: round(samples[max(0, math.ceil(count * fraction) - 1)], 2)
        for key, fraction in (("p50_ms", .5), ("p95_ms", .95), ("max_ms", 1))}}


def run(mebibytes, binary, samples):
    with tempfile.TemporaryDirectory(prefix="sessiondock-read-bench-") as directory:
        corpus = Corpus(Path(directory))
        for source in ("claude", "codex", "grok"):
            (corpus.root / source).mkdir()
        count = 2000
        sid = "synthetic-benchmark"
        rows = [codex_row("session_meta", {"id": sid, "cwd": "/synthetic/benchmark"})]
        overhead = len(encoded(codex_message("assistant", "", count))) + 10
        padding = "x" * max(1, mebibytes * 1024 * 1024 // count - overhead)
        rows += [codex_message("user" if index % 2 == 0 else "assistant",
                              f"row-{index:04d} {padding}", index + 1) for index in range(count)]
        path = corpus.put(sid, "codex", rows, [])
        native_size = path.stat().st_size
        with isolated_server(corpus, binary) as (base, opener):
            get = lambda route: fetch(opener, base, route)
            listed, cold = measured(lambda: get("/api/sessions?force=1"))
            assert len(listed["sessions"]) == 1
            _, warm = measured(lambda: get("/api/sessions"), samples)
            route = "/api/messages/" + corpus.uid(sid)
            window, first = measured(lambda: get(route + "?window=1"))
            assert window["message_total"] == count and len(window["messages"]) == 600
            assert window["partial"]["omitted"] == count - 600
            assert window["messages"][99]["text"].startswith("row-0099")
            assert window["messages"][100]["text"].startswith("row-1500")
            _, hot_window = measured(lambda: get(route + "?window=1"), samples)
            delta_route = route + "?" + cursor_query(window)
            idle, idle_cost = measured(lambda: get(delta_route), samples)
            assert not idle["reset"] and idle["messages"] == []
            with path.open("ab") as stream:
                stream.write(encoded(codex_message("assistant", "synthetic new tail", count + 1)))
            delta, append_cost = measured(lambda: get(delta_route))
            assert not delta["reset"] and [m["text"] for m in delta["messages"]] == ["synthetic new tail"]
            print(json.dumps({"native_bytes": native_size, "events": count,
                "cold_list": cold, "warm_list": warm, "first_window": first,
                "hot_window": hot_window, "idle_delta": idle_cost,
                "append_reparse_delta": append_cost}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--samples", type=int, default=10)
    args = parser.parse_args()
    if not 1 <= args.samples <= 100:
        parser.error("samples must be 1..100")
    for size in (1, 10):
        run(size, args.binary, args.samples)
