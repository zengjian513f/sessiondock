#!/usr/bin/env python3
"""The report dialog on the hub page: its machine picker and the two-step cross-machine submit.

Three fake nodes behind `sessiondock-hub` (`tests/hub_fake_node.py`). The picker lists every
machine like the new-session dialog and defaults to the problem's machine (the selected
session's); a worker elsewhere first asks the problem's machine for `/api/bug-report/capture`
and hands the answer to the worker's machine as `captured` (a failed capture becomes
`captured: {error}`); the chosen machine's missing CLIs are greyed out. Wide layout keeps
source names and a bounded machine picker; 390px puts two attachments on one row.
`/api/bug-report` and `/api/bug-report/capture` are answered at the browser boundary so the
bodies the page builds can be asserted. No CLI, no session root.
"""
from __future__ import annotations

import argparse
import json
import re
import tempfile
import time
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
        self.previews = []
        self.discards = []
        self.defer = False
        self.pending = []

    def conversation(self, route):
        request=route.request;path=urlsplit(request.url).path
        query=parse_qs(urlsplit(request.url).query)
        body=request.post_data_json if request.method=='POST' and ('attachment' not in path or path.endswith('/discard')) else {}
        uid=body.get('uid') or query.get('uid',[''])[0]
        row=self.drafts.setdefault(uid, {'revision':0,'value':None})
        if path.endswith('/attachment') and request.method=='GET':
            # Staged bytes read back for an image card that holds no File.
            self.previews.append((uid,query['id'][0]))
            return route.fulfill(status=200,content_type='image/png',body=PNG)
        if path.endswith('/attachment'):
            self.uploads.append(uid)
            data={'ok':True,'upload_id':query['id'][0],'name':query['name'][0],'size':len(request.post_data_buffer or b'')}
        elif path.endswith('/attachment/discard'):
            self.discards.append((uid,body.get('id')))
            data={'ok':True,'removed':True}
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
        path = request.url.split("?")[0].rsplit("/api/", 1)[1].rstrip("/")
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
            if self.defer:
                self.pending.append(route)
                return
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
    # Resizing can unfold the button between visibility lookup and click.
    # Resolve whichever entry is currently visible on every click retry.
    page.locator("#report-bug:visible, #header-more-btn:visible").first.click()
    if not page.locator("#bug-report-dialog").is_visible():
        page.locator("#report-bug").click()


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
            const sources = document.querySelector('#bug-report-source').getBoundingClientRect();
            return Math.abs(add.bottom - input.bottom) < 1
                && Math.abs(send.bottom - input.bottom) < 1 && items.bottom <= input.top
                && add.right <= input.left && input.right <= send.left
                && Math.abs(send.right - sources.right) <= 1;
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


def check_send_busy_width(page, boundary):
    page.evaluate("openBugReportDialog()")
    page.wait_for_selector("#bug-report-dialog[open]")
    page.fill("#bug-report-description", "发送按钮不要变宽")
    idle = page.locator("#bug-report-go").evaluate("el => el.getBoundingClientRect().width")
    boundary.defer = True
    page.locator("#bug-report-go").click()
    page.wait_for_function("document.querySelector('#bug-report-go').getAttribute('aria-busy') === 'true'")
    busy = page.locator("#bug-report-go").evaluate("""el => {
      const after = getComputedStyle(el, '::after');
      return {
        width: el.getBoundingClientRect().width, text: el.textContent,
        busy: el.getAttribute('aria-busy'), spin: after.animationName,
      };
    }""")
    assert busy["text"] == "发送" and busy["busy"] == "true", busy
    assert abs(busy["width"] - idle) < 0.51, (idle, busy)
    assert "send-spin" in (busy["spin"] or ""), busy
    deadline = time.monotonic() + 10
    while not boundary.pending:
        assert time.monotonic() < deadline, ('bug-report not intercepted', busy, boundary.calls)
        page.wait_for_timeout(20)
    for route in boundary.pending:
        route.fulfill(status=500, content_type="application/json",
                      body='{"error":"synthetic hold"}')
    boundary.pending.clear()
    boundary.defer = False
    page.wait_for_function("!bugReportSending")
    page.evaluate("""() => {
      document.querySelector('#bug-report-dialog').close();
      clearBugReportDraft();
    }""")
    wait_drafts(page)
    boundary.calls.clear()


def wait_drafts(page):
    page.evaluate("async () => { for (;;) { const pending = composerDraftWrites; await pending; if (pending === composerDraftWrites) break; } }")


def check_report_layout(page):
    # Wide dialog: source buttons keep their names, and the machine picker
    # stays on the same row without eating the leftover width.
    page.set_viewport_size({"width": 1280, "height": 900})
    page.evaluate("openBugReportDialog()")
    page.wait_for_selector("#bug-report-dialog[open]")
    page.wait_for_function("!document.querySelector('#bug-report-node-label').hidden")
    wide = page.evaluate("""() => {
      const labels = [...document.querySelectorAll('#bug-report-source .src-label')];
      const hidden = labels.filter(el => !el.offsetWidth || getComputedStyle(el).display === 'none');
      const row = document.querySelector('.report-row').getBoundingClientRect();
      const select = document.querySelector('#bug-report-node-label').getBoundingClientRect();
      const sources = document.querySelector('#bug-report-source').getBoundingClientRect();
      const send = document.querySelector('#bug-report-go').getBoundingClientRect();
      return {
        names: labels.map(el => el.textContent),
        hidden: hidden.length,
        select_wider: select.width > sources.width,
        one_row: Math.abs(select.top - sources.top) <= 2
          && select.right <= sources.left + 1
          && select.bottom <= row.bottom + 1,
        select_frac: select.width / row.width,
        send_align: Math.abs(send.right - sources.right),
      };
    }""")
    assert wide["names"] == ["Claude", "Codex", "Grok"], wide
    assert wide["hidden"] == 0, wide
    assert wide["one_row"] and not wide["select_wider"] and wide["select_frac"] <= 0.42 + 1e-6, wide
    assert wide["send_align"] <= 1, wide
    page.evaluate("document.querySelector('#bug-report-dialog').close()")

    # Phone: two attachments share one row instead of stacking at 100% width.
    page.set_viewport_size({"width": 390, "height": 844})
    page.evaluate("openBugReportDialog()")
    page.wait_for_selector("#bug-report-dialog[open]")
    page.locator("#bug-report-file").set_input_files([
        {"name": f"screen-{i}.png", "mimeType": "image/png", "buffer": PNG}
        for i in range(2)])
    page.wait_for_function("document.querySelectorAll('#bug-report-items .draft-card').length === 2")
    phone = page.evaluate("""() => {
      const cards = [...document.querySelectorAll('#bug-report-items .draft-card')]
        .map(el => el.getBoundingClientRect());
      const row = document.querySelector('#bug-report-items').getBoundingClientRect();
      return {
        same_row: Math.abs(cards[0].top - cards[1].top) <= 2,
        gap: cards[1].left - cards[0].right,
        overflow: cards[0].left < row.left - 1 || cards[1].right > row.right + 1,
        frac: Math.max(cards[0].width, cards[1].width) / row.width,
      };
    }""")
    assert phone["same_row"] and phone["gap"] >= 0 and not phone["overflow"] and phone["frac"] <= 0.55, phone
    page.evaluate("""() => {
      document.querySelector('#bug-report-dialog').close();
      clearBugReportDraft();
    }""")
    wait_drafts(page)
    page.set_viewport_size({"width": 1280, "height": 900})


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
    # Selection stages the bytes on the chosen machine at once; nothing is left only in RAM.
    page.wait_for_function("bugReportDraftObject().attachments[0]?.uploaded?.upload_id && !bugReportDraftObject().attachments[0].staging")
    wait_drafts(page)
    assert boundary.uploads==[uid], boundary.uploads
    assert boundary.drafts[uid]['value']['attachments'][0]['uploaded']['upload_id']
    assert not page.evaluate('composerUnloadProtected')
    assert not page.evaluate("store.get('composerDraft.'+BUG_REPORT_DRAFT_UID,null)")
    assert not page.evaluate("async () => (await indexedDB.databases()).some(d=>d.name.endsWith('composer-drafts'))")
    page.locator('#bug-report-dialog .modal-close').click()
    page.reload(wait_until='networkidle')
    page.wait_for_function('Nodes.list.length===3')
    page.evaluate('openBugReportDialog()')
    page.wait_for_function('!bugReportDraftObject().loading')
    assert page.locator('#bug-report-description').input_value()=='服务端保存 [附件1]'
    assert not page.evaluate('bugReportDraftObject().attachments[0].file instanceof Blob')
    # That card still shows a thumbnail: the bytes come back from staging.
    staged_id=page.evaluate('bugReportDraftObject().attachments[0].uploaded.upload_id')
    page.wait_for_function("document.querySelector('#bug-report-items .draft-card .draft-thumb img')?.src.startsWith('blob:') || false", timeout=5000)
    assert boundary.previews==[(uid,staged_id)], boundary.previews
    # Removing a staged attachment needs one click and no confirmation; the
    # staged bytes are released after the draft without it is saved.
    page.locator('#bug-report-items .draft-remove').click()
    wait_drafts(page)
    assert not boundary.drafts[uid]['value']['attachments']
    deadline=time.time()+5
    while not boundary.discards and time.time()<deadline: page.wait_for_timeout(100) # Pumps the route handlers.
    assert boundary.discards and boundary.discards[0][0]==uid, boundary.discards
    # 换处理机器只是换跑处理会话的机器，不是另开一份报告：已经写好的描述跟着
    # 这次选择搬到新机器的草稿上，原机器那份随即清空（BUG-20260919-080848）。
    page.select_option('#bug-report-node',NID['b'])
    page.wait_for_function('!bugReportDraftObject().loading')
    assert page.locator('#bug-report-description').input_value()=='服务端保存 [附件1]'
    other=page.evaluate('BUG_REPORT_DRAFT_UID')
    assert other!=uid
    wait_drafts(page)
    assert boundary.drafts[other]['value']['text']=='服务端保存 [附件1]', boundary.drafts[other]
    assert boundary.drafts[uid]['value']['text']=='', boundary.drafts[uid]
    # 空着的输入框不搬任何东西：切回去看到的仍是那台机器自己的草稿。
    page.fill('#bug-report-description','');wait_drafts(page)
    page.select_option('#bug-report-node',NID['a'])
    page.wait_for_function('!bugReportDraftObject().loading')
    assert page.evaluate('BUG_REPORT_DRAFT_UID')==uid
    assert page.locator('#bug-report-description').input_value()==''
    assert not page.locator('.draft-saved').count()
    page.evaluate("async () => {for (const [uid,draft] of composerDrafts) {draft.text='';draft.attachments=[];draft.quotes=[];await persistComposerDraft(uid);}}");wait_drafts(page)
    page.locator('#bug-report-dialog .modal-close').click()
    boundary.calls.clear()


def check_draft_follows_machine(page, boundary):
    # The user's complaint: typing the report, then picking another machine,
    # emptied the box. Description, quotes and the attachments this page still
    # holds follow the selection; the previous machine keeps nothing.
    boundary.uploads.clear();boundary.discards.clear()
    page.evaluate("openBugReportDialog()")
    page.wait_for_function("!bugReportDraftObject().loading")
    page.fill('#bug-report-description','切换机器也别清空 [附件1]')
    page.locator('#bug-report-file').set_input_files([
        {'name':'switch.png','mimeType':'image/png','buffer':PNG}])
    page.wait_for_function("bugReportDraftObject().attachments[0]?.uploaded?.upload_id && !bugReportDraftObject().attachments[0].staging")
    wait_drafts(page)
    first=page.evaluate('BUG_REPORT_DRAFT_UID')
    assert boundary.uploads==[first], boundary.uploads
    page.select_option('#bug-report-node',NID['c'])
    page.wait_for_function('!bugReportDraftObject().loading')
    second=page.evaluate('BUG_REPORT_DRAFT_UID')
    assert second!=first
    assert page.locator('#bug-report-description').input_value()=='切换机器也别清空 [附件1]'
    assert not page.locator('#bug-report-error').inner_text()
    # The card's bytes are staged again on the machine that will handle it.
    page.wait_for_function("bugReportDraftObject().attachments.length===1 && bugReportDraftObject().attachments[0].uploaded?.uid===BUG_REPORT_DRAFT_UID && !bugReportDraftObject().attachments[0].staging")
    wait_drafts(page)
    assert boundary.uploads==[first,second], boundary.uploads
    assert boundary.drafts[second]['value']['text']=='切换机器也别清空 [附件1]'
    assert len(boundary.drafts[second]['value']['attachments'])==1
    assert boundary.drafts[second]['value']['attachments'][0]['uploaded']['upload_id']
    # Nothing is left behind on the machine that was dropped, and its staged
    # bytes are released once the emptied draft is saved.
    assert boundary.drafts[first]['value']['text']==''
    assert not boundary.drafts[first]['value']['attachments']
    deadline=time.time()+5
    while not boundary.discards and time.time()<deadline: page.wait_for_timeout(100)
    assert boundary.discards and boundary.discards[0][0]==first, boundary.discards
    page.evaluate("async () => {for (const [uid,draft] of composerDrafts) {draft.text='';draft.attachments=[];draft.quotes=[];await persistComposerDraft(uid);}}");wait_drafts(page)
    # An empty box carries nothing; leave the remembered machine as the suite found it.
    page.fill('#bug-report-description','')
    page.select_option('#bug-report-node',NID['a'])
    page.wait_for_function('!bugReportDraftObject().loading')
    assert page.locator('#bug-report-description').input_value()==''
    wait_drafts(page)
    page.locator('#bug-report-dialog .modal-close').click()
    boundary.calls.clear();boundary.uploads.clear();boundary.discards.clear()


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
                    check_draft_follows_machine(page, boundary)
                    check_report_scroll(page)
                    check_report_layout(page)
                    check_send_busy_width(page, boundary)

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

                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    page.evaluate("markStaleBuild('test-build')")
                    assert page.locator("#bug-report-go").is_disabled()
                    assert page.locator("#bug-report-add").is_disabled()
                    page.fill("#bug-report-description", "过期页不能再提交")
                    page.evaluate("document.querySelector('#bug-report-form').requestSubmit()")
                    assert page.locator("#bug-report-error").inner_text() == "页面已更新，请重新加载后再提交"
                    page.locator("#bug-report-dialog .modal-close").click()

                    assert not errors, errors
                    browser.close()
            finally:
                hub.stop()
    finally:
        for node in nodes:
            node.stop()
    print("PASS bug_report_node_browser: server drafts, draft follows the chosen machine, staged on selection, no browser message store, one-click removal with discard, scrollable submit, named sources, two-up attachments, picker and capture")


if __name__ == "__main__":
    main()
