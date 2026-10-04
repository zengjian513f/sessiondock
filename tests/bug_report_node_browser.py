#!/usr/bin/env python3
"""The report dialog on the hub page: its machine picker and the two-step cross-machine submit.

Three fake nodes behind `sessiondock-hub` (`tests/hub_fake_node.py`). The picker lists every
machine like the new-session dialog and defaults to the problem's machine (the selected
session's); a worker elsewhere first asks the problem's machine for `/api/bug-report/capture`
and hands the answer to the worker's machine as `captured` (a failed capture becomes
`captured: {error}`); the chosen machine's missing CLIs are greyed out. Wide layout keeps
one row of a narrow machine picker, joined agent icons and the machine's own model and effort
pickers, whose choice is sent with the report; the model list floats above the dialog without
resizing or scrolling the form; Enter submits and Shift+Enter adds a line like the composer (a
phone's Enter is a newline); 390px puts two attachments on one row.
`/api/bug-report` and `/api/bug-report/capture` are answered at the browser boundary so the
bodies the page builds can be asserted. No CLI, no session root.
"""
from __future__ import annotations

from browser_runtime import js, scoped_frontend
import argparse
import json
import re
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit, parse_qs

from playwright.sync_api import expect, sync_playwright

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
        self.hold_conversation = ''
        self.pending_conversations = []

    def conversation(self, route):
        request=route.request;path=urlsplit(request.url).path
        query=parse_qs(urlsplit(request.url).query)
        body=request.post_data_json if request.method=='POST' and ('attachment' not in path or path.endswith('/discard')) else {}
        uid=body.get('uid') or query.get('uid',[''])[0]
        if uid.startswith('report:') and (
            self.hold_conversation == 'hydrate' and path.endswith('/conversation') and request.method == 'GET'
            or self.hold_conversation == 'upload' and path.endswith('/attachment') and request.method == 'POST'
        ):
            self.pending_conversations.append(route)
            return
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


def check_report_drag_selection(page):
    open_report(page)
    dialog = page.locator('#bug-report-dialog')
    page.wait_for_selector('#bug-report-dialog[open]')
    textarea = page.locator('#bug-report-description')
    text = 'Select this report text and release outside the dialog.'
    textarea.fill(text)
    wait_drafts(page)
    box = textarea.bounding_box()
    bounds = dialog.bounding_box()
    outside = (bounds['x'] - 20, box['y'] + 16)
    page.mouse.move(box['x'] + 110, box['y'] + 16)
    page.mouse.down()
    page.mouse.move(*outside, steps=12)
    page.mouse.up()
    assert dialog.is_visible(), 'Dragging selected report text outside closed the dialog'
    assert textarea.evaluate('el => el.selectionEnd > el.selectionStart')
    assert textarea.input_value() == text
    # A gesture beginning outside and ending inside is not a backdrop click either.
    page.mouse.move(*outside)
    page.mouse.down()
    page.mouse.move(box['x'] + 20, box['y'] + 16, steps=12)
    page.mouse.up()
    assert dialog.is_visible()
    page.mouse.click(*outside)
    assert not dialog.is_visible(), 'An ordinary backdrop click must still close'
    open_report(page)
    assert textarea.input_value() == text
    page.locator('#bug-report-dialog .modal-close').click()
    assert not dialog.is_visible()
    open_report(page)
    page.keyboard.press('Escape')
    assert not dialog.is_visible()
    page.evaluate(js('clearBugReportDraft()', 'runtime.launch.clearBugReportDraft()'))
    wait_drafts(page)


def check_report_scroll(page):
    # The submit button used to be clipped by the dialog's overflow:hidden.
    # Scroll with the pointer and click by coordinates: locator.click() would
    # silently scroll inaccessible content into view and mask the regression.
    for width, height, attachments in [
            (608, 788, 1), (390, 640, 1),
            (390, 360, 1), (1280, 500, 12)]:
        page.set_viewport_size({"width": width, "height": height})
        page.evaluate(js("openBugReportDialog()", 'runtime.launch.openBugReportDialog()'))
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
            const row = document.querySelector('.report-row').getBoundingClientRect();
            return Math.abs(add.bottom - input.bottom) < 1
                && Math.abs(send.bottom - input.bottom) < 1 && items.bottom <= input.top
                && add.right <= input.left && input.right <= send.left
                && Math.abs(send.right - row.right) <= 1;
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
        input_height = textarea.evaluate("element => element.style.height")
        textarea.fill('')
        textarea.evaluate("(element, height) => element.style.height = height", input_height)
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
        expect(page.locator("#bug-report-error")).to_have_text("请先描述遇到的问题")
        page.evaluate(js("""() => {
            document.querySelector('#bug-report-dialog').close();
            document.querySelector('#bug-report-description').style.height = '';
            clearBugReportDraft();
        }""", """() => {
            document.querySelector('#bug-report-dialog').close();
            document.querySelector('#bug-report-description').style.height = '';
            runtime.launch.clearBugReportDraft();
        }"""))
    page.set_viewport_size({"width": 1280, "height": 900})


def check_send_busy_width(page, boundary):
    page.evaluate(js("openBugReportDialog()", 'runtime.launch.openBugReportDialog()'))
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
    page.wait_for_function(js("!bugReportSending", '!runtime.launch.bugReportSending'))
    page.evaluate(js("""() => {
      document.querySelector('#bug-report-dialog').close();
      clearBugReportDraft();
    }""", """() => {
      document.querySelector('#bug-report-dialog').close();
      runtime.launch.clearBugReportDraft();
    }"""))
    wait_drafts(page)
    boundary.calls.clear()


def wait_drafts(page):
    page.evaluate(js("async () => { for (;;) { const pending = composerDraftWrites; await pending; if (pending === composerDraftWrites) break; } }", 'async () => { for (;;) { const pending = runtime.composer.composerDraftWrites; await pending; if (pending === runtime.composer.composerDraftWrites) break; } }'))


def check_stale_async_gates(browser, url):
    # Legacy imperatively resets these buttons after asynchronous work. The
    # computed Vue gates must survive those completions without changing SEND.
    for phase in ('hydrate', 'upload', 'submit'):
        context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
        boundary = Boundary()
        context.route(re.compile(r'.*/api/session/conversation.*'), boundary.conversation)
        context.route(re.compile(r'.*/api/bug-report(/capture)?(\?.*)?$'), boundary.handle)
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        try:
            page.goto(url, wait_until='networkidle')
            page.wait_for_function(js('Nodes.list.length === 3', 'runtime.core.state.nodes.list.length === 3'))
            if phase == 'hydrate':
                boundary.hold_conversation = phase
            open_report(page)
            description = page.locator('#bug-report-description')
            if phase != 'hydrate':
                expect(description).to_be_enabled()
                page.fill('#bug-report-description', '异步完成后保留草稿')
                wait_drafts(page)
                if phase == 'upload':
                    boundary.hold_conversation = phase
                    page.locator('#bug-report-file').set_input_files(
                        {'name': 'late.png', 'mimeType': 'image/png', 'buffer': PNG})
                else:
                    boundary.defer = True
                    page.locator('#bug-report-go').click()

            pending = boundary.pending if phase == 'submit' else boundary.pending_conversations
            deadline = time.monotonic() + 10
            while not pending:
                assert time.monotonic() < deadline, (phase, boundary.calls)
                page.wait_for_timeout(20)  # Dispatch the held browser request.
            if phase in ('hydrate', 'submit'):
                expect(description).to_be_disabled()
                expect(page.locator('#bug-report-add')).to_be_disabled()
            if phase == 'submit':
                expect(page.locator('#bug-report-go')).to_be_disabled()
                expect(page.locator('#bug-report-node')).to_be_disabled()
                page.evaluate("document.querySelector('#bug-report-form').requestSubmit()")
                assert [p for p, _ in boundary.calls] == ['bug-report'], boundary.calls

            page.evaluate(js("markStaleBuild('late-test-build')", "runtime.build.markStaleBuild('late-test-build')"))
            expect(page.locator('#bug-report-add')).to_be_disabled()
            expect(page.locator('#bug-report-go')).to_be_disabled()
            boundary.hold_conversation = ''
            if phase == 'submit':
                pending[0].fulfill(status=500, content_type='application/json',
                                   body='{"error":"synthetic hold"}')
                page.wait_for_function(js('!bugReportSending', '!runtime.launch.bugReportSending'))
                expect(page.locator('#bug-report-error')).to_have_text('synthetic hold')
            else:
                if phase == 'hydrate':
                    uid = parse_qs(urlsplit(pending[0].request.url).query)['uid'][0]
                    boundary.drafts[uid] = {'revision': 1, 'value': {
                        'text': '异步完成后保留草稿', 'quotes': [], 'attachments': [],
                        'nextAttachmentNumber': 1}}
                for route in pending:
                    boundary.conversation(route)
                if phase == 'hydrate':
                    page.wait_for_function(js('!bugReportDraftObject().loading', '!runtime.launch.bugReportDraftObject().loading'))
                else:
                    page.wait_for_function(js('bugReportDraftObject().attachments[0]?.uploaded?.upload_id && !bugReportDraftObject().attachments[0].staging', 'runtime.launch.bugReportDraftObject().attachments[0]?.uploaded?.upload_id && !runtime.launch.bugReportDraftObject().attachments[0].staging'))
            pending.clear()
            expect(page.locator('#bug-report-add')).to_be_disabled()
            expect(page.locator('#bug-report-go')).to_be_disabled()
            expect(description).to_be_enabled()
            expect(description).to_have_value('异步完成后保留草稿')
            page.fill('#bug-report-description', '过期页仍可编辑保存：' + phase)
            wait_drafts(page)
            uid = page.evaluate(js('BUG_REPORT_DRAFT_UID', 'runtime.launch.BUG_REPORT_DRAFT_UID'))
            assert boundary.drafts[uid]['value']['text'] == '过期页仍可编辑保存：' + phase
            expect(page.locator('#bug-report-add')).to_be_disabled()
            expect(page.locator('#bug-report-go')).to_be_disabled()
            assert len(boundary.calls) == (1 if phase == 'submit' else 0), boundary.calls
            assert not errors, errors
        finally:
            context.close()
    print('PASS Vue stale gates: loading/sending lock controls; late hydration, upload and submit finally keep add/send disabled and text editable/saveable', flush=True)


def check_report_layout(page):
    # Wide dialog: one row like the new-session dialog — a narrow machine
    # picker, the joined agent icons (names on hover), the model and effort
    # pickers ending flush with the send button.
    page.set_viewport_size({"width": 1280, "height": 900})
    page.evaluate(js("openBugReportDialog()", 'runtime.launch.openBugReportDialog()'))
    page.wait_for_selector("#bug-report-dialog[open]")
    page.wait_for_function("!document.querySelector('#bug-report-node-label').hidden")
    # The picker still says what the machine is for, now as its accessible name and tooltip.
    expect(page.locator("#bug-report-node-label > span")).to_have_text("处理节点")
    expect(page.locator("#bug-report-node-label > span")).to_have_class("visually-hidden")
    expect(page.locator("#bug-report-model-label")).not_to_have_text("读取模型…")
    wide = page.evaluate("""() => {
      const box = s => document.querySelector(s).getBoundingClientRect();
      const row = box('.report-row'), select = box('#bug-report-node-label');
      const sources = box('#bug-report-source'), model = box('#bug-report-model');
      const effort = box('.report-row .new-effort');
      const send = box('#bug-report-go');
      const spans = [...document.querySelectorAll('#bug-report-source label > span')]
        .map(e => { const r = e.getBoundingClientRect(); return [r.left, r.right]; });
      return {
        titles: [...document.querySelectorAll('#bug-report-source label')].map(l => l.title),
        tops: [select.top, sources.top, model.top, effort.top].map(Math.round),
        order: select.right <= sources.left + 1 && sources.right <= model.left + 1 && model.right <= effort.left + 1,
        wrapped: select.right <= sources.left + 1 && model.right <= effort.left + 1
          && Math.max(select.bottom, sources.bottom) <= Math.min(model.top, effort.top),
        select_width: select.width,
        joined: spans.every((s, i) => !i || Math.abs(s[0] - spans[i - 1][1]) <= 0.5),
        send_align: Math.abs(send.right - effort.right), inside: effort.right <= row.right + 1,
      };
    }""")
    assert wide["titles"] == ["Claude", "Codex", "Grok", "OpenCode", "Agy"], wide
    if scoped_frontend():
        assert len(set(wide["tops"])) == 1 and wide["order"], wide
    else:
        # Preserve the restored 600px legacy dialog: five CLI icons put the
        # model/effort controls on the next aligned row. Vue uses 640px.
        assert wide["tops"][0] == wide["tops"][1] and wide["tops"][2] == wide["tops"][3], wide
        assert wide["order"] or wide["wrapped"], wide
    assert wide["joined"], wide
    assert wide["select_width"] <= 140 and wide["inside"] and wide["send_align"] <= 1, wide
    # The model list is the chosen machine's; the choice goes out with the report.
    page.select_option("#bug-report-node", NID["b"])
    page.locator('#bug-report-form label:has(input[value="codex"])').click()
    page.locator("#bug-report-model").click()
    page.locator("#bug-report-model-options [role=option]", has_text="NodeB-codex").click()
    expect(page.locator("#bug-report-model-label")).to_have_text("NodeB-codex")
    page.locator("#bug-report-effort").select_option("high")
    # Back to the default machine; the choice stays remembered for NodeB's Codex.
    page.select_option("#bug-report-node", NID["a"])
    expect(page.locator("#bug-report-model-label")).to_have_text("选择模型")
    page.evaluate("document.querySelector('#bug-report-dialog').close()")

    # Phone: two attachments share one row instead of stacking at 100% width.
    page.set_viewport_size({"width": 390, "height": 844})
    page.evaluate(js("openBugReportDialog()", 'runtime.launch.openBugReportDialog()'))
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
    page.evaluate(js("""() => {
      document.querySelector('#bug-report-dialog').close();
      clearBugReportDraft();
    }""", """() => {
      document.querySelector('#bug-report-dialog').close();
      runtime.launch.clearBugReportDraft();
    }"""))
    wait_drafts(page)
    page.set_viewport_size({"width": 1280, "height": 900})


def check_shared_draft_recovery(page, boundary):
    page.evaluate(js("openBugReportDialog()", 'runtime.launch.openBugReportDialog()'))
    page.wait_for_function(js("!bugReportDraftObject().loading", '!runtime.launch.bugReportDraftObject().loading'))
    page.fill('#bug-report-description','服务端保存 [附件1]')
    page.locator('#bug-report-file').set_input_files([
        {'name':'recover.png','mimeType':'image/png','buffer':PNG}])
    wait_drafts(page)
    uid=page.evaluate(js('BUG_REPORT_DRAFT_UID', 'runtime.launch.BUG_REPORT_DRAFT_UID'))
    assert boundary.drafts[uid]['value']['text']=='服务端保存 [附件1]'
    assert len(boundary.drafts[uid]['value']['attachments'])==1
    # Selection stages the bytes on the chosen machine at once; nothing is left only in RAM.
    page.wait_for_function(js("bugReportDraftObject().attachments[0]?.uploaded?.upload_id && !bugReportDraftObject().attachments[0].staging", 'runtime.launch.bugReportDraftObject().attachments[0]?.uploaded?.upload_id && !runtime.launch.bugReportDraftObject().attachments[0].staging'))
    wait_drafts(page)
    assert boundary.uploads==[uid], boundary.uploads
    assert boundary.drafts[uid]['value']['attachments'][0]['uploaded']['upload_id']
    assert not page.evaluate(js('composerUnloadProtected', 'runtime.composer.composerUnloadProtected'))
    assert not page.evaluate(js("store.get('composerDraft.'+BUG_REPORT_DRAFT_UID,null)", "runtime.core.preferences.get('composerDraft.'+runtime.launch.BUG_REPORT_DRAFT_UID,null)"))
    assert not page.evaluate("async () => (await indexedDB.databases()).some(d=>d.name.endsWith('composer-drafts'))")
    page.locator('#bug-report-dialog .modal-close').click()
    page.reload(wait_until='networkidle')
    page.wait_for_function(js('Nodes.list.length===3', 'runtime.core.state.nodes.list.length===3'))
    page.evaluate(js('openBugReportDialog()', 'runtime.launch.openBugReportDialog()'))
    page.wait_for_function(js('!bugReportDraftObject().loading', '!runtime.launch.bugReportDraftObject().loading'))
    assert page.locator('#bug-report-description').input_value()=='服务端保存 [附件1]'
    assert not page.evaluate(js('bugReportDraftObject().attachments[0].file instanceof Blob', 'runtime.launch.bugReportDraftObject().attachments[0].file instanceof Blob'))
    # That card still shows a thumbnail: the bytes come back from staging.
    staged_id=page.evaluate(js('bugReportDraftObject().attachments[0].uploaded.upload_id', 'runtime.launch.bugReportDraftObject().attachments[0].uploaded.upload_id'))
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
    page.wait_for_function(js('!bugReportDraftObject().loading', '!runtime.launch.bugReportDraftObject().loading'))
    assert page.locator('#bug-report-description').input_value()=='服务端保存 [附件1]'
    other=page.evaluate(js('BUG_REPORT_DRAFT_UID', 'runtime.launch.BUG_REPORT_DRAFT_UID'))
    assert other!=uid
    wait_drafts(page)
    assert boundary.drafts[other]['value']['text']=='服务端保存 [附件1]', boundary.drafts[other]
    assert boundary.drafts[uid]['value']['text']=='', boundary.drafts[uid]
    # 空着的输入框不搬任何东西：切回去看到的仍是那台机器自己的草稿。
    page.fill('#bug-report-description','');wait_drafts(page)
    page.select_option('#bug-report-node',NID['a'])
    page.wait_for_function(js('!bugReportDraftObject().loading', '!runtime.launch.bugReportDraftObject().loading'))
    assert page.evaluate(js('BUG_REPORT_DRAFT_UID', 'runtime.launch.BUG_REPORT_DRAFT_UID'))==uid
    assert page.locator('#bug-report-description').input_value()==''
    assert not page.locator('.draft-saved').count()
    page.evaluate(js("async () => {for (const [uid,draft] of composerDrafts) {draft.text='';draft.attachments=[];draft.quotes=[];await persistComposerDraft(uid);}}", "async () => {for (const [uid,draft] of runtime.composer.composerDrafts) {draft.text='';draft.attachments=[];draft.quotes=[];await runtime.composer.persistComposerDraft(uid);}}"));wait_drafts(page)
    page.locator('#bug-report-dialog .modal-close').click()
    boundary.calls.clear()


def check_draft_follows_machine(page, boundary):
    # The user's complaint: typing the report, then picking another machine,
    # emptied the box. Description, quotes and the attachments this page still
    # holds follow the selection; the previous machine keeps nothing.
    boundary.uploads.clear();boundary.discards.clear()
    page.evaluate(js("openBugReportDialog()", 'runtime.launch.openBugReportDialog()'))
    page.wait_for_function(js("!bugReportDraftObject().loading", '!runtime.launch.bugReportDraftObject().loading'))
    page.fill('#bug-report-description','切换机器也别清空 [附件1]')
    page.locator('#bug-report-file').set_input_files([
        {'name':'switch.png','mimeType':'image/png','buffer':PNG}])
    page.wait_for_function(js("bugReportDraftObject().attachments[0]?.uploaded?.upload_id && !bugReportDraftObject().attachments[0].staging", 'runtime.launch.bugReportDraftObject().attachments[0]?.uploaded?.upload_id && !runtime.launch.bugReportDraftObject().attachments[0].staging'))
    wait_drafts(page)
    first=page.evaluate(js('BUG_REPORT_DRAFT_UID', 'runtime.launch.BUG_REPORT_DRAFT_UID'))
    assert boundary.uploads==[first], boundary.uploads
    page.select_option('#bug-report-node',NID['c'])
    page.wait_for_function(js('!bugReportDraftObject().loading', '!runtime.launch.bugReportDraftObject().loading'))
    second=page.evaluate(js('BUG_REPORT_DRAFT_UID', 'runtime.launch.BUG_REPORT_DRAFT_UID'))
    assert second!=first
    assert page.locator('#bug-report-description').input_value()=='切换机器也别清空 [附件1]'
    assert not page.locator('#bug-report-error').inner_text()
    # The card's bytes are staged again on the machine that will handle it.
    page.wait_for_function(js("bugReportDraftObject().attachments.length===1 && bugReportDraftObject().attachments[0].uploaded?.uid===BUG_REPORT_DRAFT_UID && !bugReportDraftObject().attachments[0].staging", 'runtime.launch.bugReportDraftObject().attachments.length===1 && runtime.launch.bugReportDraftObject().attachments[0].uploaded?.uid===runtime.launch.BUG_REPORT_DRAFT_UID && !runtime.launch.bugReportDraftObject().attachments[0].staging'))
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
    page.evaluate(js("async () => {for (const [uid,draft] of composerDrafts) {draft.text='';draft.attachments=[];draft.quotes=[];await persistComposerDraft(uid);}}", "async () => {for (const [uid,draft] of runtime.composer.composerDrafts) {draft.text='';draft.attachments=[];draft.quotes=[];await runtime.composer.persistComposerDraft(uid);}}"));wait_drafts(page)
    # An empty box carries nothing; leave the remembered machine as the suite found it.
    page.fill('#bug-report-description','')
    page.select_option('#bug-report-node',NID['a'])
    page.wait_for_function(js('!bugReportDraftObject().loading', '!runtime.launch.bugReportDraftObject().loading'))
    assert page.locator('#bug-report-description').input_value()==''
    wait_drafts(page)
    page.locator('#bug-report-dialog .modal-close').click()
    boundary.calls.clear();boundary.uploads.clear();boundary.discards.clear()


def check_model_menu_floats(page):
    # BUG-20260928-123931-9cc8ae: the model menu opened inside the report form's
    # scroll box, which is shorter than the menu, so the form grew a scroll area
    # and the menu was cut off. The menu now floats above the dialog.
    for width, height, scale in ((1280, 900, 1), (390, 844, 1), (1280, 700, 0.8)):
        page.set_viewport_size({"width": width, "height": height})
        page.evaluate(f"document.documentElement.style.setProperty('--compact-scale', '{scale}')")
        page.evaluate(js("openBugReportDialog()", 'runtime.launch.openBugReportDialog()'))
        page.wait_for_selector("#bug-report-dialog[open]")
        page.wait_for_function("!document.querySelector('#bug-report-model').disabled")
        measure = """() => {
          const form = document.querySelector('#bug-report-form'), r = form.getBoundingClientRect();
          return {rect: [r.left, r.top, r.width, r.height].map(Math.round),
                  scroll: form.scrollHeight - form.clientHeight, top: form.scrollTop};
        }"""
        before = page.evaluate(measure)
        page.locator("#bug-report-model").click()
        expect(page.locator("#bug-report-model-menu")).to_be_visible()
        after = page.evaluate(measure)
        assert after == before, (width, before, after)
        menu = page.evaluate("""() => {
          const menu = document.querySelector('#bug-report-model-menu').getBoundingClientRect();
          const pick = document.querySelector('#bug-report-model').getBoundingClientRect();
          const choice = document.querySelector('#bug-report-form .new-choice').getBoundingClientRect();
          const last = [...document.querySelectorAll('#bug-report-model-options [role=option]')].pop();
          const at = last.getBoundingClientRect();
          const hit = document.elementFromPoint(at.left + at.width / 2, at.top + at.height / 2);
          return {below: menu.top >= pick.bottom - 1, left: Math.abs(menu.left - choice.left),
                  width: Math.abs(menu.width - choice.width), inside: menu.bottom <= innerHeight && menu.right <= innerWidth,
                  hit: last.contains(hit)};
        }""")
        assert menu["below"] and menu["left"] <= 1 and menu["width"] <= 1, (width, menu)
        assert menu["inside"] and menu["hit"], (width, menu)
        page.keyboard.press("Escape")
        expect(page.locator("#bug-report-model-menu")).to_be_hidden()
        expect(page.locator("#bug-report-dialog")).to_be_visible()
        page.evaluate("document.querySelector('#bug-report-dialog').close()")
    page.evaluate("document.documentElement.style.removeProperty('--compact-scale')")
    page.set_viewport_size({"width": 1280, "height": 900})


def check_enter_keys(page):
    # Enter and Shift+Enter match the composer: on a phone Enter is a newline too.
    page.set_viewport_size({"width": 390, "height": 844})
    page.evaluate(js("openBugReportDialog()", 'runtime.launch.openBugReportDialog()'))
    page.wait_for_selector("#bug-report-dialog[open]")
    textarea = page.locator("#bug-report-description")
    textarea.fill("手机上")
    textarea.press("Enter")
    assert textarea.input_value() == "手机上\n", textarea.input_value()
    expect(page.locator("#bug-report-dialog")).to_be_visible()
    textarea.fill("")
    page.evaluate("document.querySelector('#bug-report-dialog').close()")
    wait_drafts(page)
    page.set_viewport_size({"width": 1280, "height": 900})


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
                        js('S.sessions.length === 3 && Nodes.list.length === 3 && !!Nodes.capabilities["' + NID["b"] + '"]', 'runtime.core.state.catalog.sessions.length === 3 && runtime.core.state.nodes.list.length === 3 && !!runtime.core.state.nodes.capabilities["' + NID['b'] + '"]'))

                    check_shared_draft_recovery(page, boundary)
                    check_draft_follows_machine(page, boundary)
                    check_report_drag_selection(page)
                    check_report_scroll(page)
                    check_report_layout(page)
                    check_model_menu_floats(page)
                    check_enter_keys(page)
                    check_send_busy_width(page, boundary)

                    # 1. No session selected, all machines ticked: picker lists all three,
                    #    defaults to the first usable machine.
                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    assert page.locator("#bug-report-node-label").is_visible()
                    opts = options(page)
                    assert [o["text"] for o in opts] == ["NodeA", "NodeB", "Vega"], opts
                    assert not any(o["disabled"] for o in opts), opts
                    assert page.evaluate(js("bugReportNode()", 'runtime.launch.bugReportNode()')) == NID["a"]
                    if args.screenshot:
                        page.locator("#bug-report-dialog .report-form").screenshot(path=args.screenshot)
                    # Pick NodeB: no session → origin is the worker machine, single request.
                    page.select_option("#bug-report-node", NID["b"])
                    # Like the composer: Shift+Enter is a newline, Enter submits.
                    page.fill("#bug-report-description", "列表页卡住")
                    page.locator("#bug-report-description").press("Shift+Enter")
                    page.locator("#bug-report-description").press_sequentially("第二行")
                    expect(page.locator("#bug-report-dialog")).to_be_visible()
                    assert page.locator("#bug-report-description").input_value() == "列表页卡住\n第二行"
                    page.locator("#bug-report-description").press("Enter")
                    page.wait_for_function("!document.querySelector('#bug-report-dialog').open")
                    paths = [p for p, _ in boundary.calls]
                    assert paths == ["bug-report"], paths
                    body = boundary.calls[0][1]
                    assert body["_node"] == NID["b"], body
                    assert body["uid"] == "" and "captured" not in body, body
                    # The model and effort chosen for NodeB's Codex are remembered and sent.
                    assert (body["source"], body.get("model"), body.get("effort")) == ("codex", "NodeB-codex", "high"), body
                    assert body["origin"] == {"node_id": NID["b"], "node_name": "NodeB", "uid": ""}, body["origin"]
                    page.wait_for_function("!document.querySelector('#bug-report-toast').classList.contains('hidden')")
                    toast = page.locator("#bug-report-toast").inner_text()
                    assert "已保存到 NodeB" in toast, toast
                    # Dismissing the receipt leaves the worker and current selection alone.
                    receipt_state = page.evaluate(js("({selected: S.sel, pending: T.pending})", '({selected: runtime.core.state.selection.sel, pending: runtime.terminal.state.pending})'))
                    page.locator('#bug-report-toast').get_by_role('button', name='忽略', exact=True).click()
                    expect(page.locator('#bug-report-toast')).to_be_hidden()
                    assert page.evaluate(js("({selected: S.sel, pending: T.pending})", '({selected: runtime.core.state.selection.sel, pending: runtime.terminal.state.pending})')) == receipt_state
                    assert [p for p, _ in boundary.calls] == ['bug-report']
                    assert page.evaluate("localStorage.getItem('sessiondock.hub./.bugReportNode')") == json.dumps(NID["b"])
                    boundary.calls.clear()

                    # 2. A NodeA session selected: the picker defaults to NodeA (the problem's
                    #    machine) even though NodeB was remembered; choosing Vega goes two-step.
                    row = page.locator("#side .item[data-uid^='claude:" + NID["a"] + "~']").first
                    row.click()
                    page.wait_for_function(js("S.sel && S.sel.includes('" + NID["a"] + "~')", "runtime.core.state.selection.sel && runtime.core.state.selection.sel.includes('" + NID['a'] + "~')"))
                    sel = page.evaluate(js("S.sel", 'runtime.core.state.selection.sel'))
                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    assert page.evaluate(js("bugReportNode()", 'runtime.launch.bugReportNode()')) == NID["a"]
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
                    # A later receipt reappears, with both actions reachable on a phone.
                    page.set_viewport_size({"width": 390, "height": 844})
                    receipt = page.locator('#bug-report-toast')
                    expect(receipt).to_be_visible()
                    expect(receipt.locator('.app-float-head strong')).to_have_text('缺陷报告已提交')
                    expect(receipt.locator('.app-float-actions .primary')).to_have_text('打开')
                    layout = receipt.evaluate("""card => {
                        const body = card.querySelector(':scope > span').getBoundingClientRect();
                        const actions = card.querySelector('.app-float-actions').getBoundingClientRect();
                        return {column: getComputedStyle(card).flexDirection,
                            below: actions.top >= body.bottom, fits: card.scrollWidth <= card.clientWidth};
                    }""")
                    assert layout == {"column": "column", "below": True, "fits": True}, layout
                    expect(receipt.get_by_role('button', name='打开', exact=True)).to_be_in_viewport()
                    ignore = receipt.get_by_role('button', name='忽略', exact=True)
                    expect(ignore).to_be_in_viewport()
                    receipt_state = page.evaluate(js("({selected: S.sel, pending: T.pending})", '({selected: runtime.core.state.selection.sel, pending: runtime.terminal.state.pending})'))
                    ignore.focus()
                    ignore.press('Enter')
                    expect(receipt).to_be_hidden()
                    assert page.evaluate(js("({selected: S.sel, pending: T.pending})", '({selected: runtime.core.state.selection.sel, pending: runtime.terminal.state.pending})')) == receipt_state
                    assert [p for p, _ in boundary.calls] == ['bug-report/capture', 'bug-report']
                    page.set_viewport_size({"width": 1280, "height": 900})
                    page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
                    boundary.calls.clear()

                    # 2b. One report, not one per machine (BUG-20260930-110656-a65368): an
                    #     unsent draft reopens on the machine holding it, whichever machine's
                    #     session is open; once emptied, the picker follows the session again.
                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    assert page.evaluate(js("bugReportNode()", 'runtime.launch.bugReportNode()')) == NID["a"]
                    page.select_option("#bug-report-node", NID["b"])
                    page.wait_for_function(js("!bugReportDraftObject().loading", '!runtime.launch.bugReportDraftObject().loading'))
                    page.fill("#bug-report-description", "写了一半的报告")
                    wait_drafts(page)
                    page.locator("#bug-report-dialog .modal-close").click()
                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    page.wait_for_function(js("!bugReportDraftObject().loading", '!runtime.launch.bugReportDraftObject().loading'))
                    assert page.evaluate(js("bugReportNode()", 'runtime.launch.bugReportNode()')) == NID["b"]
                    assert page.locator("#bug-report-description").input_value() == "写了一半的报告"
                    assert not page.locator("#bug-report-error").inner_text()
                    page.locator("#bug-report-dialog .modal-close").click()
                    # Its machine going offline says where the draft is instead of hiding it.
                    nodes[1].set(term_enabled=False)
                    page.evaluate(js("loadTermList()", 'runtime.terminal.loadTermList()'))
                    page.wait_for_function(js('Nodes.capabilities["' + NID["b"] + '"]?.enabled === false', 'runtime.core.state.nodes.capabilities["' + NID['b'] + '"]?.enabled === false'))
                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    assert page.evaluate(js("bugReportNode()", 'runtime.launch.bugReportNode()')) == NID["a"]
                    assert page.locator("#bug-report-error").inner_text() \
                        == "NodeB 离线，上面没发出的报告草稿要等它恢复后才能打开"
                    page.locator("#bug-report-dialog .modal-close").click()
                    nodes[1].set(term_enabled=True)
                    page.evaluate(js("loadTermList()", 'runtime.terminal.loadTermList()'))
                    page.wait_for_function(js('Nodes.capabilities["' + NID["b"] + '"]?.enabled === true', 'runtime.core.state.nodes.capabilities["' + NID['b'] + '"]?.enabled === true'))
                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    page.wait_for_function(js("!bugReportDraftObject().loading", '!runtime.launch.bugReportDraftObject().loading'))
                    assert page.evaluate(js("bugReportNode()", 'runtime.launch.bugReportNode()')) == NID["b"]
                    page.fill("#bug-report-description", "")
                    wait_drafts(page)
                    page.locator("#bug-report-dialog .modal-close").click()
                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    assert page.evaluate(js("bugReportNode()", 'runtime.launch.bugReportNode()')) == NID["a"]
                    page.locator("#bug-report-dialog .modal-close").click()
                    boundary.calls.clear()

                    # 3. The problem's machine is unreachable: the report still goes out with
                    #    `captured: {error}`.
                    boundary.capture_status = 503
                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    assert page.evaluate(js("bugReportNode()", 'runtime.launch.bugReportNode()')) == NID["a"]
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
                    page.evaluate(js("loadTermList()", 'runtime.terminal.loadTermList()'))
                    page.wait_for_function(
                        js('Nodes.capabilities["' + NID["b"] + '"].sources.codex === false', 'runtime.core.state.nodes.capabilities["' + NID['b'] + '"].sources.codex === false'))
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

                    # 5. A machine without a usable terminal is just "（离线）" in both pickers.
                    nodes[2].set(term_enabled=False)
                    page.evaluate(js("loadTermList()", 'runtime.terminal.loadTermList()'))
                    page.wait_for_function(js('Nodes.capabilities["' + NID["c"] + '"]?.enabled === false', 'runtime.core.state.nodes.capabilities["' + NID['c'] + '"]?.enabled === false'))
                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    vega = next(o for o in options(page) if o["text"].startswith("Vega"))
                    assert vega == {**vega, "text": "Vega（离线）", "disabled": True}, vega
                    page.locator("#bug-report-dialog .modal-close").click()
                    page.locator("#new-session").click()
                    expect(page.locator("#new-session-dialog")).to_be_visible()
                    assert page.evaluate("""() => [...document.querySelectorAll('#new-node option')]
                        .filter(o => o.value === '%s').map(o => [o.textContent, o.disabled])""" % NID["c"]) == [["Vega（离线）", True]]
                    page.locator("#new-session-dialog .modal-cancel").click()
                    nodes[2].set(term_enabled=True)
                    page.evaluate(js("loadTermList()", 'runtime.terminal.loadTermList()'))
                    page.wait_for_function(js('Nodes.capabilities["' + NID["c"] + '"]?.enabled === true', 'runtime.core.state.nodes.capabilities["' + NID['c'] + '"]?.enabled === true'))

                    open_report(page)
                    page.wait_for_selector("#bug-report-dialog[open]")
                    page.evaluate(js("markStaleBuild('test-build')", "runtime.build.markStaleBuild('test-build')"))
                    assert page.locator("#bug-report-go").is_disabled()
                    assert page.locator("#bug-report-add").is_disabled()
                    page.fill("#bug-report-description", "过期页不能再提交")
                    page.evaluate("document.querySelector('#bug-report-form').requestSubmit()")
                    assert page.locator("#bug-report-error").inner_text() == "页面已更新，请重新加载后再提交"
                    page.locator("#bug-report-dialog .modal-close").click()

                    assert not errors, errors
                    if scoped_frontend():
                        check_stale_async_gates(browser, f'http://127.0.0.1:{hub.port}/')
                    browser.close()
            finally:
                hub.stop()
    finally:
        for node in nodes:
            node.stop()
    print("PASS bug_report_node_browser: server drafts, draft follows the chosen machine, one unsent draft reopens on its machine, staged on selection, no browser message store, one-click removal with discard, scrollable submit, one-row joined sources with the machine's model/effort sent, two-up attachments, picker and capture, offline machines labelled （离线）")


if __name__ == "__main__":
    main()
