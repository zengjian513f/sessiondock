#!/usr/bin/env python3
"""CodeBuddy CLI terminal mode browser suite, using a private fake CLI in a real PTY.

CodeBuddy has no PTY chat view (like Kiro): the view bar offers SDK and terminal
only. Covers resume of a seeded native session, the new-session picker (model
catalog from `--help`, effort, start mode CLI) and the exact argv.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import tempfile
import time

from codebuddy_sdk_fixtures import CodebuddyFixture
from history_fixtures import BINARY, REPO
from private_hosts import private_hosts


def terminal_text(page):
    return page.evaluate("Array.from({length:T.term.buffer.active.length}, (_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n')")


def wait_start(fixture, predicate, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        starts = [row for row in fixture.frames("cli_start") if predicate(row)]
        if starts:
            return starts[-1]
        time.sleep(.05)
    raise AssertionError("CLI start not observed: " + json.dumps(fixture.frames(), ensure_ascii=False))


def exercise(browser, base, fixture, screenshots):
    from playwright.sync_api import expect
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    page.set_default_timeout(15000)
    failures = []
    page.on("pageerror", lambda error: failures.append(str(error)))
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")

    # 1. No PTY chat view: SDK and terminal only.
    page.locator(f'#side .item[data-uid="{fixture.uid}"]').click()
    expect(page.locator("#a-chat")).to_be_hidden()
    expect(page.locator("#a-sdk")).to_be_visible()
    expect(page.locator("#a-term")).to_be_visible()

    # 2. Resume in the terminal with the native SID.
    with page.expect_response(lambda r: "/api/term/takeover" in r.url) as takeover:
        page.locator("#a-term").click()
    receipt = takeover.value.json()
    assert receipt.get("ok") and not receipt.get("needs_confirm"), receipt
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    page.wait_for_function("() => Array.from({length:T.term.buffer.active.length}, (_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('CodeBuddy fake terminal ready')")
    start = wait_start(fixture, lambda row: "--resume" in row["args"])
    assert start["args"][start["args"].index("--resume") + 1] == fixture.sid, start
    assert "--input-format=stream-json" not in start["args"], start
    page.locator("#termpane .xterm-helper-textarea").focus()
    page.keyboard.type("CLI_TERMINAL_INPUT")
    page.keyboard.press("Enter")
    page.wait_for_function("() => Array.from({length:T.term.buffer.active.length}, (_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('CodeBuddy CLI echoed: CLI_TERMINAL_INPUT')")
    if screenshots:
        page.screenshot(path=str(screenshots / "codebuddy-cli-terminal.png"), full_page=True)
    stopped = page.evaluate("async uid => (await fetch('/api/session/stop', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({uid})})).json()", fixture.uid)
    assert stopped.get("ok"), stopped

    # 3. New session through the CLI start mode, with the picker's model and effort.
    if not page.locator("#settings").is_visible():
        page.locator("#header-more-btn").click()
    page.locator("#settings").click()
    page.locator('.settings-tab[data-tab="features"]').click()
    page.locator('[data-agent-start-mode="codebuddy"]').select_option("cli")
    page.locator("#settings-dialog .modal-close").click()
    page.locator("#new-session").click()
    page.locator('#new-session-form label:has(input[name="new-source"][value="codebuddy"])').click()
    if screenshots:
        page.screenshot(path=str(screenshots / "codebuddy-new-session-wide.png"), full_page=True)
    expect(page.locator("#new-model-label")).not_to_have_text("读取模型…")
    page.locator("#new-model").click()
    expect(page.locator("#new-model-menu")).to_be_visible()
    names = page.locator("#new-model-options [role=option]").all_inner_texts()
    assert any("fake-buddy-2" in name for name in names) and any("fake-buddy" in name for name in names), names
    if screenshots:
        page.screenshot(path=str(screenshots / "codebuddy-new-session-models.png"), full_page=True)
    page.locator("#new-model-options [role=option]", has_text="fake-buddy-2").first.click()
    expect(page.locator("#new-model-menu")).to_be_hidden()
    page.locator("#new-effort").select_option("high")
    page.locator("#new-cwd").fill(str(fixture.cwd))
    with page.expect_response(lambda r: r.url.endswith("/api/term/create")) as created:
        page.locator("#new-session-go").click()
    assert created.value.status == 200, created.value.text()
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    start = wait_start(fixture, lambda row: "--session-id" in row["args"])
    sid = start["args"][start["args"].index("--session-id") + 1]
    assert len(sid) == 36 and sid != fixture.sid, start
    assert start["args"][start["args"].index("--model") + 1] == "fake-buddy-2", start
    assert start["args"][start["args"].index("--effort") + 1] == "high", start
    assert "--resume" not in start["args"] and "--input-format=stream-json" not in start["args"], start
    assert start["cwd"] == str(fixture.cwd.resolve()), start
    expect(page.locator("#a-chat")).to_be_hidden()
    if screenshots:
        page.screenshot(path=str(screenshots / "codebuddy-cli-new-session.png"), full_page=True)
    assert not failures, failures
    context.close()

    # 4. Narrow screen: the picker keeps CodeBuddy on screen with the other sources.
    narrow = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme="dark", service_workers="block")
    mobile = narrow.new_page()
    mobile.goto(base, wait_until="domcontentloaded")
    mobile.wait_for_function("T.listLoaded")
    mobile.evaluate("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))")
    if not mobile.locator("#new-session").is_visible():
        mobile.locator("#header-more-btn").click()
    mobile.locator("#new-session").click()
    radio = mobile.locator('#new-session-form label:has(input[name="new-source"][value="codebuddy"])')
    expect(radio).to_be_visible()
    box = radio.bounding_box()
    assert box and 0 <= box["x"] and box["x"] + box["width"] <= 390, box
    if screenshots:
        mobile.screenshot(path=str(screenshots / "codebuddy-new-session-mobile-dark.png"), full_page=True)
    narrow.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    from playwright.sync_api import sync_playwright
    with tempfile.TemporaryDirectory(prefix="sessiondock-codebuddy-cli-") as temporary:
        fixture = CodebuddyFixture(Path(temporary))
        config = json.loads(fixture.launcher.read_text())
        config["host_binary"] = str(args.binary.resolve(strict=True).with_name("ptyhost"))
        fixture.launcher.write_text(json.dumps(config))
        try:
            with private_hosts(fixture.root / "host"), fixture.server(args.binary) as base, sync_playwright() as playwright:
                browser = playwright.chromium.launch(headless=True)
                exercise(browser, base, fixture, args.screenshots)
                browser.close()
        finally:
            from sdkhost_fixtures import stop_private_sdkhosts
            stop_private_sdkhosts(fixture.root)
    print("PASS CodeBuddy CLI terminal: no chat view, native resume argv, picker model/effort/start mode, narrow picker")


if __name__ == "__main__":
    main()
