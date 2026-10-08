#!/usr/bin/env python3
"""Uninstalled CLI configurations do not take a node or its Hub page offline.

Only private synthetic homes, history, fake CLIs and loopback listeners are used.
Exercise missing files, dangling symlinks, non-executable files and directories,
both CLI profiles and fixed adapters, plus a restart with the stale config intact.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, claude_row, isolated_server
from hub_fixtures import Hub, free_port, scoped
from new_session_model_browser import open_dialog
from node_auth_fixtures import TOKEN, node_env
from private_hosts import private_hosts

FAKE_CODEX = '''#!/bin/sh
case "$1" in
  --version) echo 'codex-cli 0.0.1'; exit 0 ;;
  debug) echo '{"models":[]}'; exit 0 ;;
esac
stty -echo
echo CLI_SURVIVED
while IFS= read -r line; do echo "ECHO:$line"; done
'''
XTERM = """() => [...T.views.values()].map(view => {
  const b = view.term?.buffer?.active;
  return b ? Array.from({length:b.length}, (_,i) =>
    b.getLine(i)?.translateToString(true) || '').join('\\n') : '';
}).join('\\n')"""


def exercise(browser, base, root, uid, node_id=None):
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    errors = []
    page = context.new_page()
    page.on("pageerror", lambda error: errors.append(str(error)))
    try:
        page.goto(base, wait_until="networkidle")
        row_uid = scoped(node_id, uid) if node_id else uid
        page.locator(f'#side .item[data-uid="{row_uid}"]').first.click()
        expect(page.locator("#msgs")).to_contain_text("Preserved Claude history")
        open_dialog(page)
        if node_id:
            page.locator("#new-node").select_option(node_id)
        for source in ("claude", "grok", "opencode", "agy"):
            expect(page.locator(f'input[name="new-source"][value="{source}"]')).to_be_disabled()
        for source in ("codex", "shell"):
            expect(page.locator(f'input[name="new-source"][value="{source}"]')).to_be_enabled()
        page.locator('input[name="new-source"][value="codex"]').check()
        page.locator("#new-cwd").fill(str(root / "work"))
        with page.expect_response(lambda r: r.url.endswith("/api/term/create")) as created:
            page.locator("#new-session-go").click()
        assert created.value.status == 200, created.value.text()
        receipt = created.value.json()
        expect(page.locator("#new-session-dialog")).to_be_hidden()
        page.locator("#a-term").click()
        page.wait_for_function("(" + XTERM + ")().includes('CLI_SURVIVED')")
        keyboard = page.locator("#termpane .xterm-helper-textarea")
        keyboard.press_sequentially("still-working")
        keyboard.press("Enter")
        page.wait_for_function("(" + XTERM + ")().includes('ECHO:still-working')")
        assert not errors, errors
        return receipt
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    binary = parser.parse_args().binary.resolve()
    with tempfile.TemporaryDirectory(prefix="sessiondock-unavailable-cli-") as temporary, private_hosts(Path(temporary)):
        root = Path(temporary)
        for name in ("host", "ledger", "bin", "home", "work", "state", "hub", "claude", "codex", "grok"):
            (root / name).mkdir(mode=0o700)
        codex = root / "bin/codex"
        codex.write_text(FAKE_CODEX)
        codex.chmod(0o700)
        (root / "bin/opencode").symlink_to(root / "bin/removed-target")
        (root / "bin/agy").write_text("#!/bin/sh\nexit 0\n")
        (root / "bin/agy").chmod(0o600)
        (root / "bin/grok").mkdir()
        env = {"HOME": str(root / "home"), "PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}
        config = {"schema": 2, "host_binary": str(binary.parent / "ptyhost"), "host_dir": str(root / "host"),
                  "profiles": [{"id": source + "-cli", "source": source,
                                "executable": str(root / "bin" / source), "env": env}
                               for source in ("claude", "codex", "opencode", "agy")],
                  "adapters": [{"id": "grok-fixed", "source": "grok",
                                "executable": str(root / "bin/grok"), "args": [], "env": env}]}
        launcher = root / "launcher.json"
        launcher.write_text(json.dumps(config))
        original_config = launcher.read_bytes()
        subprocess.run([str(binary), "--initialize-lifecycle", str(root / "ledger")],
                       check=True, capture_output=True, timeout=15)
        corpus = Corpus(root)
        native = corpus.put("uninstalled-history", "claude", [
            claude_row("uninstalled-history", "user", "u1", None, "Preserved Claude history")], [])
        original_history = native.read_bytes()
        node = SimpleNamespace(port=free_port(), name="FixtureNode", token=TOKEN)
        network = node_env(root, node.port, "127.0.0.0/8")
        options = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        with sync_playwright() as pw:
            browser = pw.chromium.launch(**options)
            try:
                receipt = None
                for phase in ("startup", "restart"):
                    with isolated_server(corpus, binary, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                         launcher_config=launcher, state_dir=root / "state", extra_env=network) as (base, _):
                        if receipt:
                            context = browser.new_context()
                            pending = context.request.get(base + "/api/term/list").json()["pending"]
                            assert any(r["name"] == receipt["name"] and r["running"] for r in pending), pending
                            context.close()
                        receipt = exercise(browser, base, root, corpus.uid("uninstalled-history"))
                        node.nid = (root / "ids/node-id").read_text().strip()
                        hub_root = root / "hub" / phase
                        hub_root.mkdir(mode=0o700)
                        hub = Hub(binary.with_name("sessiondock-hub"), hub_root, [node])
                        hub.start()
                        try:
                            exercise(browser, f"http://127.0.0.1:{hub.port}", root,
                                     corpus.uid("uninstalled-history"), node.nid)
                            _, health = hub.json("GET", "/api/nodes")
                            assert health["nodes"][0]["online"] is True, health
                        finally:
                            hub.stop()
                        assert launcher.read_bytes() == original_config
                        assert native.read_bytes() == original_history
                        print(f"PASS {phase}: local + Hub history, disabled unavailable CLIs, Codex create/input", flush=True)
            finally:
                browser.close()


if __name__ == "__main__":
    main()
