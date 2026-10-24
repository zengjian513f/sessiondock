#!/usr/bin/env python3
"""Kiro quiet/background runtimes survive 30 minutes and remain explicitly stoppable."""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace

from kiro_sdk_fixtures import FakeKiroAcp, KiroFixture


class QuietPeer(FakeKiroAcp):
    def run_prompt(self, message):
        text = " ".join(b.get("text", "") for b in message["params"].get("prompt", []))
        if text != "launch background":
            return super().run_prompt(message)
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(180)"])
        (self.sessions_root / "background.pid").write_text(str(child.pid))
        self.append_v3_message("user", text)
        self.update("tool_call", toolCallId="background-launch", title="Launch background worker",
                    kind="execute", status="completed", rawInput={"command": "worker &", "run_in_background": False},
                    rawOutput={"output": f"pid={child.pid}", "exitCode": 0})
        self.update("agent_message_chunk", content={"type": "text", "text": "Background worker started"})
        self.reply(message, {"stopReason": "end_turn"})


def run(args):
    from playwright.sync_api import sync_playwright, expect
    from codex_sdk_browser import server
    from grok_sdk_lifecycle_browser import advance, alive, wait_for_process
    from kiro_sdk_browser import watch
    from sdk_ui_fixtures import start_sdk
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize

    with tempfile.TemporaryDirectory(prefix="sessiondock-kiro-idle-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok", "state", "trash"):
            (root / name).mkdir()
        fixture = KiroFixture(root)
        config = json.loads(fixture.launcher.read_text())
        config["profiles"][0]["args"] = [str(Path(__file__).resolve()), "--fake-quiet"]
        fixture.launcher.write_text(json.dumps(config))
        # The removed Kiro timer used wall time; advance both clocks only inside
        # this private service. Native peers and the browser retain real time.
        clock = root / "clock.c"
        clock.write_text(Path(__file__).with_name("grok_sdk_idle_clock.c").read_text().replace(
            "clock_id != CLOCK_MONOTONIC", "(clock_id != CLOCK_MONOTONIC && clock_id != CLOCK_REALTIME)"))
        library = root / "clock.so"
        subprocess.run(["cc", "-shared", "-fPIC", "-O2", str(clock), "-o", str(library), "-ldl"], check=True, timeout=30)
        marker = root / "clock-seconds"
        marker.write_text("0")
        initialize("--initialize-lifecycle", root / "ledger", args.binary.resolve())
        env = {**fixture.env, "SESSIONDOCK_NATIVE_SDKS": "auto", "SESSIONDOCK_STATE_DIR": str(root / "state"),
               "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"), "SESSIONDOCK_TRASH_DIR": str(root / "trash"),
               "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir), "LD_PRELOAD": str(library),
               "SESSIONDOCK_TEST_CLOCK_FILE": str(marker)}
        try:
            with server(SimpleNamespace(root=root), args.binary, "off", env) as base, sync_playwright() as pw:
                browser = pw.chromium.launch(headless=True)
                page = browser.new_page(viewport={"width": 1280, "height": 900})
                page.set_default_timeout(20000)
                errors = []
                page.on("pageerror", lambda e: errors.append(str(e)))
                page.goto(base, wait_until="domcontentloaded")
                page.wait_for_function("T.listLoaded")
                page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(r => r.sid === sid); }", arg=fixture.sid)
                uid = page.evaluate("sid => S.sessions.find(r => r.sid === sid).uid", fixture.sid)
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.locator("#a-sdk").click()
                start_sdk(page)
                watch(page, uid)
                def pid():
                    return next(json.loads(p.read_text())["pid"] for p in (root / "state/sdkhost").glob("kiro_open_*/status.json"))
                original = pid()
                observer = browser.new_page()
                observer.goto(base + "/api/health")
                advance(observer, marker, 1900)
                assert alive(original), "quiet subscribed Kiro runtime was killed by idle reclamation"
                page.locator("#sdk-input").fill("launch background")
                page.locator("#sdk-send").click()
                expect(page.locator(".sdk-live")).to_contain_text("Background worker started")
                page.wait_for_function("window.__kiroSnapshot?.last_turn?.status === 'completed' && !window.__kiroSnapshot.running")
                child = int((fixture.sessions / "background.pid").read_text())
                assert alive(child)
                before = len(fixture.received("session/load")), len(fixture.received("session/prompt"))
                page.close()
                advance(observer, marker, 1900)
                advance(observer, marker, 120)
                assert alive(original) and alive(child), "closed page reclaimed a native runtime or background worker"
                page = browser.new_page(viewport={"width": 1280, "height": 900})
                page.set_default_timeout(20000)
                page.on("pageerror", lambda e: errors.append(str(e)))
                page.goto(base, wait_until="domcontentloaded")
                page.wait_for_function("T.listLoaded")
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                start_sdk(page)
                watch(page, uid)
                assert pid() == original
                assert before == (len(fixture.received("session/load")), len(fixture.received("session/prompt"))), "reopen replayed native load or input"
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "kiro-quiet-reopened.png"), full_page=True)
                with page.expect_response(lambda r: r.url.endswith(('/api/session/stop', '/api/native-sdk/stop')) and r.request.method == 'POST') as stopped:
                    page.locator("#a-session-toggle").click()
                    popup = page.locator('dialog.app-popup')
                    if popup.is_visible():
                        popup.get_by_role('button', name='确定', exact=True).click()
                assert stopped.value.ok, stopped.value.text()
                wait_for_process(observer, original, False)
                wait_for_process(observer, child, False)
                assert not errors, errors
                browser.close()
        finally:
            stop_private_sdkhosts(root)
    print("PASS Kiro quiet/subscribed/closed-page background survival, same-instance reopen, explicit stop")


if __name__ == "__main__":
    if "--fake-quiet" in sys.argv:
        if "acp" in sys.argv:
            peer = QuietPeer()
            for line in sys.stdin:
                if line.strip():
                    peer.handle(json.loads(line))
        else:
            from kiro_sdk_fixtures import main
            sys.argv.remove("--fake-quiet")
            main()
    else:
        from history_fixtures import BINARY
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument("--binary", type=Path, default=BINARY)
        parser.add_argument("--screenshots", type=Path)
        args = parser.parse_args()
        if args.screenshots:
            args.screenshots.mkdir(parents=True, exist_ok=True)
        run(args)
