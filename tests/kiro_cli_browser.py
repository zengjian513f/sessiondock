#!/usr/bin/env python3
"""Kiro CLI PTY terminal mode (`#a-console`) & CLI->SDK mutual exclusion browser suite.

Exercises:
- Resuming a Kiro V3 session in CLI mode (`kiro-cli chat --agent-engine=v3 --resume-id <sid>`)
- Terminal view (`#a-console`) rendering xterm.js output while `#a-chat` stays hidden
- CLI -> SDK mutual exclusion: while CLI PTY is active, SDK open/turn returns `kiro_sdk_session_busy`
- Stopping CLI PTY releases the lock and allows SDK mode (`#a-sdk`) to open and run a turn
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import server
from history_fixtures import BINARY
from kiro_sdk_fixtures import KiroFixture
from private_hosts import private_hosts
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize


def exercise(browser, base: str, fixture: KiroFixture, screenshots: Path | None):
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    context.route(
        "**/*",
        lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort(),
    )
    page = context.new_page()
    page.set_default_timeout(15000)
    failures = []
    page.on("pageerror", lambda error: failures.append(str(error)))

    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(row => row.sid === sid); }", arg=fixture.sid, polling=500, timeout=30000)
    rows = page.evaluate("S.sessions")
    v3_row = next(r for r in rows if r.get("sid") == fixture.sid)
    uid = v3_row["uid"]

    # 1. Select Kiro V3 session and verify view bar shows only #a-sdk and #a-term (#a-chat hidden)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator("#a-chat")).to_be_hidden()
    expect(page.locator("#a-sdk")).to_be_visible()
    expect(page.locator("#a-term")).to_be_visible()

    # 2. Click #a-term to takeover/resume Kiro CLI PTY session and open xterm.js terminal
    with page.expect_response(lambda r: "/api/term/takeover" in r.url) as response:
        page.locator("#a-term").click()
    receipt = response.value.json()
    assert receipt.get("ok") and not receipt.get("needs_confirm"), receipt
    expect(page.locator("#a-chat")).to_be_hidden()
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    page.wait_for_function(
        "Array.from({length:T.term.buffer.active.length}, (_,i)=>"
        "T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('Kiro CLI v3 Interactive Terminal')"
    )

    # Observe the configured native process through the fake peer's protocol log,
    # never through Kiro's private lock file layout.
    deadline = time.monotonic() + 5.0
    starts = []
    while time.monotonic() < deadline:
        starts = [r["fixture_cli_start"] for r in fixture.received() if "fixture_cli_start" in r]
        if starts:
            break
        time.sleep(0.05)
    assert starts and starts[-1]["sessionId"] == fixture.sid, starts
    assert starts[-1]["argv"][starts[-1]["argv"].index("--resume-id") + 1] == fixture.sid
    # Send through xterm and retain the same native identity for SDK replay.
    page.locator("#termpane .xterm-helper-textarea").focus()
    page.keyboard.type("CLI_TO_SDK_CONTINUITY")
    page.keyboard.press("Enter")
    page.wait_for_function("Array.from({length:T.term.buffer.active.length}, (_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('Kiro CLI echoed: CLI_TO_SDK_CONTINUITY')")

    if screenshots:
        page.screenshot(path=str(screenshots / "kiro-cli-terminal.png"), full_page=True)

    # 3. Verify CLI -> SDK mutual exclusion: SDK open is refused while CLI is running
    sdk_open_while_cli = page.evaluate(
        """async uid => {
          const r = await fetch('/api/native-sdk/open', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({uid}),
          });
          return {status: r.status, body: await r.json()};
        }""",
        uid,
    )
    assert sdk_open_while_cli["status"] >= 400 or sdk_open_while_cli["body"].get("ok") is False, sdk_open_while_cli
    assert "kiro_sdk_session_busy" in json.dumps(sdk_open_while_cli["body"]), sdk_open_while_cli

    # 4. Stop CLI PTY session
    stop_res = page.evaluate(
        """async ({uid}) => {
          const r = await fetch('/api/session/stop', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({uid}),
          });
          return {status: r.status, body: await r.json()};
        }""",
        {"uid": uid},
    )
    assert stop_res["status"] < 300 and stop_res["body"].get("ok") is True, stop_res

    # 5. Now switch to SDK mode (#a-sdk) and verify SDK open succeeds!
    sdk_open_after_stop = page.evaluate(
        """async uid => {
          const r = await fetch('/api/native-sdk/open', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({uid}),
          });
          return {status: r.status, body: await r.json()};
        }""",
        uid,
    )
    assert sdk_open_after_stop["status"] == 200 and sdk_open_after_stop["body"].get("session_id") == fixture.sid, sdk_open_after_stop

    assert any("CLI_TO_SDK_CONTINUITY" in (item.get("content") or "") for item in sdk_open_after_stop["body"]["raw"]["items"]), sdk_open_after_stop
    # Create through the CLI preference: backend assigns a native SID before PTY resume.
    page.evaluate("async uid => fetch('/api/native-sdk/stop', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({uid})})", uid)
    if not page.locator("#settings").is_visible():
        page.locator("#header-more-btn").click()
    page.locator("#settings").click()
    page.locator('.settings-tab[data-tab="features"]').click()
    page.locator('[data-agent-start-mode="kiro"]').select_option("cli")
    page.locator("#settings-dialog .modal-close").click()
    new_before = len(fixture.received("session/new"))
    page.locator("#new-session").click()
    page.locator('label:has(input[name="new-source"][value="kiro"])').click()
    page.locator("#new-cwd").fill(str(fixture.work))
    with page.expect_response(lambda r: r.url.endswith("/api/term/create")) as created:
        page.locator("#new-session-go").click()
    assert created.value.status == 200, created.value.text()
    create_request = created.value.request.post_data_json
    create_receipt = created.value.json()
    assert create_receipt.get("record_id") and create_receipt.get("name"), create_receipt
    assert create_request.get("request_id"), create_request
    assert len(fixture.received("session/new")) == new_before + 1
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    deadline = time.monotonic() + 5
    new_start = None
    while time.monotonic() < deadline:
        candidates = [r["fixture_cli_start"] for r in fixture.received() if "fixture_cli_start" in r and r["fixture_cli_start"]["sessionId"] != fixture.sid]
        if candidates:
            new_start = candidates[-1]
            break
        time.sleep(0.05)
    assert new_start, fixture.received()
    new_sid = new_start["sessionId"]
    assert new_start["argv"][new_start["argv"].index("--resume-id") + 1] == new_sid
    page.wait_for_function("sid => Array.from({length:T.term.buffer.active.length}, (_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes(sid)", arg=new_sid)
    expect(page.locator("#a-chat")).to_be_hidden()
    page.wait_for_function("async sid => {await loadSessions(true); return S.sessions.some(row => row.sid === sid)}", arg=new_sid, polling=500, timeout=30000)
    new_uid = page.evaluate("sid => S.sessions.find(row => row.sid === sid).uid", new_sid)
    if screenshots:
        page.screenshot(path=str(screenshots / "kiro-cli-new-native-sid.png"), full_page=True)
    stopped = page.evaluate("async uid => (await fetch('/api/session/stop', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({uid})})).json()", new_uid)
    assert stopped.get("ok"), stopped
    opened = page.evaluate("async uid => (await fetch('/api/native-sdk/open', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({uid})})).json()", new_uid)
    assert opened.get("session_id") == new_sid, opened
    loads_before_retry = len(fixture.received("session/load"))
    retried = page.evaluate("async body => (await fetch('/api/term/create', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)})).json()", create_request)
    assert retried.get("record_id") == create_receipt.get("record_id") and retried.get("name") == create_receipt.get("name"), (retried, create_receipt)
    reopened = page.evaluate("async uid => (await fetch('/api/native-sdk/open', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({uid})})).json()", new_uid)
    assert reopened.get("session_id") == new_sid, reopened
    assert len(fixture.received("session/new")) == new_before + 1
    assert len(fixture.received("session/load")) == loads_before_retry, "Retry stopped or replaced an independently reopened SDK host"
    assert not failures, failures
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-kiro-cli-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok"):
            (root / name).mkdir()
        ptyhost_bin = args.binary.resolve(strict=True).with_name("ptyhost")
        fixture = KiroFixture(root, host_binary=ptyhost_bin)
        initialize("--initialize-lifecycle", root / "ledger", args.binary.resolve(strict=True))
        (root / "state").mkdir()
        extra = {
            **fixture.env,
            "SESSIONDOCK_NATIVE_SDKS": "auto",
            "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"),
            "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir),
            "SESSIONDOCK_STATE_DIR": str(root / "state"),
        }
        try:
            with private_hosts(fixture.host_dir), server(SimpleNamespace(root=root), args.binary, "off", extra) as base:
                with sync_playwright() as playwright:
                    browser = playwright.chromium.launch(headless=True)
                    exercise(browser, base, fixture, args.screenshots)
                    browser.close()
        finally:
            stop_private_sdkhosts(root)
    print("Kiro CLI PTY + CLI->SDK mutual exclusion browser: PASS")


if __name__ == "__main__":
    main()
