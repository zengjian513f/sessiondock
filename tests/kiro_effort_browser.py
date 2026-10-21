#!/usr/bin/env python3
"""Kiro native effort capability and stale-value handling through Chromium."""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace

from kiro_sdk_fixtures import FakeKiroAcp, KiroFixture


class EffortPeer(FakeKiroAcp):
    def handle(self, message):
        # Native normalization can remove effort while acknowledging an effort
        # request; the UI must retire that pending selection before the next send.
        if message.get("method") == "session/set_config_option" and message.get("params", {}).get("value") == "max":
            self.model = "auto"
        super().handle(message)

    def run_prompt(self, message):
        self.update("agent_message_chunk", content={"type": "text", "text": "Synthetic no-effort reply"})
        self.reply(message, {"stopReason": "end_turn"})

    def config_options(self):
        options = super().config_options()
        for option in options:
            if option["id"] == "model":
                for model in option["options"]:
                    model["_meta"] = {"kiro": {"hasEffort": model["value"] != "auto"}}
        # The peer deliberately retains self.effort when the model changes,
        # while its native config no longer advertises that setting.
        return [option for option in options if self.model != "auto" or option["id"] != "effortLevel"]


def fake_main():
    if "--list-models" in sys.argv:
        print(json.dumps({"models": [
            {"model_id": "auto", "model_name": "Auto", "hasEffort": False},
            {"model_id": "claude-haiku-4.5", "model_name": "Claude Haiku 4.5",
             "efforts": ["low", "medium", "high"], "default_effort": "low"},
            {"model_id": "unknown-effort", "model_name": "No effort advertised"},
        ], "default_model": "auto"}))
    elif "acp" in sys.argv:
        peer = EffortPeer()
        for line in sys.stdin:
            if line.strip():
                peer.handle(json.loads(line))
    else:
        from kiro_sdk_fixtures import main
        sys.argv.remove("--fake-effort")
        main()


def run(args):
    from playwright.sync_api import expect, sync_playwright
    from codex_sdk_browser import server
    from kiro_sdk_browser import api, api_raw, watch
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize

    with tempfile.TemporaryDirectory(prefix="sessiondock-kiro-effort-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok", "state", "trash"):
            (root / name).mkdir()
        fixture = KiroFixture(root, args.binary.resolve().with_name("ptyhost"))
        config = json.loads(fixture.launcher.read_text())
        config["profiles"][0]["args"] = [str(Path(__file__).resolve()), "--fake-effort"]
        fixture.launcher.write_text(json.dumps(config))
        initialize("--initialize-lifecycle", root / "ledger", args.binary.resolve())
        env = {**fixture.env, "SESSIONDOCK_NATIVE_SDKS": "auto", "SESSIONDOCK_STATE_DIR": str(root / "state"),
               "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"), "SESSIONDOCK_TRASH_DIR": str(root / "trash"),
               "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir)}
        try:
            with server(SimpleNamespace(root=root), args.binary, "off", env) as base, sync_playwright() as playwright:
                browser = playwright.chromium.launch(headless=True)
                page = browser.new_page(viewport={"width": 1280, "height": 900})
                page.set_default_timeout(20000)
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base, wait_until="domcontentloaded")
                page.wait_for_function("T.listLoaded")
                page.wait_for_function("SessionDockCapabilities.config.native_sdks?.kiro === true")
                page.locator("#new-session").click()
                page.locator("label:has(input[name=new-source][value=kiro])").click()
                expect(page.locator("#new-model")).to_be_enabled()

                def choose_new(model):
                    page.locator("#new-model").click()
                    page.locator(f'#new-model-options [title="{model}"]').click()

                # Both explicit false and absence of advertised choices must
                # remain unavailable even after choosing high on another model.
                choose_new("claude-haiku-4.5")
                page.locator("#new-effort").select_option("high")
                choose_new("unknown-effort")
                expect(page.locator("#new-effort")).to_be_disabled()
                choose_new("auto")
                expect(page.locator("#new-effort")).to_be_disabled()
                assert page.locator("#new-effort option").count() == 0
                page.locator("#new-cwd").fill(str(fixture.work))
                with page.expect_response(lambda response: response.url.endswith("/api/native-sdk/create")) as response:
                    page.locator("#new-session-go").click()
                assert response.value.ok, response.value.text()
                receipt = response.value.json()
                uid = receipt["uid"]
                assert "effort" not in response.value.request.post_data_json
                expect(page.locator("#sdk-input")).to_be_visible()
                watch(page, uid)
                page.wait_for_function("window.__kiroSnapshot.model === 'auto'")
                expect(page.locator("#sdk-effort")).to_be_hidden()
                assert page.evaluate("window.__kiroSnapshot.effort") is None
                assert page.evaluate("window.__kiroSnapshot.capabilities.effort") is False
                native_new = fixture.received("session/new")[-1]
                assert "effortLevel" not in native_new["params"]["_meta"]["kiro"]

                # A supported model can still set high. Switching back drops
                # the formerly confirmed value in snapshots and the picker.
                page.locator("#sdk-model").select_option("claude-haiku-4.5")
                page.wait_for_function("window.__kiroSnapshot.model === 'claude-haiku-4.5'")
                expect(page.locator("#sdk-effort")).to_be_visible()
                expect(page.locator("#sdk-effort")).to_be_enabled()
                page.locator("#sdk-effort").select_option("high")
                page.wait_for_function("window.__kiroSnapshot.effort === 'high'")
                expect(page.locator("#sdk-effort")).to_be_enabled()
                # The snapshot can confirm before the settings receipt. An idle
                # session must not keep the receipt text until a later snapshot.
                expect(page.locator(".native-sdk-pane .sdk-status")).to_have_attribute("title", "就绪", timeout=5000)
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "kiro-effort-confirmed-1280.png"), full_page=True)
                page.locator("#sdk-effort").select_option("max")
                page.wait_for_function("window.__kiroSnapshot.model === 'auto' && window.__kiroSnapshot.effort === null")
                expect(page.locator("#sdk-effort")).to_be_hidden()
                before = len(fixture.received("session/set_config_option"))
                rejected = api_raw(page, "settings", {"uid": uid, "effort": "high"})
                assert rejected["status"] >= 400 and "kiro_sdk_setting_unavailable" in json.dumps(rejected), rejected
                assert len(fixture.received("session/set_config_option")) == before
                models = api(page, "models", {"uid": uid})["raw"]
                assert models["effort"] is None, models
                auto = next(model for model in models["models"] if model["id"] == "auto")
                assert auto["efforts"] == [] and auto["hasEffort"] is False, auto
                page.locator("#sdk-input").fill("Please confirm this synthetic conversation is ready.")
                page.locator("#sdk-send").click()
                page.wait_for_function("window.__kiroSnapshot.items.some(item => item.text?.includes('Synthetic no-effort reply'))")
                page.wait_for_function("!window.__kiroSnapshot.running")
                page.locator("#sdk-model").select_option("claude-haiku-4.5")
                page.wait_for_function("window.__kiroSnapshot.model === 'claude-haiku-4.5'")
                expect(page.locator("#sdk-effort")).to_be_visible()
                expect(page.locator("#sdk-effort")).to_have_value("max")
                expect(page.locator("#sdk-model")).to_be_enabled()
                page.locator("#sdk-model").select_option("auto")
                page.wait_for_function("window.__kiroSnapshot.model === 'auto' && window.__kiroSnapshot.effort === null")

                # Refresh restores the checkpoint without reviving old effort.
                page.reload(wait_until="domcontentloaded")
                page.wait_for_function("T.listLoaded")
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                expect(page.locator("#sdk-input")).to_be_visible()
                expect(page.locator("#sdk-effort")).to_be_hidden()
                if args.screenshots:
                    for width in (1280, 390):
                        page.set_viewport_size({"width": width, "height": 900})
                        if width < 500:
                            page.evaluate("showMobileDetail()")
                        for theme in ("light", "dark"):
                            page.emulate_media(color_scheme=theme)
                            page.screenshot(path=str(args.screenshots / f"kiro-no-effort-{width}-{theme}.png"), full_page=True)
                assert not errors, errors
                assert len(fixture.received("session/prompt")) == 1
                browser.close()
        finally:
            stop_private_sdkhosts(root)
    print("Kiro effort capability browser: PASS")


def main():
    if "--fake-effort" in sys.argv:
        return fake_main()
    from history_fixtures import BINARY
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    run(args)


if __name__ == "__main__":
    main()
