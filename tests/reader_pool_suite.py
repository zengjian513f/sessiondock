#!/usr/bin/env python3
"""Concurrent reads queue until a worker is available."""
from __future__ import annotations

import argparse
import http.client
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from urllib.parse import urlencode

from history_parity import BINARY, REPO, Corpus, claude_row, isolated_server, get_json

CHECKS = 0


def fail(area, why, body=""):
    raise SystemExit(f"FAIL {area}: {why}; {str(body)[:240]}")


def passed(area):
    global CHECKS
    CHECKS += 1
    print(f"PASS {area}", flush=True)


def fat_rows(sid):
    blob, parent, rows = "x" * 5120, None, []
    for i in range(100):
        u, a = f"u{i}", f"a{i}"
        rows.append(claude_row(sid, "user", u, parent, blob))
        rows.append(claude_row(sid, "assistant", a, u, blob))
        parent = a
    return rows


def build_corpus(root: Path) -> Corpus:
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    for i in range(6):
        sid = f"pool-{i:02d}"
        corpus.put(sid, "claude", fat_rows(sid), [])
    return corpus


def hit(base, route, timeout=90):
    host, port = base.replace("http://", "").split(":")
    conn = http.client.HTTPConnection(host, int(port), timeout=timeout)
    try:
        conn.request("GET", route)
        resp = conn.getresponse()
        return resp.status, resp.read()
    finally:
        conn.close()


def fanout(base, routes, timeout=90):
    out = [None] * len(routes)

    def work(i, route):
        out[i] = hit(base, route, timeout)

    threads = [threading.Thread(target=work, args=(i, r)) for i, r in enumerate(routes)]
    t0 = time.monotonic()
    for th in threads:
        th.start()
    for th in threads:
        th.join()
    return time.monotonic() - t0, out


def codes_ok(area, results, want=200):
    bad = [(st, raw[:120]) for st, raw in results if st != want]
    if bad:
        fail(area, f"{len(bad)} non-{want}", bad[0])
    passed(area)


def run_a(base, opener, uids):
    msg = [f"/api/messages/{uids[i % 6]}" for i in range(16)]
    lst = ["/api/sessions"] * 4
    wall, results = fanout(base, msg + lst)
    print(f"NOTE run A wall={wall:.3f}s", flush=True)
    codes_ok("queue 16 messages + 4 list", results)
    snap = get_json(opener, base, f"/api/messages/{uids[0]}")
    q = urlencode({"append": 1, "start": snap["end"]})
    wall2, app = fanout(base, [f"/api/messages/{uids[i % 6]}?{q}" for i in range(8)])
    print(f"NOTE run A append wall={wall2:.3f}s", flush=True)
    codes_ok("append=1 start=end all 200", app)


def run_b(base, uids):
    _, results = fanout(base, [f"/api/messages/{uids[i % 6]}" for i in range(24)], timeout=90)
    codes_ok("queued burst of 24 reads", results)


def check_env(binary: Path, corpus: Corpus, extra):
    env = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
    env.update({
        "SESSIONDOCK_BIND": "127.0.0.1:0",
        "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"),
        **extra,
    })
    for source in ("claude", "codex", "grok"):
        env["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(corpus.root / source)
    return subprocess.run([str(binary), "--check-config"], env=env, capture_output=True, timeout=15)


def run_c(binary: Path, corpus: Corpus):
    bad = check_env(binary, corpus, {"SESSIONDOCK_READ_WORKERS": "0"})
    if bad.returncode == 0:
        fail("check-config workers=0", "expected non-zero", bad.stdout.decode("utf-8", "replace")[:200])
    passed("check-config READ_WORKERS=0 fails")
    ok = check_env(binary, corpus, {"SESSIONDOCK_READ_WORKERS": "4"})
    text = (ok.stdout + ok.stderr).decode("utf-8", "replace")
    if ok.returncode != 0 or "read_workers=4" not in text:
        fail("check-config workers=4", f"rc={ok.returncode}", text[:240])
    passed("check-config read_workers=4")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-reader-pool-") as tmp:
        root = Path(tmp)
        corpus = build_corpus(root / "corpus")
        state = root / "corpus" / "state"
        state.mkdir()
        state.chmod(0o700)
        uids = [corpus.uid(f"pool-{i:02d}") for i in range(6)]
        with isolated_server(corpus, binary, state_dir=state, extra_env={
            "SESSIONDOCK_READ_WORKERS": "2",
        }) as (base, opener):
            run_a(base, opener, uids)
        with isolated_server(corpus, binary, state_dir=state, extra_env={
            "SESSIONDOCK_READ_WORKERS": "1",
        }) as (base, opener):
            run_b(base, uids)
        run_c(binary, corpus)
    print(f"reader_pool_suite: {CHECKS} checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
