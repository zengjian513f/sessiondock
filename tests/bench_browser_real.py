#!/usr/bin/env python3
# run_validation: skip
"""End-to-end browser timings (headless Chromium) for both backends: page→list, open session→messages, search→results."""
import os
import json, statistics, sys, time, urllib.request
from playwright.sync_api import sync_playwright

CHROMIUM = os.path.expanduser("~/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome")
RS, PY = "http://127.0.0.1:8741", "http://127.0.0.1:8710"
op = urllib.request.build_opener(urllib.request.ProxyHandler({}))
rows = json.load(op.open(PY + "/api/sessions"))["sessions"]
rows.sort(key=lambda r: r.get("size") or 0)
pick = lambda f: rows[min(len(rows)-1, int(len(rows)*f))]
targets = [("median", pick(0.5)), ("p90", pick(0.9)),
           ("largest claude", max((r for r in rows if r["uid"].startswith("claude:")), key=lambda r: r.get("size") or 0))]

def run(base, label):
    out = {}
    with sync_playwright() as p:
        b = p.chromium.launch(executable_path=CHROMIUM, headless=True)
        for rep in range(2):
            ctx = b.new_context(viewport={"width": 1440, "height": 900}, service_workers="block")
            page = ctx.new_page()
            t = time.perf_counter()
            page.goto(base + "/", wait_until="domcontentloaded")
            page.wait_for_function("document.querySelectorAll('#side .item').length > 100", timeout=60000)
            out.setdefault("page->list", []).append(time.perf_counter() - t)
            for name, r in targets:
                t = time.perf_counter()
                page.evaluate("uid => openSession(uid)", r["uid"])
                page.wait_for_function("document.querySelectorAll('#msgs .msg').length > 0 && !document.querySelector('#msgs .loading')", timeout=120000)
                # settle: wait until the count is unchanged for 300 ms
                last = -1
                while True:
                    n = page.evaluate("document.querySelectorAll('#msgs .msg').length")
                    if n == last: break
                    last = n; page.wait_for_timeout(300)
                out.setdefault(f"open {name} {(r.get('size') or 0)/1024/1024:.1f}MB", []).append(time.perf_counter() - t)
            t = time.perf_counter()
            page.fill("#q", "ddp_guard"); page.press("#q", "Enter")
            page.wait_for_function("(document.querySelector('#stat')?.textContent || '').includes('命中')", timeout=180000)
            out.setdefault("search ddp_guard", []).append(time.perf_counter() - t)
            ctx.close()
        b.close()
    return out

rs, py = run(RS, "rust"), run(PY, "py")
for k in rs:
    r, p = rs[k], py[k]
    print(f"{k:<32} rust 1st {r[0]*1000:8.0f} ms 2nd {r[1]*1000:8.0f} ms | py 1st {p[0]*1000:8.0f} ms 2nd {p[1]*1000:8.0f} ms")
