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

    # Verify fake CLI wrote its lock file
    lock_file = fixture.sessions / "cli" / f"{fixture.sid}.lock"
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline and not lock_file.exists():
        time.sleep(0.05)
    assert lock_file.exists(), "Fake Kiro CLI did not create lock file"
    cli_pid = int(lock_file.read_text().strip())
    proc_dir = fixture.proc / str(cli_pid)
    proc_dir.mkdir(exist_ok=True)
    (proc_dir / "stat").write_text(f"{cli_pid} (kiro-cli) S 1 " + "0 " * 30 + "\n")

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
    (proc_dir / "stat").unlink(missing_ok=True)
    proc_dir.rmdir()

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
