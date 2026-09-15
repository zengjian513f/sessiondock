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
from urllib.parse import urlsplit, parse_qs

from playwright.sync_api import sync_playwright

from hub_http_suite import REPO, FakeNode, Hub
from hub_fake_node import PNG

CHROMIUM = Path.home() / ".cache/ms-playwright/chromium-1234/chrome-linux64/chrome"
NID = {"a": "a" * 32, "b": "b" * 32, "c": "c" * 32}
NAMES = {NID["a"]: "NodeA", NID["b"]: "NodeB", NID["c"]: "Vega"}


class Boundary:
    def __init__(self):
        self.calls = []
        self.capture_status = 200
        self.drafts = {}
        self.uploads = []

    def conversation(self, route):
        request=route.request;path=urlsplit(request.url).path
        query=parse_qs(urlsplit(request.url).query)
        body=request.post_data_json if request.method=='POST' and 'attachment' not in path else {}
        uid=body.get('uid') or query.get('uid',[''])[0]
        row=self.drafts.setdefault(uid, {'revision':0,'value':None})
        if path.endswith('/attachment'):
            self.uploads.append(uid)
            data={'ok':True,'upload_id':query['id'][0],'name':query['name'][0],'size':len(request.post_data_buffer or b'')}
        elif path.endswith('/drafts'):
            data={'drafts':[]}
        elif path.endswith('/check') or path.endswith('/import'):
            data={'ok':True}
        elif request.method=='POST':
            if body['revision']!=row['revision']:
                return route.fulfill(status=409,content_type='application/json',body='{"error":"另一页面已更新草稿"}')
            row={'revision':row['revision']+1,'value':body['value']};self.drafts[uid]=row
            data={'ok':True,'draft':row}
        else:
            data={'ok':True,'draft':row}
        return route.fulfill(status=200,content_type='application/json',body=json.dumps(data))

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


def open_report(page):
    button = page.locator("#report-bug")
    if not button.is_visible():
        page.locator("#header-more-btn").click()
    button.click()


def check_report_scroll(page):
    # The submit button used to be clipped by the dialog's overflow:hidden.
    # Scroll with the pointer and click by coordinates: locator.click() would
    # silently scroll inaccessible content into view and mask the regression.
    for width, height, attachments in [
            (608, 788, 1), (390, 640, 1),
            (390, 360, 1), (1280, 500, 12)]:
        page.set_viewport_size({"width": width, "height": height})
        page.evaluate("openBugReportDialog()")
        page.wait_for_selector("#bug-report-dialog[open]")
        page.locator("#bug-report-file").set_input_files([
            {"name": f"screen-{i}.png", "mimeType": "image/png", "buffer": PNG}
            for i in range(attachments)])
        textarea = page.locator('#bug-report-description')
        initial_height = textarea.bounding_box()['height']
        textarea.fill('提交按钮看不到了\n添加截图后被挡住了\n无法继续提交。')
        assert textarea.bounding_box()['height'] > initial_height
        page.wait_for_function("""() => {
            const input = document.querySelector('#bug-report-description').getBoundingClientRect();
            const add = document.querySelector('#bug-report-add').getBoundingClientRect();
            const send = document.querySelector('#bug-report-go').getBoundingClientRect();
            const items = document.querySelector('#bug-report-items').getBoundingClientRect();
            return Math.abs(add.bottom - input.bottom) < 1
                && Math.abs(send.bottom - input.bottom) < 1 && items.bottom <= input.top
                && add.right <= input.left && input.right <= send.left;
        }""")
        # Grow to the same 180px cap as the session composer, without imposing
        # any input limit. Overflowing text remains editable inside the textarea.
        textarea.fill('描述问题\n' * 30)
        assert textarea.bounding_box()['height'] == 180
        page.locator('#bug-report-add').click()
        page.wait_for_function("""() => {
            const menu = document.querySelector('#bug-report-attach-menu').getBoundingClientRect();
            const form = document.querySelector('#bug-report-form').getBoundingClientRect();
            return menu.top >= form.top && menu.bottom <= form.bottom;
        }""", timeout=5000)
        with page.expect_file_chooser():
            page.locator('#bug-report-attach-menu [data-attach=image]').click()
        # Keep the tall input for the reachability check, but submit an empty
        # draft so no report or CLI is created by the layout scenarios.
        page.evaluate("document.querySelector('#bug-report-description').value = '';bugReportDraftObject().text='';persistComposerDraft(BUG_REPORT_DRAFT_UID)")
        wait_drafts(page)
        bounds = page.locator("#bug-report-dialog").bounding_box()
        page.mouse.move(bounds["x"] + bounds["width"] / 2, bounds["y"] + bounds["height"] / 2)
        page.mouse.wheel(0, 3000)
        page.wait_for_function("""() => {
            const form = document.querySelector('#bug-report-form');
            const button = document.querySelector('#bug-report-go').getBoundingClientRect();
            const dialog = document.querySelector('#bug-report-dialog').getBoundingClientRect();
            return (form.scrollHeight <= form.clientHeight || form.scrollTop > 0)
                && button.top >= dialog.top && button.bottom <= dialog.bottom
                && button.bottom <= innerHeight;
        }""", timeout=5000)
        button = page.locator("#bug-report-go").bounding_box()
        page.mouse.click(button["x"] + button["width"] / 2, button["y"] + button["height"] / 2)
        assert page.locator("#bug-report-error").inner_text() == "请先描述遇到的问题"
        page.evaluate("""() => {
            document.querySelector('#bug-report-dialog').close();
            document.querySelector('#bug-report-description').style.height = '';
            clearBugReportDraft();
        }""")
    page.set_viewport_size({"width": 1280, "height": 900})



def wait_drafts(page):
    page.evaluate("async () => { for (;;) { const pending = composerDraftWrites; await pending; if (pending === composerDraftWrites) break; } }")


def check_shared_draft_recovery(page, boundary):
    page.evaluate("openBugReportDialog()")
    page.wait_for_function("!bugReportDraftObject().loading")
    page.fill('#bug-report-description','服务端保存 [附件1]')
    page.locator('#bug-report-file').set_input_files([
        {'name':'recover.png','mimeType':'image/png','buffer':PNG}])
    wait_drafts(page)
    uid=page.evaluate('BUG_REPORT_DRAFT_UID')
    assert boundary.drafts[uid]['value']['text']=='服务端保存 [附件1]'
    assert len(boundary.drafts[uid]['value']['attachments'])==1
    assert not boundary.uploads, boundary.uploads
    assert page.evaluate('composerUnloadProtected') # Unuploaded bytes are only in RAM.
    assert not page.evaluate("store.get('composerDraft.'+BUG_REPORT_DRAFT_UID,null)")
    assert not page.evaluate("async () => (await indexedDB.databases()).some(d=>d.name.endsWith('composer-drafts'))")
    page.locator('#bug-report-dialog .modal-close').click()
    page.reload(wait_until='networkidle')
    page.wait_for_function('Nodes.list.length===3')
    page.evaluate('openBugReportDialog()')
    page.wait_for_function('!bugReportDraftObject().loading')
    assert page.locator('#bug-report-description').input_value()=='服务端保存 [附件1]'
    assert not page.evaluate('bugReportDraftObject().attachments[0].file instanceof Blob')
    # Removing a missing File needs one click and no confirmation.
    page.locator('#bug-report-items .draft-remove').click()
    wait_drafts(page)
    assert not boundary.drafts[uid]['value']['attachments']
    page.select_option('#bug-report-node',NID['b'])
    page.wait_for_function('!bugReportDraftObject().loading')
    assert page.locator('#bug-report-description').input_value()==''
    page.fill('#bug-report-description','NodeB 自己的草稿');wait_drafts(page)
    page.select_option('#bug-report-node',NID['a'])
    page.wait_for_function('!bugReportDraftObject().loading')
    assert page.locator('#bug-report-description').input_value()=='服务端保存 [附件1]'
    assert not page.locator('.draft-saved').count()
    page.evaluate("async () => {for (const [uid,draft] of composerDrafts) {draft.text='';draft.attachments=[];draft.quotes=[];await persistComposerDraft(uid);}}");wait_drafts(page)
    page.locator('#bug-report-dialog .modal-close').click()
    boundary.calls.clear()


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
                    context.route(re.compile(r".*/api/session/conversation.*"),boundary.conversation)
                    context.route(re.compile(r".*/api/bug-report(/capture)?(\?.*)?$"), boundary.handle)
                    page = context.new_page()
                    errors = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="networkidle")
                    page.wait_for_function(
                        'S.sessions.length === 3 && Nodes.list.length === 3 && !!Nodes.capabilities["' + NID["b"] + '"]')

                    check_shared_draft_recovery(page, boundary)
                    check_report_scroll(page)

                    # 1. No session selected, all machines ticked: picker lists all three,
                    #    defaults to the first usable machine.
                    open_report(page)
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
                    open_report(page)
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
                    open_report(page)
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
                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    page.select_option("#bug-report-node", NID["b"])
                    assert page.evaluate("document.querySelector('#bug-report-source input[value=codex]').disabled")
                    assert page.evaluate("document.querySelector('#bug-report-source input[value=codex]').title") \
                        == "NodeB找不到 codex 命令"
                    assert not page.evaluate("document.querySelector('#bug-report-source input[value=claude]').disabled")
                    page.select_option("#bug-report-node", NID["a"])
                    assert not page.evaluate("document.querySelector('#bug-report-source input[value=codex]').disabled")
                    page.locator("#bug-report-dialog .modal-close").click()

                    assert not errors, errors
                    browser.close()
            finally:
                hub.stop()
    finally:
        for node in nodes:
            node.stop()
    print("PASS bug_report_node_browser: server drafts, session/node isolation, no selection upload, no browser message store, one-click removal, scrollable submit, picker and capture")


if __name__ == "__main__":
    main()
