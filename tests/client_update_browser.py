#!/usr/bin/env python3
"""Machine settings: the AI client matrix — versions, newest versions and a manual update.

A real Rust node with synthetic CLI profiles only: a fake Codex whose `update`
prints coloured, carriage-return progress, waits, then raises its version; a
fake Claude whose `update` fails with an error on stderr; and a Grok profile
whose command is missing (status 127), which gets no column. A fake `curl`
first on the profiles' PATH answers the newest-version lookups (npm dist-tags
for Claude's `latest` channel, npm `latest` for Codex), so no network is used.
The matrix is exercised twice through the real UI: on the node's own page, and
on a real `sessiondock-hub` page whose requests reach the node through the
explicit `/api/nodes/<id>/api/…` proxy, next to a fake node that has Codex only
(its Claude cell is 无). The update receives the profile's fixed arguments
and a closed stdin; a second request while one is running is refused, and an
unknown profile ID is 404. No real CLI, native CLI home or production directory
is touched.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import TimeoutError as PlaywrightTimeout, expect, sync_playwright

from history_parity import REPO, BINARY, Corpus, isolated_server
from hub_http_suite import FakeNode, Hub, free_port

HUB_BINARY = BINARY.parent / ("sessiondock-hub.exe" if os.name == "nt" else "sessiondock-hub")
CODEX_ARGS = ["--enable", "default_mode_request_user_input", "-c", "suppress_unstable_features_warning=true"]
OLD, NEW = "0.159.0", "0.200.0"

FAKE_CODEX = """#!/bin/sh
state="$SESSIONDOCK_TEST_CLI_STATE"
eval "last=\\${$#}"
case "$last" in
  --version) printf 'codex-cli %s\\n' "$(cat "$state/codex.version")" ;;
  update)
    printf '%s\\n' "$@" > "$state/codex.argv"
    if IFS= read -r line; then echo stdin-open > "$state/codex.stdin"; else echo stdin-closed > "$state/codex.stdin"; fi
    printf '\\033[32m==>\\033[0m Updating Codex CLI\\n10%%\\r50%%\\r100%%\\n'
    sleep 1.5
    printf '%s' "$SESSIONDOCK_TEST_NEW" > "$state/codex.version"
    printf 'Codex CLI %s installed successfully.\\n' "$SESSIONDOCK_TEST_NEW"
    ;;
  *) exit 2 ;;
esac
"""
FAKE_CLAUDE = """#!/bin/sh
eval "last=\\${$#}"
case "$last" in
  --version) printf '2.1.1 (Claude Code)\\n' ;;
  update) printf 'Checking for updates\\n'; printf 'Error: network unreachable\\n' >&2; exit 3 ;;
  *) exit 2 ;;
esac
"""
MISSING = "#!/bin/sh\nexit 127\n"
FAKE_CURL = """#!/bin/sh
eval "url=\\${$#}"
case "$url" in
  */@anthropic-ai/claude-code/dist-tags) printf '{"stable":"2.0.9","latest":"2.1.1"}\\n' ;;
  */@openai/codex/latest) printf '{"name":"@openai/codex","version":"%s"}\\n' "$SESSIONDOCK_TEST_NEW" ;;
  *) exit 22 ;;
esac
"""


def open_machines(page):
    # A narrow header folds Settings into the More menu once the machine list has loaded,
    # so a visibility check can go stale before the click: decide again on each attempt.
    for attempt in range(5):
        try:
            if not page.locator("#settings").is_visible():
                page.locator("#header-more-btn").click(timeout=3000)
            page.locator("#settings").click(timeout=3000)
            break
        except PlaywrightTimeout:
            if attempt == 4:
                raise
            page.keyboard.press("Escape")
    page.click(".settings-tab[data-tab='machines']")
    page.wait_for_selector("#settings-machines:not([hidden])")


def matrix_row(page, machine):
    return page.locator("#client-matrix tbody tr").filter(has=page.locator("th", has_text=machine))


def check_machine(page, base, state, machine):
    page.goto(base + "/")
    open_machines(page)
    matrix = page.locator("#client-matrix table")
    # Columns are the clients installed on some machine: Grok is missing, OpenCode unconfigured.
    expect(matrix.locator("thead th")).to_have_text(["机器", "Claude", "Codex"])
    row = matrix_row(page, machine)
    codex = row.locator('td[data-client-source="codex"]')
    claude = row.locator('td[data-client-source="claude"]')
    expect(codex.locator(".client-version")).to_have_text(OLD)
    expect(codex).to_have_attribute("data-state", "outdated")
    expect(codex).to_have_text(f"{OLD}↑")                      # just the version and the arrow
    assert f"可更新到 {NEW}" in codex.get_attribute("title")
    expect(claude.locator(".client-version")).to_have_text("2.1.1")
    expect(claude).to_have_attribute("data-state", "current")
    expect(claude).to_have_text("2.1.1↑")

    # A successful update: the button waits while the CLI runs, then the cell is current.
    codex.locator(".client-update").click()
    expect(codex.locator(".client-update")).to_have_text("…")
    expect(codex.locator(".client-update")).to_be_disabled()
    expect(codex.locator(".client-version")).to_have_text(NEW, timeout=20000)
    expect(page.locator("#machine-note")).to_have_text(f"{machine}：Codex 已更新 {OLD} → {NEW}。")
    expect(codex).to_have_attribute("data-state", "current")
    expect(codex.locator(".client-update")).to_have_text("↑")
    expect(codex.locator(".client-update")).to_be_enabled()
    title = codex.get_attribute("title")
    assert f"Codex CLI {NEW} installed successfully." in title and "\x1b" not in title, title
    assert f"已是最新（{NEW}）" in title and "10%" not in title and "100%" in title, title
    argv = (state / "codex.argv").read_text().splitlines()
    assert argv == CODEX_ARGS + ["update"], argv
    assert (state / "codex.stdin").read_text().strip() == "stdin-closed"

    # A failing update reports the exit status and the CLI's last line of output.
    claude.locator(".client-update").click()
    expect(page.locator("#machine-note")).to_have_text(
        f"{machine}：Claude 更新失败（退出码 3）：Error: network unreachable", timeout=20000)
    expect(page.locator("#machine-note")).to_have_attribute("data-state", "error")
    expect(claude.locator(".client-version")).to_have_text("2.1.1")
    expect(claude.locator(".client-update")).to_be_enabled()


def api_contract(page, base):
    """Unknown profile → 404; a second update while one runs → 409."""
    result = page.evaluate("""async base => {
      const post = body => fetch(base + '/api/clients/update', {method: 'POST',
        headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body)})
        .then(async r => [r.status, (await r.json()).code || '']);
      return [await post({id: 'no-such-profile'}), await post({id: 'shell'}),
              await post({id: 'codex-cli-v1'}), await post({id: 'codex-cli-v1'})];
    }""", base)
    assert result == [[404, "unknown_client"], [404, "unknown_client"], [200, ""], [409, "client_update_running"]], result
    # The accepted update must finish before the next pass resets the fake version.
    deadline = time.monotonic() + 20
    while page.evaluate("""base => fetch(base + '/api/clients').then(r => r.json())
      .then(d => d.clients.some(c => c.update && c.update.running))""", base):
        assert time.monotonic() < deadline, "the contract's update did not finish"
        time.sleep(0.2)


def check_phone(page, base):
    page.set_viewport_size({"width": 390, "height": 844})
    page.goto(base + "/")
    open_machines(page)
    expect(page.locator("#client-matrix tbody tr")).to_have_count(2)   # Pavo and the offline Vega
    # The matrix may scroll inside its own box; the dialog and the page never scroll sideways.
    overflow = page.evaluate("""() => [document.scrollingElement, document.querySelector('#settings-machines')]
      .map(box => box.scrollWidth - box.clientWidth)""")
    assert all(extra <= 0 for extra in overflow), overflow


def main():
    if os.name != "posix":
        raise SystemExit("Fake CLI scripts need POSIX sh.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-client-update-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "ledger", "bin", "state", "hub", "home"]:
            (root / name).mkdir(mode=0o700)
        state = root / "state"
        for name, body in [("fake-codex", FAKE_CODEX), ("fake-claude", FAKE_CLAUDE), ("missing-grok", MISSING),
                           ("curl", FAKE_CURL)]:
            (root / "bin" / name).write_text(body)
            (root / "bin" / name).chmod(0o700)
        env = {"HOME": str(root / "home"), "PATH": str(root / "bin") + ":/usr/bin:/bin", "TERM": "xterm-256color",
               "SESSIONDOCK_TEST_CLI_STATE": str(state), "SESSIONDOCK_TEST_NEW": NEW}
        launcher = {"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
                    "host_dir": str(root / "host"), "adapters": [], "profiles": [
                        {"id": "codex-cli-v1", "source": "codex", "executable": str(root / "bin/fake-codex"),
                         "args": CODEX_ARGS, "resume_args": ["resume", "{sid}"], "env": env},
                        {"id": "claude-cli-v1", "source": "claude", "executable": str(root / "bin/fake-claude"),
                         "args": [], "env": env},
                        {"id": "grok-cli-v1", "source": "grok", "executable": str(root / "bin/missing-grok"),
                         "args": [], "env": env}]}
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps(launcher))
        initialized = subprocess.run([str(BINARY), "--initialize-lifecycle", str(root / "ledger")],
                                     cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        assert initialized.returncode == 0, initialized.stderr.decode()
        token = "t" * 40
        (root / "node.token").write_text(token + "\n")
        (root / "node.token").chmod(0o600)
        node_port = free_port()
        node_env = {"SESSIONDOCK_NODE_BIND": f"127.0.0.1:{node_port}",
                    "SESSIONDOCK_NODE_TOKEN_FILE": str(root / "node.token"),
                    "SESSIONDOCK_NODE_ID_FILE": str(root / "node.id"),
                    "SESSIONDOCK_NODE_PEERS": "127.0.0.0/8"}
        corpus = Corpus(root)
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, extra_env=node_env) as (base, _):
                    errors = []
                    page = browser.new_page(viewport={"width": 1280, "height": 900})
                    page.on("pageerror", lambda error: errors.append(str(error)))

                    (state / "codex.version").write_text(OLD)
                    check_machine(page, base, state, "本机")
                    api_contract(page, base)
                    print("PASS local machine settings: versions, update, failure, API contract")

                    (state / "codex.version").write_text(OLD)
                    other = FakeNode("b" * 32, "Vega")
                    hub = Hub(HUB_BINARY, root / "hub", [SimpleNamespace(name="Pavo", port=node_port, token=token), other])
                    hub.start()
                    try:
                        hub_base = f"http://127.0.0.1:{hub.port}"
                        check_machine(page, hub_base, state, "Pavo")
                        vega = matrix_row(page, "Vega")
                        expect(vega.locator('td[data-client-source="claude"]')).to_have_text("无")
                        expect(vega.locator('td[data-client-source="codex"] .client-version')).to_have_text("0.1.0")
                        expect(vega.locator('td[data-client-source="codex"]')).to_have_attribute("data-state", "current")
                        print("PASS hub matrix through the explicit node proxy, with an empty cell")
                        # A machine that cannot be reached reads 离线, never 失败.
                        other.stop()
                        page.goto(hub_base + "/")
                        open_machines(page)
                        expect(matrix_row(page, "Vega").locator("td.client-status")).to_have_text("离线", timeout=20000)
                        expect(matrix_row(page, "Pavo").locator('td[data-client-source="codex"] .client-version')).to_have_text(NEW)
                        print("PASS an unreachable machine reads 离线")
                        check_phone(page, hub_base)
                        print("PASS 390px machine row without horizontal overflow")
                    finally:
                        hub.stop()
                        other.stop()
                    assert not errors, errors
            finally:
                browser.close()
    print("client_update_browser: all checks passed")


if __name__ == "__main__":
    main()
