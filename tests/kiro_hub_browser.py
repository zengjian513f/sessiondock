#!/usr/bin/env python3
"""Kiro SDK/CLI capability gating and interaction through real isolated nodes and Hub."""
from __future__ import annotations
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace

from kiro_sdk_fixtures import FakeKiroAcp, KiroFixture


class HubKiroPeer(FakeKiroAcp):
    def run_prompt(self, message):
        text = " ".join(block.get("text", "") for block in message["params"].get("prompt", []))
        self.append_v3_message("user", text)
        answer = "合成回复：这条消息已经通过 Hub 到达 Kiro 会话。"
        self.append_v3_message("assistant", answer)
        self.update("agent_message_chunk", content={"type": "text", "text": answer})
        self.reply(message, {"stopReason": "end_turn"})


def fake_main():
    if "acp" in sys.argv:
        peer = HubKiroPeer()
        for line in sys.stdin:
            if line.strip():
                peer.handle(json.loads(line))
    else:
        from kiro_sdk_fixtures import main
        sys.argv.remove("--fake-hub-kiro")
        main()


def exercise(browser, base, fixture, disabled_fixture, width, theme, screenshots, missing=False):
    from playwright.sync_api import expect
    context = browser.new_context(viewport={"width": width, "height": 900}, color_scheme=theme,
                                  service_workers="block")
    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
    page = context.new_page()
    page.set_default_timeout(20000)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded && Object.values(Nodes.capabilities).some(cap => cap.native_sdks?.kiro === true)")
    page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(row => row.sid === sid); }", arg=fixture.sid, polling=300)
    uid = page.evaluate("sid => S.sessions.find(row => row.sid === sid && Nodes.capabilities[nodeOf(row.uid)]?.native_sdks?.kiro === true)?.uid", fixture.sid)
    assert uid and "~" in uid, uid
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator("#a-chat")).to_be_hidden()
    expect(page.locator("#a-term")).to_be_visible()
    if missing:
        expect(page.locator("#a-sdk")).to_be_hidden()
        print("REPRODUCED: real Hub meta omits Kiro while node advertises it", page.evaluate("({hub:SessionDockCapabilities.config.native_sdks,node:Nodes.capabilities[nodeOf(S.sel)].native_sdks})"), flush=True)
        if screenshots:
            page.screenshot(path=str(screenshots / "kiro-hub-before.png"), full_page=True)
        context.close()
        return

    def header_pair():
        expect(page.locator("#a-sdk")).to_be_visible()
        expect(page.locator("#a-term")).to_be_visible()
        expect(page.locator("#a-chat")).to_be_hidden()
        # Header metadata updates replace these buttons. Read both rectangles
        # in the same browser turn so a replacement cannot split the assertion.
        page.wait_for_function("""() => {
          const sdk = document.querySelector('#a-sdk')?.getBoundingClientRect();
          const cli = document.querySelector('#a-term')?.getBoundingClientRect();
          return sdk?.width > 0 && cli?.width > 0 && Math.abs(sdk.y-cli.y) < 3 && sdk.x < cli.x;
        }""")

    def shot(state):
        header_pair()
        if screenshots:
            page.screenshot(path=str(screenshots / f"kiro-hub-{width}-{theme}-{state}.png"), full_page=True)

    def stop():
        button = page.locator('#a-session-toggle[aria-label="停止会话"]')
        if not button.is_visible():
            page.locator("#a-more").click()
        with page.expect_response(lambda response: response.url.endswith(("/api/session/stop", "/api/native-sdk/stop")) and response.request.method == "POST") as result:
            button.click()
            popup = page.locator("dialog.app-popup")
            if popup.is_visible():
                popup.get_by_role("button", name="确定", exact=True).click()
        assert result.value.ok, result.value.text()

    header_pair()
    page.locator("#a-sdk").click()
    page.wait_for_function("SdkView.pane && !SdkView.historyReady")
    page.locator("#sdk-input").click()
    page.wait_for_function("SdkView.connected && !SdkView.readOnly")
    page.locator("#sdk-input").fill("请确认这条消息能正常到达当前会话。")
    page.locator("#sdk-send").click()
    expect(page.locator(".sdk-live")).to_contain_text("合成回复：这条消息已经通过 Hub 到达 Kiro 会话。")
    page.wait_for_function("!SdkView.running && SdkView.raw.last_turn?.status === 'completed'")
    shot("sdk")
    page.reload(wait_until="domcontentloaded")
    expect(page.locator("#sdk-input")).to_be_visible()
    page.wait_for_function("SdkView.connected && !SdkView.readOnly")
    assert page.evaluate("S.sel") == uid
    shot("refreshed-sdk")
    stop()
    expect(page.locator("#a-term")).to_be_enabled()
    with page.expect_response(lambda response: response.url.endswith("/api/term/takeover")) as takeover:
        page.locator("#a-term").click()
    assert takeover.value.ok and takeover.value.json().get("ok") and not takeover.value.json().get("needs_confirm"), takeover.value.text()
    assert takeover.value.json().get("state") != "failed", takeover.value.text()
    # The UI waits for the newly launched terminal to enter the node's live
    # inventory before attaching; the Hub's routed inventory adds a round trip.
    expect(page.locator("#termpane")).to_be_visible(timeout=20000)
    try:
        page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    except Exception:
        print("CLI diagnostics", json.dumps({"receipt": takeover.value.json(), "frames": fixture.received(),
              "browser": page.evaluate("""async () => ({selected:S.sel, uid:T.uid, name:T.name,
                list:T.list, ws:T.ws?.readyState, text:document.querySelector('#termpane')?.innerText,
                live:await (await fetch('/api/live?force=1')).json(),
                terminals:await (await fetch('/api/term/list')).json()})""")}, ensure_ascii=False), flush=True)
        raise
    page.wait_for_function("Array.from({length:T.term.buffer.active.length},(_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('Kiro CLI v3 Interactive Terminal')")
    shot("cli")
    page.reload(wait_until="domcontentloaded")
    expect(page.locator("#termpane")).to_be_visible(timeout=20000)
    shot("refreshed-cli")
    stop()

    # SDK creation must use the native SDK model endpoint through the Hub,
    # rather than silently falling back to the CLI model catalog.
    if page.locator("#detail .mobile-back").is_visible():
        page.locator("#detail .mobile-back").click()
    from new_session_model_browser import open_dialog
    model_responses = []
    def record_models(response):
        if response.url.endswith("/api/native-sdk/models") and response.request.post_data_json.get("source") == "kiro":
            model_responses.append(response)
    page.on("response", record_models)
    open_dialog(page)
    page.locator("#new-node").select_option(page.evaluate("uid => nodeOf(uid)", uid))
    page.locator('label:has(input[name="new-source"][value="kiro"])').click()
    expect(page.locator("#new-model")).to_be_enabled()
    page.locator("#new-model").click()
    page.locator('#new-model-options [title="claude-haiku-4.5"]').click()
    page.remove_listener("response", record_models)
    assert model_responses and all(response.ok for response in model_responses), [response.status for response in model_responses]
    assert any(response.request.post_data_json.get("_node") == page.evaluate("uid => nodeOf(uid)", uid) for response in model_responses)
    page.locator("#new-cwd").fill(str(fixture.work))
    with page.expect_response(lambda response: response.url.endswith("/api/native-sdk/create")) as created:
        page.locator("#new-session-go").click()
    assert created.value.ok, created.value.text()
    assert "~" in created.value.json()["uid"]
    expect(page.locator("#sdk-input")).to_be_visible()
    page.locator("#sdk-input").fill("请确认新建会话也能收到这条消息。")
    page.locator("#sdk-send").click()
    expect(page.locator(".sdk-live")).to_contain_text("合成回复：这条消息已经通过 Hub 到达 Kiro 会话。")
    page.wait_for_function("!SdkView.running && SdkView.raw.last_turn?.status === 'completed'")
    shot("created-sdk")
    stop()

    # A second real node has the provider explicitly disabled. Hub support
    # must not bypass that node's capability or expose its SDK controls.
    if page.locator("#detail .mobile-back").is_visible():
        page.locator("#detail .mobile-back").click()
    page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(row => row.sid === sid && Nodes.capabilities[nodeOf(row.uid)]?.native_sdks?.kiro !== true); }", arg=disabled_fixture.sid, polling=300)
    disabled_uid = page.evaluate("sid => S.sessions.find(row => row.sid === sid && Nodes.capabilities[nodeOf(row.uid)]?.native_sdks?.kiro !== true).uid", disabled_fixture.sid)
    page.locator(f'#side .item[data-uid="{disabled_uid}"]').click()
    expect(page.locator("#a-sdk")).to_be_hidden()
    expect(page.locator("#a-chat")).to_be_hidden()
    expect(page.locator("#a-term")).to_be_visible()
    if screenshots:
        page.screenshot(path=str(screenshots / f"kiro-hub-{width}-{theme}-disabled.png"), full_page=True)
    assert not disabled_fixture.received("session/load"), "Disabled node unexpectedly started Kiro SDK"
    assert not errors, errors
    context.close()


def main():
    if "--fake-hub-kiro" in sys.argv:
        return fake_main()
    from playwright.sync_api import sync_playwright
    from codex_sdk_browser import server
    from history_fixtures import BINARY
    from hub_fixtures import Hub, free_port
    from node_auth_fixtures import TOKEN, node_env
    from private_hosts import private_hosts
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--hub-binary", type=Path)
    parser.add_argument("--screenshots", type=Path)
    parser.add_argument("--expect-missing-sdk", action="store_true", help="Reproduce the pre-fix missing Hub capability")
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    # Keep the generated PTY Unix socket below Linux's 108-byte sun_path,
    # including node subdirectories and the host's native session name.
    with tempfile.TemporaryDirectory(prefix="sd-khub-") as temporary:
        root = Path(temporary)
        fixtures = []
        try:
            with ExitStack() as stack:
                nodes = []
                for name, enabled in (("KiroEnabled", True), ("KiroDisabled", False)):
                    node_root = root / name
                    node_root.mkdir()
                    for directory in ("claude", "codex", "grok", "state"):
                        (node_root / directory).mkdir()
                    fixture = KiroFixture(node_root, args.binary.resolve().with_name("ptyhost"))
                    fixtures.append(fixture)
                    config = json.loads(fixture.launcher.read_text())
                    config["profiles"][0]["args"] = [str(Path(__file__).resolve()), "--fake-hub-kiro"]
                    fixture.launcher.write_text(json.dumps(config))
                    initialize("--initialize-lifecycle", node_root / "ledger", args.binary.resolve())
                    port = free_port()
                    env = {**fixture.env, **node_env(node_root, port, "127.0.0.0/8"),
                           "SESSIONDOCK_NATIVE_SDKS": "auto" if enabled else "off",
                           "SESSIONDOCK_LIFECYCLE_DIR": str(node_root / "ledger"),
                           "SESSIONDOCK_STATE_DIR": str(node_root / "state"),
                           "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir)}
                    stack.enter_context(private_hosts(fixture.host_dir))
                    stack.enter_context(server(SimpleNamespace(root=node_root), args.binary, "off", env))
                    nodes.append(SimpleNamespace(name=name, token=TOKEN, port=port))
                hub_root = root / "hub"
                hub_root.mkdir()
                hub = Hub((args.hub_binary or args.binary.with_name("sessiondock-hub")).resolve(), hub_root, nodes)
                hub.start()
                stack.callback(hub.stop)
                with sync_playwright() as playwright:
                    browser = playwright.chromium.launch(headless=True)
                    try:
                        combinations = [(1280, "light")] if args.expect_missing_sdk else [(1280, "light"), (390, "dark"), (1280, "dark"), (390, "light")]
                        for width, theme in combinations:
                            print(f"Kiro Hub browser: {width} {theme}", flush=True)
                            exercise(browser, f"http://127.0.0.1:{hub.port}", fixtures[0], fixtures[1], width, theme, args.screenshots, args.expect_missing_sdk)
                    finally:
                        browser.close()
        finally:
            for fixture in fixtures:
                stop_private_sdkhosts(fixture.root)
    print("Kiro Hub missing SDK reproduced" if args.expect_missing_sdk else "Kiro Hub SDK/CLI browser: PASS")


if __name__ == "__main__":
    main()
