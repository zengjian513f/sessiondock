#!/usr/bin/env python3
"""Kiro replay throughput and slow native startup/catalog through Chromium and Hub."""
from __future__ import annotations
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from types import SimpleNamespace

from kiro_sdk_fixtures import FakeKiroAcp, KiroFixture


def fake_main():
    if "--list-models" in sys.argv:
        root = Path(os.environ["KIRO_SDK_FAKE_ROOT"])
        (root / "models-started").touch()
        if (root / "slow-models").exists():
            time.sleep(11)
    if "acp" not in sys.argv:
        from kiro_sdk_fixtures import main
        sys.argv.remove("--fake-startup")
        return main()

    class Peer(FakeKiroAcp):
        def handle(self, message):
            if message.get("method") == "session/load" and (self.sessions_root / "slow-open").exists():
                time.sleep(11)
            super().handle(message)

    peer = Peer()
    for line in sys.stdin:
        if line.strip():
            peer.handle(json.loads(line))


def exercise(browser, base, fixture, screenshots, baseline):
    from playwright.sync_api import expect
    page = browser.new_page(viewport={"width": 1280, "height": 900}, service_workers="block")
    page.set_default_timeout(30000)
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded && Object.values(Nodes.capabilities).some(c => c.native_sdks?.kiro)")
    page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(r => r.sid === sid); }", arg=fixture.sid, polling=300)
    uid = page.evaluate("sid => S.sessions.find(r => r.sid === sid).uid", fixture.sid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.locator("#a-sdk").click()
    page.wait_for_function("SdkView.pane && !SdkView.historyReady")
    start = time.monotonic()
    with page.expect_response(lambda r: r.url.endswith("/api/native-sdk/open")) as opened:
        page.locator("#sdk-input").click()
    elapsed = time.monotonic() - start
    print(f"Large replay: {elapsed:.3f}s, HTTP {opened.value.status}", flush=True)
    if baseline:
        if screenshots:
            page.screenshot(path=str(screenshots / "kiro-startup-before.png"), full_page=True)
        page.close()
        return
    assert opened.value.ok, opened.value.text()
    assert elapsed < 8, f"Replay still blocks startup: {elapsed:.3f}s"
    page.wait_for_function("SdkView.connected && !SdkView.raw.loading")
    # No lost or duplicated replay messages, including frames spanning host reads.
    items = page.evaluate("SdkView.items.map(i => i.text || i.content)")
    assert len(items) == 400, len(items)
    assert "回放记录 399" in json.dumps(items, ensure_ascii=False)
    if screenshots:
        page.screenshot(path=str(screenshots / "kiro-startup-ready.png"), full_page=True)
    page.locator("#sdk-input").fill("确认回放后能够继续发送")
    page.locator("#sdk-send").click()
    expect(page.locator(".sdk-live")).to_contain_text("Kiro ACP V3 final answer for: 确认回放后能够继续发送")
    page.wait_for_function("!SdkView.running")
    loads = len(fixture.received("session/load"))
    page.reload(wait_until="domcontentloaded")
    page.wait_for_function("SdkView.connected && !SdkView.raw.loading")
    assert len(fixture.received("session/load")) == loads, "Reopen replayed the native load"

    # The ordinary ten-second Hub proxy deadline must not reject a valid slow
    # native open. A different session keeps the existing host undisturbed.
    (fixture.sessions / "slow-open").touch()
    empty_uid = page.evaluate("sid => S.sessions.find(r => r.sid === sid).uid", fixture.empty_sid)
    page.locator(f'#side .item[data-uid="{empty_uid}"]').click()
    page.locator("#a-sdk").click()
    page.wait_for_function("SdkView.pane && !SdkView.historyReady")
    with page.expect_response(lambda r: r.url.endswith("/api/native-sdk/open")) as slow:
        page.locator("#sdk-input").click()
    assert slow.value.ok, slow.value.text()
    page.wait_for_function("SdkView.connected && !SdkView.raw.loading")
    assert not page.evaluate("SdkView.openError")
    (fixture.sessions / "slow-open").unlink()
    print("Slow native open (>10s): PASS", flush=True)

    # The real new-session picker forces fresh models (subscription changes).
    # Even an eleven-second CLI catalog must not block another session's SSE.
    from new_session_model_browser import open_dialog
    (fixture.sessions / "slow-models").touch()
    open_dialog(page)
    page.locator("#new-node").select_option(page.evaluate("uid => nodeOf(uid)", uid))
    with page.expect_response(lambda r: r.url.endswith("/api/native-sdk/models") and r.request.post_data_json.get("source") == "kiro", timeout=25000) as models:
        page.locator('label:has(input[name="new-source"][value="kiro"])').click()
        page.wait_for_function("document.querySelector('#new-model').disabled")
        health_ms = page.evaluate("""async uid => {
          const started = performance.now();
          const result = await fetch(appUrl('api/native-sdk/usage'), {method:'POST',
            headers:{'Content-Type':'application/json'}, body:JSON.stringify({uid})});
          if (!result.ok) throw new Error(await result.text());
          return performance.now() - started;
        }""", empty_uid)
        assert health_ms < 2000, f"Catalog blocked SDK requests: {health_ms}ms"
    assert models.value.ok, models.value.text()
    expect(page.locator("#new-model")).to_be_enabled()
    page.locator("#new-model").click()
    page.locator('#new-model-options [title="claude-haiku-4.5"]').click()
    if screenshots:
        page.screenshot(path=str(screenshots / "kiro-startup-models.png"), full_page=True)
    assert len(fixture.received("session/new")) == 0, "Model discovery created a native session"
    print("Fresh slow catalog (>10s), picker and no native creation: PASS", flush=True)
    page.close()


def main():
    if "--fake-startup" in sys.argv:
        return fake_main()
    from playwright.sync_api import sync_playwright
    from codex_sdk_browser import server
    from history_fixtures import BINARY
    from hub_fixtures import Hub, free_port
    from node_auth_fixtures import TOKEN, node_env
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--hub-binary", type=Path)
    parser.add_argument("--screenshots", type=Path)
    parser.add_argument("--baseline", action="store_true")
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sd-kstart-") as temporary:
        root = Path(temporary)
        for directory in ("claude", "codex", "grok", "state"):
            (root / directory).mkdir()
        fixture = KiroFixture(root)
        records = []
        for i in range(400):
            records.append({"id": f"replay-{i}", "timestamp": "2026-10-18T00:01:00Z", "message": {"kind": "user" if i % 2 == 0 else "assistant", "data": {"content": [{"kind": "text", "data": f"回放记录 {i}：" + "已有会话的上下文与工具输出。" * 35}]}}})
        (fixture.v3_dir / "messages.jsonl").write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in records))
        config = json.loads(fixture.launcher.read_text())
        config["profiles"][0]["args"] = [str(Path(__file__).resolve()), "--fake-startup"]
        fixture.launcher.write_text(json.dumps(config))
        initialize("--initialize-lifecycle", root / "ledger", args.binary.resolve())
        port = free_port()
        env = {**fixture.env, **node_env(root, port, "127.0.0.0/8"), "TOKIO_WORKER_THREADS": "1", "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"), "SESSIONDOCK_STATE_DIR": str(root / "state"), "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir)}
        try:
            with ExitStack() as stack:
                stack.enter_context(server(SimpleNamespace(root=root), args.binary.resolve(), "off", env))
                hub_root = root / "hub"
                hub_root.mkdir()
                hub = Hub((args.hub_binary or BINARY.with_name("sessiondock-hub")).resolve(), hub_root, [SimpleNamespace(name="Kiro", token=TOKEN, port=port)])
                hub.start()
                stack.callback(hub.stop)
                with sync_playwright() as playwright:
                    browser = playwright.chromium.launch(headless=True)
                    try:
                        exercise(browser, f"http://127.0.0.1:{hub.port}", fixture, args.screenshots, args.baseline)
                    finally:
                        browser.close()
        finally:
            stop_private_sdkhosts(root)
    print("Kiro startup browser: " + ("baseline recorded" if args.baseline else "PASS"), flush=True)


if __name__ == "__main__":
    main()
