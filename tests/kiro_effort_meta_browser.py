#!/usr/bin/env python3
"""Kiro per-model effort (`_meta.kiro.effortLevels`) through Chromium.

Regression for BUG-20261021-210732: the real Kiro CLI advertises effort only
inside each model option's `_meta.kiro` (effortLevels / defaultEffortLevel /
hasEffort) and exposes no standalone effort config option, and its
`chat --list-models` carries no effort fields at all. The earlier synthetic
fixture used a separate `effortLevel` option, so the new-session/bug-report
picker and the live footer silently offered no effort. This suite reproduces
the native schema and asserts effort surfaces on every picker.
"""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace

from kiro_sdk_fixtures import FakeKiroAcp, KiroFixture

# Native per-model effort metadata, mirroring kiro-cli 2.24.1 configOptions.
META_MODELS = [
    {"value": "auto", "name": "Auto", "description": "Auto model",
     "_meta": {"kiro": {"hasEffort": False}}},
    {"value": "claude-opus-5.5", "name": "Claude Opus 5.5", "description": "Opus 5.5",
     "_meta": {"kiro": {"hasEffort": True, "defaultEffortLevel": "medium",
                        "effortLevels": ["low", "medium", "high", "xhigh", "max"],
                        "effortSchemaPath": "output_config"}}},
    {"value": "claude-sonnet-4.5", "name": "Claude Sonnet 4.5", "description": "Sonnet 4.5",
     "_meta": {"kiro": {"hasEffort": False}}},
]


class MetaEffortPeer(FakeKiroAcp):
    def __init__(self, *posargs, **kwargs):
        super().__init__(*posargs, **kwargs)
        # Default to a model present in the native per-model schema below.
        self.model = "auto"
        self.effort = ""

    def config_options(self):
        options = super().config_options()
        # Replace the model option's choices with the native per-model effort
        # schema and drop the standalone effortLevel option entirely.
        result = []
        for option in options:
            if option["id"] == "effortLevel":
                continue
            if option["id"] == "model":
                option = {**option, "options": [dict(model) for model in META_MODELS]}
            result.append(option)
        return result

    def run_prompt(self, message):
        self.update("agent_message_chunk", content={"type": "text", "text": "Synthetic meta-effort reply"})
        self.reply(message, {"stopReason": "end_turn"})


def fake_main():
    if "--list-models" in sys.argv:
        # The real CLI catalog has no effort fields; effort comes only from the
        # live ACP configOptions. Mirror that so the picker must reuse the
        # running session rather than invent effort choices.
        print(json.dumps({"models": [
            {"model_id": model["value"], "model_name": model["name"]} for model in META_MODELS
        ], "default_model": "auto"}))
    elif "acp" in sys.argv:
        peer = MetaEffortPeer()
        for line in sys.stdin:
            if line.strip():
                peer.handle(json.loads(line))
    else:
        from kiro_sdk_fixtures import main
        sys.argv.remove("--fake-meta-effort")
        main()


def run(args):
    from playwright.sync_api import expect, sync_playwright
    from codex_sdk_browser import server
    from kiro_sdk_browser import api, api_raw, watch
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize

    with tempfile.TemporaryDirectory(prefix="sessiondock-kiro-meta-effort-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok", "state", "trash"):
            (root / name).mkdir()
        fixture = KiroFixture(root, args.binary.resolve().with_name("ptyhost"))
        config = json.loads(fixture.launcher.read_text())
        config["profiles"][0]["args"] = [str(Path(__file__).resolve()), "--fake-meta-effort"]
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

                # Create a live Kiro session on an effort-bearing model so later
                # pickers can reuse its native configOptions.
                page.locator("#new-session").click()
                page.locator("label:has(input[name=new-source][value=kiro])").click()
                expect(page.locator("#new-model")).to_be_enabled()
                page.locator("#new-cwd").fill(str(fixture.work))
                with page.expect_response(lambda response: response.url.endswith("/api/native-sdk/create")) as created:
                    page.locator("#new-session-go").click()
                assert created.value.ok, created.value.text()
                uid = created.value.json()["uid"]
                expect(page.locator("#sdk-input")).to_be_visible()
                watch(page, uid)
                page.wait_for_function("window.__kiroSnapshot.model === 'auto'")
                # auto advertises hasEffort:false -> no effort on the live footer.
                expect(page.locator("#sdk-effort")).to_be_hidden()
                assert page.evaluate("window.__kiroSnapshot.capabilities.effort") is False

                # Switch to the effort-bearing model: the live footer must now
                # surface the native effortLevels and accept a change.
                page.locator("#sdk-model").select_option("claude-opus-5.5")
                page.wait_for_function("window.__kiroSnapshot.model === 'claude-opus-5.5'")
                page.wait_for_function("window.__kiroSnapshot.capabilities.effort === true")
                expect(page.locator("#sdk-effort")).to_be_visible()
                expect(page.locator("#sdk-effort")).to_be_enabled()
                assert page.locator("#sdk-effort option[value='xhigh']").count() == 1
                # A per-model effort set is accepted natively (configId
                # effortLevel), not rejected as unavailable.
                accepted = api(page, "settings", {"uid": uid, "effort": "xhigh"})
                assert accepted.get("ok") is not False, accepted
                set_calls = fixture.received("session/set_config_option")
                assert any(call["params"].get("configId") == "effortLevel"
                           and call["params"].get("value") == "xhigh" for call in set_calls), set_calls

                # The models endpoint must carry per-model effortLevels now.
                models = api(page, "models", {"uid": uid})["raw"]
                opus = next(model for model in models["models"] if model["id"] == "claude-opus-5.5")
                assert opus["efforts"] == ["low", "medium", "high", "xhigh", "max"], opus
                assert opus["supportedReasoningEfforts"] == opus["efforts"], opus
                assert opus["hasEffort"] is True, opus
                assert opus["default_effort"] == "medium", opus
                auto = next(model for model in models["models"] if model["id"] == "auto")
                assert auto["efforts"] == [] and auto["hasEffort"] is False, auto

                # Setting effort on a hasEffort:false model is rejected natively.
                page.locator("#sdk-model").select_option("auto")
                page.wait_for_function("window.__kiroSnapshot.model === 'auto'")
                rejected = api_raw(page, "settings", {"uid": uid, "effort": "high"})
                assert rejected["status"] >= 400 and "kiro_sdk_setting_unavailable" in json.dumps(rejected), rejected
                page.locator("#sdk-model").select_option("claude-opus-5.5")
                page.wait_for_function("window.__kiroSnapshot.capabilities.effort === true")

                # The NEW-SESSION picker reuses the running session's native
                # configOptions, so it offers effort for the effort model.
                page.locator("#new-session").click()
                page.locator("label:has(input[name=new-source][value=kiro])").click()
                expect(page.locator("#new-model")).to_be_enabled()

                def choose_new(model):
                    page.locator("#new-model").click()
                    page.locator(f'#new-model-options [title="{model}"]').click()

                choose_new("claude-opus-5.5")
                expect(page.locator("#new-effort")).to_be_enabled()
                assert page.locator("#new-effort option[value='max']").count() == 1
                if args.screenshots:
                    for width in (1280, 390):
                        page.set_viewport_size({"width": width, "height": 900})
                        for theme in ("light", "dark"):
                            page.emulate_media(color_scheme=theme)
                            page.screenshot(path=str(args.screenshots / f"kiro-meta-effort-newsession-{width}-{theme}.png"), full_page=True)
                    page.set_viewport_size({"width": 1280, "height": 900})
                    page.emulate_media(color_scheme="light")
                choose_new("auto")
                expect(page.locator("#new-effort")).to_be_disabled()
                assert page.locator("#new-effort option").count() == 0
                choose_new("claude-sonnet-4.5")
                expect(page.locator("#new-effort")).to_be_disabled()
                page.locator("#new-session-dialog .modal-close").click()

                # The BUG-REPORT picker uses the same factory and must behave
                # identically for the effort-bearing model.
                if page.evaluate("SessionDockCapabilities.config.bug_report_worker === true"):
                    page.locator("[data-report-bug]").first.click()
                    expect(page.locator("#bug-report-dialog")).to_be_visible()
                    page.locator("label:has(#bug-report-source input[value=kiro])").click()
                    expect(page.locator("#bug-report-model")).to_be_enabled()

                    def choose_report(model):
                        page.locator("#bug-report-model").click()
                        page.locator(f'#bug-report-model-options [title="{model}"]').click()

                    choose_report("claude-opus-5.5")
                    expect(page.locator("#bug-report-effort")).to_be_enabled()
                    assert page.locator("#bug-report-effort option[value='high']").count() == 1
                    choose_report("auto")
                    expect(page.locator("#bug-report-effort")).to_be_disabled()
                    if args.screenshots:
                        choose_report("claude-opus-5.5")
                        page.screenshot(path=str(args.screenshots / "kiro-meta-effort-bugreport-1280.png"), full_page=True)
                    page.locator("#bug-report-dialog .modal-close").click()

                assert not errors, errors
                browser.close()
        finally:
            stop_private_sdkhosts(root)
    print("Kiro per-model effort browser: PASS")


def main():
    if "--fake-meta-effort" in sys.argv:
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
