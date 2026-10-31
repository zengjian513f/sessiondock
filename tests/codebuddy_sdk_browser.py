#!/usr/bin/env python3
"""CodeBuddy native SDK browser contract, using a private fake CLI only.

Requires an already built SessionDock binary. This suite never builds Rust,
calls a real provider or reads a real CLI home. The peer reproduces the
CodeBuddy 2.175.0 protocol facts recorded in docs/codebuddy-sdk.md: history
replay after `initialize`, a CLI-minted user echo uuid and `allowed`-style
permission answers.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import tempfile
import time

from codebuddy_sdk_fixtures import SECRET, CodebuddyFixture, native_uid, project_key
from claude_sdk_browser import post, receipt, snapshot, wait_snapshot, open_sdk, send
from history_fixtures import BINARY


def user_prompts(fixture, text):
    return [frame for frame in fixture.frames("user") if frame["message"]["content"] == text]


def browser_check(fixture, base, screenshots):
    from playwright.sync_api import sync_playwright, expect

    def capture(page, name):
        if screenshots:
            screenshots.mkdir(parents=True, exist_ok=True)
            page.screenshot(path=str(screenshots / name), full_page=True)

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True, args=["--no-sandbox"])
        context = browser.new_context(viewport={"width": 1280, "height": 900}, color_scheme="light", service_workers="block")
        page = context.new_page()
        errors, turns, settings = [], [], []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.on("request", lambda request: turns.append(request.post_data_json)
                if request.method == "POST" and "/api/native-sdk/turn" in request.url else None)
        page.on("request", lambda request: settings.append(request.post_data_json)
                if request.method == "POST" and "/api/native-sdk/settings" in request.url else None)
        page.goto(base, wait_until="networkidle")

        # The seeded native transcript is listed as a CodeBuddy session from the read model.
        row = page.locator(f'#side .item[data-uid="{fixture.uid}"]')
        expect(row).to_be_visible()
        expect(row).to_contain_text("earlier synthetic CodeBuddy prompt")
        # A CLI installed or repaired after the node started shows up in the live node capability
        # (term/list) but never in the startup snapshot. SDK availability, the SDK model catalog
        # and its effort choices must follow the live value.
        page.evaluate("SessionDockCapabilities.config.native_sdks.codebuddy = false")
        assert page.evaluate("SessionDockCapabilities.config.native_sdks.codebuddy") is False
        assert page.evaluate("T.native_sdks.codebuddy") is True
        catalog_requests = []
        page.on("request", lambda r: catalog_requests.append(r.url) if "/api/native-sdk/models" in r.url or "/api/term/models" in r.url else None)
        page.locator("#new-session:visible, #header-more-btn:visible").first.click()
        if not page.locator("#new-session-dialog").is_visible():
            page.locator("#new-session").click()
        page.locator('#new-session-form label:has(input[name="new-source"][value="codebuddy"])').click()
        page.wait_for_function("() => document.querySelector('#new-model-label')?.textContent?.trim() && !/读取模型/.test(document.querySelector('#new-model-label').textContent)")
        assert any("/api/native-sdk/models" in url for url in catalog_requests), catalog_requests
        assert not any("/api/term/models?source=codebuddy" in url for url in catalog_requests), catalog_requests
        page.locator("#new-model").click()
        page.locator("#new-model-options [role=option]", has_text="Synthetic Buddy 2").first.click()
        expect(page.locator("#new-effort")).to_be_enabled()
        assert "high" in page.locator("#new-effort option").all_inner_texts()
        page.keyboard.press("Escape")
        expect(page.locator("#new-session-dialog")).to_be_hidden()
        # The SDK model catalog probe above may spawn its own short-lived CLI; only the session start is checked.
        spawned = len(fixture.frames("spawn"))
        open_sdk(page, fixture, expect)
        assert len(fixture.frames("spawn")) == spawned + 1
        spawn = fixture.frames("spawn")[-1]
        assert spawn["args"][spawn["args"].index("--resume") + 1] == fixture.sid, spawn
        # History shown from the read model; the replayed stdout history must not be projected live.
        expect(page.locator(".sdk-history")).to_contain_text("earlier synthetic CodeBuddy answer")
        quiet = wait_snapshot(page, fixture.uid, lambda raw: raw.get("capabilities") is not None)["raw"]
        assert quiet["items"] == [] and not quiet["running"], quiet
        assert quiet["capabilities"]["rename"] is False and quiet["capabilities"]["fork"] is False
        assert SECRET not in json.dumps(quiet), "initialize account token leaked into the snapshot"

        # Choices apply immediately, without a model prompt.
        page.locator("#sdk-model").select_option("fake-buddy-2")
        expect(page.locator("#sdk-model")).to_be_enabled()
        page.locator("#sdk-effort").select_option("high")
        expect(page.locator("#sdk-send")).to_be_enabled()
        assert not fixture.frames("user"), "selection submitted a native user message"
        controls = [frame["request"] for frame in fixture.frames("control_request")]
        assert any(r.get("subtype") == "set_model" and r["model"] == "fake-buddy-2" and r["session_id"] == fixture.sid for r in controls)
        assert any(r.get("subtype") == "set_config" and r["config"] == {"reasoningEffort": "high"} for r in controls)

        send(page, "hello from browser")
        expect(page.locator(".sdk-live")).to_contain_text("CodeBuddy synthetic final hello from browser")
        final = wait_snapshot(page, fixture.uid, lambda raw: not raw["running"])
        assert final["raw"]["session_id"] == fixture.sid
        ids = [item["id"] for item in final["raw"]["items"]]
        assert len(ids) == len(set(ids)), ids
        users = [item for item in final["raw"]["items"] if item["role"] == "user"]
        assert len(users) == 1 and users[0]["native_echo"] is True, users
        assert users[0]["native_id"] != users[0]["id"], "echo must carry the CLI-minted identity"
        assert len(user_prompts(fixture, "hello from browser")) == 1
        # History (read model) and live projection meet at the CLI-minted identity: one bubble each.
        expect(page.locator(".native-sdk-pane").get_by_text("hello from browser", exact=True)).to_have_count(1)
        expect(page.locator(".native-sdk-pane").get_by_text("CodeBuddy synthetic final hello from browser", exact=True)).to_have_count(1)
        post(page, "turn", turns[-1])
        assert len(user_prompts(fixture, "hello from browser")) == 1, "retried request was sent twice"
        usage = final["raw"]["usage"]
        assert usage["tokens"]["input"] == 100 and usage["tokens"]["output"] == 20, usage
        assert SECRET not in json.dumps(final), "account token leaked"
        capture(page, "codebuddy-sdk-wide-light.png")

        reveal_mode = page.locator("#native-sdk-permission_mode")
        if not reveal_mode.is_visible():
            page.locator("#sdk-more, [data-sdk-more]").first.click()
        reveal_mode.select_option("plan")
        wait_snapshot(page, fixture.uid, lambda raw: raw.get("settings", {}).get("permission_mode") == "plan")
        assert any(r.get("subtype") == "set_permission_mode" and r["mode"] == "plan" and r["session_id"] == fixture.sid
                   for r in (f["request"] for f in fixture.frames("control_request")))

        # The turn continues across a page close; stop uses the native interrupt.
        send(page, "hold")
        expect(page.locator(".sdk-live")).to_contain_text("streaming CodeBuddy progress")
        wait_snapshot(page, fixture.uid, lambda raw: raw["running"])
        context.close()
        context = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme="dark", service_workers="block")
        page = context.new_page()
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.goto(base, wait_until="networkidle")
        open_sdk(page, fixture, expect)
        assert snapshot(page, fixture.uid)["raw"]["running"] is True
        assert len(fixture.frames("spawn")) == spawned + 1, "reload launched another independent runtime"
        assert len(user_prompts(fixture, "hold")) == 1
        page.locator("#sdk-stop").click()
        expect(page.locator(".sdk-live")).to_contain_text("CodeBuddy interrupted")
        wait_snapshot(page, fixture.uid, lambda raw: not raw["running"])
        assert any(f["request"]["subtype"] == "interrupt" and f["request"]["session_id"] == fixture.sid
                   for f in fixture.frames("control_request"))

        send(page, "approval")
        request = wait_snapshot(page, fixture.uid, lambda raw: bool(raw["requests"]))["raw"]["requests"][0]
        assert request["type"] == "approval" and request["input"]["command"] == "printf synthetic"
        page.reload(wait_until="networkidle")
        open_sdk(page, fixture, expect)
        assert snapshot(page, fixture.uid)["raw"]["requests"][0]["key"] == request["key"]
        post(page, "respond", {"uid": fixture.uid, "key": request["key"], "result": {"behavior": "deny"}})
        expect(page.locator(".sdk-live")).to_contain_text("CodeBuddy synthetic final approval")
        denied = wait_snapshot(page, fixture.uid, lambda raw: not raw["running"])
        tools = [row for row in denied["raw"]["items"] if row["role"] == "tool"]
        assert any(row.get("result", {}).get("error") is True and "denied" in row["result"]["text"] for row in tools), tools
        answers = [f["response"]["response"] for f in fixture.frames("control_response") if "allowed" in f["response"].get("response", {})]
        assert answers[-1]["allowed"] is False and answers[-1]["reason"] and answers[-1]["tool_use_id"], answers

        send(page, "approval")
        request = wait_snapshot(page, fixture.uid, lambda raw: bool(raw["requests"]))["raw"]["requests"][0]
        post(page, "respond", {"uid": fixture.uid, "key": request["key"], "result": {"behavior": "allow"}})
        wait_snapshot(page, fixture.uid, lambda raw: not raw["running"])
        allowed = [f["response"]["response"] for f in fixture.frames("control_response") if f["response"].get("response", {}).get("allowed") is True]
        assert allowed[-1]["updatedInput"]["command"] == "printf synthetic", allowed

        send(page, "question")
        request = wait_snapshot(page, fixture.uid, lambda raw: bool(raw["requests"]))["raw"]["requests"][0]
        assert request["type"] == "question" and request["questions"][0]["multiSelect"] is False
        post(page, "respond", {"uid": fixture.uid, "key": request["key"], "result": {"answers": {"Choose a synthetic answer?": "Beta"}}})
        expect(page.locator(".sdk-live")).to_contain_text("CodeBuddy synthetic final question")
        wait_snapshot(page, fixture.uid, lambda raw: not raw["running"])
        assert any("Beta" in json.dumps(frame) for frame in fixture.frames("control_response"))
        capture(page, "codebuddy-sdk-mobile-dark.png")

        send(page, "error")
        error = wait_snapshot(page, fixture.uid, lambda raw: bool(raw.get("error")))
        assert "synthetic native execution failure" in error["raw"]["error"] and error["raw"]["error_kind"] == "turn"

        # New sessions get a client-minted UUID and the native project directory naming.
        created = receipt(post(page, "create", {"source": "codebuddy", "request_id": "create-one", "cwd": str(fixture.cwd),
                                                "model": "fake-buddy", "effort": "low"}))
        path = fixture.projects / project_key(fixture.cwd) / (created["session_id"] + ".jsonl")
        assert created["uid"] == native_uid(path) and not path.exists(), created
        again = receipt(post(page, "create", {"source": "codebuddy", "request_id": "create-one", "cwd": str(fixture.cwd),
                                              "model": "fake-buddy", "effort": "low"}))
        assert again["uid"] == created["uid"]
        post(page, "open", {"uid": created["uid"]})
        assert not path.exists(), "open/initialize wrote a fake native history"
        spawn = fixture.frames("spawn")[-1]
        assert spawn["args"][spawn["args"].index("--session-id") + 1] == created["session_id"], spawn
        assert "--model" in spawn["args"] and "--effort" in spawn["args"], spawn
        post(page, "turn", {"uid": created["uid"], "request_id": "new-first-turn", "text": "created in SDK"})
        wait_snapshot(page, created["uid"], lambda raw: not raw["running"] and bool(raw["items"]))
        assert path.is_file()
        assert json.loads(path.read_text().splitlines()[0])["sessionId"] == created["session_id"]
        assert not any(frame["args"][frame["args"].index("--resume") + 1:][:1] == [created["session_id"]]
                       for frame in fixture.frames("spawn") if "--resume" in frame["args"])

        # Unsupported controls are refused, never forwarded to the model.
        refused = post(page, "rename", {"uid": fixture.uid, "title": "x", "request_id": "r1"}, ok=False)
        assert "unsupported" in json.dumps(refused), refused
        assert not user_prompts(fixture, "/rename x")

        # An independently running CLI for the same SID is observed and respected.
        independent = fixture.root / "proc/424242"
        independent.mkdir()
        (independent / "cmdline").write_bytes(("node\0/opt/bin/codebuddy\0--resume\0" + created["session_id"] + "\0").encode())
        (independent / "stat").write_text("424242 (node) S 1 " + " ".join(["0"] * 17) + " 100\n")
        conflict = post(page, "turn", {"uid": created["uid"], "request_id": "busy", "text": "must not send"}, ok=False)
        assert "busy" in json.dumps(conflict) or "independent" in json.dumps(conflict), conflict
        assert not user_prompts(fixture, "must not send")
        assert (independent / "stat").is_file(), "independent writer was modified"

        assert not errors, errors
        assert not list((fixture.root / "host").glob("*.session")), "SDK path started a PTY"
        context.close()
        browser.close()


def restart_check(fixture, binary):
    """A node restart re-attaches the sdkhost child instead of launching again."""
    from playwright.sync_api import sync_playwright, expect
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True, args=["--no-sandbox"])
        with fixture.server(binary, preserve_sdkhosts=True) as base:
            page = browser.new_page(service_workers="block")
            page.goto(base, wait_until="networkidle")
            open_sdk(page, fixture, expect)
            send(page, "hold")
            wait_snapshot(page, fixture.uid, lambda raw: raw["running"])
            page.close()
        with fixture.server(binary) as base:
            page = browser.new_page(service_workers="block")
            page.goto(base, wait_until="networkidle")
            open_sdk(page, fixture, expect)
            assert len(fixture.frames("spawn")) == 1, "restart launched a second runtime"
            assert len(user_prompts(fixture, "hold")) == 1
            page.locator("#sdk-stop").click()
            expect(page.locator(".sdk-live")).to_contain_text("CodeBuddy interrupted")
            wait_snapshot(page, fixture.uid, lambda raw: not raw["running"])
        browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-codebuddy-sdk-") as temporary:
        fixture = CodebuddyFixture(Path(temporary))
        with fixture.server(args.binary) as base:
            browser_check(fixture, base, args.screenshots)
    with tempfile.TemporaryDirectory(prefix="sessiondock-codebuddy-sdk-restart-") as temporary:
        restart_check(CodebuddyFixture(Path(temporary)), args.binary)
    print("PASS CodeBuddy native SDK: read-model listing, replay suppression, settings, send/dedupe/stop, approval/question answers, create naming, unsupported controls, writer conflict, restart re-attach")


if __name__ == "__main__":
    main()
