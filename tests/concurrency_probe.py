#!/usr/bin/env python3
"""Read-path load probe: sessions, messages window, and search.

--self-host starts an isolated loopback server around 10×300 synthetic Codex
records (history_parity helpers); --base hammers a running loopback server.
Threads cycle GET /api/sessions (plain and ?sig=), /api/messages/{uid}?window=1,
and /api/search?q=probe. 503 *_busy is listed, not a failure.
"""
# run_validation: skip
import argparse
import http.client
import json
import math
import random
import sys
import tempfile
import threading
import time
from collections import Counter
from pathlib import Path
from urllib.parse import quote, urlencode, urlsplit

from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server

CAP, WORD, N_SESS, N_REC = 16 * 1024 * 1024, "probe", 10, 300
ROUTES = ("sessions", "sessions_sig", "messages", "search")


def die(message, code=1):
    print(message, file=sys.stderr)
    raise SystemExit(code)


def loopback(base):
    parsed = urlsplit(base.rstrip("/"))
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1":
        die("refusing non-loopback --base; use http://127.0.0.1:PORT")
    return parsed.hostname, parsed.port or 80


def cell(xs, p, nd):
    if not xs:
        return "-"
    xs = sorted(xs)
    return f"{xs[max(0, math.ceil(len(xs) * p) - 1)]:.{nd}f}"


def synthetic(root: Path):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    for i in range(N_SESS):
        sid = f"probe-{i:02d}"
        rows = [codex_row("session_meta", {"id": sid, "session_id": sid, "cwd": "/synthetic/probe"})]
        rows += [codex_message("user" if j % 2 == 0 else "assistant",
                               f"{WORD} {sid} record {j} alpha", j + 1) for j in range(N_REC)]
        corpus.put(sid, "codex", rows, [])
    return corpus


def get(host, port, path, timeout):
    conn = http.client.HTTPConnection(host, port, timeout=timeout)
    started = time.perf_counter()
    try:
        conn.request("GET", path, headers={"Accept": "application/json"})
        resp = conn.getresponse()
        body = resp.read(CAP + 1)
        ms = (time.perf_counter() - started) * 1000
        try:
            payload = json.loads(body)
        except (json.JSONDecodeError, ValueError):
            payload = None
        payload = payload if isinstance(payload, dict) else None
        code = str(payload.get("code") or "") if payload else ""
        return resp.status, body, ms, code, payload
    except (TimeoutError, OSError, http.client.HTTPException) as err:
        return 0, b"", (time.perf_counter() - started) * 1000, type(err).__name__, None
    finally:
        conn.close()


def path_for(kind, uids, sig, rng):
    if kind == "sessions":
        return "/api/sessions"
    if kind == "sessions_sig":
        return "/api/sessions?" + urlencode({"sig": sig}) if sig else "/api/sessions"
    if kind == "messages":
        return f"/api/messages/{quote(rng.choice(uids), safe=':')}?window=1"
    return "/api/search?" + urlencode({"q": WORD})


def record(stats, kind, status, body, ms, code, payload):
    busy = status == 503 and code.endswith("_busy")
    row = stats["routes"][kind]
    row["n"] += 1
    row["ms"].append(ms)
    row["bytes"].append(len(body))
    if status == 200:
        row["ok"] += 1
        if payload and payload.get("sig"):
            stats["sig"] = str(payload["sig"])
    elif busy:
        row["busy"] += 1
        stats["admission"][code] += 1
    else:
        row["other"] += 1
        stats["other"][(status, code or "-")] += 1
        if status >= 500:
            stats["bad5"][(status, code or "-")] += 1


def worker(host, port, uids, stats, stop, timeout, tid, lock):
    rng, step = random.Random(tid + 1), tid
    while time.monotonic() < stop:
        kind = ROUTES[step % 4]
        step += 1
        with lock:
            sig = stats["sig"]
        status, body, ms, code, payload = get(host, port, path_for(kind, uids, sig, rng), timeout)
        with lock:
            record(stats, kind, status, body, ms, code, payload)


def dump(title, counter):
    parts = [f"{k}={n}" if isinstance(k, str) else f"{k[0]} {k[1]}={n}"
             for k, n in sorted(counter.items(), key=lambda kv: (-kv[1], str(kv[0])))]
    print(f"{title}: {' '.join(parts) or '-'}", flush=True)


def print_table(stats):
    cols = ("route", "n", "200", "busy", "other", "p50_ms", "p95_ms", "max_ms",
            "p50_B", "p95_B", "max_B")
    rows = [cols]
    for kind in ROUTES:
        r = stats["routes"][kind]
        rows.append((kind, str(r["n"]), str(r["ok"]), str(r["busy"]), str(r["other"]),
                     cell(r["ms"], .5, 1), cell(r["ms"], .95, 1), cell(r["ms"], 1, 1),
                     cell(r["bytes"], .5, 0), cell(r["bytes"], .95, 0), cell(r["bytes"], 1, 0)))
    widths = [max(len(row[i]) for row in rows) for i in range(len(cols))]
    for i, row in enumerate(rows):
        print("  ".join(c.rjust(widths[j]) if j else c.ljust(widths[j])
                        for j, c in enumerate(row)), flush=True)
        if i == 0:
            print("  ".join("-" * w for w in widths), flush=True)
    dump("admission 503 (not failures)", stats["admission"])
    dump("other non-200", stats["other"])
    dump("unexpected 5xx", stats["bad5"])


def probe(base, args):
    host, port = loopback(base)
    timeout = max(8.0, float(args.seconds))
    status, body, _, code, payload = get(host, port, "/api/sessions?force=1", timeout)
    if status != 200 or not payload:
        die(f"bootstrap GET /api/sessions HTTP {status} {code}: {body[:200]!r}")
    uids = [row["uid"] for row in payload.get("sessions") or []
            if isinstance(row, dict) and row.get("uid")]
    if not uids:
        die("bootstrap: /api/sessions returned no uid")
    stats = {"sig": str(payload.get("sig") or ""), "admission": Counter(),
             "other": Counter(), "bad5": Counter(),
             "routes": {k: {"n": 0, "ok": 0, "busy": 0, "other": 0, "ms": [], "bytes": []}
                        for k in ROUTES}}
    lock, stop = threading.Lock(), time.monotonic() + args.seconds
    threads = [threading.Thread(target=worker, daemon=True,
        args=(host, port, uids, stats, stop, timeout, i, lock)) for i in range(args.threads)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=timeout + 2)
    print_table(stats)
    if args.expect_no_5xx and stats["bad5"]:
        die("--expect-no-5xx: unexpected 5xx " + " ".join(
            f"{st} {cd}={n}" for (st, cd), n in sorted(stats["bad5"].items())))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-host", action="store_true")
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--base")
    parser.add_argument("--threads", type=int, default=16)
    parser.add_argument("--seconds", type=float, default=5)
    parser.add_argument("--expect-no-5xx", action="store_true")
    args = parser.parse_args()
    if args.threads < 1 or args.seconds <= 0:
        parser.error("--threads >= 1 and --seconds > 0")
    if args.self_host:
        with tempfile.TemporaryDirectory(prefix="sessiondock-concurrency-") as tmp:
            corpus = synthetic(Path(tmp))
            with isolated_server(corpus, args.binary) as (base, _opener):
                raise SystemExit(probe(base, args))
    if not args.base:
        parser.error("need --self-host or --base")
    raise SystemExit(probe(args.base, args))


if __name__ == "__main__":
    main()
