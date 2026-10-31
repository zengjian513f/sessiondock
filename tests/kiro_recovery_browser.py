#!/usr/bin/env python3
"""Kiro sdkhost restart recovery, approval receipts and cancellation fencing in Chromium."""
from __future__ import annotations
import argparse
from contextlib import ExitStack
import json
import os
import shutil
from pathlib import Path
from storage_tiering_fixtures import read_inputs, legacy_inputs
import sys
import tempfile
import threading
import time
from types import SimpleNamespace

from kiro_sdk_fixtures import FakeKiroAcp, KiroFixture


class RecoveryPeer(FakeKiroAcp):
    def __init__(self):
        super().__init__()
        self.previous_prompt = None

    def call_client(self, req_id, method, params):
        event, box = threading.Event(), {"result": None}
        self.pending_rpc[req_id] = (event, box)
        self.send({"jsonrpc": "2.0", "id": req_id, "method": method, "params": params})
        event.wait(timeout=90)
        return box["result"]

    def run_prompt(self, message):
        text = " ".join(block.get("text", "") for block in message["params"].get("prompt", [])).strip()
        if self.previous_prompt:
            # A duplicate old result must not finish this later prompt.
            self.reply(self.previous_prompt, {"stopReason": "end_turn"})
        if text == "WORK_THEN_ERROR":
            self.append_v3_message("user", text)
            self.update("tool_call", toolCallId="completed-work", title="Apply changes", kind="edit", status="completed",
                        rawOutput={"output": "Changes applied"})
            self.update("agent_message_chunk", content={"type": "text", "text": "Work executed before the model error"})
            self.error(message, -32000, "The model is not available after executing work")
            return
        if text == "RECOVERY_WAIT":
            self.append_v3_message("user", text)
            self.update("agent_message_chunk", content={"type": "text", "text": "Durable partial before restart"})
            self.cancel.wait(timeout=90)
            release = self.sessions_root / "release-cancel"
            deadline = time.monotonic() + 90
            while not release.exists() and time.monotonic() < deadline:
                time.sleep(.03)
            self.reply(message, {"stopReason": "cancelled"})
            self.previous_prompt = message
            return
        if text == "AFTER_CANCEL":
            self.update("agent_message_chunk", content={"type": "text", "text": "Next turn remains active"})
            release = self.sessions_root / "release-next"
            deadline = time.monotonic() + 90
            while not release.exists() and time.monotonic() < deadline:
                time.sleep(.03)
            self.reply(message, {"stopReason": "end_turn"})
            return
        if text == "MALFORMED_PERMISSION":
            self.call_client("malformed-permission", "session/request_permission", {
                "sessionId": self.sid, "toolCall": {"title": "Native request without choices"}, "options": []})
            self.reply(message, {"stopReason": "cancelled"})
            return
        if text == "EXIT_RESULT":
            self.update("agent_message_chunk", content={"type": "text", "text": "Final result survives process exit"})
            self.reply(message, {"stopReason": "end_turn"})
            # Both streams may end without LF; skipping quiet stderr during
            # replay must still drain its final bytes before declaring exit.
            sys.stderr.write("fixture final stderr diagnostic")
            sys.stderr.flush()
            os._exit(0)
        super().run_prompt(message)


def fake_main():
    if "acp" in sys.argv:
        peer = RecoveryPeer()
        for line in sys.stdin:
            if line.strip():
                peer.handle(json.loads(line))
    else:
        from kiro_sdk_fixtures import main
        sys.argv.remove("--fake-recovery")
        main()


def run(args):
    from playwright.sync_api import expect, sync_playwright
    from codex_sdk_browser import server
    from kiro_sdk_browser import watch, api_raw
    from sdk_ui_fixtures import start_sdk
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize

    with tempfile.TemporaryDirectory(prefix="sessiondock-kiro-recovery-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok", "state", "trash"):
            (root / name).mkdir()
        # This suite uses only SDK hosts; a service-only release has no sibling
        # ptyhost binary. Keep the fixture's unused launcher placeholder.
        fixture = KiroFixture(root)
        config = json.loads(fixture.launcher.read_text())
        config["profiles"][0]["args"] = [str(Path(__file__).resolve()), "--fake-recovery"]
        fixture.launcher.write_text(json.dumps(config))
        initialize("--initialize-lifecycle", root / "ledger", args.binary.resolve())
        env = {**fixture.env, "SESSIONDOCK_NATIVE_SDKS": "auto", "SESSIONDOCK_STATE_DIR": str(root / "state"),
               "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"), "SESSIONDOCK_TRASH_DIR": str(root / "trash"),
               "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir)}
        stack = ExitStack()
        try:
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch(headless=True)
                page = browser.new_page(viewport={"width": 1280, "height": 900})
                page.set_default_timeout(20000)
                page.on("request", lambda req: print("SDK HTTP", req.url.rsplit("/", 1)[-1], req.post_data, flush=True)
                        if "/api/native-sdk/" in req.url and req.method == "POST" else None)
                uid = None

                def restart(start=False):
                    nonlocal stack, uid
                    page.goto("about:blank")
                    stack.close()
                    stack = ExitStack()
                    base = stack.enter_context(server(SimpleNamespace(root=root), args.binary, "off", env))
                    page.goto(base, wait_until="domcontentloaded")
                    page.wait_for_function("T.listLoaded")
                    page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(r => r.sid === sid); }", arg=fixture.sid, polling=250)
                    if uid is None:
                        uid = page.evaluate("sid => S.sessions.find(r => r.sid === sid).uid", fixture.sid)
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    if start:
                        page.locator("#a-sdk").click()
                        start_sdk(page)
                    watch(page, uid)
                    return base

                def send(text):
                    page.locator("#sdk-input").fill(text)
                    page.locator("#sdk-send").click()

                def shot(name):
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / name), full_page=True)

                restart(start=True)
                page.wait_for_function("window.__kiroSnapshot?.session_id")
                assert page.evaluate("window.__kiroSnapshot.session_id") == fixture.sid
                caps = page.evaluate("window.__kiroSnapshot.capabilities")
                assert caps["fork"] and caps["compact"] and caps["history"] and caps["rename"] and caps["steer"], caps
                # Drop the HTTP acknowledgement after native creation completes.
                creation = {}
                def lose_create_ack(route):
                    creation["request"] = route.request.post_data_json
                    response = route.fetch()
                    assert response.ok, response.text()
                    creation["receipt"] = response.json()
                    route.abort("failed")
                page.route("**/api/native-sdk/create", lose_create_ack)
                page.locator("#new-session").click()
                page.locator("label:has(input[name=new-source][value=kiro])").click()
                expect(page.locator("#new-model")).to_be_enabled()
                page.locator("#new-cwd").fill(str(fixture.work))
                page.locator("#new-session-go").click()
                expect(page.locator("#new-session-error")).not_to_be_empty()
                assert creation.get("receipt"), creation
                page.unroute("**/api/native-sdk/create", lose_create_ack)
                page.locator("#new-session-dialog .modal-cancel").click()
                created_count = len(fixture.received("session/new"))
                send("permission-test")
                page.wait_for_function("window.__kiroSnapshot?.requests?.length === 1")
                card_key = page.evaluate("window.__kiroSnapshot.requests[0].key")
                native_prompt = fixture.received("session/prompt")[-1]
                before_loads = len(fixture.received("session/load"))
                shot("kiro-recovery-approval-before.png")

                restart()
                retry_create = api_raw(page, "create", creation["request"])
                assert retry_create["status"] == 200 and retry_create["body"]["uid"] == creation["receipt"]["uid"], retry_create
                assert len(fixture.received("session/new")) == created_count, "Lost creation ACK created a second native session"
                page.wait_for_function("window.__kiroSnapshot?.requests?.length === 1")
                assert page.evaluate("window.__kiroSnapshot.requests[0].key") == card_key
                assert len(fixture.received("session/load")) == before_loads
                assert len(fixture.received("session/prompt")) == 1
                card = page.locator('.sdk-live [data-sdk-request]').filter(has_text="Execute bash command")
                expect(card).to_be_visible()
                card.get_by_role("button", name="Allow once", exact=True).click()
                page.wait_for_function("window.__kiroSnapshot?.last_turn?.status === 'completed' && !window.__kiroSnapshot.running")
                expect(card).to_have_count(0)
                answers = [row for row in fixture.received() if row.get("id") == "perm-req-1" and "result" in row]
                assert len(answers) == 1, answers
                # Native JSON-RPC IDs are reusable. An old page cannot approve the new request.
                send("permission-test")
                page.wait_for_function("window.__kiroSnapshot?.requests?.length === 1")
                fresh_key = page.evaluate("window.__kiroSnapshot.requests[0].key")
                assert fresh_key != card_key, (fresh_key, card_key)
                stale = api_raw(page, "respond", {"uid": uid, "key": card_key, "result": "allow_once"})
                assert stale["status"] >= 400 and "expired" in json.dumps(stale), stale
                assert len([r for r in fixture.received() if r.get("id") == "perm-req-1" and "result" in r]) == 1
                page.locator('.sdk-live [data-sdk-request]').filter(has_text="Execute bash command").get_by_role("button", name="Allow once", exact=True).click()
                page.wait_for_function("window.__kiroSnapshot?.last_turn?.status === 'completed' && !window.__kiroSnapshot.running")
                answers = [r for r in fixture.received() if r.get("id") == "perm-req-1" and "result" in r]
                assert len(answers) == 2, answers
                send("RECOVERY_WAIT")
                page.wait_for_function("window.__kiroSnapshot?.running && window.__kiroSnapshot.items.some(i => i.text?.includes('Durable partial before restart'))")
                # BUG-20261031-100103-13d7b8: an accepted steer must survive restart.
                page.locator("#sdk-input").fill("STEER_SURVIVES_RESTART")
                with page.expect_response(lambda r: "/api/native-sdk/steer" in r.url):
                    page.locator("#sdk-input").press("Control+Enter")
                page.wait_for_function("window.__kiroSnapshot.items.some(i => i.steered && i.text === 'STEER_SURVIVES_RESTART')")
                page.wait_for_function("window.__kiroSnapshot.items.some(i => i.text?.includes('continuing after steer'))")
                before_prompts = len(fixture.received("session/prompt"))

                # Exercise the real deployment guard against this running turn,
                # including old ledgers that omitted the sdkhost result marker.
                sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "deploy"))
                from sdtargets.base import Target, DeployOptions, ShellError
                from sdtargets.linux import LinuxNodeHandler
                guard_root = root / "deploy-guard"
                guard_state = guard_root / "state"
                guard_state.mkdir(parents=True)
                ledger = read_inputs(root / "state/native-sdk-inputs.json")
                active = [r for rows in ledger.values() for r in rows
                          if r["state"] in ("sending", "accepted", "recovering") or r.get("runtime_state")]
                assert active, ledger
                for row in active:
                    (row.get("result") or {}).pop("sdkhost", None)
                (guard_state / "native-sdk-inputs.json").write_text(json.dumps(ledger))
                shutil.copytree(root / "state/sdk-adapters", guard_state / "sdk-adapters")
                for receipt in (root / "state/sdkhost").glob("*/receipts.jsonl"):
                    dest = guard_state / "sdkhost" / receipt.parent.name / receipt.name
                    dest.parent.mkdir(parents=True)
                    shutil.copyfile(receipt, dest)
                guard = LinuxNodeHandler(Target("fixture", "linux-node", str(guard_root)),
                                         None, DeployOptions(), lambda _: None)
                guard.check_native_work()
                (guard_state / "sdkhost").rename(guard_state / "sdkhost-hidden")
                try:
                    guard.check_native_work()
                    raise AssertionError("Unconfirmed in-process work must still block restart")
                except ShellError:
                    pass
                print("Deployment guard: active legacy Kiro host allowed; missing host evidence blocked", flush=True)

                # Crash after native write but before the shared admission ACK:
                # consult the original host receipt, never submit a second time.
                page.goto("about:blank")
                stack.close()
                journal_path = root / "state/native-sdk-inputs.json"
                interrupted = read_inputs(journal_path)
                pending = next(r for rows in interrupted.values() for r in rows if r['draft'].get('text') == 'RECOVERY_WAIT')
                pending.update(state='sending', result=None, runtime_state='')
                legacy_inputs(journal_path, interrupted)
                # The steer body is a committed checkpoint row. Removing it
                # reproduces checkpoints written by builds that skipped the row.
                import gzip
                import sqlite3
                steer_rows = []
                for database in (root / "state/sdk-adapters/kiro").glob("*.sqlite"):
                    with sqlite3.connect(database) as db:
                        for owner, ordinal, value in db.execute("SELECT owner, ordinal, value FROM items"):
                            body = json.loads(gzip.decompress(value) if isinstance(value, bytes) else value)
                            if body.get("steered") and body.get("text") == "STEER_SURVIVES_RESTART":
                                steer_rows.append((database, owner, ordinal))
                assert len(steer_rows) == 1, steer_rows
                database, owner, ordinal = steer_rows[0]
                with sqlite3.connect(database) as db:
                    db.execute("DELETE FROM items WHERE owner=? AND ordinal=?", (owner, ordinal))
                restart()
                page.wait_for_function("window.__kiroSnapshot?.running === true")
                assert not page.evaluate("window.__kiroSnapshot.error"), page.evaluate("window.__kiroSnapshot.error")
                expect(page.locator(".sdk-live")).to_contain_text("STEER_SURVIVES_RESTART")
                steered = page.evaluate("window.__kiroSnapshot.items.filter(i => i.steered).map(i => i.text)")
                assert steered == ["STEER_SURVIVES_RESTART"], steered
                assert len(fixture.received("session/prompt")) == before_prompts
                assert len(fixture.received("session/load")) == before_loads
                items = page.evaluate("window.__kiroSnapshot.items")
                assert sum(item.get("text", "").count("Durable partial before restart") for item in items) == 1, items
                shot("kiro-recovery-stream-restored.png")
                page.locator("#sdk-stop").click()
                page.wait_for_function("window.__kiroSnapshot?.cancel_requested === true")
                assert page.evaluate("window.__kiroSnapshot.running") is True
                send("AFTER_CANCEL")
                page.wait_for_timeout(700)
                assert len(fixture.received("session/prompt")) == before_prompts, "cancel ACK released next prompt before native terminal result"
                (fixture.sessions / "release-cancel").touch()
                page.wait_for_function("window.__kiroSnapshot?.running && window.__kiroSnapshot.items.some(i => i.text?.includes('Next turn remains active'))")
                # The fake repeats the older prompt's completed response immediately before this chunk.
                assert page.evaluate("window.__kiroSnapshot.last_turn.status") == "in_progress"
                shot("kiro-recovery-next-turn.png")
                (fixture.sessions / "release-next").touch()
                page.wait_for_function("window.__kiroSnapshot?.last_turn?.status === 'completed' && !window.__kiroSnapshot.running")
                send("MALFORMED_PERMISSION")
                page.wait_for_function("window.__kiroSnapshot?.requests?.some(r => r.title === 'Native request without choices')")
                malformed = page.evaluate("window.__kiroSnapshot.requests.find(r => r.title === 'Native request without choices')")
                assert malformed["options"] == [], malformed
                bad = api_raw(page, "respond", {"uid": uid, "key": malformed["key"], "result": "allow_once"})
                assert bad["status"] >= 400, bad
                missing = api_raw(page, "respond", {"uid": uid, "key": malformed["key"], "result": {}})
                assert missing["status"] >= 400, missing
                assert not [r for r in fixture.received() if r.get("id") == "malformed-permission" and "result" in r]
                malformed_card = page.locator('.sdk-live [data-sdk-request]').filter(has_text="Native request without choices")
                expect(malformed_card).to_contain_text("Kiro 未提供可选的审批项")
                expect(malformed_card.locator("input, textarea")).to_have_count(0)
                expect(malformed_card.get_by_role("button")).to_have_count(1)
                shot("kiro-recovery-malformed-permission.png")
                if args.screenshots:
                    for width in (1280, 390):
                        page.set_viewport_size({"width": width, "height": 900})
                        if width == 390:
                            page.evaluate("showMobileDetail()")
                        for theme in ("light", "dark"):
                            page.emulate_media(color_scheme=theme)
                            shot(f"kiro-recovery-malformed-{width}-{theme}.png")
                    page.set_viewport_size({"width": 1280, "height": 900})
                    page.emulate_media(color_scheme="light")
                malformed_card.get_by_role("button", name="取消", exact=True).click()
                page.wait_for_function("!window.__kiroSnapshot.running")
                send("WORK_THEN_ERROR")
                page.wait_for_function("window.__kiroSnapshot?.error?.includes('after executing work')")
                queue = page.locator('.sdk-queue')
                expect(queue.locator('[data-queue-id]')).to_have_count(0)
                before_error_restart = len(fixture.received("session/prompt"))
                shot("kiro-execution-error.png")
                page.goto("about:blank")
                stack.close()
                journal_path = root / "state/native-sdk-inputs.json"
                journal = read_inputs(journal_path)
                legacy = next(r for rows in journal.values() for r in rows if r['draft'].get('text') == 'WORK_THEN_ERROR')
                assert legacy['state'] == 'completed' and legacy['result']
                legacy['state'] = 'failed'
                legacy['error'] = 'The model is not available after executing work'
                legacy.pop('runtime_state', None)
                legacy.pop('runtime_error', None)
                legacy_inputs(journal_path, journal)
                restart()
                page.wait_for_function("window.__kiroSnapshot?.error?.includes('after executing work')")
                page.wait_for_function("NativeSdkView.connected && NativeSdkView.raw?.error?.includes('after executing work')")
                expect(page.locator('#sdk-send')).to_be_enabled()
                assert len(fixture.received("session/prompt")) == before_error_restart
                expect(queue.locator('[data-queue-id]')).to_have_count(0)
                migrated = next(r for rows in read_inputs(journal_path).values() for r in rows if r['draft'].get('text') == 'WORK_THEN_ERROR')
                assert migrated['state'] == 'completed' and migrated['error'] is None
                assert 'after executing work' in migrated['runtime_error']
                for width, theme in ((1280, 'light'), (390, 'dark')):
                    page.set_viewport_size({'width': width, 'height': 900})
                    page.emulate_media(color_scheme=theme)
                    if width == 390:
                        page.evaluate('showMobileDetail()')
                    shot(f'kiro-execution-error-restored-{width}-{theme}.png')
                page.set_viewport_size({'width': 1280, 'height': 900})
                page.emulate_media(color_scheme='light')
                send("EXIT_RESULT")
                page.wait_for_function("window.__kiroSnapshot?.runtime_error?.includes('fixture final stderr diagnostic')")
                assert page.evaluate("window.__kiroSnapshot.last_turn.status") == "completed"
                assert page.evaluate("window.__kiroSnapshot.error") is None
                assert page.evaluate("window.__kiroSnapshot.items.some(i => i.text?.includes('Final result survives process exit'))")
                prompt_count = len(fixture.received("session/prompt"))
                shot("kiro-recovery-completed-exit.png")

                # Cold exited-host output remains readable with original offsets.
                import gzip
                for status_path in (root / 'state/sdkhost').glob('*/status.json'):
                    status = json.loads(status_path.read_text())
                    if status.get('exit') is None or not status.get('stdout_eof') or not status.get('stderr_eof'):
                        continue
                    for name in ('stdout', 'stderr'):
                        stream = status_path.parent / name
                        with gzip.open(stream.with_suffix('.gz'), 'wb') as compressed:
                            compressed.write(stream.read_bytes())
                        stream.unlink()
                restart()
                print("exit-restart terminal", json.dumps(page.evaluate("({last_turn:window.__kiroSnapshot.last_turn,runtime_error:window.__kiroSnapshot.runtime_error})")), flush=True)
                page.wait_for_function("window.__kiroSnapshot?.last_turn?.status === 'completed'")
                assert len(fixture.received("session/prompt")) == prompt_count
                assert len(fixture.received("session/load")) == before_loads, "restart spawned a replacement for a finished host"
                assert native_prompt == fixture.received("session/prompt")[0]
                if args.rollback_binary:
                    import subprocess
                    page.goto("about:blank")
                    stack.close()
                    subprocess.run([sys.executable, str(Path(__file__).resolve().parents[1] / 'deploy/storage_rollback.py'), str(root / 'state')], check=True, timeout=30)
                    args.binary = args.rollback_binary
                    restart()
                    page.wait_for_function("window.__kiroSnapshot?.last_turn?.status === 'completed'")
                    assert len(fixture.received('session/prompt')) == prompt_count
                    assert len(fixture.received('session/load')) == before_loads
                    print('PASS: tiered state exported, previous binary restores verdict without replay', flush=True)
                print(json.dumps({"uid": uid, "sid": fixture.sid, "prompts": prompt_count, "loads": before_loads, "approval_responses": len(answers)}))
                browser.close()
        finally:
            stack.close()
            stop_private_sdkhosts(root)
    print("Kiro durable host recovery browser: PASS")


def main():
    if "--fake-recovery" in sys.argv:
        return fake_main()
    from history_fixtures import BINARY
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    parser.add_argument("--rollback-binary", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    run(args)


if __name__ == "__main__":
    main()
