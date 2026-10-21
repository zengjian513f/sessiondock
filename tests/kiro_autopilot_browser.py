#!/usr/bin/env python3
"""Kiro default Autopilot, explicit opt-out and durable reconnect via node and Hub."""
from __future__ import annotations
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace


def fake_main():
    from kiro_sdk_fixtures import FakeKiroAcp
    if "acp" not in sys.argv:
        from kiro_sdk_fixtures import main
        sys.argv.remove("--fake-autopilot")
        return main()

    class Peer(FakeKiroAcp):
        def config_options(self):
            options = super().config_options()
            if os.environ.get("KIRO_TEST_AUTOPILOT") == "missing":
                options = [option for option in options if option["id"] != "autopilot"]
            return options

        def handle(self, message):
            behavior = os.environ.get("KIRO_TEST_AUTOPILOT")
            if behavior in ("refuse", "unchanged") and message.get("method") == "session/set_config_option" and message.get("params", {}).get("configId") == "autopilot":
                with self.log.open("a") as output:
                    output.write(json.dumps(message) + "\n")
                if behavior == "refuse":
                    self.error(message, -32000, "Synthetic native autopilot refusal")
                else:
                    self.reply(message, {"configOptions": self.config_options()})
                return
            super().handle(message)

    peer = Peer()
    for line in sys.stdin:
        if line.strip():
            peer.handle(json.loads(line))


def negative(browser, fixture, restart, behavior, screenshots=None):
    from playwright.sync_api import expect
    page = browser.new_page(viewport={"width": 1280, "height": 900})
    page.set_default_timeout(20000)
    page.goto(restart(), wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(row => row.sid === sid); }", arg=fixture.sid, polling=300)
    uid = page.evaluate("sid => S.sessions.find(row => row.sid === sid).uid", fixture.sid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    assert not fixture.received("session/set_config_option"), "Read-only selection must not change permissions"
    if behavior in ("explicit-open", "explicit-setting"):
        from kiro_sdk_browser import api_raw
        action = "open" if behavior == "explicit-open" else "settings"
        payload = {"uid": uid, **({"autopilot": False} if behavior == "explicit-open" else {"permission_mode": "default"})}
        result = api_raw(page, action, payload)
        assert result["status"] == 200 and result["body"]["raw"]["autopilot"] is False, result
        writes = fixture.received("session/set_config_option")
        assert all(row["params"].get("value") != "on" for row in writes if row["params"].get("configId") == "autopilot"), writes
        page.locator("#a-sdk").click()
        page.wait_for_function("SdkView.connected && SdkView.raw?.autopilot === false")
        expect(page.locator("#native-sdk-permission_mode")).to_have_value("default")
        page.close()
        print(f"Kiro {behavior} opt-out never transiently enables Autopilot: PASS", flush=True)
        return
    if behavior == "limited-cli":
        with page.expect_response(lambda response: response.url.endswith("/api/term/takeover")) as result:
            page.locator("#a-term").click()
        assert result.value.ok and result.value.json().get("state") != "failed", result.value.text()
        page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
        page.wait_for_function("Array.from({length:T.term.buffer.active.length},(_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('Kiro CLI v3 Interactive Terminal')")
        launched = [row["fixture_cli_start"] for row in fixture.received() if "fixture_cli_start" in row][-1]
        assert "--trust-tools=read_file" in launched["argv"] and "--trust-all-tools" not in launched["argv"], launched
        page.close()
        print("Kiro explicit CLI trust-tools profile preserved: PASS", flush=True)
        return
    page.locator("#a-sdk").click()
    page.wait_for_function("SdkView.pane && !SdkView.historyReady")
    with page.expect_response(lambda response: response.url.endswith("/api/native-sdk/open")) as result:
        page.locator("#sdk-input").click()
    if behavior in ("refuse", "unchanged"):
        assert not result.value.ok, result.value.text()
        if behavior == "refuse":
            assert "Synthetic native autopilot refusal" in result.value.text()
            expect(page.locator("#sdk-composer .sdk-status")).to_have_attribute("data-state", "blocked")
            page.locator("#sdk-composer .sdk-status").click()
            expect(page.locator("#sdk-status-menu")).to_be_visible()
            expect(page.locator("#sdk-status-menu")).to_contain_text("Synthetic native autopilot refusal")
        writes = fixture.received("session/set_config_option")
        assert len(writes) == 1 and writes[0]["params"]["value"] == "on", writes
    else:
        assert result.value.ok, result.value.text()
        page.wait_for_function("SdkView.connected && !SdkView.readOnly")
        assert not fixture.received("session/set_config_option"), "Missing native control must not be fabricated"
    assert page.evaluate("SdkView.raw?.autopilot !== true")
    assert not fixture.received("session/prompt")
    if screenshots:
        page.screenshot(path=str(screenshots / f"kiro-autopilot-{behavior}.png"), full_page=True)
    page.close()
    print(f"Kiro Autopilot {behavior}: no false confirmation PASS", flush=True)


def exercise(browser, fixture, restart, hub_mode, screenshots):
    from playwright.sync_api import expect
    from new_session_model_browser import open_dialog
    from sdk_ui_fixtures import start_sdk

    label = "hub" if hub_mode else "node"
    width, theme = (390, "dark") if hub_mode else (1280, "light")
    context = browser.new_context(viewport={"width": width, "height": 900}, color_scheme=theme, service_workers="block")
    page = context.new_page()
    page.set_default_timeout(20000)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))

    def native_settings():
        return [row for row in fixture.received("session/set_config_option") if row["params"]["configId"] == "autopilot"]

    def navigate(base, uid=None):
        page.goto(base, wait_until="domcontentloaded")
        page.wait_for_function("T.listLoaded")
        page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(row => row.sid === sid); }", arg=fixture.sid, polling=300)
        selected = uid or page.evaluate("sid => S.sessions.find(row => row.sid === sid).uid", fixture.sid)
        # A Hub restart keeps the origin and can restore the selected mobile
        # detail automatically; its sidebar is intentionally hidden then.
        if page.evaluate("S.sel") != selected:
            if page.locator("#detail .mobile-back").is_visible():
                page.locator("#detail .mobile-back").click()
            page.locator(f'#side .item[data-uid="{selected}"]').click()
        expect(page.locator("#a-sdk")).to_be_visible()
        return selected

    def confirmed(value):
        try:
            page.wait_for_function("value => SdkView.connected && !SdkView.readOnly && SdkView.raw?.autopilot === value", arg=value)
        except Exception:
            print("Autopilot state diagnostics", json.dumps(page.evaluate("""async () => ({selected:S.sel,
                uid:SdkView.uid,connected:SdkView.connected,readOnly:SdkView.readOnly,selection:SdkView.selection,
                autopilot:SdkView.raw?.autopilot,raw:SdkView.raw,capabilities:SessionDockCapabilities.config,
                nodes:typeof Nodes === 'undefined' ? null : Nodes.capabilities,
                live:await (await fetch('/api/live?force=1')).json()})"""), ensure_ascii=False), flush=True)
            if screenshots:
                page.screenshot(path=str(screenshots / f"kiro-autopilot-{label}-failure.png"), full_page=True)
            raise
        expect(page.locator("#native-sdk-permission_mode")).to_have_value("autopilot" if value else "default")
        assert page.evaluate("SdkView.raw.configOptions.find(option => option.id === 'autopilot').currentValue") == ("on" if value else "off")

    def capture(state):
        if not screenshots:
            return
        control = page.locator("#native-sdk-permission_mode")
        if not control.is_visible():
            page.locator("#sdk-more").click()
        expect(control).to_be_visible()
        page.screenshot(path=str(screenshots / f"kiro-autopilot-{label}-{state}-{width}-{theme}.png"), full_page=True)
        if page.locator("#sdk-more-menu").is_visible():
            page.locator("#sdk-more").click()

    def stop():
        button = page.locator('#a-session-toggle[aria-label="停止会话"]')
        expect(button).to_be_enabled()
        if not button.is_visible():
            page.locator("#a-more").click()
        with page.expect_response(lambda response: response.url.endswith(("/api/session/stop", "/api/native-sdk/stop")) and response.request.method == "POST") as result:
            button.click()
            popup = page.locator("dialog.app-popup")
            if popup.is_visible():
                popup.get_by_role("button", name="确定", exact=True).click()
        assert result.value.ok, result.value.text()

    base = restart()
    uid = navigate(base)
    assert not native_settings(), "Read-only selection must not change permissions"
    page.locator("#a-sdk").click()
    start_sdk(page)
    confirmed(True)
    settings = native_settings()
    assert len(settings) == 1 and settings[0]["params"]["value"] == "on", settings
    assert settings[0]["params"]["sessionId"] == fixture.sid
    capture("loaded-on")

    def opt_out_and_reconnect(current_uid, name):
        nonlocal base
        control = page.locator("#native-sdk-permission_mode")
        if not control.is_visible():
            page.locator("#sdk-more").click()
        expect(control).to_be_enabled()
        with page.expect_response(lambda response: response.url.endswith("/api/native-sdk/settings")) as result:
            control.select_option("default")
        assert result.value.ok, result.value.text()
        confirmed(False)
        writes = native_settings()
        assert writes[-1]["params"]["value"] == "off", writes
        loads = len(fixture.received("session/load"))
        new_sessions = len(fixture.received("session/new"))
        # The app can restore a Kiro SDK selection before deferred term.js
        # supplies Hub node capabilities. Delay the real asset once to make
        # that ordering deterministic without replacing capability responses.
        held_terminal_scripts = []
        def delayed_terminal_script(route):
            held_terminal_scripts.append(route)
        delayed = hub_mode and name == "loaded"
        if delayed:
            page.route("**/term.js*", delayed_terminal_script)
        page.reload(wait_until="commit" if delayed else "domcontentloaded")
        if delayed:
            page.wait_for_function("typeof T === 'undefined' && typeof SdkView !== 'undefined' && SdkView.selection?.mode === 'loading'")
            assert held_terminal_scripts, "term.js must still be held while the SDK selection waits"
            for route in held_terminal_scripts:
                route.continue_()
            page.wait_for_load_state("domcontentloaded")
            page.unroute("**/term.js*", delayed_terminal_script)
        confirmed(False)
        assert native_settings() == writes, "Page reconnect reapplied the default over the user's opt-out"
        page.goto("about:blank")
        base = restart()
        navigate(base, current_uid)
        confirmed(False)
        assert native_settings() == writes, "Service recovery reapplied the default over the user's opt-out"
        assert len(fixture.received("session/load")) == loads, "Recovery issued a fresh native load"
        assert len(fixture.received("session/new")) == new_sessions, "Recovery created a replacement native session"
        capture(name + "-recovered-off")

    opt_out_and_reconnect(uid, "loaded")
    stop()
    expect(page.locator("#a-term")).to_be_enabled()
    with page.expect_response(lambda response: response.url.endswith("/api/term/takeover")) as result:
        page.locator("#a-term").click()
    assert result.value.ok and result.value.json().get("state") != "failed", result.value.text()
    expect(page.locator("#termpane")).to_be_visible(timeout=20000)
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    page.wait_for_function("Array.from({length:T.term.buffer.active.length},(_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('Kiro CLI v3 Interactive Terminal')")
    cli = [row["fixture_cli_start"] for row in fixture.received() if "fixture_cli_start" in row][-1]
    assert cli["sessionId"] == fixture.sid and "--trust-all-tools" in cli["argv"], cli
    stop()
    before = len(native_settings())
    page.locator("#a-sdk").click()
    start_sdk(page)
    confirmed(True)
    assert len(native_settings()) == before + 1 and native_settings()[-1]["params"]["value"] == "on"
    stop()

    if page.locator("#detail .mobile-back").is_visible():
        page.locator("#detail .mobile-back").click()
    open_dialog(page)
    if hub_mode:
        page.locator("#new-node").select_option(page.evaluate("uid => nodeOf(uid)", uid))
    page.locator('label:has(input[name="new-source"][value="kiro"])').click()
    expect(page.locator("#new-model")).to_be_enabled()
    page.locator("#new-cwd").fill(str(fixture.work))
    before = len(native_settings())
    with page.expect_response(lambda response: response.url.endswith("/api/native-sdk/create")) as result:
        page.locator("#new-session-go").click()
    assert result.value.ok, result.value.text()
    created = result.value.json()
    assert not result.value.request.post_data_json.get("autopilot"), "Test must exercise the default, not an explicit override"
    confirmed(True)
    assert len(native_settings()) == before + 1 and native_settings()[-1]["params"]["value"] == "on"
    assert native_settings()[-1]["params"]["sessionId"] == created["session"]["sid"]
    capture("created-on")
    opt_out_and_reconnect(created["uid"], "created")
    stop()

    # Explicit creation overrides also participate in the durable receipt key.
    from kiro_sdk_browser import api_raw
    explicit = {"source": "kiro", "cwd": str(fixture.work), "request_id": f"autopilot-explicit-{label}", "autopilot": False}
    if hub_mode:
        explicit["_node"] = page.evaluate("uid => nodeOf(uid)", uid)
    receipt = api_raw(page, "create", explicit)
    assert receipt["status"] == 200, receipt
    explicit_uid = receipt["body"]["uid"]
    opened = api_raw(page, "open", {"uid": explicit_uid})
    assert opened["status"] == 200 and opened["body"]["raw"]["autopilot"] is False, opened
    assert not any(row["params"]["sessionId"] == opened["body"]["session_id"] and row["params"]["value"] == "on" for row in native_settings())
    creations = len(fixture.received("session/new"))
    conflict = api_raw(page, "create", {**explicit, "permission_mode": "autopilot"})
    assert conflict["status"] >= 400, conflict
    assert len(fixture.received("session/new")) == creations
    assert api_raw(page, "stop", {"uid": explicit_uid})["status"] == 200

    # The same default must reach a new CLI launch, not only resume takeover.
    if page.locator("#detail .mobile-back").is_visible():
        page.locator("#detail .mobile-back").click()
    if not page.locator("#settings").is_visible():
        page.locator("#header-more-btn").click()
    page.locator("#settings").click()
    page.locator('.settings-tab[data-tab="features"]').click()
    page.locator('[data-agent-start-mode="kiro"]').select_option("cli")
    page.locator("#settings-dialog .modal-close").click()
    open_dialog(page)
    if hub_mode:
        page.locator("#new-node").select_option(page.evaluate("uid => nodeOf(uid)", uid))
    page.locator('label:has(input[name="new-source"][value="kiro"])').click()
    page.locator("#new-cwd").fill(str(fixture.work))
    with page.expect_response(lambda response: response.url.endswith("/api/term/create")) as result:
        page.locator("#new-session-go").click()
    assert result.value.ok and result.value.json().get("state") != "failed", result.value.text()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    page.wait_for_function("Array.from({length:T.term.buffer.active.length},(_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('Kiro CLI v3 Interactive Terminal')")
    launched = [row["fixture_cli_start"] for row in fixture.received() if "fixture_cli_start" in row][-1]
    assert launched["sessionId"] != fixture.sid and "--trust-all-tools" in launched["argv"], launched
    stop()
    assert not fixture.received("session/prompt"), "Permission defaults must not trigger a model request"
    assert not errors, errors
    context.close()
    print(f"Kiro Autopilot {label}: load/create on, explicit off survives refresh/service restart, new host on, CLI trust flag PASS", flush=True)


def main():
    if "--fake-autopilot" in sys.argv:
        return fake_main()
    from playwright.sync_api import sync_playwright
    from codex_sdk_browser import server
    from history_fixtures import BINARY, REPO
    from hub_fixtures import Hub, free_port
    from kiro_sdk_fixtures import KiroFixture
    from node_auth_fixtures import TOKEN, node_env
    from private_hosts import private_hosts
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--hub-binary", type=Path)
    parser.add_argument("--screenshots", type=Path)
    parser.add_argument("--only", action="append", choices=["node", "hub", "refuse", "unchanged", "missing", "limited-cli", "explicit-open", "explicit-setting"])
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    def host_binary(name):
        sibling = args.binary.with_name(name)
        return (sibling if sibling.is_file() else REPO / "target" / "debug" / name).resolve(strict=True)
    ptyhost_binary = host_binary("ptyhost")
    sdkhost_binary = host_binary("sdkhost")
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        try:
            for hub_mode, behavior in ((False, "default"), (True, "default"), (False, "refuse"), (False, "unchanged"), (False, "missing"), (False, "limited-cli"), (False, "explicit-open"), (False, "explicit-setting")):
                case = ("hub" if hub_mode else "node") if behavior == "default" else behavior
                if args.only and case not in args.only:
                    continue
                with tempfile.TemporaryDirectory(prefix="sd-kap-") as temporary:
                    root = Path(temporary)
                    for name in ("claude", "codex", "grok", "state"):
                        (root / name).mkdir()
                    fixture = KiroFixture(root, ptyhost_binary)
                    config = json.loads(fixture.launcher.read_text())
                    config["profiles"][0]["args"] = [str(Path(__file__).resolve()), "--fake-autopilot"]
                    config["profiles"][0]["env"]["KIRO_TEST_AUTOPILOT"] = behavior
                    if behavior == "limited-cli":
                        config["profiles"][0]["resume_args"].append("--trust-tools=read_file")
                    fixture.launcher.write_text(json.dumps(config))
                    initialize("--initialize-lifecycle", root / "ledger", args.binary.resolve())
                    port = free_port()
                    env = {**fixture.env, **node_env(root, port, "127.0.0.0/8"),
                           "SESSIONDOCK_NATIVE_SDKS": "auto", "SESSIONDOCK_STATE_DIR": str(root / "state"),
                           "SESSIONDOCK_SDKHOST_BIN": str(sdkhost_binary),
                           "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"), "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir)}
                    node = ExitStack()
                    hub = None
                    try:
                        with private_hosts(fixture.host_dir):
                            def restart():
                                nonlocal node, hub
                                node.close()
                                node = ExitStack()
                                direct = node.enter_context(server(SimpleNamespace(root=root), args.binary, "off", env))
                                if not hub_mode:
                                    return direct
                                if hub is None:
                                    hub_root = root / "hub"
                                    hub_root.mkdir()
                                    hub = Hub((args.hub_binary or args.binary.with_name("sessiondock-hub")).resolve(), hub_root,
                                              [SimpleNamespace(name="KiroAutopilot", token=TOKEN, port=port)])
                                    hub.start()
                                return f"http://127.0.0.1:{hub.port}"
                            if behavior == "default":
                                exercise(browser, fixture, restart, hub_mode, args.screenshots)
                            else:
                                negative(browser, fixture, restart, behavior, args.screenshots)
                    finally:
                        if hub:
                            hub.stop()
                        node.close()
                        stop_private_sdkhosts(root)
        finally:
            browser.close()
    print("Kiro default Autopilot browser: PASS" + (" (" + ", ".join(args.only) + ")" if args.only else ""))


if __name__ == "__main__":
    main()
