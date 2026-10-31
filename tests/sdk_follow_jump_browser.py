#!/usr/bin/env python3
"""Following a live SDK turn must not bounce the page as new bubbles arrive.

BUG-20261031-123548-ff50a3: in a streaming Kiro SDK turn, the exposed tail tool
(with its output preview) folds into the group summary as soon as the next
bubble arrives. The content below the reading position becomes shorter, and
bottom-follow pulled the whole page down, then the next tool pushed it back
up. While following, the page now only moves up; the extra space is released
when the turn ends. A reader scrolled up stays exactly where they are.
"""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace

from kiro_sdk_fixtures import KiroFixture

SAMPLER = """() => {
  const box = document.querySelector('.sdk-msgs');
  const log = window.__follow = [];
  const tick = () => {
    const user = [...box.querySelectorAll('.sdk-live [data-role=user]')].at(-1);
    log.push({ref: Math.round(user?.getBoundingClientRect().top ?? 0),
      gap: box.scrollHeight - box.scrollTop - box.clientHeight,
      running: NativeSdkView.running, tail: !!box.querySelector('.tool-active-tail')});
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}"""


def seed(fixture: KiroFixture):
    lines = []
    for i in range(25):
        lines.append(json.dumps({"id": f"msg-u{i:04d}", "timestamp": "2026-10-18T00:01:00Z",
            "message": {"kind": "user", "data": {"content": [{"kind": "text", "data": f"user turn {i}"}]}}}))
        lines.append(json.dumps({"id": f"msg-a{i:04d}", "timestamp": "2026-10-18T00:01:05Z",
            "message": {"kind": "assistant", "data": {"content": [{"kind": "text",
                "data": f"assistant reply {i} " + "long text " * 80}]}}}))
    (fixture.v3_dir / "messages.jsonl").write_text("\n".join(lines) + "\n")


def stream_turn(page, prompt: str, scroll_up: bool, screenshot: Path | None):
    page.locator("#sdk-input").fill(prompt)
    page.keyboard.press("Enter")
    page.wait_for_function("document.querySelector('.sdk-live')?.textContent.includes('Step 0 finished')")
    if scroll_up:
        page.mouse.move(640, 300)
        page.mouse.wheel(0, -900)
        page.wait_for_function("!document.querySelector('.sdk-msgs')._messageScroll.stick")
        page.wait_for_timeout(300)
    page.evaluate(SAMPLER)
    page.wait_for_function("document.querySelector('.sdk-live')?.textContent.includes('Step 7 finished')", timeout=30000)
    if screenshot:
        page.screenshot(path=str(screenshot), full_page=True)
    page.wait_for_function("!NativeSdkView.running", timeout=30000)
    page.wait_for_timeout(600)
    log = page.evaluate("window.__follow")
    live = [s for s in log if s["running"]]
    assert any(s["tail"] for s in live), "fixture never exposed a tail tool"
    moves = [b["ref"] - a["ref"] for a, b in zip(live, live[1:])]
    if scroll_up:
        assert not any(moves), f"scrolled-up reader moved: {moves}"
    else:
        assert all(s["gap"] <= 2 for s in live), "lost bottom follow"
        assert all(move <= 0 for move in moves), f"following page jumped down: {[m for m in moves if m > 0]}"
    space = page.evaluate("document.querySelector('.sdk-msgs').style.getPropertyValue('--follow-space')")
    assert not space, f"follow space kept after the turn: {space}"


def run(args):
    from playwright.sync_api import sync_playwright
    from codex_sdk_browser import server
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize

    with tempfile.TemporaryDirectory(prefix="sessiondock-sdk-follow-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok", "state", "trash"):
            (root / name).mkdir()
        binary = args.binary.resolve(strict=True)
        fixture = KiroFixture(root, host_binary=binary.with_name("ptyhost"))
        seed(fixture)
        initialize("--initialize-lifecycle", root / "ledger", binary)
        extra = {**fixture.env, "SESSIONDOCK_NATIVE_SDKS": "auto",
                 "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"),
                 "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir),
                 "SESSIONDOCK_STATE_DIR": str(root / "state"),
                 "SESSIONDOCK_TRASH_DIR": str(root / "trash"),
                 "KIRO_SDK_FAKE_BUBBLES": "9", "KIRO_SDK_FAKE_TOOL_SECONDS": "0.5"}
        try:
            with server(SimpleNamespace(root=root), binary, "off", extra) as base, sync_playwright() as playwright:
                browser = playwright.chromium.launch(headless=True)
                page = browser.new_page(viewport={"width": 1280, "height": 800})
                page.set_default_timeout(20000)
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base, wait_until="domcontentloaded")
                page.wait_for_function("T.listLoaded")
                page.wait_for_function(
                    "async sid => { await loadSessions(true); return S.sessions.some(r => r.sid === sid); }",
                    arg=fixture.sid, polling=500, timeout=30000)
                uid = next(r["uid"] for r in page.evaluate("S.sessions") if r.get("sid") == fixture.sid)
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.wait_for_function("SessionContentView.kind === 'sdk'")
                with page.expect_response(lambda r: r.url.endswith("/api/native-sdk/open") and r.request.method == "POST"):
                    page.locator(".sdk-status-row .composer-run-action:visible").first.click()
                page.wait_for_function("document.querySelector('.sdk-msgs')?.textContent.includes('assistant reply 24')")
                page.wait_for_function("!document.querySelector('#sdk-input').disabled")
                shots = args.screenshots
                stream_turn(page, "stream-bubbles following", False, shots and shots / "sdk-follow-bottom.png")
                page.locator("#sdk-input").click()
                stream_turn(page, "stream-bubbles reading", True, shots and shots / "sdk-follow-scrolled-up.png")
                assert not errors, errors
                browser.close()
        finally:
            stop_private_sdkhosts(root)
    print("SDK follow without jumps browser: PASS")


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
