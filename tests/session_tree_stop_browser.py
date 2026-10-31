#!/usr/bin/env python3
"""Row-menu "停止会话树" for sessions with mounted (nest_parent) children.

A synthetic Codex tree ROOT → CHILD → GRAND plus an unrelated LONE session
run in private ptyhost instances (fake CLI = shell that exits on Ctrl-D).
The item appears only on rows with mounted child sessions, is unavailable
while nothing in the tree runs, and stops exactly the running members of the
tree (LONE keeps running). In 按需展开 mode a collapsed branch still offers
the action and stops unloaded descendants from the complete catalog. Desktop
light and 390 px dark menus are captured for visual review. No model binary,
native CLI home or production host is touched.
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
from urllib.parse import urlsplit
from playwright.sync_api import sync_playwright, expect
from history_fixtures import REPO, BINARY, Corpus, codex_row, codex_message, isolated_server
from private_hosts import private_hosts
from popups import on_popup

SIDS = {name: f"7a1b2c3d-0000-4000-8000-{index:012d}" for index, name in
        enumerate(["root", "child", "grand", "lone"])}
FAKE_CODEX = """#!/bin/sh
exec /bin/sh -c 'stty -echo 2>/dev/null; printf "TREE_READY\\n"; while IFS= read -r line; do printf "OK\\n"; done'
"""


def menu_item(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').click(button="right")
    expect(page.locator("#item-menu")).to_be_visible()
    return page.locator('#item-menu [data-act="stop-tree"]')


def close_menu(page):
    page.locator("#detail").dispatch_event("pointerdown", {"bubbles": True})
    expect(page.locator("#item-menu")).to_be_hidden()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-tree-stop-") as temporary, private_hosts(Path(temporary)):
        root = Path(temporary).resolve()
        for name in ["host", "work", "ledger", "bin", "state", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        titles = {"root": "Tree root session", "child": "Mounted child session",
                  "grand": "Mounted grandchild session", "lone": "Unrelated running session"}
        for name, sid in SIDS.items():
            corpus.put(sid, "codex", [codex_row("session_meta", {"id": sid, "cwd": str(root / "work")}),
                                      codex_message("user", titles[name])], [])
        uid = {name: corpus.uid(sid) for name, sid in SIDS.items()}
        native = {name: path.read_bytes() for name, path in corpus.paths.items()}
        (root / "bin/fake-codex").write_text(FAKE_CODEX)
        (root / "bin/fake-codex").chmod(0o700)
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [
                {"id": "codex-cli-v1", "source": "codex", "executable": str(root / "bin/fake-codex"),
                 "args": [], "resume_args": ["resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}}]}))
        initialized = subprocess.run([str(BINARY), "--initialize-lifecycle", str(root / "ledger")],
            cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        assert initialized.returncode == 0, initialized.stderr.decode()
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, state_dir=root / "state", host_dir=root / "host",
                                     lifecycle_dir=root / "ledger", launcher_config=configuration) as (base, opener):
                    errors, dialogs, stops = [], [], []

                    def open_page(context, child_mode):
                        context.route("**/*", lambda route: route.continue_()
                                      if route.request.url.startswith(base + "/") else route.abort())
                        context.on("request", lambda request: stops.append(request.post_data_json["uid"])
                                   if urlsplit(request.url).path == "/api/session/stop" else None)
                        context.add_init_script(f"localStorage.setItem('sessiondock.childMode', {json.dumps(json.dumps(child_mode))})")
                        page = context.new_page()
                        page.on("pageerror", lambda error: errors.append(str(error)))

                        def on_dialog(dialog):
                            dialogs.append((dialog.type, dialog.message))
                            dialog.accept()
                        on_popup(page, on_dialog)
                        page.goto(base, wait_until="networkidle")
                        assert page.evaluate("SessionDockCapabilities.config.session_stop") is True
                        return page

                    def start(page, name):
                        started = page.request.post(base + "/api/term/takeover", data={"uid": uid[name]})
                        assert started.ok, started.text()
                        page.wait_for_function("uid => (refreshLive(true), loadTermList(), sessionStoppable(uid))",
                                               arg=uid[name], timeout=15000, polling=400)

                    # ---- Desktop, layered list: mount the tree through the real attach menu.
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    page = open_page(context, "nested")
                    for child, parent in (("child", "root"), ("grand", "child")):
                        page.locator(f'#side .item[data-uid="{uid[child]}"]').click(button="right")
                        page.locator('#item-menu [data-act="attach"]').click()
                        with page.expect_response(lambda r: urlsplit(r.url).path == "/api/session/nest") as nested:
                            page.locator(f'#side .item[data-uid="{uid[parent]}"]').click()
                        assert nested.value.ok, nested.value.text()
                    expect(page.locator(f'#side .item[data-uid="{uid["grand"]}"]')).to_have_attribute("data-depth", "2")
                    # Only rows with mounted children show the action.
                    for name in ("grand", "lone"):
                        expect(menu_item(page, uid[name])).to_be_hidden()
                        close_menu(page)
                    item = menu_item(page, uid["root"])
                    expect(item).to_be_visible()
                    expect(item).to_have_text("停止会话树")
                    expect(item).to_have_attribute("aria-disabled", "true")
                    expect(item).to_have_attribute("title", "会话树中没有运行中的会话。")
                    item.click(force=True)
                    expect(page.locator("#item-menu")).to_be_visible()
                    close_menu(page)

                    for name in ("root", "grand", "lone"):
                        start(page, name)
                    item = menu_item(page, uid["child"])
                    expect(item).to_be_visible()
                    expect(item).not_to_have_attribute("aria-disabled", "true")
                    close_menu(page)
                    item = menu_item(page, uid["root"])
                    expect(item).not_to_have_attribute("aria-disabled", "true")
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / "desktop-tree-stop-menu-light.png"))
                    before = len(stops)
                    item.click()
                    page.wait_for_function("!sessionStopBusy && !sessionStoppable(arguments[0]) && !sessionStoppable(arguments[1])"
                                           .replace("arguments[0]", json.dumps(uid["root"]))
                                           .replace("arguments[1]", json.dumps(uid["grand"])), timeout=20000)
                    confirm = [message for kind, message in dialogs if kind == "confirm"]
                    assert confirm and "停止会话树「Tree root session」" in confirm[-1] and "2 个挂载子会话" in confirm[-1] \
                        and "运行中的 2 个会话" in confirm[-1], dialogs
                    assert sorted(stops[before:]) == sorted([uid["root"], uid["grand"]]), stops[before:]
                    assert not [d for d in dialogs if d[0] == "alert"], dialogs
                    page.evaluate("refreshLive(true)")
                    assert page.evaluate("uid => sessionStoppable(uid)", uid["lone"]), "unrelated session must keep running"
                    expect(page.locator("#float-stack > :visible")).to_have_count(0)
                    expect(menu_item(page, uid["root"])).to_have_attribute("aria-disabled", "true")
                    close_menu(page)
                    assert not errors, errors
                    print("PASS layered tree stop: mounted rows only, stops root and grandchild, unrelated session kept", flush=True)
                    context.close()

                    # ---- 按需展开: collapsed branch, unloaded grandchild; 390 px dark.
                    mobile = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme="dark",
                                                 service_workers="block", has_touch=True)
                    page = open_page(mobile, "hidden")
                    expect(page.locator(f'#side .item[data-uid="{uid["child"]}"]')).to_have_count(0)
                    start(page, "grand")
                    row = page.locator(f'#side .item[data-uid="{uid["root"]}"]')
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / "mobile-lazy-list-dark.png"))
                    box = row.bounding_box()
                    row.dispatch_event("pointerdown", {"pointerType": "touch", "clientX": box["x"] + 40,
                                                       "clientY": box["y"] + 20, "bubbles": True})
                    expect(page.locator("#item-menu")).to_be_visible(timeout=3000)
                    row.dispatch_event("pointerup", {"pointerType": "touch", "bubbles": True})
                    row.dispatch_event("click", {"bubbles": True})
                    item = page.locator('#item-menu [data-act="stop-tree"]')
                    expect(item).to_be_visible()
                    expect(item).not_to_have_attribute("aria-disabled", "true")
                    bounds = page.locator("#item-menu").bounding_box()
                    assert bounds and bounds["x"] >= 0 and bounds["x"] + bounds["width"] <= 391, bounds
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / "mobile-lazy-tree-stop-menu-dark.png"))
                    page.locator(f'#side .item[data-uid="{uid["lone"]}"]').dispatch_event("contextmenu", {"bubbles": True})
                    expect(item).to_be_hidden()
                    row.dispatch_event("contextmenu", {"bubbles": True})
                    before = len(stops)
                    item.click()
                    page.wait_for_function(f"!sessionStopBusy && !sessionStoppable({json.dumps(uid['grand'])})", timeout=20000)
                    assert stops[before:] == [uid["grand"]], stops[before:]
                    page.evaluate("refreshLive(true)")
                    assert page.evaluate("uid => sessionStoppable(uid)", uid["lone"])
                    # Nothing left running in the tree: a fresh catalog read explains it.
                    row.dispatch_event("contextmenu", {"bubbles": True})
                    before_dialogs = len(dialogs)
                    item.click()
                    for _ in range(50):
                        if len(dialogs) > before_dialogs:
                            break
                        page.wait_for_timeout(100)
                    assert [kind for kind, _ in dialogs[before_dialogs:]] == ["alert"] \
                        and "及其 2 个挂载子会话都没有在运行" in dialogs[-1][1], dialogs[before_dialogs:]
                    assert len(stops) == before + 1, "an idle tree must not send stop requests"
                    assert not errors, errors
                    print("PASS lazy tree stop: collapsed branch offered, unloaded grandchild stopped at 390px", flush=True)
                    mobile.close()
                    assert {name: path.read_bytes() for name, path in corpus.paths.items()} == native
            finally:
                browser.close()
    print("PASS session tree stop browser")


if __name__ == "__main__":
    main()
