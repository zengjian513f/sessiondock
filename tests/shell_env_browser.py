#!/usr/bin/env python3
"""Login-shell drift: the page offers to restart a backend whose shell env changed.

The service runs behind a shell wrapper (production: `with-zshrc`) so launched
CLIs inherit one prepared environment. A synthetic wrapper sources a temporary
rc file; editing it makes `GET /api/shell-env` report the changed variable
names (never values). Chromium on the hub page shows the per-machine notice,
the node's own page can ignore it, and 重启后端 makes the node exit with 75
after a graceful shutdown; a supervisor-like restart clears the notice.

A real hub and one real isolated node; no real CLI or production data.
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

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, REPO, Corpus, codex_row
from hub_http_suite import Hub, free_port
from node_auth_suite import node_env, TOKEN

NID = "e" * 32
RESTART_EXIT_CODE = 75


def start_node(binary, corpus, extra):
    port = free_port()
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    environment.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}",
                        "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web")})
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
        wrapper.write_text(f"#!/bin/bash\n. {rc}\nexec \"$@\"\n")
        wrapper.chmod(0o755)
        node = SimpleNamespace(name="shellnode", nid=NID, port=free_port(), token=TOKEN)
        extra = {**node_env(root, node.port, "127.0.0.0/8"),
                 "SESSIONDOCK_SHELL_ENV_COMMAND": wrapper, "SESSIONDOCK_SHELL_ENV_WATCH": rc}
        process, local = start_node(binary, corpus, extra)
        stack.callback(lambda: stop(process))
        (root / "hubroot").mkdir()
        hub = Hub(binary.with_name("sessiondock-hub"), root / "hubroot", [node])
        hub.start()
        stack.callback(hub.stop)
        # Unconfigured node answers configured:false; this one has a baseline and no drift.
        data, _ = shell_env(local)
        assert data["configured"] and not data["stale"] and data["changed"] == [], data
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = pw.chromium.launch(**launch)
        stack.callback(browser.close)
        context = browser.new_context(service_workers="block", viewport={"width": 390, "height": 860})
        hub_page = context.new_page()
        errors = []
        hub_page.on("pageerror", lambda error: errors.append(str(error)))
        hub_page.goto(f"http://127.0.0.1:{hub.port}", wait_until="networkidle")
        hub_page.wait_for_function("Nodes.list.length === 1")
        hub_page.evaluate("checkShellEnv()")
        notice = hub_page.locator("#shell-env-notice")
        expect(notice).to_have_count(0)
        # Edit the startup file: a changed value and a new variable.
        time.sleep(0.02)
        rc.write_text("export SD_TEST_TOKEN=secret-two\nexport SD_TEST_NEW=1\n")
        data, raw = shell_env(local)
        assert data["stale"] and data["changed"] == ["SD_TEST_NEW", "SD_TEST_TOKEN"], data
        assert b"secret" not in raw, raw
        hub_page.evaluate("checkShellEnv()")
        expect(notice).to_be_visible()
        expect(notice).to_contain_text("shellnode 的登录环境（zshrc）已变化：SD_TEST_NEW、SD_TEST_TOKEN")
        expect(notice).not_to_contain_text("secret")
        box = notice.bounding_box()
        assert box["x"] >= 0 and box["x"] + box["width"] <= 390.5, box
        # The node's own page: 忽略 hides this change for the page.
        local_page = context.new_page()
        local_page.goto(local, wait_until="networkidle")
        local_page.evaluate("checkShellEnv()")
        local_notice = local_page.locator("#shell-env-notice")
        expect(local_notice).to_contain_text("SD_TEST_TOKEN")
        local_notice.get_by_role("button", name="忽略").click()
        expect(local_notice).to_have_count(0)
        local_page.evaluate("checkShellEnv()")
        expect(local_notice).to_have_count(0)
        local_page.close()
        # Restart from the hub page: the node shuts down gracefully and exits 75.
        hub_page.get_by_role("button", name="重启 shellnode 后端").click()
        expect(notice).to_contain_text("shellnode 的后端正在重启…")
        assert process.wait(timeout=15) == RESTART_EXIT_CODE, process.returncode
        # The supervisor starts it again: the new baseline has the edited rc, the notice goes.
        process, local = start_node(binary, corpus, extra)
        stack.callback(lambda: stop(process))
        expect(notice).to_have_count(0, timeout=40000)
        data, _ = shell_env(local)
        assert not data["stale"], data
        assert not errors, errors
    print("PASS shell env browser: drift names only (no values), hub + node notice at 390px, ignore, "
          "restart exits 75 after graceful shutdown, a fresh start clears the notice")


if __name__ == "__main__":
    main()
