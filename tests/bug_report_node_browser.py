#!/usr/bin/env python3
"""The report dialog on the hub page: its machine picker and the two-step cross-machine submit.

Three fake nodes behind `sessiondock-hub` (`tests/hub_fake_node.py`). The picker lists every
machine like the new-session dialog and defaults to the problem's machine (the selected
session's); a worker elsewhere first asks the problem's machine for `/api/bug-report/capture`
and hands the answer to the worker's machine as `captured` (a failed capture becomes
`captured: {error}`); the chosen machine's missing CLIs are greyed out. `/api/bug-report` and
`/api/bug-report/capture` are answered at the browser boundary so the bodies the page builds
can be asserted. No CLI, no session root.
"""
from __future__ import annotations

import argparse
import json
import re
import tempfile
from pathlib import Path

from playwright.sync_api import sync_playwright

from hub_http_suite import REPO, FakeNode, Hub

CHROMIUM = Path.home() / ".cache/ms-playwright/chromium-1234/chrome-linux64/chrome"
NID = {"a": "a" * 32, "b": "b" * 32, "c": "c" * 32}
NAMES = {NID["a"]: "NodeA", NID["b"]: "NodeB", NID["c"]: "Vega"}


class Boundary:
    def __init__(self):
        self.calls = []
        self.capture_status = 200

    def handle(self, route):
        request = route.request
        if request.method != "POST":
            return route.continue_()
        body = json.loads(request.post_data or "{}")
        path = request.url.split("?")[0].rsplit("/api/", 1)[1]
        self.calls.append((path, body))
        if path == "bug-report/capture":
            if self.capture_status != 200:
                return route.fulfill(status=self.capture_status, content_type="application/json",
                                     body=json.dumps({"error": "NodeA 离线：中央站未能连接该机器", "node_offline": True}))
            return route.fulfill(status=200, content_type="application/json", body=json.dumps({
                "ok": True, "hostname": "nodea-host", "captured_at": "2026-09-15T08:00:00Z",
                "uid": body.get("uid", ""), "terminal_name": body.get("terminal_name", ""),
                "session": {"uid": body.get("uid", ""), "cwd": "/srv/a"}, "outbox": {},
                "terminal_capture": "frame", "events": [{"event": "browser.click"}]}))
        if path == "bug-report":
            node = body.get("_node", "")
            name = NAMES.get(node, "")
            return route.fulfill(status=202, content_type="application/json", body=json.dumps({
                "ok": True, "report_id": "BUG-TEST", "path": "/tmp/BUG-TEST",
                "worker": {"name": f"{node}~w", "source": body.get("source"), "node_id": node,
                           "node_name": name, "kind": "bug-report", "title": "处理 BUG-TEST"}}))
        return route.continue_()


def options(page):
    return page.evaluate("""() => [...document.querySelectorAll('#bug-report-node option')]
        .map(o => ({value: o.value, text: o.textContent, disabled: o.disabled}))""")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", default=str(REPO / "target/release/sessiondock"))
    parser.add_argument("--hub-binary", default=None)
    parser.add_argument("--screenshot", default=None, help="write a screenshot of the open dialog here")
    args = parser.parse_args()
    hub_binary = Path(args.hub_binary) if args.hub_binary else Path(args.binary).resolve().parent / "sessiondock-hub"
    nodes = [FakeNode(nid, name) for nid, name in NAMES.items()]
    try:
        with tempfile.TemporaryDirectory(prefix="sessiondock-bugnode-") as temp:
            hub = Hub(hub_binary, Path(temp), nodes)
            hub.start()
            try:
                with sync_playwright() as playwright:
                    launch = {"headless": True}
                    if CHROMIUM.is_file():
                        launch["executable_path"] = str(CHROMIUM)
                    browser = playwright.chromium.launch(**launch)
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    boundary = Boundary()
                    context.route(re.compile(r".*/api/bug-report(/capture)?(\?.*)?$"), boundary.handle)
                    page = context.new_page()
                    errors = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="networkidle")
                    page.wait_for_function(
                        'S.sessions.length === 3 && Nodes.list.length === 3 && !!Nodes.capabilities["' + NID["b"] + '"]')

                    # 1. No session selected, all machines ticked: picker lists all three,
                    #    defaults to the first usable machine.
                    page.locator("#report-bug").click()
                    page.wait_for_selector("#bug-report-dialog[open]")
                    assert page.locator("#bug-report-node-label").is_visible()
                    opts = options(page)
                    assert [o["text"] for o in opts] == ["NodeA", "NodeB", "Vega"], opts
                    assert not any(o["disabled"] for o in opts), opts
                    assert page.evaluate("bugReportNode()") == NID["a"]
                    if args.screenshot:
                        page.locator("#bug-report-dialog .report-form").screenshot(path=args.screenshot)
                    # Pick NodeB: no session → origin is the worker machine, single request.
                    page.select_option("#bug-report-node", NID["b"])
                    page.fill("#bug-report-description", "列表页卡住")
                    page.locator("#bug-report-go").click()
                    page.wait_for_function("!document.querySelector('#bug-report-dialog').open")
                    paths = [p for p, _ in boundary.calls]
                    assert paths == ["bug-report"], paths
                    body = boundary.calls[0][1]
                    assert body["_node"] == NID["b"], body
                    assert body["uid"] == "" and "captured" not in body, body
                    assert body["origin"] == {"node_id": NID["b"], "node_name": "NodeB", "uid": ""}, body["origin"]
                    page.wait_for_function("!document.querySelector('#bug-report-toast').classList.contains('hidden')")
                    toast = page.locator("#bug-report-toast").inner_text()
                    assert "已保存到 NodeB" in toast, toast
                    assert page.evaluate("localStorage.getItem('sessiondock.hub./.bugReportNode')") == json.dumps(NID["b"])
                    boundary.calls.clear()

                    # 2. A NodeA session selected: the picker defaults to NodeA (the problem's
                    #    machine) even though NodeB was remembered; choosing Vega goes two-step.
                    row = page.locator("#side .item[data-uid^='claude:" + NID["a"] + "~']").first
                    row.click()
                    page.wait_for_function("S.sel && S.sel.includes('" + NID["a"] + "~')")
                    sel = page.evaluate("S.sel")
                    page.locator("[data-report-bug]").first.click()
                    page.wait_for_selector("#bug-report-dialog[open]")
                    assert page.evaluate("bugReportNode()") == NID["a"]
                    page.select_option("#bug-report-node", NID["c"])
                    page.fill("#bug-report-description", "NodeA 上的会话消息不刷新")
                    page.locator("#bug-report-go").click()
                    page.wait_for_function("!document.querySelector('#bug-report-dialog').open")
                    paths = [p for p, _ in boundary.calls]
                    assert paths == ["bug-report/capture", "bug-report"], paths
                    capture = boundary.calls[0][1]
                    assert capture["_node"] == NID["a"] and capture["uid"] == sel, capture
                    report = boundary.calls[1][1]
                    assert report["_node"] == NID["c"] and report["uid"] == "" and report["terminal_name"] == "", report
                    assert report["origin"] == {"node_id": NID["a"], "node_name": "NodeA", "uid": sel}, report["origin"]
                    assert report["captured"]["hostname"] == "nodea-host", report["captured"]
                    assert report["captured"]["session"]["cwd"] == "/srv/a", report["captured"]
                    assert report["captured"]["events"] == [{"event": "browser.click"}], report["captured"]
                    toast = page.locator("#bug-report-toast").inner_text()
                    assert "已保存到 Vega" in toast, toast
                    boundary.calls.clear()

                    # 3. The problem's machine is unreachable: the report still goes out with
                    #    `captured: {error}`.
                    boundary.capture_status = 503
                    page.locator("[data-report-bug]").first.click()
                    page.wait_for_selector("#bug-report-dialog[open]")
                    assert page.evaluate("bugReportNode()") == NID["a"]
                    page.select_option("#bug-report-node", NID["b"])
                    page.fill("#bug-report-description", "抓取失败也要能报")
                    page.locator("#bug-report-go").click()
                    page.wait_for_function("!document.querySelector('#bug-report-dialog').open")
                    paths = [p for p, _ in boundary.calls]
                    assert paths == ["bug-report/capture", "bug-report"], paths
                    report = boundary.calls[1][1]
                    assert report["_node"] == NID["b"], report
                    assert report["captured"] == {"error": "NodeA 离线：中央站未能连接该机器"}, report["captured"]
                    assert report["origin"]["node_name"] == "NodeA", report["origin"]

                    # 4. A source the chosen machine lacks is greyed out.
                    nodes[1].set(term_sources={"claude": True, "codex": False})
                    page.evaluate("loadTermList()")
                    page.wait_for_function(
                        'Nodes.capabilities["' + NID["b"] + '"].sources.codex === false')
                    page.locator("[data-report-bug]").first.click()
                    page.wait_for_selector("#bug-report-dialog[open]")
                    page.select_option("#bug-report-node", NID["b"])
                    assert page.evaluate("document.querySelector('#bug-report-source input[value=codex]').disabled")
                    assert page.evaluate("document.querySelector('#bug-report-source input[value=codex]').title") \
                        == "NodeB找不到 codex 命令"
                    assert not page.evaluate("document.querySelector('#bug-report-source input[value=claude]').disabled")
                    page.select_option("#bug-report-node", NID["a"])
                    assert not page.evaluate("document.querySelector('#bug-report-source input[value=codex]').disabled")
                    page.locator("#bug-report-dialog .modal-cancel").click()

                    assert not errors, errors
                    browser.close()
            finally:
                hub.stop()
    finally:
        for node in nodes:
            node.stop()
    print("PASS bug_report_node_browser: picker, two-step submit, failed capture, greyed sources")


if __name__ == "__main__":
    main()
