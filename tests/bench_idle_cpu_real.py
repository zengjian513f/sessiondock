#!/usr/bin/env python3
# run_validation: skip
"""CPU seconds each backend burns while one headless page sits idle on an active session for N seconds."""
import os
import subprocess, sys, time
from playwright.sync_api import sync_playwright
CHROMIUM = os.path.expanduser("~/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome")
N = int(sys.argv[1]) if len(sys.argv) > 1 else 60
def cpu(pid):
    with open(f"/proc/{pid}/stat") as f:
        parts = f.read().split(")")[-1].split()
    return (int(parts[11]) + int(parts[12])) / 100.0  # utime+stime in seconds (CLK_TCK=100)
rs_pid = subprocess.check_output(["pgrep", "-x", "sessiondock"]).split()[0].decode()
py_pid = subprocess.check_output(["pgrep", "-f", "sessiondock.server --host"]).split()[0].decode()
import json, urllib.request
op = urllib.request.build_opener(urllib.request.ProxyHandler({}))
live = json.load(op.open("http://127.0.0.1:8710/api/live"))["uids"]
uid = live[0] if live else None
with sync_playwright() as p:
    b = p.chromium.launch(executable_path=CHROMIUM, headless=True)
    for base, pid, name in [("http://127.0.0.1:8741", rs_pid, "rust"), ("http://127.0.0.1:8710", py_pid, "py")]:
        ctx = b.new_context(viewport={"width": 1440, "height": 900}, service_workers="block")
        page = ctx.new_page(); page.goto(base + "/", wait_until="domcontentloaded")
        page.wait_for_function("document.querySelectorAll('#side .item').length > 100", timeout=60000)
        if uid: page.evaluate("uid => openSession(uid)", uid); page.wait_for_timeout(3000)
        c0 = cpu(pid); t0 = time.time(); page.wait_for_timeout(N * 1000); c1 = cpu(pid)
        print(f"{name:<5} idle page on active session {uid}: {c1-c0:6.2f} CPU-s over {time.time()-t0:.0f} s  ({(c1-c0)/(time.time()-t0)*100:.1f}% of one core)")
        ctx.close()
    b.close()
