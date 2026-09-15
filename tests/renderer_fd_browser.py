#!/usr/bin/env python3
"""The real page must not leak renderer file descriptors through its audit posts.

An unread fetch response keeps a 2 MiB shared-memory data pipe — one fd — in
the renderer until garbage collection. The page posts one audit batch per
second under terminal output, the renderer's soft limit is 1024 fds, and once
it is reached the GPU command buffer cannot allocate shared memory: the tab
freezes in native code for good (2026-09-15, four times in a morning). Here the
page is driven to flush ~75 audit batches in 30 s while the renderer's fd count
is read from /proc (Chromium launched without its sandbox so /proc is readable;
Linux only). Growth must stay well under one fd per batch.
"""
import argparse
import os
import sys
import tempfile
import time
from pathlib import Path

from playwright.sync_api import sync_playwright
from history_parity import BINARY, build_corpus, isolated_server

SECONDS = 30
MAX_GROWTH_PER_BATCH = 0.1


def fds(pid):
    return len(os.listdir(f"/proc/{pid}/fd"))


def renderer_pids(browser):
    cdp = browser.new_browser_cdp_session()
    try:
        return [p["id"] for p in cdp.send("SystemInfo.getProcessInfo")["processInfo"] if p["type"] == "renderer"]
    finally:
        cdp.detach()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    if not sys.platform.startswith("linux"):
        print("SKIP renderer fd browser: needs /proc")
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-renderer-fd-") as temporary:
        corpus = build_corpus(Path(temporary))
        audit = corpus.root / "audit"
        audit.mkdir(mode=0o700)
        with sync_playwright() as playwright, isolated_server(corpus, args.binary, audit_dir=audit) as (base, _opener):
            launch = {"headless": True, "args": ["--no-sandbox"]}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 900})
                page = context.new_page()
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base, wait_until="networkidle")
                page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]').click()
                page.wait_for_timeout(1000)
                pid = max(renderer_pids(browser), key=fds)
                start = fds(pid)
                batches = 0
                deadline = time.monotonic() + SECONDS
                while time.monotonic() < deadline:
                    page.evaluate("for (let i = 0; i < 20; i++) browserAuditEvent('probe.tick', {i}); flushBrowserAudit();")
                    batches += 1
                    page.wait_for_timeout(400)
                page.wait_for_timeout(1500)
                end = fds(pid)
                assert not errors, errors
                assert batches >= 40, batches
                growth = (end - start) / batches
                assert growth <= MAX_GROWTH_PER_BATCH, f"renderer {pid} fds {start} -> {end} over {batches} audit batches ({growth:.2f}/batch)"
                print(f"PASS renderer fd browser: renderer fds {start} -> {end} over {batches} audit batches in {SECONDS}s ({growth:+.2f}/batch)")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
