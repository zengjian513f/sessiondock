#!/usr/bin/env python3
"""Kiro stop on a loaded multi-megabyte session over a throttled link, node and Hub.

BUG-20261031-071358-bef353: every SSE frame carried the whole restored
transcript, so the turn-end frame arrived long after the native cancel and the
stop button stayed visible. Chromium throttling stands in for the Hub link.
"""
import argparse
import json
from pathlib import Path
import re
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from native_sdk_rename_browser import native
from node_auth_fixtures import TOKEN, node_env
from sdk_ui_fixtures import start_sdk
from sdkhost_fixtures import stop_private_sdkhosts

SID = "019a0000-0000-7000-8000-000000000301"
TURNS = 12
ANSWER_BYTES = 256 * 1024


def write_history(root: Path):
    rows = []
    for turn in range(TURNS):
        filler = "".join(f"tool output line {turn}-{n:06d}\n" for n in range(ANSWER_BYTES // 26))
        for kind, text in (("user", f"history turn {turn} question"),
                           ("assistant", f"history turn {turn} answer\n{filler}")):
            rows.append(json.dumps({"id": f"msg-{kind}-{turn}", "timestamp": f"2026-10-18T00:{turn:02d}:00Z",
                                    "message": {"kind": kind, "data": {"content": [{"kind": "text", "data": text}]}}}))
    path = root / "fake-native-store" / "bucket_v3" / SID / "messages.jsonl"
    path.write_text("\n".join(rows) + "\n")
    return path.stat().st_size


def exercise(browser, base, root, shots, label):
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    page.set_default_timeout(30000)
    errors, opens = [], []
    page.on("request", lambda request: opens.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/native-sdk/open") else None)
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    uid = page.wait_for_function(
        "async sid => { await loadSessions(true); return S.sessions.find(row => row.sid === sid)?.uid; }",
        arg=SID, polling=500).json_value()
    row = page.locator(f'#side .item[data-uid="{uid}"]')
    if not row.is_visible():
        page.locator('#chips button[data-source="kiro"]').click()
    # About 1 MB/s, below the measured node-to-Hub rate of the incident.
    cdp = context.new_cdp_session(page)
    cdp.send("Network.enable")
    cdp.send("Network.emulateNetworkConditions", {
        "offline": False, "latency": 20, "downloadThroughput": 1_000_000, "uploadThroughput": 1_000_000})
    row.click()
    page.locator("#a-sdk").click()
    start_sdk(page)
    msgs = page.locator(".sdk-msgs")
    # Earlier turns stay readable through history and appear once.
    expect(msgs).to_contain_text("history turn 0 question")
    expect(msgs).to_contain_text(f"history turn {TURNS - 1} question")
    box = page.locator("#sdk-input")
    box.fill("stream-until-cancel")
    box.press("Enter")
    page.wait_for_function("NativeSdkView.running")
    expect(page.locator("#sdk-stop")).to_be_visible()
    expect(page.locator(".sdk-live")).to_contain_text("streaming thought")
    page.wait_for_timeout(3000)
    live_bytes = page.evaluate("JSON.stringify(NativeSdkView.raw.items).length")
    assert live_bytes < 1024 * 1024, f"{label}: live frame still carries {live_bytes} bytes"
    page.locator("#sdk-stop").click()
    # The native fake ends the prompt within one 50 ms tick of the cancel.
    expect(page.locator("#sdk-stop")).to_be_hidden(timeout=5000)
    expect(page.locator(".sdk-status-row > .sdk-status:visible")).to_have_attribute("title", re.compile("已中断"), timeout=5000)
    assert page.evaluate("!NativeSdkView.running")
    for turn in (0, TURNS - 1):
        count = page.evaluate("text => [...document.querySelectorAll('.sdk-msgs .msg')].filter(n => n.textContent.includes(text)).length",
                              f"history turn {turn} question")
        assert count == 1, f"{label}: turn {turn} rendered {count} times"
    cdp.send("Network.emulateNetworkConditions", {
        "offline": False, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1})
    if shots:
        for width, theme in ((1280, "light"), (390, "dark")):
            page.set_viewport_size({"width": width, "height": 900})
            page.emulate_media(color_scheme=theme)
            page.wait_for_timeout(250)
            if not box.is_visible():
                if row.is_visible():
                    row.click()
                else:
                    page.locator("#a-sdk").click()
            expect(box).to_be_visible()
            expect(page.locator(".sdk-status-row > .sdk-status:visible")).to_have_attribute("title", re.compile("已中断"))
            expect(page.locator("#sdk-composer .composer-run-action")).to_be_hidden()
            page.screenshot(path=str(shots / f"kiro-stop-{label}-{width}-{theme}.png"), full_page=True)
    # A cancelled turn is not a failed SDK open. Keep the instance usable,
    # including after background status refreshes, without showing Start.
    action = page.locator("#sdk-composer .composer-run-action")
    expect(action).to_be_hidden()
    assert page.evaluate("NativeSdkView.connected && !NativeSdkView.openFailed")
    open_count = len(opens)
    box.fill("continue-after-interrupt")
    page.locator("#sdk-send").click()
    page.wait_for_function("NativeSdkView.raw?.last_turn?.status === 'completed'")
    assert len(opens) == open_count, "sending after interrupt reopened the SDK"
    expect(action).to_be_hidden()
    assert not errors, errors
    context.close()
    print("PASS", label, "stop hides within 5 s; history kept once; next turn sends without reopening SDK", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sd-kiro-stop-") as temp, sync_playwright() as p:
        root = Path(temp)
        port = free_port()
        browser = p.chromium.launch(headless=True)
        try:
            with native("kiro", root, args.binary, node_env(root, port, "127.0.0.0/8")) as base:
                size = write_history(root)
                assert size > 3 * 1024 * 1024, size
                exercise(browser, base, root, args.screenshots, "node")
                hub_root = root / "hub"
                hub_root.mkdir()
                hub = Hub(args.binary.with_name("sessiondock-hub").resolve(), hub_root,
                          [SimpleNamespace(name="StopNode", token=TOKEN, port=port)])
                hub.start()
                try:
                    exercise(browser, f"http://127.0.0.1:{hub.port}", root, args.screenshots, "hub")
                finally:
                    hub.stop()
        finally:
            stop_private_sdkhosts(root)
            browser.close()


if __name__ == "__main__":
    main()
