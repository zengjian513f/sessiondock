#!/usr/bin/env python3
"""New-session receipts survive partial hub lists; recovery and real exit stay authoritative."""

import argparse
import json
import os
from pathlib import Path
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from hub_fixtures import REPO, FakeNode, Hub
from hub_browser import CHROMIUM
from popups import on_popup  # noqa: E402


def open_new_session(page):
    page.evaluate('() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
    if page.locator('#a-global-new-session').count():
        if not page.locator('#a-global-new-session').is_visible():
            page.locator('#a-more').click()
        page.locator('#a-global-new-session').click()
        return
    if not page.locator("#new-session").is_visible():
        page.locator("#header-more-btn").click()
    page.locator("#new-session").click()


def scenario(browser, hub, node, other):
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    errors, dialogs, partials = [], [], []
    page.on("pageerror", lambda error: errors.append(str(error)))
    on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
    # A new receipt is newer than the hub's cached empty terminal list. Fail
    # the node immediately after create succeeds, before its next list read.
    def create_then_disconnect(route):
        response = route.fetch()
        assert response.status == 200
        node.set(term_error=True)
        other.set(pending=[])
        route.fulfill(response=response)

    def observe(response):
        if urlsplit(response.url).path == "/api/term/list" and response.status == 200:
            data = response.json()
            if data.get("partial"):
                partials.append(data)

    page.on("response", observe)
    context.route("**/api/term/create", create_then_disconnect)
    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded && Nodes.list.length === 2 && T.pending.length === 1")
    open_new_session(page)
    page.locator("#new-node").select_option(node.nid)
    page.locator('#new-session-form label:has(input[name="new-source"][value="codex"])').click()
    page.locator("#new-cwd").fill("/synthetic/work")
    page.locator("#new-session-go").click()
    uid = f"tmux:{node.nid}~same-terminal"
    row = page.locator(f'#side .item[data-uid="{uid}"]')
    page.wait_for_function("uid => S.sel === uid", arg=uid)
    page.locator("#cinput").fill("keep this unsent draft")
    page.wait_for_function("Nodes.errors.get('term')?.length > 0", timeout=20000)
    expect(row).to_contain_text("运行状态不确定")
    expect(row).not_to_contain_text("已结束")
    expect(page.locator(".new-session-wait")).to_have_text("暂时无法确认会话状态。")
    assert partials and all(not data.get("pending") for data in partials), partials
    receipt = page.evaluate("T.pending[0]")
    assert receipt["instance_id"] == "fixture-new-instance" and receipt["stale"]
    assert page.evaluate("T.pending.length") == 1, "healthy node removal must still apply"
    row.click()
    expect(page.locator("#cinput")).to_have_value("keep this unsent draft")
    expect(page.locator(".new-session-wait")).not_to_contain_text("已结束")

    # No in-memory receipt after reload: the persisted draft must also remain
    # uncertain while this node's list is unavailable.
    page.wait_for_function("composerDrafts.get(S.sel)?.savedVersion === composerDrafts.get(S.sel)?.editVersion")
    page.reload(wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded && Nodes.errors.get('term')?.length > 0")
    row.click()
    expect(row).to_contain_text("运行状态不确定")
    expect(page.locator(".new-session-wait")).to_have_text("暂时无法确认会话状态。")
    expect(page.locator("#cinput")).to_have_value("keep this unsent draft")

    node.pop("term_error")
    # Normal monitor + page polling recover the node, without injecting JS state.
    page.wait_for_function("!Nodes.errors.get('term')?.length && T.pending.some(r => !r.stale)", timeout=30000)
    expect(row).to_contain_text("等待首条消息")
    expect(page.locator(".new-session-wait")).not_to_contain_text("已结束")
    expect(page.locator("#cinput")).to_have_value("keep this unsent draft")
    assert page.evaluate("T.pending[0].instance_id") == receipt["instance_id"]

    # A successful authoritative exit is still shown as ended.
    ended = {**node.state()["pending"][0], "state": "exited", "running": False, "stale": True}
    node.set(pending=[ended])
    page.wait_for_function("T.pending.some(r => r.state === 'exited')", timeout=20000)
    expect(row).to_contain_text("已结束")
    expect(page.locator(".new-session-wait")).to_have_text("会话已结束。")
    # A later partial result cannot erase an already confirmed exit either.
    node.set(term_error=True)
    page.wait_for_function("Nodes.errors.get('term')?.length > 0", timeout=20000)
    expect(row).to_contain_text("已结束")
    expect(page.locator("#cinput")).to_have_value("keep this unsent draft")
    assert not errors, errors
    assert not dialogs, dialogs
    context.close()


def node_source_picker(browser, hub, node, other):
    """The picker follows the chosen machine: OpenCode only where configured,
    and the machine select plus five sources stay inside the dialog."""
    node.set(term_sources={"claude": True, "codex": True, "grok": True, "shell": True})
    other.set(term_sources={"claude": True, "codex": True, "grok": True, "opencode": True, "shell": True})
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded && Nodes.list.length === 2")
    open_new_session(page)
    opencode = page.locator('input[name="new-source"][value="opencode"]')
    page.locator("#new-node").select_option(node.nid)
    expect(opencode).to_be_disabled()
    page.locator("#new-node").select_option(other.nid)
    expect(opencode).to_be_enabled()
    page.locator('#new-session-form label:has(input[value="opencode"])').click()
    expect(opencode).to_be_checked()
    # The model list comes from the selected machine's own CLI.
    expect(page.locator("#new-model-label")).to_have_text("选择模型")
    page.locator("#new-model").click()
    expect(page.locator("#new-model-options [role=option]", has_text=f"{other.name}-opencode")).to_be_visible()
    page.keyboard.press("Escape")
    for width in (1280, 390):
        page.set_viewport_size({"width": width, "height": 900})
        dialog = page.locator("#new-session-dialog").bounding_box()
        for item in page.locator("#new-session-form .new-row > *:not([hidden]), #new-session-form .new-source label, #new-model, #new-session-form .new-effort").all():
            box = item.bounding_box()
            assert box and box["x"] >= dialog["x"] and box["x"] + box["width"] <= dialog["x"] + dialog["width"] + 0.5, (width, box, dialog)
        rows = page.evaluate("""() => new Set([...document.querySelectorAll('#new-session-form .new-source label')]
            .map(l => Math.round(l.getBoundingClientRect().top))).size""")
        assert rows == 1, (width, rows)
        # The machine picker is first, sharing the top row with the agent buttons.
        where = page.evaluate("""() => { const r = s => document.querySelector(s).getBoundingClientRect();
            const node = r('#new-node-label'), agents = r('#new-session-form .new-source'),
                  cwd = r('#new-session-form .new-cwd-field'), dialog = r('#new-session-dialog');
            return {top: Math.abs(node.top - agents.top), order: node.right <= agents.left + 0.5,
                    inside: cwd.right <= dialog.right, directory_below: cwd.top > node.bottom,
                    node_width: node.width, cwd_width: cwd.width,
                    in_agent_row: document.querySelector('#new-session-form .new-row').firstElementChild.id === 'new-node-label'}; }""")
        assert where["top"] < 1 and where["order"] and where["inside"] and where["in_agent_row"], (width, where)
        assert where["directory_below"], (width, where)
        assert where["node_width"] <= 140 and where["cwd_width"] >= 180, (width, where)
        # Buttons never overlap, and each keeps a usable tap width.
        spans = page.evaluate("""() => [...document.querySelectorAll('#new-session-form .new-source label > span')]
            .map(e => { const r = e.getBoundingClientRect(); return [r.left, r.right]; })""")
        assert all(b[0] >= a[1] - 0.5 for a, b in zip(spans, spans[1:])), (width, spans)
        assert all(r - l >= 34 for l, r in spans), (width, spans)
        # Like the header Agent filter, the buttons are one joined segment: no gaps between them.
        assert all(abs(b[0] - a[1]) <= 0.5 for a, b in zip(spans, spans[1:])), (width, spans)
        for theme in ("dark", "light"):
            page.evaluate("theme => document.documentElement.dataset.theme = theme", theme)
            page.locator("#new-cwd").fill("/shared/project")
            if os.environ.get("SESSIONDOCK_TEST_SHOTS"):
                page.screenshot(path=os.path.join(os.environ["SESSIONDOCK_TEST_SHOTS"], f"hub-picker-{width}-{theme}.png"))
            page.locator("#new-cwd-clear").click()
            expect(page.locator("#new-cwd")).to_have_value("")
            expect(page.locator("#new-cwd")).to_be_focused()
            expect(page.locator("#new-cwd")).to_have_attribute("placeholder", "输入或选择目录")
            assert page.locator("#new-cwd").evaluate("el => el.matches(':placeholder-shown')")
            # Clearing again must not reveal a sample path that looks like a default.
            page.locator("#new-cwd-clear").click()
            expect(page.locator("#new-cwd")).to_have_value("")
            page.locator("#new-session-go").click()
            expect(page.locator("#new-session-error")).to_have_text("请选择启动目录")
            expect(page.locator("#new-session-dialog")).to_be_visible()
            if os.environ.get("SESSIONDOCK_TEST_SHOTS"):
                page.screenshot(path=os.path.join(os.environ["SESSIONDOCK_TEST_SHOTS"], f"hub-picker-{width}-{theme}-empty.png"))
    assert not errors, errors
    context.close()
    node.pop("term_sources")
    other.pop("term_sources")


def node_switch_cwd(browser, hub, node, other):
    """Existing directories survive machine switches regardless of how they were chosen."""
    node.set(dirs=["/shared/proj", "/node-a/default", "/node-a/default/child"])
    other.set(dirs=["/shared/proj", "/only-b/work"])
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded && Nodes.list.length === 2")
    page.evaluate("""([a, b]) => {
        store.set('newDirs.' + a, ['/node-a/default']);
        store.set('newDirs.' + b, ['/node-b/default']);
    }""", [node.nid, other.nid])
    open_new_session(page)
    cwd = page.locator("#new-cwd")
    expect(cwd).to_have_value("")
    expect(cwd).to_have_attribute("placeholder", "输入或选择目录")
    for nid in (node.nid, other.nid, node.nid):
        page.locator("#new-node").select_option(nid)
        expect(cwd).to_have_value("")
    cwd.fill("/node-a/default")
    page.locator("#new-node").select_option(other.nid)
    expect(cwd).to_have_value("")
    cwd.fill("/shared/proj")
    page.locator("#new-cwd-clear").click()
    expect(cwd).to_have_value("")
    expect(cwd).to_be_focused()
    expect(page.locator("#new-cwd-options-title")).to_have_text("最近使用")
    page.locator("#new-session-go").click()
    expect(page.locator("#new-session-error")).to_have_text("请选择启动目录")
    expect(page.locator("#new-session-dialog")).to_be_visible()
    # Typing swaps the list title, groups and the lookup note; the dialog and
    # the list keep their geometry on every frame instead of jumping.
    cwd.fill("")
    page.wait_for_timeout(100)
    page.evaluate("""() => { window.__cwdFrames = []; const rect = s => {
        const r = document.querySelector(s).getBoundingClientRect(); return [Math.round(r.top), Math.round(r.height)]; };
        const sample = () => { window.__cwdFrames.push([...rect('#new-session-dialog'), ...rect('#new-cwd-options')]);
          if (!window.__cwdStop) requestAnimationFrame(sample); };
        requestAnimationFrame(sample); }""")
    with page.expect_response(lambda r: "/api/term/complete-dir" in r.url and "path=%2Fsh" in r.url):
        cwd.press_sequentially("/sh", delay=40)
    page.wait_for_timeout(200)
    expect(page.locator("#new-cwd-options [data-cwd-kind=completion]", has_text="/shared/proj")).to_be_visible()
    frames = page.evaluate("() => { window.__cwdStop = true; return window.__cwdFrames; }")
    assert len(frames) > 5 and len({tuple(f) for f in frames}) == 1, sorted({tuple(f) for f in frames})
    for value in ("/shared/proj", "  /shared/proj/  ", "/only-b/work", "/missing/work", "relative/path", "   "):
        cwd.fill(value)
        with page.expect_response(lambda r: "/api/term/check-dirs" in r.url
                                  and r.request.post_data_json.get('_node') == other.nid):
            page.locator("#new-node").select_option(other.nid)
        expect(cwd).to_have_value(value if value.strip().rstrip("/") in ("/shared/proj", "/only-b/work") else "")
        page.locator("#new-node").select_option(node.nid)
        expect(cwd).to_have_value(value if value.strip().rstrip("/") == "/shared/proj" else "")
    # Candidate clicks and Tab completion follow the same existence rule.
    page.locator("#new-session-dialog .modal-close").click()
    open_new_session(page)
    expect(cwd).to_have_value("")
    page.locator('#new-cwd-options [role=option]').filter(has=page.get_by_text("/node-a/default", exact=True)).or_(page.locator('#new-cwd-options [role=option][title="/node-a/default/"]')).click()
    page.locator("#new-node").select_option(other.nid)
    expect(cwd).to_have_value("")
    page.locator("#new-node").select_option(node.nid)
    expect(cwd).to_have_value("")
    cwd.fill("/node-a/default/ch")
    cwd.press("Tab")
    expect(cwd).to_have_value("/node-a/default/child/")
    page.locator("#new-node").select_option(other.nid)
    expect(cwd).to_have_value("")
    # A delayed missing-path result must not overwrite newer typing or a switch.
    held = []
    def delay_check(route):
        if "path=%2Fdelayed" in route.request.url:
            held.append(route)
        else:
            route.continue_()
    context.route("**/api/term/complete-dir?*", delay_check)
    for action in ("type", "switch", "reopen"):
        cwd.fill("/delayed")
        page.locator("#new-node").select_option(node.nid)
        page.wait_for_timeout(200)
        assert held
        if action == "switch":
            page.locator("#new-node").select_option(other.nid)
        elif action == "reopen":
            page.locator("#new-session-dialog .modal-close").click()
            open_new_session(page)
        cwd.fill("/new-input")
        for route in held:
            route.fulfill(json={"directories": []})
        held.clear()
        page.wait_for_timeout(200)
        expect(cwd).to_have_value("/new-input")
    assert not errors, errors
    context.close()
    node.pop("dirs")
    other.pop("dirs")


def shared_recent_cwd(browser, hub, node, other):
    """Other-node history and saved recents are offered only after a local check."""
    node.set(dirs=['/shared/session', '/shared/recent', '/shared/prefix-extra'])
    other.set(dirs=['/same/project'])
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))

    def sessions(route):
        response = route.fetch(headers={key: value for key, value in route.request.headers.items()
                                        if key.lower() != 'x-sessiondock-list'})
        data = response.json()
        for row in data.get('sessions', []):
            if row.get('node_id') == other.nid:
                row['cwd'] = '/shared/session'
        route.fulfill(json=data)

    context.route('**/api/sessions*', sessions)
    page.goto(f'http://127.0.0.1:{hub.port}/', wait_until='domcontentloaded')
    page.wait_for_function('S.sessions.length === 2 && T.listLoaded && Nodes.list.length === 2')
    page.evaluate('''nid => store.set('newDirs.' + nid,
      ['/shared/recent', '/missing/recent', '/shared/recent/', '/shared/prefix'])''', other.nid)
    open_new_session(page)
    page.locator('#new-node').select_option(node.nid)
    cwd = page.locator('#new-cwd')
    cwd.fill('')
    options = page.locator('#new-cwd-options [role=option]')
    expect(options.filter(has=page.locator('.new-cwd-option-path', has_text='/shared/session'))).to_have_count(1)
    recent = page.locator('#new-cwd-options [role=option][title="/shared/recent"]')
    expect(recent).to_be_visible()
    expect(options.filter(has_text='/missing/recent')).to_have_count(0)
    expect(options.filter(has_text='/shared/prefix')).to_have_count(0)
    expect(options.filter(has_text='/shared/recent')).to_have_count(1)
    recent.click()
    expect(cwd).to_have_value('/shared/recent')
    cwd.fill('session')
    expect(page.locator('#new-cwd-options [role=option][title="/shared/session"]')).to_be_visible()
    # Hold one old-machine response while switching. It must not replace the
    # new machine's home or bring an unchecked old-machine directory back.
    page.locator('#new-session-dialog .modal-actions button[type=button]').click()
    held = []

    def delay_check(route):
        response = route.fetch()
        body = route.request.post_data_json
        if body.get('_node') == node.nid and '/shared/session' in body['paths']:
            held.append((route, response))
            page.evaluate('window.__sharedCwdHeld = true')
        else:
            route.fulfill(response=response)

    context.route('**/api/term/check-dirs', delay_check)
    open_new_session(page)
    page.wait_for_function('window.__sharedCwdHeld === true')
    page.locator('#new-node').select_option(other.nid)
    cwd.fill('')
    expect(options.filter(has_text='/home/' + other.name)).to_be_visible()
    for route, response in held:
        route.fulfill(response=response)
    page.wait_for_function('''home => cwdCompletion.common.some(row => row.cwd === home)
      && !cwdCompletion.common.some(row => row.cwd === '/home/NodeA')''', arg='/home/' + other.name)
    expect(options.filter(has_text='/home/' + node.name)).to_have_count(0)
    assert not errors, errors
    context.close()
    node.pop('dirs')
    other.pop('dirs')


def current_cwd_defaults(browser, hub, node, other):
    """Open on the selected session; switches validate rather than choose a cwd."""
    node.set(dirs=["/same/project"])
    other.set(dirs=["/same/project"])
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="domcontentloaded")
    page.wait_for_function("S.sessions.length === 2 && T.listLoaded && Nodes.list.length === 2")
    uid = page.evaluate("nid => S.sessions.find(row => row.node_id === nid).uid", node.nid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.wait_for_function("uid => S.sel === uid", arg=uid)
    page.evaluate("""([a, b]) => {
        store.set('newNode', b);
        store.set('newDirs.' + a, ['/recent/first', '/same/project/']);
        store.set('newDirs.' + b, ['/recent/first', '/same/project/']);
    }""", [node.nid, other.nid])
    # A single-machine list filter must not replace the selected session's node.
    page.locator(f'#node-chips [data-node="{other.nid}"]').dblclick()
    open_new_session(page)
    cwd = page.locator("#new-cwd")
    options = page.locator('#new-cwd-options [role=option]')
    expect(page.locator('#new-node')).to_have_value(node.nid)
    expect(cwd).to_have_value('/same/project')
    page.locator('#new-cwd-clear').click()
    expect(options.first).to_have_attribute('title', '/recent/first')
    expect(options.filter(has_text='/same/project')).to_have_count(1)
    page.locator('#new-session-dialog .modal-close').click()
    open_new_session(page)
    expect(cwd).to_have_value('/same/project')
    for nid in (other.nid, node.nid):
        page.locator('#new-node').select_option(nid)
        expect(cwd).to_have_value('/same/project')
    # Defaults and empty/missing states retain the established layout in both
    # themes on desktop and mobile. Open anew each time, ignoring last machine.
    for width, height in ((1280, 900), (390, 844)):
        page.set_viewport_size({"width": width, "height": height})
        for theme in ('dark', 'light'):
            page.evaluate("theme => document.documentElement.dataset.theme = theme", theme)
            page.locator('#new-session-dialog .modal-close').click()
            open_new_session(page)
            expect(page.locator('#new-node')).to_have_value(node.nid)
            expect(cwd).to_have_value('/same/project')
            if os.environ.get('SESSIONDOCK_TEST_SHOTS'):
                page.screenshot(path=os.path.join(os.environ['SESSIONDOCK_TEST_SHOTS'],
                    f'cwd-default-{width}-{theme}.png'))
            # No manual input: the initial directory itself survives a switch,
            # then clears when that same directory disappears at the destination.
            page.locator('#new-node').select_option(other.nid)
            expect(cwd).to_have_value('/same/project')
            node.set(dirs=[])
            page.locator('#new-node').select_option(node.nid)
            expect(cwd).to_have_value('')
            if os.environ.get('SESSIONDOCK_TEST_SHOTS'):
                page.screenshot(path=os.path.join(os.environ['SESSIONDOCK_TEST_SHOTS'],
                    f'cwd-missing-{width}-{theme}.png'))
            node.set(dirs=['/same/project'])
    # Reopening resets to the selected session, including after explicit clear.
    page.locator('#new-cwd-clear').click()
    page.locator('#new-session-dialog .modal-close').click()
    open_new_session(page)
    expect(cwd).to_have_value('/same/project')
    # A missing initial directory clears too, without choosing a saved recent.
    page.locator('#new-session-dialog .modal-close').click()
    node.set(dirs=[])
    open_new_session(page)
    expect(cwd).to_have_value('')
    expect(options.filter(has_text='/same/project')).to_have_count(0)
    # A held successful check cannot overwrite a subsequent switch or typing.
    held = []
    def delay(route):
        if 'path=%2Fsame%2Fproject' in route.request.url and 'node=' + other.nid in route.request.url:
            held.append(route)
        else:
            route.continue_()
    context.route('**/api/term/complete-dir?*', delay)
    cwd.fill('/same/project')
    page.locator('#new-node').select_option(other.nid)
    page.wait_for_timeout(200)
    assert held
    page.locator('#new-node').select_option(node.nid)
    cwd.fill('recent')
    for route in held:
        route.fulfill(json={'directories': ['/same/project/']})
    page.wait_for_timeout(200)
    expect(cwd).to_have_value('recent')
    expect(options.filter(has_text='/same/project')).to_have_count(0)
    assert not errors, errors
    context.close()
    node.pop('dirs')
    other.pop('dirs')


def stable_open_cwd(browser, hub, node, other):
    """Attached sessions never become recent cwd candidates or disk checks."""
    shared = '/shared/Projects/跨机器推荐目录-with-a-long-project-name'
    nested_dirs = [f'/shared/历史目录-{i:04d}/work' for i in range(3660)]
    node.set(dirs=['/same/project', shared, '/shared/native-child', *nested_dirs])
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    page = context.new_page()
    errors, held = [], []
    batches = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    context.route('**/api/term/complete-dir?*', lambda route: held.append(route))
    def hold_batch(route):
        batches.append(route.request.post_data_json)
        held.append(route)
    context.route('**/api/term/check-dirs', hold_batch)
    def many_sessions(route):
        response = route.fetch(headers={key: value for key, value in route.request.headers.items()
                                        if key.lower() != 'x-sessiondock-list'})
        data = response.json()
        foreign = next(row for row in data['sessions'] if row['node_id'] == other.nid)
        local = next(row for row in data['sessions'] if row['node_id'] == node.nid)
        for i, path in enumerate(nested_dirs):
            original = foreign if i % 2 else local
            data['sessions'].append({**original, 'uid': f"claude:{original['node_id']}~batch-{i}",
                'cwd': path, 'nest_parent': {'source': 'claude', 'sid': 'background-parent'}})
        data['sessions'].append({**foreign, 'uid': f'claude:{other.nid}~native-child',
            'cwd': '/shared/native-child', 'agent_id': 'native-child'})
        route.fulfill(json=data)
    context.route('**/api/sessions*', many_sessions)
    page.goto(f'http://127.0.0.1:{hub.port}/', wait_until='domcontentloaded')
    page.wait_for_function('S.sessions.length === 3663 && T.listLoaded && Nodes.list.length === 2')
    uid = page.evaluate('nid => S.sessions.find(row => row.node_id === nid).uid', node.nid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.wait_for_function('uid => S.sel === uid', arg=uid)
    page.evaluate("([nid, path]) => store.set('newDirs.' + nid, [path])", [other.nid, shared])
    for width in (1280, 390):
        page.set_viewport_size({'width': width, 'height': 900})
        for theme in ('dark', 'light'):
            page.evaluate('theme => document.documentElement.dataset.theme = theme', theme)
            page.evaluate('''() => {
                window.__cwdPublications = [];
                window.__cwdObserver = new MutationObserver(() => {
                    const rows = [...document.querySelectorAll('#new-cwd-options [role=option]')]
                        .map(el => el.title);
                    if (rows.length) window.__cwdPublications.push(rows);
                });
                window.__cwdObserver.observe(document.querySelector('#new-cwd-options'), {childList: true});
            }''')
            open_new_session(page)
            cwd = page.locator('#new-cwd')
            options = page.locator('#new-cwd-options [role=option]')
            expect(cwd).to_have_value('/same/project')
            expect(options).to_have_count(0)
            expect(page.locator('#new-cwd-options')).to_contain_text('正在读取目录')
            before = page.locator('#new-session-dialog').bounding_box()
            if os.environ.get('SESSIONDOCK_TEST_SHOTS'):
                page.screenshot(path=os.path.join(os.environ['SESSIONDOCK_TEST_SHOTS'],
                    f'cwd-loading-{width}-{theme}.png'))
            assert held
            assert len(batches) == 1, 'one batch, not one request per historical cwd'
            assert set(batches[0]['paths']) == {'/same/project', shared}
            assert batches[0]['_node'] == node.nid
            while held:
                held.pop(0).continue_()
            batches.clear()
            expect(options.first).to_have_attribute('title', shared)
            expect(options).to_have_count(3)
            expect(options.filter(has_text='/same/project')).to_contain_text('2 个会话')
            expect(page.locator('#new-cwd-options-title')).to_have_text('最近使用')
            expect(cwd).to_have_value('/same/project')
            after = page.locator('#new-session-dialog').bounding_box()
            assert before == after, (before, after)
            publications = page.evaluate('''() => {
                window.__cwdObserver.disconnect(); return window.__cwdPublications;
            }''')
            assert publications and all(rows == publications[-1] for rows in publications), publications
            if os.environ.get('SESSIONDOCK_TEST_SHOTS'):
                page.screenshot(path=os.path.join(os.environ['SESSIONDOCK_TEST_SHOTS'],
                    f'cwd-ready-{width}-{theme}.png'))
            # The chosen row remains a real directory input; typing still filters.
            options.first.click()
            expect(cwd).to_have_value(shared)
            cwd.fill('跨机器')
            expect(page.locator('#new-cwd-options-title')).to_have_text('匹配目录')
            expect(options).to_have_count(1)
            expect(options.first).to_have_attribute('title', shared)
            if os.environ.get('SESSIONDOCK_TEST_SHOTS'):
                page.screenshot(path=os.path.join(os.environ['SESSIONDOCK_TEST_SHOTS'],
                    f'cwd-matching-{width}-{theme}.png'))
            cwd.fill('历史目录')
            expect(options).to_have_count(0)
            expect(page.locator('#new-cwd-options')).to_contain_text('没有匹配的目录')
            page.locator('#new-session-dialog .modal-close').click()
    assert not errors, errors
    context.close()
    node.pop('dirs')


def native_stop(browser, hub, node, other):
    """A session whose console is open on its machine but not in live state
    is stoppable from the hub page; the stop names that session."""
    uid = f"claude:{node.nid}~same-file-hash"
    node.set(term_sessions=[{"name": "native-term", "uid": "claude:same-file-hash", "source": "claude",
        "sid": "same-native-id", "instance_id": "n" * 32, "cwd": "/synthetic/work", "state": "running"}])
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    errors, dialogs, stops = [], [], []
    page.on("pageerror", lambda error: errors.append(str(error)))
    on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.accept()))

    def stop(route):
        stops.append(route.request.post_data_json)
        route.fulfill(json={"ok": True, "stopped": True, "stage": "graceful"})
    context.route("**/api/session/stop", stop)
    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="domcontentloaded")
    page.wait_for_function("uid => T.listLoaded && T.list.some(row => row.uid === uid)", arg=uid)
    assert not page.evaluate("uid => S.live.has(uid)", uid)
    row = page.locator(f'#side .item[data-uid="{uid}"]')
    row.click(button="right")
    page.locator('#item-menu [data-act="pick"]').click()
    expect(page.locator("#side-pick-stop")).to_be_enabled()
    page.locator("#side-pick-stop").click()
    page.wait_for_function("() => !sessionStopBusy")
    assert len(stops) == 1 and stops[0]["uid"] == uid and stops[0].get("request_id"), stops
    page.locator("#side-pick-cancel").click()
    assert not errors, errors
    context.close()
    node.pop("term_sessions")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release/sessiondock"))
    parser.add_argument("--hub-binary")
    args = parser.parse_args()
    binary = Path(args.hub_binary) if args.hub_binary else Path(args.binary).parent / "sessiondock-hub"
    nodes = [FakeNode("a" * 32, "NodeA"), FakeNode("b" * 32, "NodeB")]
    try:
        nodes[0].set(create_info={"sid": None, "record_id": "fixture-new-record",
            "instance_id": "fixture-new-instance", "launch_id": "fixture-new-launch",
            "state": "running", "running": True})
        nodes[1].set(pending=[{"name": "other-terminal", "source": "codex", "cwd": "/synthetic/work",
            "record_id": "fixture-other-record", "instance_id": "fixture-other-instance",
            "state": "running", "running": True}])
        with tempfile.TemporaryDirectory(prefix="sessiondock-hub-pending-") as temp:
            hub = Hub(binary, Path(temp), nodes)
            hub.start()
            try:
                with sync_playwright() as playwright:
                    launch = {"headless": True}
                    executable = os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE") or (str(CHROMIUM) if CHROMIUM.is_file() else "")
                    if executable:
                        launch["executable_path"] = executable
                    browser = playwright.chromium.launch(**launch)
                    print("RUN stable_open_cwd", flush=True)
                    stable_open_cwd(browser, hub, *nodes)
                    print("RUN node_source_picker", flush=True)
                    node_source_picker(browser, hub, *nodes)
                    print("RUN node_switch_cwd", flush=True)
                    node_switch_cwd(browser, hub, *nodes)
                    print("RUN shared_recent_cwd", flush=True)
                    shared_recent_cwd(browser, hub, *nodes)
                    print("RUN current_cwd_defaults", flush=True)
                    current_cwd_defaults(browser, hub, *nodes)
                    print("RUN native_stop", flush=True)
                    native_stop(browser, hub, *nodes)
                    print("RUN scenario", flush=True)
                    scenario(browser, hub, *nodes)
                    browser.close()
            finally:
                hub.stop()
    finally:
        for node in nodes:
            node.stop()
    print("PASS hub_pending_state_browser: per-node OpenCode picker, multi-select stop of a console-only session, machine select on the first row, steady cwd list while typing, selected-session machine/cwd defaults without pinning, node switch retains existing directories and clears missing paths, with stale-check protection, create, partial list, draft/reload, recovery, confirmed exit")


if __name__ == "__main__":
    main()
