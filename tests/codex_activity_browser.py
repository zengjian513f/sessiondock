#!/usr/bin/env python3
"""Opening a running Codex thread must not promote it in the sidebar."""
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from browser_runtime import choose_toolbar_option
from codex_activity_fixtures import ActivityDaemon, CHILD, NEW, OLD, at, corpus, maintenance
from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, server
from history_fixtures import BINARY, codex_message, encoded
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


def exercise(browser, base, data, fake, screenshots, label, width):
    context = browser.new_context(viewport={"width": width, "height": 900},
                                  color_scheme="dark" if width < 720 else "light",
                                  service_workers="block")
    page = context.new_page()
    page.set_default_timeout(15000)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(base, wait_until="networkidle")
    page.wait_for_function("S.sessions.length === 3 && T.listLoaded && S.liveSdk.size === 1")
    uid = page.evaluate("S.sessions.find(s => s.sid === " + repr(THREAD) + ").uid")
    path = data.paths["old"]
    initial_size = path.stat().st_size

    def show_list():
        back = page.locator("#detail .mobile-back")
        if back.is_visible():
            back.click()

    def stamp():
        return page.evaluate("uid => S.sessions.find(s => s.uid === uid).updated", uid)

    def order():
        return page.locator("#side .item[data-uid]").evaluate_all(
            "rows => rows.map(row => row.dataset.uid)")

    def refresh():
        page.evaluate("loadSessions(true)")

    before = order()
    assert before[-1] == uid, before
    assert stamp() == OLD
    page.screenshot(path=str(screenshots / f"{label}-{width}-before.png"), full_page=True)
    page.locator(f'#side .item[data-uid="{uid}"] .t').click()
    page.wait_for_function("SdkView.connected")
    assert len(fake.methods("thread/resume")) == 1
    assert path.stat().st_size > initial_size
    assert not fake.methods("turn/start")
    refresh()
    assert stamp() == OLD, {"updated": stamp(), "expected": OLD}
    page.screenshot(path=str(screenshots / f"{label}-{width}-connected.png"), full_page=True)
    show_list()
    assert order() == before
    page.screenshot(path=str(screenshots / f"{label}-{width}-attached-list.png"), full_page=True)
    print(f"PASS {label} {width}: live attach writes settings but keeps activity and order", flush=True)

    # A new browser and a fresh service index must also derive the old activity
    # from disk; retaining one frontend timestamp would not fix this.
    page.reload(wait_until="networkidle")
    show_list()
    assert stamp() == OLD
    assert order() == before
    for view in ("directory", "date", "tree"):
        choose_toolbar_option(page, "view", view)
        before = order()
        page.locator(f'#side .item[data-uid="{uid}"] .t').click()
        expect(page.locator(".sdk-history")).to_contain_text("synthetic 已完成")
        show_list()
        assert order() == before
    # A child's non-conversation writes must not promote its parent in nesting.
    with data.paths[CHILD].open("ab") as stream:
        stream.write(b"".join(map(encoded, maintenance())))
    choose_toolbar_option(page, "nest", "nested")
    refresh()
    assert order()[-1] == uid
    child = page.evaluate("uid => S.sessions.find(s => s.uid === uid).agent_items[0]", uid)
    assert child["updated"] == "2026-09-10T10:00:00.000Z", child

    # Native input/output still advances activity, after its complete JSONL
    # record arrives. Equal-size rewrite, replacement and truncation rebuild it.
    baseline = path.read_bytes()
    response = encoded(at(codex_message("assistant", "synthetic 真正的新答复"), NEW))
    with path.open("ab") as stream:
        stream.write(response[:-1])
    refresh()
    assert stamp() == OLD
    with path.open("ab") as stream:
        stream.write(b"\n")
    refresh()
    assert stamp() == NEW
    assert order()[0] == uid
    rewritten = response.replace(b"09-14", b"09-15")
    path.write_bytes(baseline + rewritten)
    refresh()
    assert stamp() == NEW.replace("09-14", "09-15")
    replacement = path.with_suffix(".replacement")
    replacement.write_bytes(baseline + response)
    replacement.replace(path)
    refresh()
    assert stamp() == NEW
    path.write_bytes(baseline)
    refresh()
    assert stamp() == OLD
    assert order()[-1] == uid

    # Cold indexing reads past metadata larger than both summary windows.
    # Touching mtime with no new conversation remains harmless.
    path.touch()
    refresh()
    assert stamp() == OLD
    page.locator("#q").fill("synthetic")
    page.locator("#q").press("Enter")
    page.wait_for_function("S.results !== null && S.results.length === 3")
    assert page.evaluate("uid => S.results.find(s => s.uid === uid).updated", uid) == OLD
    page.locator(f'#side .item[data-uid="{uid}"] .t').click()
    expect(page.locator(".sdk-history")).to_contain_text("synthetic 已完成")
    detail = page.request.get(base + "/api/messages/" + uid + "?window=1").json()
    assert detail["meta"]["updated"] == OLD
    show_list()
    page.screenshot(path=str(screenshots / f"{label}-{width}-after.png"), full_page=True)
    assert not errors, errors
    context.close()
    print(f"PASS {label} {width}: views, child, real activity, partial/rewrite/replace/truncate, search/detail", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path,
                        default=Path(tempfile.gettempdir()) / "sessiondock-codex-activity")
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sd-codex-activity-") as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for label, width in (("node", 1280), ("hub", 390)):
                root = Path(tmp) / label
                root.mkdir()
                data = corpus(root)
                extra = {**environment(root, args.binary), **node_env(root, free_port(), "127.0.0.0/8")}
                with ActivityDaemon(root / "app.sock", data.paths["old"], THREAD) as fake:
                    with server(data, args.binary, root / "app.sock", extra) as base:
                        hub = None
                        if label == "hub":
                            hub_root = root / "hub"
                            hub_root.mkdir()
                            hub = Hub(args.binary.resolve().with_name("sessiondock-hub"), hub_root,
                                      [SimpleNamespace(name="ActivityNode", token=TOKEN,
                                       port=int(extra["SESSIONDOCK_NODE_BIND"].rsplit(":", 1)[1]))])
                            hub.start()
                            base = f"http://127.0.0.1:{hub.port}"
                        try:
                            exercise(browser, base, data, fake, args.screenshots, label, width)
                        finally:
                            if hub:
                                hub.stop()
                    # Restart the isolated node: no previous Scan/cache survives.
                    with server(data, args.binary, root / "app.sock", extra) as base:
                        page = browser.new_page()
                        page.goto(base, wait_until="networkidle")
                        page.locator(f'#side .item[data-uid="{data.uid("old")}"]').click()
                        page.wait_for_function("SdkView.connected")
                        page.evaluate("loadSessions(true)")
                        assert page.evaluate("S.sessions.find(s => s.sid === " + repr(THREAD) + ").updated") == OLD
                        page.close()
                        print(f"PASS {label}: cold service restart keeps activity beyond metadata tail", flush=True)
        finally:
            browser.close()


if __name__ == "__main__":
    main()
