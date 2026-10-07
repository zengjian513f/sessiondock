#!/usr/bin/env python3
"""Login-shell drift: the page offers to restart a backend whose shell env changed.

The service runs behind a shell wrapper (production: `with-zshrc`) so launched
CLIs inherit one prepared environment. A synthetic wrapper sources a temporary
rc file; editing it makes `GET /api/shell-env` report the changed variable
names (never values). Chromium on the hub page shows the per-machine notice,
the node's own page can ignore it, and 重启后端 makes the node exit with 75
after a graceful shutdown; a supervisor-like restart clears the notice.
Removed shell cards return with a new DOM identity at the stack end; a later
HTTP build mismatch appends its card after the existing shell card.

A real hub and two real isolated nodes; no real CLI or production data.
"""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from types import SimpleNamespace
from urllib.request import urlopen

from frontend_paths import frontend_dir
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO, Corpus, codex_row
from hub_fixtures import Hub, free_port
from node_auth_fixtures import node_env, TOKEN

NID = "e" * 32
RESTART_EXIT_CODE = 75


def start_node(binary, corpus, extra):
    port = free_port()
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    environment.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}",
                        "SESSIONDOCK_WEB_DIR": str(frontend_dir())})
    for source in ("claude", "codex", "grok"):
        environment[f"SESSIONDOCK_{source.upper()}_ROOT"] = str(corpus.root / source)
    environment.update({key: str(value) for key, value in extra.items()})
    log = open(corpus.root / f"node-{port}.log", "wb")
    process = subprocess.Popen([str(binary)], cwd=REPO, env=environment, stdout=log, stderr=log)
    base = f"http://127.0.0.1:{port}"
    for _ in range(200):
        if process.poll() is not None:
            raise AssertionError(f"node exited early ({process.returncode})")
        try:
            with urlopen(base + "/api/health", timeout=2):
                return process, base
        except OSError:
            time.sleep(0.05)
    raise AssertionError("node health timed out")


def stop(process):
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=10)


def shell_env(base):
    with urlopen(base + "/api/shell-env", timeout=40) as response:
        raw = response.read()
    return json.loads(raw), raw


def restart_failure_fixture(context, hub_url):
    """Actual buttons use held HTTP requests, then recover from their failures."""
    page = context.new_page()
    errors, requests = [], []
    page.on("pageerror", lambda error: errors.append(str(error)))
    second = "f" * 32
    data = {nid: {"configured": True, "stale": True, "started_at": "fixture-start",
                  "changed": ["SD_FIXTURE_CHANGED"]} for nid in (NID, second)}

    def env_route(route):
        nid = route.request.url.split("/api/nodes/", 1)[1].split("/", 1)[0]
        route.fulfill(json=data[nid])

    def restart_route(route):
        assert route.request.method == "POST", route.request.method
        requests.append(route)  # Leave the real fetch pending until the assertions finish.

    page.route("**/api/nodes/*/api/shell-env", env_route)
    page.route("**/api/nodes/*/api/shell-env/restart", restart_route)
    try:
        # Let the startup timer's own check finish before holding any POST:
        # legacy would otherwise recreate its directly disabled button at 3s.
        with ExitStack() as initial_check:
            for nid in (NID, second):
                initial_check.enter_context(page.expect_response(
                    lambda response, nid=nid: response.request.method == "GET"
                    and response.url.endswith(f"/api/nodes/{nid}/api/shell-env")))
            page.goto(hub_url, wait_until="networkidle")
            page.wait_for_function("Nodes.list.length === 2")
        notice = page.locator("#shell-env-notice")
        first_row = notice.locator(f'tr[data-node="{NID}"]')
        second_row = notice.locator(f'tr[data-node="{second}"]')
        expect(first_row).to_be_visible()
        expect(second_row).to_be_visible()
        # Ignore only this machine's exact names; the other row and card remain.
        card = notice.element_handle()
        second_row.get_by_role("button", name="忽略 shellnode2", exact=True).click()
        expect(second_row).to_have_count(0)
        expect(first_row).to_be_visible()
        assert card.evaluate("d => d === document.querySelector('#shell-env-notice')")
        page.evaluate("checkShellEnv()")
        expect(second_row).to_have_count(0)
        data[second]["changed"] = ["SD_FIXTURE_CHANGED_AGAIN"]
        page.evaluate("checkShellEnv()")
        expect(second_row.locator("td").nth(1)).to_have_text("SD_FIXTURE_CHANGED_AGAIN")
        assert not requests, "ignore sent a restart request"

        for label, targets in (("重启 shellnode 后端", (NID,)),
                               ("全部重启 (2)", (NID, second))):
            # Retry the same recovered control, rather than merely inspecting it.
            for attempt in range(2):
                button = notice.get_by_role("button", name=label, exact=True)
                head = notice.locator(".app-float-head").element_handle()
                table = notice.locator("table").element_handle()
                original_button = button.element_handle()
                before = len(requests)
                with ExitStack() as started:
                    for nid in targets:
                        started.enter_context(page.expect_request(
                            lambda request, nid=nid: request.method == "POST"
                            and f"/api/nodes/{nid}/api/shell-env/restart" in request.url))
                    button.click()
                expect(button).to_be_disabled()
                box = button.bounding_box()
                assert box, label
                page.mouse.dblclick(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
                page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
                assert len(requests) == before + len(targets), "pending clicks sent duplicate restart requests"
                failure = f"fixture_restart_failed_{attempt}"
                for route in requests[before:]:
                    route.fulfill(status=503, json={"error": failure})
                popup = page.locator('.app-popup[data-popup-type="alert"]')
                expect(popup).to_be_visible()
                expect(popup).to_contain_text("重启后端失败:")
                for nid in targets:
                    name = "shellnode" if nid == NID else "shellnode2"
                    expect(popup).to_contain_text(f"{name}: {failure}")
                popup.get_by_role("button", name="知道了", exact=True).click()
                expect(popup).to_have_count(0)
                expect(button).to_be_enabled()
                expect(first_row.locator("td").nth(1)).to_have_text("SD_FIXTURE_CHANGED")
                expect(second_row.locator("td").nth(1)).to_have_text("SD_FIXTURE_CHANGED_AGAIN")
                assert card.evaluate("d => d === document.querySelector('#shell-env-notice')")
                for handle in (head, table, original_button):
                    handle.dispose()
        card.dispose()
        assert not errors, errors
    finally:
        page.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-shell-env-") as temporary, sync_playwright() as pw, \
            ExitStack() as stack:
        root = Path(temporary).resolve()
        corpus = Corpus(root)
        for name in ("claude", "codex", "grok", "ids"):
            (root / name).mkdir(exist_ok=True)
        (root / "ids/node-id").write_text(NID + "\n")
        corpus.put("shell-env-row", "codex", [
            codex_row("session_meta", {"id": "shell-env-row", "cwd": "/synthetic/shell",
                                       "timestamp": "2026-09-11T10:00:00Z"}),
            codex_row("response_item", {"type": "message", "role": "user", "content": "shell env row"})], [])
        rc = root / "rc.sh"
        rc.write_text("export SD_TEST_TOKEN=secret-one\n")
        wrapper = root / "with-rc"
        # Like atuin/starship: a value minted by every shell start is not a configuration change.
        wrapper.write_text(f"#!/bin/bash\n. {rc}\nexport SD_PER_SHELL=$RANDOM$RANDOM$$\nexec \"$@\"\n")
        wrapper.chmod(0o755)
        nodes, processes, locals_, extras = [], {}, {}, {}
        for name, nid in (("shellnode", NID), ("shellnode2", "f" * 32)):
            nroot = root / name
            corpus_n = Corpus(nroot)
            for sub in ("claude", "codex", "grok", "ids"):
                (nroot / sub).mkdir(parents=True, exist_ok=True)
            (nroot / "ids/node-id").write_text(nid + "\n")
            node = SimpleNamespace(name=name, nid=nid, port=free_port(), token=TOKEN, corpus=corpus_n)
            extras[name] = {**node_env(nroot, node.port, "127.0.0.0/8"),
                            "SESSIONDOCK_SHELL_ENV_COMMAND": wrapper, "SESSIONDOCK_SHELL_ENV_WATCH": rc}
            processes[name], locals_[name] = start_node(binary, corpus_n, extras[name])
            nodes.append(node)
        stack.callback(lambda: [stop(process) for process in processes.values()])

        def restart_node(name):
            assert processes[name].wait(timeout=15) == RESTART_EXIT_CODE, processes[name].returncode
            node = next(n for n in nodes if n.name == name)
            processes[name], locals_[name] = start_node(binary, node.corpus, extras[name])

        (root / "hubroot").mkdir()
        hub = Hub(binary.with_name("sessiondock-hub"), root / "hubroot", nodes)
        hub.start()
        stack.callback(hub.stop)
        local = locals_["shellnode"]
        # A baseline and no drift yet.
        data, _ = shell_env(local)
        assert data["configured"] and not data["stale"] and data["changed"] == [], data
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = pw.chromium.launch(**launch)
        stack.callback(browser.close)
        context = browser.new_context(service_workers="block", viewport={"width": 390, "height": 860})
        restart_failure_fixture(context, f"http://127.0.0.1:{hub.port}")
        hub_page = context.new_page()
        errors = []
        hub_page.on("pageerror", lambda error: errors.append(str(error)))
        hub_page.goto(f"http://127.0.0.1:{hub.port}", wait_until="networkidle")
        hub_page.wait_for_function("Nodes.list.length === 2")
        hub_page.evaluate("checkShellEnv()")
        notice = hub_page.locator("#shell-env-notice")
        expect(notice).to_have_count(0)
        row = lambda nid: notice.locator(f'tr[data-node="{nid}"]')  # noqa: E731
        NID2 = "f" * 32
        # Edit the shared startup file: a changed value and a new variable, on both machines.
        rc.write_text("export SD_TEST_TOKEN=secret-two\nexport SD_TEST_NEW=1\n")
        data, raw = shell_env(local)
        assert data["stale"] and data["changed"] == ["SD_TEST_NEW", "SD_TEST_TOKEN"], data
        assert b"secret" not in raw, raw
        hub_page.evaluate("checkShellEnv()")
        expect(notice).to_be_visible()
        first_notice = notice.element_handle()
        assert notice.evaluate("d => d.parentElement.id === 'float-stack' && d.parentElement.lastElementChild === d")
        expect(notice.locator("th")).to_have_text(["机器", "变化", "", ""])
        expect(row(NID).locator("td")).to_have_text(["shellnode", "SD_TEST_NEW、SD_TEST_TOKEN", "重启", "忽略"])
        expect(row(NID2).locator("td")).to_have_text(["shellnode2", "SD_TEST_NEW、SD_TEST_TOKEN", "重启", "忽略"])
        expect(notice).not_to_contain_text("SD_PER_SHELL")
        expect(notice).not_to_contain_text("secret")
        box = notice.bounding_box()
        assert box["x"] >= 0 and box["x"] + box["width"] <= 390.5, box
        # The node's own page: 忽略 hides this change for the page.
        local_page = context.new_page()
        local_page.goto(local, wait_until="networkidle")
        local_page.evaluate("checkShellEnv()")
        local_notice = local_page.locator("#shell-env-notice")
        expect(local_notice.locator('tr[data-node="local"]')).to_contain_text("SD_TEST_TOKEN")
        expect(local_notice.get_by_role("button", name="全部重启")).to_have_count(0)
        ignored_notice = local_notice.element_handle()
        local_notice.get_by_role("button", name="忽略").click()
        expect(local_notice).to_have_count(0)
        assert not ignored_notice.evaluate("d => d.isConnected")
        local_page.evaluate("checkShellEnv()")
        expect(local_notice).to_have_count(0)
        # Ignoring one set of names does not hide a later, genuinely different drift.
        # A removed card gets a new DOM identity and is appended to the existing stack.
        rc.write_text("export SD_TEST_TOKEN=secret-two\nexport SD_TEST_NEW=1\nexport SD_TEST_AFTER_IGNORE=1\n")
        local_page.evaluate("checkShellEnv()")
        expect(local_notice).to_be_visible()
        expect(local_notice.locator('tr[data-node="local"] td').nth(1)).to_have_text(
            "SD_TEST_AFTER_IGNORE、SD_TEST_NEW、SD_TEST_TOKEN")
        assert not ignored_notice.evaluate("d => d === document.querySelector('#shell-env-notice')")
        assert local_notice.evaluate("d => d.parentElement.id === 'float-stack' && d.parentElement.lastElementChild === d")
        rc.write_text("export SD_TEST_TOKEN=secret-two\nexport SD_TEST_NEW=1\n")
        local_page.evaluate("checkShellEnv()")
        expect(local_notice).to_have_count(0)
        ignored_notice.dispose()
        local_page.close()
        # 全部重启: both machines shut down gracefully and exit 75; a supervisor restart clears the notice.
        hub_page.get_by_role("button", name="全部重启 (2)").click()
        expect(row(NID).locator("td").nth(1)).to_have_text("正在重启…")
        expect(row(NID2).locator("td").nth(1)).to_have_text("正在重启…")
        restart_node("shellnode")
        restart_node("shellnode2")
        # Both restarted nodes need a baseline before the next edit. A vanished
        # notice alone can also mean the Hub temporarily sees an offline node.
        for name in ("shellnode", "shellnode2"):
            fresh, _ = shell_env(locals_[name])
            assert fresh["configured"] and not fresh["stale"], fresh
        hub_page.wait_for_function(
            "Nodes.list.length === 2 && Nodes.list.every(node => node.online === true)"
            " && shellEnvRestarting.size === 0", timeout=40000)
        expect(notice).to_have_count(0, timeout=40000)
        assert not first_notice.evaluate("d => d.isConnected")
        # Another edit, one machine at a time: the other row stays, no 全部重启 for a single one.
        rc.write_text("export SD_TEST_TOKEN=secret-three\nexport SD_TEST_NEW=1\n")
        hub_page.evaluate("checkShellEnv()")
        expect(hub_page.get_by_role("button", name="全部重启 (2)")).to_be_visible()
        assert not first_notice.evaluate("d => d === document.querySelector('#shell-env-notice')")
        assert notice.evaluate("d => d.parentElement.id === 'float-stack' && d.parentElement.lastElementChild === d")
        first_notice.dispose()
        hub_page.evaluate("""() => { window.__noticeGone = false;
            new MutationObserver(() => { if (!document.querySelector('#shell-env-notice')) window.__noticeGone = true; })
                .observe(document.querySelector('#float-stack'), {childList: true}); }""")
        hub_page.get_by_role("button", name="重启 shellnode 后端").click()
        # Only that row changes; the other machine's row stays through the whole restart.
        expect(row(NID).locator("td").nth(1)).to_have_text("正在重启…")
        expect(row(NID2).locator("td").nth(1)).to_have_text("SD_TEST_TOKEN")
        restart_node("shellnode")
        for _ in range(3):   # across polls while the node is down and back, the notice never vanishes
            expect(row(NID2)).to_be_visible()
            hub_page.evaluate("checkShellEnv()")
        expect(row(NID)).to_have_count(0, timeout=40000)
        expect(row(NID2).locator("td").nth(1)).to_have_text("SD_TEST_TOKEN")
        expect(notice.get_by_role("button", name="全部重启")).to_have_count(0)
        assert not hub_page.evaluate("window.__noticeGone"), "the notice vanished during a one-machine restart"
        data, _ = shell_env(locals_["shellnode"])
        assert not data["stale"], data
        # The shell card appeared first. A real build poll must append the update
        # card after it.
        shell_card = notice.element_handle()
        hub_page.route("**/api/meta", lambda route: route.fulfill(json={"build": "shell-env-test-newer"}))
        hub_page.evaluate("checkServerBuild()")
        expect(hub_page.locator("#float-stack > .version-stale")).to_be_visible()
        expect(hub_page.locator("#float-stack > :is(#shell-env-notice, .version-stale)")).to_have_count(2)
        assert hub_page.locator("#float-stack > :is(#shell-env-notice, .version-stale)").evaluate_all(
            "cards => cards.map(d => d.id || (d.classList.contains('version-stale') ? 'stale' : d.className))"
        ) == ["shell-env-notice", "stale"]
        assert shell_card.evaluate("d => d === document.querySelector('#shell-env-notice')")
        shell_card.dispose()
        assert not errors, errors
    print("PASS shell env browser: drift names only (no values, per-shell values ignored), two-machine table at 390px, ignore, "
          "fixture restart failures recover row/bulk buttons and retries, pending double-clicks send no duplicate POST, "
          "全部重启 and single restart exit 75 after graceful shutdown, a fresh start clears each row; "
          "removed shell cards return with new identity at the stack end, shell precedes the later build notice")


if __name__ == "__main__":
    main()
