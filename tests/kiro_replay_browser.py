#!/usr/bin/env python3
"""Large Kiro subagent load replay through Chromium and an isolated real Hub."""
# run_validation: serial
from __future__ import annotations
import argparse
from contextlib import ExitStack
import json
import sqlite3
import os
import shutil
from pathlib import Path
from storage_tiering_fixtures import checkpoints, read_checkpoint, legacy_checkpoint
import sys
import tempfile
import time
from types import SimpleNamespace
from kiro_sdk_fixtures import FakeKiroAcp, KiroFixture


class ReplayPeer(FakeKiroAcp):
    @staticmethod
    def history_updates(directory):
        yield from FakeKiroAcp.history_updates(directory)
        # Similar shape to a long review session: many independent children,
        # each replayed in small ACP frames, without a large parent DOM.
        for index in range(94):
            aid = f"review-{index}"
            meta = {"kiro": {"agentSubtaskId": aid, "replay": True}}
            yield {"sessionUpdate": "tool_call", "toolCallId": "invoke_" + aid,
                   "title": aid, "status": "in_progress",
                   "_meta": {"kiro": {**meta["kiro"], "kind": "agent-subtask"}}}
            for part in range(40):
                yield {"sessionUpdate": "tool_call", "toolCallId": f"read-{part}",
                       "title": "Read File", "status": "completed",
                       "rawOutput": f"{aid}/{part}:" + "synthetic output " * 1024,
                       "_meta": meta}
            yield {"sessionUpdate": "tool_call_update", "toolCallId": "invoke_" + aid,
                   "status": "completed", "rawOutput": "Review complete", "_meta": {"kiro": {"replay": True}}}


def main():
    if "--fake-replay" in sys.argv:
        if "acp" in sys.argv:
            peer = ReplayPeer()
            for line in sys.stdin:
                if line.strip():
                    peer.handle(json.loads(line))
        else:
            from kiro_sdk_fixtures import main as fake_main
            sys.argv.remove("--fake-replay")
            fake_main()
        return
    from playwright.sync_api import sync_playwright, expect
    from codex_sdk_browser import server
    from history_fixtures import BINARY
    from hub_fixtures import Hub, free_port
    from node_auth_fixtures import TOKEN, node_env
    from sdkhost_fixtures import stop_private_sdkhosts
    from sdk_ui_fixtures import start_sdk
    from send_browser import initialize
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--hub-binary", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sd-replay-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok", "state", "hub"):
            (root / name).mkdir()
        fixture = KiroFixture(root)
        config = json.loads(fixture.launcher.read_text())
        config["profiles"][0]["args"] = [str(Path(__file__).resolve()), "--fake-replay"]
        fixture.launcher.write_text(json.dumps(config))
        initialize("--initialize-lifecycle", root / "ledger", args.binary.resolve())
        port = free_port()
        env = {**fixture.env, **node_env(root, port, "127.0.0.0/8"),
               "SESSIONDOCK_STATE_DIR": str(root / "state"),
               "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir),
               "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"),
               "SESSIONDOCK_NATIVE_SDKS": "auto"}
        try:
            with sync_playwright() as playwright, ExitStack() as stack:
                stack.enter_context(server(SimpleNamespace(root=root), args.binary.resolve(), "off", env))
                hub = Hub((args.hub_binary or args.binary.with_name("sessiondock-hub")).resolve(),
                          root / "hub", [SimpleNamespace(name="Replay", token=TOKEN, port=port)])
                hub.start()
                stack.callback(hub.stop)
                browser = playwright.chromium.launch(headless=True)
                stack.callback(browser.close)
                page = browser.new_page()
                page.set_default_timeout(15000)
                page.on('response', lambda r: print('SDK response', r.status, r.url.rsplit('/', 1)[-1], flush=True) if '/api/native-sdk/open' in r.url else None)
                page.goto(f"http://127.0.0.1:{hub.port}", wait_until="domcontentloaded")
                page.wait_for_function("T.listLoaded && Object.values(Nodes.capabilities).some(c => c.native_sdks?.kiro)")
                page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(r => r.sid === sid); }", arg=fixture.sid)
                uid = page.evaluate("sid => S.sessions.find(r => r.sid === sid).uid", fixture.sid)
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.wait_for_function("SdkView.pane && !SdkView.historyReady")
                print("Starting 94-child / 60 MB replay through Hub", flush=True)
                started = time.monotonic()
                page.set_default_timeout(55000)
                with page.expect_response(lambda r: r.url.endswith("/api/native-sdk/open"), timeout=55000) as opened:
                    try:
                        start_sdk(page)
                    except Exception:
                        print('UI',page.evaluate("({connected:SdkView.connected,readOnly:SdkView.readOnly,status:document.querySelector('.sdk-status')?.textContent,error:NativeSdkView.raw?.error})"),flush=True)
                        for path in (root/'state/sdk-adapters/kiro').glob('*.sqlite'):
                            with sqlite3.connect(path) as db:
                                value=json.loads(db.execute('SELECT state FROM checkpoint').fetchone()[0])
                                print('checkpoint', {k:value.get(k) for k in ('loading','stdout_offset','error','runtime_error')}, 'children',len(value.get('subtasks',{})),flush=True)
                        raise
                page.set_default_timeout(15000)
                response = opened.value
                elapsed = time.monotonic() - started
                print(f"Open: HTTP {response.status}, {elapsed:.2f}s", flush=True)
                assert response.ok, response.text()
                page.wait_for_function("SdkView.connected")
                expect(page.locator(".sdk-pane .sdk-status")).not_to_contain_text("无法打开")
                paths = checkpoints(root / "state/sdk-adapters/kiro")
                states = (read_checkpoint(p) for p in paths)
                state = next(s for s in states if s.get("session_id") == fixture.sid)
                for checkpoint in paths:
                    with sqlite3.connect(checkpoint.with_suffix('.sqlite')) as db:
                        header = db.execute('SELECT length(state) FROM checkpoint').fetchone()[0]
                        assert header < 1024 * 1024, header
                        assert db.execute('SELECT count(*) FROM items').fetchone()[0] >= 94 * 40
                assert len(state["subtasks"]) == 94
                for task in state["subtasks"].values():
                    assert task["status"] == "completed"
                    assert len(task["state"]["items"]) == 40
                assert len(fixture.received("session/load")) == 1
                page.reload(wait_until="domcontentloaded")
                page.wait_for_function("SdkView.connected")
                assert len(fixture.received("session/load")) == 1, "Refresh replayed the load"
                stopped = page.request.post(f"http://127.0.0.1:{hub.port}/api/native-sdk/stop", data={"uid": uid})
                assert stopped.ok, stopped.text()
                print("PASS: complete child projection, ready SDK and refresh without repeated load", flush=True)
                # A separate, unbound copy exercises stock migration and background
                # archival without changing any live adapter's files.
                source = root / 'state/sdkhost' / paths[0].stem
                closed = root / 'state/sdkhost/cold-fixture'
                closed.mkdir()
                status = json.loads((source / 'status.json').read_text())
                assert status['exit'] is not None and status['stdout_eof'] and status['stderr_eof']
                (closed / 'status.json').write_text(json.dumps(status))
                shutil.copyfile(source / 'stdout', closed / 'stdout')
                (closed / 'stderr').write_bytes(b'')
                legacy = root / 'state/sdk-adapters/kiro/cold-fixture.json'
                legacy.write_text(json.dumps(state))
                old = time.time() - 2 * 86400
                for name in ('status.json', 'stdout', 'stderr'):
                    os.utime(closed / name, (old, old))
                page.expose_function('coldStorageReady', lambda: (closed/'stdout.gz').is_file() and not (closed/'stdout').exists() and not legacy.exists())
                print('Waiting for first background cold-storage pass', flush=True)
                page.evaluate("window.__coldPoll=setInterval(()=>window.coldStorageReady().then(ready=>window.__coldReady=ready),250)")
                page.wait_for_function('window.__coldReady === true', timeout=75000)
                page.evaluate('clearInterval(window.__coldPoll)')
                restored = read_checkpoint(legacy)
                assert len(restored['subtasks']) == 94
                assert all(len(task['state']['items']) == 40 for task in restored['subtasks'].values())
                assert (closed/'stdout.gz').stat().st_size < status['stdout_len']
                page.reload(wait_until='domcontentloaded')
                page.wait_for_function('T.listLoaded')
                print('PASS: closed checkpoint migrated, raw output archived, all child bodies retained', flush=True)
        finally:
            stop_private_sdkhosts(root)


if __name__ == "__main__":
    main()
