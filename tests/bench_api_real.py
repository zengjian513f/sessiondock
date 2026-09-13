#!/usr/bin/env python3
# run_validation: skip
"""Read-only latency comparison of the two running backends (GET only)."""
import json, statistics, sys, time, urllib.parse, threading
from urllib.request import ProxyHandler, build_opener
from urllib.error import HTTPError

RS, PY = "http://127.0.0.1:8741", "http://127.0.0.1:8710"
op = build_opener(ProxyHandler({}))

def get(base, path, timeout=180):
    t = time.perf_counter()
    try:
        with op.open(base + path, timeout=timeout) as r:
            body = r.read()
            code = r.status
    except HTTPError as e:
        body, code = e.read(), e.code
    return time.perf_counter() - t, code, body

def timed(base, path, n=3):
    xs, codes, size = [], set(), 0
    for _ in range(n):
        dt, code, body = get(base, path)
        xs.append(dt); codes.add(code); size = len(body)
    return statistics.median(xs), min(xs), codes, size

def row(label, path, n=3, uid_note=""):
    r = timed(RS, path, n); p = timed(PY, path, n)
    ratio = p[0] / r[0] if r[0] else float("inf")
    print(f"{label:<44} rust {r[0]*1000:8.1f} ms (min {r[1]*1000:7.1f}) {str(sorted(r[2])):>10} {r[3]/1024:8.0f}K | py {p[0]*1000:8.1f} ms (min {p[1]*1000:7.1f}) {str(sorted(p[2])):>10} {p[3]/1024:8.0f}K | py/rust {ratio:5.1f}x {uid_note}")

# --- pick sessions by size from the Python list (both list the same public sessions)
_, _, body = get(PY, "/api/sessions")
rows = json.loads(body)["sessions"]
rows.sort(key=lambda r: r.get("size") or 0)
def pick(frac):
    return rows[min(len(rows)-1, int(len(rows)*frac))]
small, median, p90, largest = pick(0.05), pick(0.5), pick(0.9), rows[-1]
claude_big = max((r for r in rows if r["uid"].startswith("claude:")), key=lambda r: r.get("size") or 0)

print("== 静态页与列表（n=3，取中位数）")
row("GET /  (index.html)", "/")
row("GET /app.js", "/app.js")
row("GET /api/meta", "/api/meta")
row("GET /api/sessions (list)", "/api/sessions")
row("GET /api/live", "/api/live")
row("GET /api/term/list", "/api/term/list")
row("GET /api/trash", "/api/trash")

print("\n== 会话详情（首次=冷，随后=热；各 3 次，看 min 与中位数）")
for label, r in [("small p5", small), ("median", median), ("p90", p90), ("largest claude", claude_big), ("largest overall", largest)]:
    uid = urllib.parse.quote(r["uid"], safe="")
    row(f"GET /api/messages {label} {(r.get('size') or 0)/1024/1024:.1f}MB", f"/api/messages/{uid}", n=3, uid_note=r["uid"])

print("\n== 增量读取（append=1，模拟 SSE 后台补读）")
uid = urllib.parse.quote(median["uid"], safe="")
row("GET /api/messages median append=1", f"/api/messages/{uid}?append=1")

print("\n== 搜索（各 1 次）")
for q, opts in [("ddp_guard", ""), ("guard", "&word=1"), ("sessiondock.*rust", "&regex=1")]:
    row(f"GET /api/search q={q}{opts}", f"/api/search?q={urllib.parse.quote(q)}&limit=60{opts}", n=1)

print("\n== 并发：8 个并行 GET /api/sessions 的总墙钟与状态码")
def burst(base, path, k=8):
    res = [None]*k
    def one(i):
        res[i] = get(base, path)
    t = time.perf_counter()
    th = [threading.Thread(target=one, args=(i,)) for i in range(k)]
    [x.start() for x in th]; [x.join() for x in th]
    wall = time.perf_counter() - t
    codes = sorted(str(r[1]) for r in res)
    return wall, codes
for base, name in [(RS, "rust"), (PY, "py")]:
    w, c = burst(base, "/api/sessions")
    print(f"{name:<5} 8x /api/sessions wall {w*1000:8.1f} ms codes {c}")
uid = urllib.parse.quote(p90["uid"], safe="")
for base, name in [(RS, "rust"), (PY, "py")]:
    w, c = burst(base, f"/api/messages/{uid}")
    print(f"{name:<5} 8x /api/messages p90 wall {w*1000:8.1f} ms codes {c}")
