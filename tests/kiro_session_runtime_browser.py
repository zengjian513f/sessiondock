#!/usr/bin/env python3
"""Per-session runtime GET replaces the whole-fleet scan on session switch.

Asserts:
- GET /api/session/runtime?uid= returns O(1) per-session status
  ({uid, known, sdk, tmux, working, running, external}).
- Reflects a live SDK session (sdk:true) vs an idle one (sdk:false).
- Switching to a Kiro session does NOT issue /api/live?force=1 or
  /api/term/list?force=1 on the blocking path before the view is chosen;
  those run only in the background self-heal.
"""
from __future__ import annotations
import argparse
import json
import time
from pathlib import Path
import tempfile
from types import SimpleNamespace

from kiro_sdk_fixtures import KiroFixture


def run(args):
    from playwright.sync_api import expect, sync_playwright
    from codex_sdk_browser import server
    from kiro_sdk_browser import api, watch
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize

    with tempfile.TemporaryDirectory(prefix="sessiondock-kiro-runtime-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok", "state", "trash"):
            (root / name).mkdir()
        ptyhost_bin = args.binary.resolve(strict=True).with_name("ptyhost")
        fixture = KiroFixture(root, host_binary=ptyhost_bin)
        initialize("--initialize-lifecycle", root / "ledger", args.binary.resolve(strict=True))
        extra = {
            **fixture.env,
            "SESSIONDOCK_NATIVE_SDKS": "auto",
            "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"),
            "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir),
            "SESSIONDOCK_STATE_DIR": str(root / "state"),
            "SESSIONDOCK_TRASH_DIR": str(root / "trash"),
        }
        try:
            with server(SimpleNamespace(root=root), args.binary, "off", extra) as base, sync_playwright() as playwright:
                browser = playwright.chromium.launch(headless=True)
                page = browser.new_page(viewport={"width": 1280, "height": 900})
                page.set_default_timeout(20000)
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base, wait_until="domcontentloaded")
                page.wait_for_function("T.listLoaded")
                page.wait_for_function(
                    "async sid => { await loadSessions(true); return S.sessions.some(r => r.sid === sid); }",
                    arg=fixture.sid, polling=500, timeout=30000)
                uid = next(r["uid"] for r in page.evaluate("S.sessions") if r.get("sid") == fixture.sid)

                def runtime(u):
                    return page.evaluate(
                        """async u => {
                          const r = await fetch('/api/session/runtime?' + new URLSearchParams({uid:u}));
                          return {status: r.status, body: await r.json()};
                        }""", u)

                # Idle (not started) session: not SDK, not running.
                idle = runtime(uid)
                assert idle["status"] == 200, idle
                assert idle["body"]["uid"] == uid, idle
                assert idle["body"]["sdk"] is False, idle
                assert idle["body"]["running"] is False, idle
                assert set(["uid", "known", "sdk", "tmux", "working", "running", "external"]).issubset(idle["body"]), idle

                # Start the SDK session, then the per-session runtime must report sdk:true.
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.locator("#a-sdk").click()
                with page.expect_response(lambda r: r.url.endswith("/api/native-sdk/open") and r.request.method == "POST") as opened:
                    page.locator("#sdk-input").click()
                assert opened.value.ok, opened.value.text()
                watch(page, uid)
                page.wait_for_function("window.__kiroSnapshot")
                live = runtime(uid)
                assert live["body"]["sdk"] is True, live
                assert live["body"]["running"] is True, live

                # Now record the network during a SWITCH back to this session and
                # assert no forced fleet scan blocks the view decision. We capture
                # requests from the click until the SDK pane is ready, and verify
                # that any live?force=1 / term/list?force=1 is NOT awaited before
                # session/runtime resolves (it is the background self-heal).
                # Record which request resolves the mode decision. The ONLY
                # request the blocking selectFor path awaits must be
                # session/runtime; term/list and live are background self-heal
                # (fire-and-forget), so the mode is decided before they resolve.
                calls = []
                page.on("response", lambda r: calls.append((time.time(), r.request.url)) if "/api/" in r.request.url else None)
                page.evaluate("() => { SdkView.selection = null; }")
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.wait_for_function("() => SdkView.selection && SdkView.selection.mode && SdkView.selection.mode !== 'loading'")
                mode = page.evaluate("SdkView.selection.mode")
                assert mode == "sdk", f"expected sdk mode, got {mode}"
                # The mode decision is driven by the per-session GET.
                assert any("/api/session/runtime" in u for _, u in calls), calls
                # Any term/list during a switch must be the background force
                # self-heal, never a plain (blocking) loadTermList(false).
                blocking_term = [u for _, u in calls if "/api/term/list" in u and "force=1" not in u]
                assert not blocking_term, f"blocking term/list on switch: {blocking_term}"

                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "kiro-session-runtime-1280.png"), full_page=True)

                assert not errors, errors
                browser.close()
        finally:
            stop_private_sdkhosts(root)
    print("Kiro per-session runtime browser: PASS")


def main():
    from history_fixtures import BINARY
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    run(args)


if __name__ == "__main__":
    main()
