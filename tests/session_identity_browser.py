#!/usr/bin/env python3
"""Copy session identities through real Chromium menus and clipboard; private fixtures only."""
from browser_runtime import js
import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, build_corpus, isolated_server
from hub_http_suite import FakeNode, Hub, scoped


def menu(page, uid, touch=False):
    row = page.locator(f'#side .item[data-uid="{uid}"]')
    row.scroll_into_view_if_needed()
    page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
    if touch:
        box = row.bounding_box()
        event = {"pointerType": "touch", "pointerId": 17, "button": 0,
                 "clientX": box["x"] + 40, "clientY": box["y"] + 20}
        row.dispatch_event("pointerdown", event)
        expect(page.locator("#item-menu")).to_be_visible()
        row.dispatch_event("pointerup", event)
        row.dispatch_event("click")
    else:
        row.click(button="right")
    return page.get_by_role("menuitem", name="复制会话标识", exact=True)


def copy(page, uid, expected, touch=False):
    selected = page.evaluate(js("S.sel", 'runtime.core.state.selection.sel'))
    menu(page, uid, touch).click()
    expect(page.locator("#item-menu")).to_be_hidden()
    expect(page.locator("#session-stop-notice")).to_have_text("会话标识已复制。")
    assert page.evaluate("navigator.clipboard.readText()") == expected
    assert page.evaluate(js("S.sel", 'runtime.core.state.selection.sel')) == selected, "copy must not open another session"
    page.evaluate(js("showSessionStopNotice('')", "runtime.sessionUi.showSessionStopNotice('')"))


def context_for(browser, base, width=1280):
    context = browser.new_context(viewport={"width": width, "height": 900},
                                  permissions=["clipboard-read", "clipboard-write"])
    context.route("**/*", lambda route: route.continue_()
                  if route.request.url.startswith(base + "/") else route.abort())
    page = context.new_page()
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(base, wait_until="networkidle")
    page.wait_for_function(js("S.sessions.length > 0 && serverHostname", 'runtime.core.state.catalog.sessions.length > 0 && runtime.build.state.hostname'))
    return context, page, errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="session-identity-") as temporary, sync_playwright() as pw:
        root = Path(temporary)
        corpus = build_corpus(root / "corpus")
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = pw.chromium.launch(**launch)
        with isolated_server(corpus, args.binary, extra_env={"SESSIONDOCK_HOSTNAME": "FixtureLocal"}) as (base, _):
            for width in (1280, 390):
                context, page, errors = context_for(browser, base, width)
                for sid in ("claude-compact", "codex-parent"):
                    print(f"Copy {sid}, width={width}", flush=True)
                    uid = corpus.uid(sid)
                    row = page.evaluate(js("uid => S.sessions.find(s => s.uid === uid)", 'uid => runtime.core.state.catalog.sessions.find(s => s.uid === uid)'), uid)
                    expected = f"机器：FixtureLocal\n目录：{row['cwd']}\nagent：{row['source']}\nUUID：{sid}"
                    copy(page, uid, expected, width == 390)
                # Denied Clipboard API falls back to the browser's actual copy command.
                page.evaluate("() => { navigator.clipboard.writeText = async () => { throw new Error('denied'); }; }")
                copy(page, uid, expected, width == 390)
                # If both browser paths fail, report the failure visibly.
                page.evaluate("() => { document.execCommand = () => false; }")
                menu(page, uid, width == 390).click()
                expect(page.locator("dialog.app-popup")).to_contain_text("复制会话标识失败")
                page.get_by_role("button", name="知道了", exact=True).click()
                # Rows without a native ID must not copy a UID or launch name as a UUID.
                page.evaluate(js("uid => { S.sessions.find(s => s.uid === uid).sid = ''; }", "uid => { runtime.core.state.catalog.sessions.find(s => s.uid === uid).sid = ''; }"), uid)
                expect(menu(page, uid, width == 390)).to_have_attribute("aria-disabled", "true")
                assert not errors, errors
                context.close()
                print(f"PASS local identity: clipboard, fallback, failure, missing SID; width={width}", flush=True)

        nodes = [FakeNode("a" * 32, "FixtureA"), FakeNode("b" * 32, "FixtureB")]
        hub = None
        try:
            for node, source in zip(nodes, ("grok", "opencode")):
                row = node.state()["row"]
                node.set(row={**row, "source": source, "uid": f"{source}:same-file-hash",
                              "cwd": "/synthetic/中文 project"})
            hub_root = root / "hub"
            hub_root.mkdir()
            hub = Hub(args.binary.resolve().with_name("sessiondock-hub"), hub_root, nodes)
            hub.start()
            base = f"http://127.0.0.1:{hub.port}"
            context, page, errors = context_for(browser, base)
            page.wait_for_function(js("S.sessions.length === 2", 'runtime.core.state.catalog.sessions.length === 2'))
            for node, source in zip(nodes, ("grok", "opencode")):
                uid = scoped(node.nid, f"{source}:same-file-hash")
                copy(page, uid, f"机器：{node.name}\n目录：/synthetic/中文 project\nagent：{source}\nUUID：same-native-id")
            assert not errors, errors
            context.close()
            print("PASS Hub identity: owning machine, full Unicode path, agent, native SID instead of qualified UID", flush=True)
        finally:
            if hub:
                hub.stop()
            for node in nodes:
                node.stop()
            browser.close()


if __name__ == "__main__":
    main()
