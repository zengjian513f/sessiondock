#!/usr/bin/env python3
# run_validation: skip
"""Which Rust threads burn CPU while a page idles on an active session; also count requests per second."""
import os
import subprocess, time, collections
from playwright.sync_api import sync_playwright
CHROMIUM = os.path.expanduser("~/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome")
pid = subprocess.check_output(["pgrep", "-x", "sessiondock"]).split()[0].decode()
def threads():
    out = subprocess.check_output(["ps", "-L", "-o", "tid=,comm=,cputimes=", "-p", pid]).decode().splitlines()
    return {l.split()[0]: (l.split()[1], int(l.split()[2])) for l in out}
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=CHROMIUM, headless=True)
    ctx = b.new_context(viewport={"width": 1440, "height": 900}, service_workers="block")
    page = ctx.new_page()
    reqs = collections.Counter()
    page.on("request", lambda r: reqs[r.url.split("?")[0].replace("http://127.0.0.1:8741", "")] .__class__ and reqs.update([r.url.split("?")[0].replace("http://127.0.0.1:8741", "")]))
    page.goto("http://127.0.0.1:8741/", wait_until="domcontentloaded")
    page.wait_for_function("document.querySelectorAll('#side .item').length > 100", timeout=60000)
    page.evaluate("openSession('claude:179009468904dece')"); page.wait_for_timeout(3000)
    reqs.clear(); t0 = threads(); page.wait_for_timeout(40000); t1 = threads()
    delta = sorted(((t1[t][1] - t0[t][1], t1[t][0]) for t in t1 if t in t0), reverse=True)
    by_name = collections.Counter()
    for secs, name in delta: by_name[name] += secs
    print("CPU-s per thread name over 40 s:", by_name.most_common(8))
    print("requests over 40 s:", reqs.most_common(12))
    ctx.close(); b.close()
