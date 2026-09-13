#!/usr/bin/env python3
"""Real hub -> authenticated Rust node -> fake Claude text-send/version regression.

Private loopback fixtures only. No real model or native CLI home is used.
The hub and node deliberately have different builds, and the hub is upgraded
while a desktop tab survives. Also covers refresh, 390 px, retry/raw-input
gates, metadata and forged hub headers on the ordinary browser listener.
"""
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time
from types import SimpleNamespace
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect

from history_parity import REPO, BINARY, Corpus, isolated_server
from hub_http_suite import Hub, free_port, scoped
from send_browser import (FAKE_CLI, SETTINGS, initialize, create_claude, claude_uid,
                          xterm_includes, send_from_composer, wait_history)


def cleanup_hosts(root):
    # Only the private host instances created by this test, with their exact
    # guarded identities (the service stopping does not terminate ptyhost).
    for path in (root / "host").glob("*.json"):
        record = json.loads(path.read_text())
        meta = record["meta"]
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.settimeout(2)
                stream.connect(str(root / "host" / (record["name"] + ".sock")))
                stream.sendall(json.dumps({"op": "launch_guard_v1", "expected_source": meta["source"],
                    "expected_launch_id": meta["launch_id"], "expected_instance_id": meta["instance_id"],
                    "request": {"op": "kill", "force": True}}).encode() + b"\n")
        except OSError:
            pass
    deadline = time.monotonic() + 6
    while list((root / "host").glob("*.sock")) and time.monotonic() < deadline:
        time.sleep(.05)


def prepare(root):
    for name in ["host", "work", "work/claude-area", "ledger", "delivery", "bin", "home",
                 "claude", "codex", "grok", "hub", "state"]:
        (root / name).mkdir(mode=0o700)
    python = Path(shutil.which("python3")).resolve()
    wrapper = root / "bin/fake-claude"
    wrapper.write_text("#!/bin/sh\nexec %s %s \"$@\"\n" % (python, FAKE_CLI))
    wrapper.chmod(0o700)
    config = root / "launcher.json"
    config.touch(mode=0o600)
    config.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
        "host_dir": str(root / "host"), "adapters": [], "profiles": [
            {"id": "claude-cli-v1", "source": "claude", "executable": str(wrapper),
             "args": ["--settings", SETTINGS, "--reply"], "new_args": ["--session-id", "{session_id}"],
             "resume_args": ["--resume", "{sid}"],
             "env": {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color",
                     "LANG": "C.UTF-8", "AGENTHUB_TEST_CLAUDE_ROOT": str(root / "claude")}}]}))
    initialize("--initialize-lifecycle", root / "ledger")
    initialize("--initialize-delivery", root / "delivery")
    return config


def check_browser(browser, root, config):
    node = SimpleNamespace(nid="a" * 32, name="FixtureNode", port=free_port(), token="f" * 64)
    token_file, id_file = root / "node-token", root / "node-id"
    for path, value in [(token_file, node.token), (id_file, node.nid)]:
        path.touch(mode=0o600)
        path.write_text(value)
    node_env = {"SESSIONDOCK_NODE_BIND": f"127.0.0.1:{node.port}",
                "SESSIONDOCK_NODE_TOKEN_FILE": str(token_file), "SESSIONDOCK_NODE_ID_FILE": str(id_file),
                "SESSIONDOCK_NODE_PEERS": "127.0.0.0/8"}
    with isolated_server(Corpus(root), BINARY, state_dir=root / "state", host_dir=root / "host",
                         lifecycle_dir=root / "ledger", launcher_config=config,
                         delivery_dir=root / "delivery", extra_env=node_env) as (local, _):
        context = browser.new_context(service_workers="block")
        page = context.new_page()
        page.goto(local, wait_until="networkidle")
        receipt = create_claude(page, local, root / "work")
        page.locator("#termpane .xterm-helper-textarea").press_sequentially("hub fixture seed")
        page.locator("#termpane .xterm-helper-textarea").press("Enter")
        xterm_includes(page, "> hub fixture seed")
        uid = claude_uid(root, receipt["declared_sid"])
        page.wait_for_function("uid => S.sel === uid", arg=uid, timeout=20000)
        name = page.evaluate("takenOver(S.sel)")
        node_build = context.request.get(local + "/api/meta").json()["build"]
        page.evaluate("backgroundTerm()")
        page.close()
        # Closing the setup console releases its Python-compatible ownership
        # lease asynchronously; a claim that never finished binding also has
        # Python's 15-second reservation lifetime. Wait on the actual draft
        # probe before opening a different hub page.
        deadline = time.monotonic() + 20
        while True:
            released = context.request.post(
                local + "/api/session/draft-status",
                data={"uid": uid, "name": name, "_build": node_build},
            )
            if released.status == 200:
                break
            if time.monotonic() >= deadline:
                raise AssertionError(
                    f"setup terminal lease did not release: {released.status} {released.text()}"
                )
            time.sleep(.05)
        context.close()
        print("PASS setup: private fake Claude session ready", flush=True)

        hub = Hub(REPO / "target/debug/sessiondock-hub", root / "hub", [node])
        web = root / "hub-web"
        shutil.copytree(REPO / "legacy-web", web)
        hub.env["SESSIONDOCK_WEB_DIR"] = str(web)
        hub.start()
        try:
            base = f"http://127.0.0.1:{hub.port}"
            hub_uid = scoped(node.nid, uid)
            errors = []

            def open_page(width):
                ctx = browser.new_context(viewport={"width": width, "height": 900}, service_workers="block")
                ctx.route("**/*", lambda route: route.continue_()
                          if route.request.url.startswith(base + "/") else route.abort())
                p = ctx.new_page()
                p._sessiondock_dialogs = []
                p._sessiondock_responses = []
                p.on("pageerror", lambda error: errors.append(str(error)))
                p.on("dialog", lambda dialog: (p._sessiondock_dialogs.append(dialog.message), dialog.dismiss()))
                p.on("response", lambda response: p._sessiondock_responses.append(
                    (response.status, urlsplit(response.url).path))
                    if "/api/" in response.url else None)
                p.goto(base, wait_until="networkidle")
                select(p)
                return ctx, p

            def select(p):
                p.locator(f'#side .item[data-uid="{hub_uid}"]').click()
                p.wait_for_function("uid => S.sel === uid", arg=hub_uid)
                expect(p.locator("#composer")).to_be_visible()

            ctx, page = open_page(1280)
            build = ctx.request.get(base + "/api/meta").json()["build"]
            assert build != node_build
            assert page.evaluate("BUILD_ID") == build
            assert not page.evaluate("staleBuildShown")

            # A node's marker is minted after auth, never from browser headers.
            bodies = {
                "/api/session/send": {"uid": "claude:missing", "text": "", "request_id": "gate-test"},
                "/api/session/outbox/retry": {"uid": "claude:missing", "id": "missing"},
                "/api/term/send": {"name": "missing", "data": "probe", "page": "test", "token": "0" * 64,
                                   "instance_id": "missing"},
            }
            auth = {"X-AgentHub-Protocol": "1", "X-AgentHub-Node-Token": node.token}
            for path, body in bodies.items():
                body = {**body, "_build": build}
                response = ctx.request.post(local + path, data=body)
                assert response.status == 409 and response.json().get("reload") is True, (path, response.text())
                spoofed = ctx.request.post(local + path, data=body, headers=auth)
                assert spoofed.status == 403 and spoofed.json().get("code") == "hub_unsupported", (path, spoofed.text())
                denied = ctx.request.post(f"http://127.0.0.1:{node.port}" + path, data=body,
                                          headers={"X-AgentHub-Protocol": "1"})
                assert denied.status == 403
                through = ctx.request.post(f"http://127.0.0.1:{node.port}" + path, data=body, headers=auth)
                assert not through.json().get("reload") and through.json().get("code") != "stale_build", (path, through.text())
            star = ctx.request.post(base + "/api/session/star", data={"uid": hub_uid, "starred": True, "_build": build})
            assert star.status == 200, star.text()
            print("PASS gates: local/spoofed build rejected, node authenticated, metadata through hub", flush=True)

            try:
                response = send_from_composer(page, "desktop hub send")
            except Exception:
                print("hub send diagnostics:", page.evaluate("""() => ({
                    selected: S.sel, composerUid, name: takenOver(S.sel),
                    lease: termSendLease(takenOver(S.sel)),
                    outbox: AgentHubCapabilities.allows('outbox'),
                    input: document.querySelector('#cinput')?.value,
                    disabled: document.querySelector('#csend')?.disabled,
                    staleBuildShown,
                })"""), "dialogs=", page._sessiondock_dialogs,
                      "responses=", page._sessiondock_responses[-20:], flush=True)
                raise
            assert response.status == 200, response.text()
            wait_history(page, "OK: desktop hub send")
            assert not page.evaluate("staleBuildShown")
            page.reload(wait_until="networkidle")
            select(page)
            response = send_from_composer(page, "after refresh hub send")
            assert response.status == 200, response.text()
            wait_history(page, "OK: after refresh hub send")
            print("PASS desktop: different hub/node builds send and survive refresh", flush=True)

            # A real new hub asset snapshot must still stop the old page.
            page.locator("#cinput").fill("keep draft across upgrade")
            hub.stop()
            with (web / "app.js").open("a") as stream:
                stream.write("\n// synthetic upgrade fixture\n")
            hub.start()
            fresh = ctx.request.get(base + "/api/meta").json()["build"]
            assert fresh != build
            for path in [*bodies, "/api/term/create"]:
                response = ctx.request.post(base + path, data={"uid": hub_uid, "_node": node.nid, "_build": build})
                assert response.status == 409 and response.json().get("reload") is True
                assert response.json()["build"] == fresh
            page.evaluate("checkServerBuild()")
            expect(page.locator(".version-stale")).to_contain_text("SessionDock 已更新")
            expect(page.locator("#csend")).to_be_disabled()
            expect(page.locator("#cinput")).to_have_value("keep draft across upgrade")
            page.locator(".version-stale button").click()
            page.wait_for_function("expected => typeof BUILD_ID !== 'undefined' && BUILD_ID === expected", arg=fresh)
            select(page)
            assert not page.evaluate("staleBuildShown")
            response = send_from_composer(page, "after real hub upgrade")
            assert response.status == 200, response.text()
            wait_history(page, "OK: after real hub upgrade")
            ctx.close()
            print("PASS upgrade: old page blocked, draft retained, reload sends again", flush=True)

            mobile, page = open_page(390)
            page.locator("#cinput").fill("mobile hub send")
            with page.expect_response(lambda response: response.url.endswith("/api/session/send"), timeout=20000) as sent:
                page.locator("#csend").click()
            response = sent.value
            assert response.status == 200, response.text()
            wait_history(page, "OK: mobile hub send")
            assert not page.evaluate("staleBuildShown")
            bounds = page.locator("#composer").bounding_box()
            assert bounds and bounds["x"] >= 0 and bounds["x"] + bounds["width"] <= 391
            mobile.close()
            assert not errors, errors
            print("PASS mobile: 390 px composer send confirmed from native fixture", flush=True)
        finally:
            hub.stop()


def main():
    started = time.monotonic()
    stop = threading.Event()

    def heartbeat():
        while not stop.wait(10):
            print(f"RUN hub_send_browser: {time.monotonic() - started:.0f}s elapsed", flush=True)

    threading.Thread(target=heartbeat, daemon=True).start()
    if os.name != "posix":
        raise SystemExit("Hub send browser acceptance currently requires POSIX.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-hub-send-") as tmp:
        root = Path(tmp).resolve()
        config = prepare(root)
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                check_browser(browser, root, config)
            finally:
                browser.close()
                cleanup_hosts(root)
    stop.set()
    print("PASS hub_send_browser", flush=True)


if __name__ == "__main__":
    main()
