#!/usr/bin/env python3
"""An ended SSH session keeps its final screen, on node and through the Hub.

A synthetic shell prints more lines than the screen holds, then exits with
code 3. The page watching it switches the console to the read-only final
screen at once: history above the screen, the last line, and a status line
naming the exit code, with no timeline and no input. The exited row stays
listed with `final_screen` (no recording), the host wrote `screens/<id>/` and
no `records/`, and a fresh page opening the row (node, then Hub) shows the
same content read-only and fitted to a narrow window. No page errors.
"""

import json
import sys
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parent))
from playwright.sync_api import expect, sync_playwright  # noqa: E402

from history_fixtures import REPO, BINARY, Corpus, isolated_server  # noqa: E402
from hub_fixtures import Hub, free_port  # noqa: E402
from send_browser import initialize  # noqa: E402
from private_hosts import private_hosts  # noqa: E402

SHELL = ('stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; '
         'while IFS= read -r c; do case "$c" in '
         'lines) i=1; while [ $i -le 120 ]; do printf "FINAL_LINE_%03d\\n" $i; i=$((i+1)); done ;; '
         'quit) printf "RS_BYE\\n"; exit 3 ;; '
         '*) printf "RS_UNKNOWN\\n" ;; esac; done')
XTERM_TEXT = """() => { const view = currentTermViewObject(), buffer = view?.term?.buffer?.active;
  return buffer ? Array.from({length:buffer.length}, (_,i) =>
    buffer.getLine(i)?.translateToString(true) || '').join('\\n') : ''; }"""


def shows_final_screen(page):
    page.wait_for_function("document.querySelector('#termpane').classList.contains('final-screen')"
                           " && getComputedStyle(document.querySelector('#term-final-status')).display === 'block'"
                           " && currentTermViewObject()?.finalScreen", timeout=15000)
    page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_BYE')", timeout=15000)
    text = page.evaluate(XTERM_TEXT)
    assert 'FINAL_LINE_001' in text and 'FINAL_LINE_120' in text, text[-400:]
    expect(page.locator('#term-final-status')).to_contain_text('退出码 3')
    assert page.locator('#term-timeline, #tl-seek').count() == 0
    assert page.locator('.new-session-wait').is_hidden()
    expect(page.locator('#term-output-notice')).to_be_hidden()
    assert page.evaluate("getComputedStyle(document.querySelector('#xterm')).overflow") in ("auto", "scroll")
    assert page.locator("#a-term").get_attribute("data-unavailable") == "false"
    # Read-only: typing changes nothing and opens no terminal socket.
    before = page.evaluate(XTERM_TEXT)
    page.locator("#termpane .xterm-helper-textarea").press_sequentially("zzz")
    page.keyboard.press("Enter")
    page.wait_for_timeout(500)
    assert page.evaluate(XTERM_TEXT) == before
    assert not page.evaluate("T.ws")


def open_row(browser, base, uid, errors):
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.route('**/api/term/final?*', lambda route:
               route.fulfill(status=500, json={'error':'synthetic final screen failure'}))
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("uid => !!document.querySelector(`#side .item[data-uid=\"${uid}\"]`)", arg=uid, timeout=15000)
    page.locator(f'#side .item[data-uid="{uid}"]').first.click()
    expect(page.locator('#term-output-notice')).to_be_visible()
    expect(page.locator('#term-output-notice')).to_contain_text('最终画面读取失败')
    expect(page.locator('#float-stack > :visible, dialog.app-popup')).to_have_count(0)
    page.unroute('**/api/term/final?*')
    page.reload(wait_until='networkidle')
    page.locator(f'#side .item[data-uid="{uid}"]').first.click()
    shows_final_screen(page)
    # The final screen keeps the host's columns; a narrow window shrinks the font.
    page.set_viewport_size({'width': 700, 'height': 800})
    page.wait_for_timeout(300)
    assert page.locator('#xterm').evaluate('(e) => e.scrollWidth <= e.clientWidth + 1')
    context.close()


def run(browser, base, hub_base, node_id, root):
    errors = []
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(base, wait_until="networkidle")
    shell = context.request.post(base + "/api/term/create", data={
        "source": "shell", "cwd": str(root / "work"), "request_id": "final-screen"}).json()
    name = shell["name"]
    uid = "tmux:" + name
    page.evaluate("info => openPendingSession(info)", shell)
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN", timeout=10000)
    assert not page.evaluate("document.querySelector('#termpane').classList.contains('final-screen')")
    kb = page.locator("#termpane .xterm-helper-textarea")
    kb.press_sequentially("lines")
    kb.press("Enter")
    page.wait_for_function("(" + XTERM_TEXT + ")().includes('FINAL_LINE_120')", timeout=10000)
    kb.press_sequentially("quit")
    kb.press("Enter")
    # The page that watched the exit switches to the final screen itself.
    shows_final_screen(page)
    print("PASS live exit: the same page shows the final screen with history, read-only", flush=True)

    deadline = time.monotonic() + 15
    row = None
    while time.monotonic() < deadline:
        rows = [r for r in context.request.get(base + "/api/term/list?force=1").json().get("pending", [])
                if r["name"] == name]
        if rows and rows[0].get("state") == "exited" and rows[0].get("final_screen"):
            row = rows[0]
            break
        time.sleep(0.3)
    assert row and "recording" not in row, row
    screen_id = row["final_screen"]["id"]
    assert (root / "host" / "screens" / screen_id / "snapshot.json").is_file()
    assert not (root / "host" / "records").exists()
    print("PASS exited row lists final_screen; host wrote screens/ and no records/", flush=True)
    context.close()

    open_row(browser, base, uid, errors)
    print("PASS node: a fresh page opens the exited row on its final screen", flush=True)
    open_row(browser, hub_base, f"tmux:{node_id}~{name}", errors)
    print("PASS hub: a fresh Hub page opens the exited row on its final screen", flush=True)
    assert not errors, errors


def main():
    with tempfile.TemporaryDirectory(prefix="sessiondock-final-screen-") as temporary, private_hosts(Path(temporary)):
        root = Path(temporary)
        for name in ["host", "work", "ledger", "delivery", "state", "bin", "home", "claude", "codex", "grok", "hub"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        cfg = root / "launcher.json"
        cfg.touch(mode=0o600)
        cfg.write_text(json.dumps({
            "schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"), "host_dir": str(root / "host"),
            "adapters": [{"id": "synthetic-shell-v1", "source": "shell", "executable": str(Path("/bin/sh").resolve()),
                          "args": ["-c", SHELL], "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}}],
            "profiles": []}))
        initialize("--initialize-lifecycle", root / "ledger")
        node = SimpleNamespace(nid="a" * 32, name="FixtureNode", port=free_port(), token="f" * 64)
        token, identity = root / "node-token", root / "node-id"
        for path, value in [(token, node.token), (identity, node.nid)]:
            path.touch(mode=0o600)
            path.write_text(value)
        env = {"SESSIONDOCK_NODE_BIND": f"127.0.0.1:{node.port}",
               "SESSIONDOCK_NODE_TOKEN_FILE": str(token), "SESSIONDOCK_NODE_ID_FILE": str(identity),
               "SESSIONDOCK_NODE_PEERS": "127.0.0.0/8"}
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True)
            with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                 launcher_config=cfg, state_dir=root / "state", extra_env=env) as (base, _):
                hub = Hub(BINARY.with_name("sessiondock-hub"), root / "hub", [node])
                hub.start()
                try:
                    run(browser, base, f"http://127.0.0.1:{hub.port}", node.nid, root)
                finally:
                    hub.stop()
            browser.close()
        print("PASS terminal_final_screen_browser", flush=True)


if __name__ == "__main__":
    main()
