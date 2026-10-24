#!/usr/bin/env python3
"""Durable native SDK queues via Chromium, synthetic peers and real node restarts.

No native account, model, production directory or CLI default is used.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import tempfile
import time

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO
from native_sdk_queue_fixtures import NativeQueueFixture
from popups import on_popup
from sdk_ui_fixtures import start_sdk



def debug(page):
    page.on('pageerror', lambda error: print('queue pageerror', error, flush=True))
    page.on('requestfailed', lambda request: print('queue requestfailed', request.url, request.failure, flush=True))
    page.on('console', lambda message: print('queue console', message.type, message.text, flush=True) if message.type == 'error' else None)
    page.on('request', lambda request: print('queue action request', request.url, request.post_data_json, flush=True)
            if '/api/native-sdk/queue_' in request.url else None)


def open_page(browser, base, fixture, *, accept_popups=True):
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    if accept_popups:
        on_popup(page, lambda dialog: dialog.accept())
    page.set_default_timeout(20000)
    debug(page)
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=fixture.sid)
    uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', fixture.sid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    if not page.locator('#sdk-input').is_visible():
        page.locator('#a-sdk').click()
    start_sdk(page)
    return context, page, uid


def send(page, text, *, lose_ack=False):
    page.locator('#sdk-input').fill(text)
    if lose_ack:
        def lose(route):
            response = route.fetch()
            assert response.ok, response.text()
            route.abort('failed')
        page.route('**/api/native-sdk/turn', lose, times=1)
        with page.expect_event('requestfailed', predicate=lambda request: request.url.endswith('/api/native-sdk/turn')) as event:
            page.locator('#sdk-send').click()
        return event.value.post_data_json
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/turn')) as response:
        page.locator('#sdk-send').click()
    assert response.value.ok, response.value.text()
    return response.value.request.post_data_json


def card(page, request):
    return page.locator(f'.sdk-queue [data-queue-id="bridge:{request}"]')


def state(page, request, expected, attempts=None):
    page.wait_for_function('''([request,expected,attempts]) => {
        const row=NativeSdkView.raw?.input_queue?.find(row=>row.request_id===request);
        return row?.state===expected && (attempts==null || row.attempts===attempts);
    }''', arg=[request, expected, attempts])


def gone(page, request):
    expect(card(page, request)).to_have_count(0)


def settled(page, fixture, request):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        row = fixture.entry(request)
        if row['state'] == 'completed' and not row.get('runtime_state'):
            return
        page.wait_for_timeout(25)
    raise AssertionError(('runtime did not settle', fixture.entry(request)))


def click_action(page, request, method, title):
    try:
        with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/' + method), timeout=5000) as response:
            card(page, request).get_by_role('button', name=title, exact=True).click()
        assert response.value.ok, response.value.text()
        return response.value.request.post_data_json
    except Exception:
        print('queue click diagnostic', page.evaluate('''() => ({uid:NativeSdkView.uid,generation:NativeSdkView.generation,connected:NativeSdkView.connected,
            capabilities:NativeSdkView.capabilities,queue:NativeSdkView.raw?.input_queue,buttons:[...document.querySelectorAll('.sdk-queue button')].map(b=>({text:b.textContent,disabled:b.disabled}))})'''), flush=True)
        raise


def replay(page, base, method, data):
    response = page.request.post(base + '/api/native-sdk/' + method, data=data)
    assert response.ok, (method, response.status, response.text())
    return response.json()


def attach(page):
    page.locator('#sdk-add').click()
    with page.expect_file_chooser() as chooser:
        page.locator('#sdk-attach-menu [data-attach="file"]').click()
    chooser.value.set_files({'name': 'queue-研究记录.txt', 'mimeType': 'text/plain',
                            'buffer': b'private durable queue attachment\n'})
    expect(page.locator('#sdk-compose-items')).to_contain_text('queue-研究记录.txt')


def pictures(page, uid, folder, prefix, *, all_sizes=False):
    if not folder:
        return
    folder.mkdir(parents=True, exist_ok=True)
    modes = [(1280, 'light'), (390, 'dark'), (320, 'light'), (1280, 'dark'), (390, 'light'), (320, 'dark')] if all_sizes else [(1280, 'light'), (390, 'dark')]
    for width, theme in modes:
        page.set_viewport_size({'width': width, 'height': 900})
        page.emulate_media(color_scheme=theme)
        page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
        if width < 500 and page.evaluate("!document.body.classList.contains('mobile-detail')"):
            page.locator(f'#side .item[data-uid="{uid}"]').click()
        expect(page.locator('#sdk-input')).to_be_visible()
        if page.locator('.sdk-queue').is_visible():
            page.locator('.sdk-queue').scroll_into_view_if_needed()
        else:
            page.locator('.sdk-live .msg').last.scroll_into_view_if_needed()
        page.screenshot(path=str(folder / f'{prefix}-{width}-{theme}.png'), full_page=True)
    page.set_viewport_size({'width': 1280, 'height': 900})
    page.emulate_media(color_scheme='light')


def basic_queue(browser, fixture, screenshots):
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            print(f'{fixture.source}: native blocker and durable queued input', flush=True)
            if fixture.source == 'grok':
                page.locator('#a-chat').click()
                page.wait_for_function('uid => !!_es && _esUid===uid', arg=uid)
                other_uid = page.evaluate('sid => S.sessions.find(row => row.sid===sid).uid', fixture.other_sid)
                page.locator(f'#side .item[data-uid="{other_uid}"]').click()
                # This second session is only being viewed, so its idle SDK
                # history need not expose a composer or start a native peer.
                expect(page.locator('.native-sdk-pane')).to_be_visible()
                page.wait_for_function('uid => NativeSdkView.uid===uid && !_es', arg=other_uid)
                page.locator('#a-chat').click()
                page.wait_for_function('uid => !!_es && _esUid===uid', arg=other_uid)
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.locator('#a-sdk').click()
                expect(page.locator('#sdk-input')).to_be_visible()
                page.wait_for_function('uid => NativeSdkView.uid===uid && !_es', arg=uid)
                print('Grok: actual chat/SDK switches restore watch only for the current identity', flush=True)
            first = send(page, fixture.blocker)
            first_id = first['request_id']
            gone(page, first_id)
            page.wait_for_function('NativeSdkView.raw?.running === true')
            assert fixture.entry(first_id)['state'] == 'completed'
            assert fixture.entry(first_id)['runtime_state'] == 'observing'
            rejected = page.request.post(base + '/api/native-sdk/queue_retry', data={
                'uid': uid, 'id': 'bridge:' + first_id, 'request_id': 'cannot-retry-delivered', 'confirmed': True})
            assert not rejected.ok, 'completed delivery must not be reopened through retry'
            expect(page.locator('#sdk-stop')).to_be_visible()
            # A second tab sees the same queue via actual SSE, and cancels only
            # undelivered work. The running native turn must remain active.
            queued = send(page, 'cancel queued ' + fixture.source)
            queued_id = queued['request_id']
            state(page, queued_id, 'queued')
            observer = context.new_page()
            debug(observer)
            observer.goto(base, wait_until='domcontentloaded')
            observer.wait_for_function('uid => S.sessions.some(row => row.uid===uid)', arg=uid)
            observer.locator(f'#side .item[data-uid="{uid}"]').click()
            expect(observer.locator('#sdk-input')).to_be_visible()
            state(observer, queued_id, 'queued')
            cancelled = click_action(observer, queued_id, 'queue_cancel', '撤销')
            gone(page, queued_id)
            assert fixture.entry(queued_id)['state'] == 'cancelled'
            assert not fixture.inputs('cancel queued ')
            expect(page.locator('#sdk-stop')).to_be_visible()
            observer.close()
            replay(page, base, 'queue_cancel', cancelled)

            attach(page)
            page.locator('#sdk-add').click()
            page.locator('#sdk-attach-menu [data-attach="quote"]').click()
            page.locator('.draft-quote textarea').fill('撤回需要保留的引用')
            recalled = send(page, 'recall with attachment ' + fixture.source)
            state(page, recalled['request_id'], 'queued')
            page.locator('#sdk-input').fill('保留后续草稿')
            if screenshots:
                pictures(page, uid, screenshots, fixture.source + '-recall', all_sizes=True)
            click_action(page, recalled['request_id'], 'queue_cancel', '撤回编辑')
            gone(page, recalled['request_id'])
            expected = '保留后续草稿\n\n' + fixture.entry(recalled['request_id'])['input']['text']
            expect(page.locator('#sdk-input')).to_have_value(expected)
            assert '撤回需要保留的引用' in expected and 'queue-研究记录.txt' in expected
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('#sdk-input')).to_have_value(expected)
            with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/turn')) as resubmitted:
                page.locator('#sdk-send').click()
            fresh = resubmitted.value.request.post_data_json
            assert fresh['request_id'] != recalled['request_id']
            state(page, fresh['request_id'], 'queued')
            click_action(page, fresh['request_id'], 'queue_cancel', '撤销')
            assert not fixture.inputs('recall with attachment ')

            # The queued payload and request ID survive reload, while later
            # editor text is independent of worker delivery and completion.
            follower = send(page, 'queued after blocker ' + fixture.source)
            follower_id = follower['request_id']
            state(page, follower_id, 'queued')
            with page.expect_response(lambda response: '/api/session/conversation' in response.url and response.request.method == 'POST' and response.ok):
                page.locator('#sdk-input').fill('later editor text 中文 ' + fixture.source)
            expect(page.locator('#sdk-input')).to_have_value('later editor text 中文 ' + fixture.source)
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('#sdk-input')).to_have_value('later editor text 中文 ' + fixture.source)
            state(page, follower_id, 'queued')
            if fixture.source == 'grok':
                pictures(page, uid, screenshots, 'queued', all_sizes=True)
            page.locator('#sdk-stop').click()
            gone(page, first_id)
            gone(page, follower_id)
            settled(page, fixture, follower_id)
            expect(page.locator('#sdk-input')).to_have_value('later editor text 中文 ' + fixture.source)
            assert len(fixture.inputs('queued after blocker ')) == 1
            entry = fixture.entry(follower_id)
            assert entry['state'] == 'completed', entry
            assert entry['consumption'] == ('unverified' if fixture.source == 'grok' else 'observed'), entry
            repeat = replay(page, base, 'turn', follower)
            assert repeat['accepted'] and repeat['state'] == 'completed'
            assert len(fixture.inputs('queued after blocker ')) == 1
            print(f'{fixture.source}: shared SSE/cancel, STOP advances, durable request receipt, consumption={entry["consumption"]} passed', flush=True)
        except Exception:
            print('queue basic diagnostic', fixture.source, [(row['request_id'], row['state'], row['error']) for row in fixture.journal()], flush=True)
            if screenshots:
                page.screenshot(path=str(screenshots / f'{fixture.source}-failure.png'), full_page=True)
            raise
        finally:
            context.close()


def cancelled_draft(browser, fixture, screenshots):
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            print('Grok: lost HTTP ACK, cancelled submission, attachments publish once', flush=True)
            first = send(page, fixture.blocker)
            gone(page, first['request_id'])
            attach(page)
            original = send(page, 'lost ACK attachment 中文', lose_ack=True)
            original_id = original['request_id']
            state(page, original_id, 'queued')
            expect(page.locator('#sdk-input')).to_have_value('lost ACK attachment 中文')
            click_action(page, original_id, 'queue_cancel', '撤销')
            gone(page, original_id)
            expect(page.locator('#sdk-input')).to_have_value('lost ACK attachment 中文')
            expect(page.locator('#sdk-compose-items')).to_contain_text('queue-研究记录.txt')
            old = send(page, 'lost ACK attachment 中文')
            assert old['request_id'] == original_id
            expect(page.locator('#sdk-input')).to_have_value('lost ACK attachment 中文')
            expect(page.locator('#sdk-compose-items')).to_contain_text('queue-研究记录.txt')
            page.wait_for_function('!composerDraft(S.sel).sdkSubmission')
            fresh = send(page, 'lost ACK attachment 中文')
            assert fresh['request_id'] != original_id
            state(page, fresh['request_id'], 'queued')
            expect(page.locator('#sdk-input')).to_have_value('')
            paths = [row['input']['attachments'][0]['path'] for row in fixture.journal()
                     if row['draft'].get('text') == 'lost ACK attachment 中文']
            assert len(paths) == 2 and paths[0] == paths[1] and Path(paths[0]).is_file(), paths
            page.locator('#sdk-input').fill('fresh edits after admission')
            page.locator('#sdk-stop').click()
            gone(page, fresh['request_id'])
            expect(page.locator('#sdk-input')).to_have_value('fresh edits after admission')
            assert len(fixture.inputs('lost ACK attachment 中文')) == 1
            print('Grok: cancelled receipt preserves draft, explicit SEND uses fresh ID, same published attachment passed', flush=True)
        finally:
            context.close()


def failure_and_unknown(browser, fixture, screenshots):
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture, accept_popups=False)
        try:
            for text in ('native-error', 'disconnect'):
                submitted = send(page, text)
                page.wait_for_function('NativeSdkView.raw?.running === false && NativeSdkView.raw?.error')
                gone(page, submitted['request_id'])
                assert fixture.entry(submitted['request_id'])['state'] == 'completed'
                assert fixture.inputs(text) == [text]
                assert replay(page, base, 'turn', submitted)['state'] == 'completed'
                pictures(page, uid, screenshots, 'grok-execution-' + text)
                follower = send(page, 'continue after ' + text)
                expect(page.locator('.sdk-live')).to_contain_text('Grok final answer')
                gone(page, follower['request_id'])
                assert fixture.inputs(text) == [text]
            print('Grok: accepted sends stay completed through execution errors and exit; next turn needs no resend/dismissal', flush=True)
        finally:
            context.close()


def restart_queue(browser, fixture, screenshots):
    print('Claude: persist accepted/queued inputs and newer editor, restart the real isolated node', flush=True)
    holds_before = len(fixture.inputs('hold'))
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        first = send(page, fixture.blocker)
        gone(page, first['request_id'])
        native_pid = fixture.native_pid()
        attach(page)
        queued = send(page, 'durable queued attachment after restart')
        state(page, queued['request_id'], 'queued')
        with page.expect_response(lambda response: '/api/session/conversation' in response.url and response.request.method == 'POST' and response.ok):
            page.locator('#sdk-input').fill('newer draft survives service restart 中文')
        expect(page.locator('#sdk-input')).to_have_value('newer draft survives service restart 中文')
        assert len(fixture.inputs('hold')) == holds_before + 1 and not fixture.inputs('durable queued attachment')
        paths = fixture.entry(queued['request_id'])['input']['attachments']
        context.close()
    assert fixture.alive(native_pid), 'node shutdown killed the active native SDK child'
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            gone(page, first['request_id'])
            state(page, queued['request_id'], 'queued')
            assert fixture.native_pid() == native_pid and fixture.alive(native_pid)
            expect(page.locator('#sdk-input')).to_have_value('newer draft survives service restart 中文')
            assert len(fixture.inputs('hold')) == holds_before + 1 and not fixture.inputs('durable queued attachment')
            assert replay(page, base, 'turn', first)['state'] == 'completed'
            pictures(page, uid, screenshots, 'restart-running')
            page.locator('#sdk-stop').click()
            gone(page, queued['request_id'])
            settled(page, fixture, queued['request_id'])
            assert len(fixture.inputs('hold')) == holds_before + 1 and len(fixture.inputs('durable queued attachment')) == 1
            expect(page.locator('#sdk-input')).to_have_value('newer draft survives service restart 中文')
            entry = fixture.entry(queued['request_id'])
            assert entry['state'] == 'completed' and entry['consumption'] == 'observed', entry
            assert entry['input']['attachments'] == paths
            repeat = replay(page, base, 'turn', queued)
            assert repeat['state'] == 'completed' and len(fixture.inputs('durable queued attachment')) == 1
            assert fixture.entry(first['request_id'])['state'] == 'completed'
            print('Claude: same native PID survives restart, active input not replayed, explicit interrupt advances queue, draft and attachment retained', flush=True)
        finally:
            context.close()


def restart_output(browser, fixture):
    """The native peer finishes while no SessionDock process is connected."""
    text = 'hold:offline-replay'
    before = len(fixture.inputs(text))
    with fixture.server() as base:
        context, page, _ = open_page(browser, base, fixture)
        try:
            first = send(page, text)
            gone(page, first['request_id'])
            expect(page.locator('.sdk-live')).to_contain_text('Agy running offline-replay')
            pid = fixture.native_pid()
        finally:
            context.close()
    assert fixture.alive(pid), 'node shutdown killed Agy before offline completion'
    fixture.peer.release('offline-replay')
    fixture.wait_native_text('Agy running offline-replay finished offline-replay')
    with fixture.server() as base:
        context, page, _ = open_page(browser, base, fixture)
        try:
            expect(page.locator('.sdk-live')).to_contain_text('Agy running offline-replay finished offline-replay')
            page.wait_for_function('NativeSdkView.raw && !NativeSdkView.raw.running')
            gone(page, first['request_id'])
            assert fixture.entry(first['request_id'])['state'] == 'completed'
            assert len(fixture.inputs(text)) == before + 1
            assert replay(page, base, 'turn', first)['state'] == 'completed'
            assert len(fixture.inputs(text)) == before + 1
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('#sdk-input')).to_be_enabled()
            assert len(fixture.inputs(text)) == before + 1
            # A separate active turn proves explicit session stop still kills
            # its native child after reconnect (unlike merely closing the node).
            stopping = send(page, 'hold:explicit-stop-after-restart')
            gone(page, stopping['request_id'])
            stop_pid = fixture.native_pid()
            expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label', '停止会话')
            page.locator('#a-session-toggle').click()
            expect(page.locator('#sdk-composer')).to_be_visible()
            expect(page.locator('#sdk-send')).to_be_disabled()
            fixture.wait_exited(stop_pid)
            print('Agy: output while node offline replayed, HTTP retry deduplicated, explicit session stop ends native child', flush=True)
        finally:
            context.close()


def legacy_unknown(browser, fixture, screenshots, original_state='unknown'):
    """A pre-sdkhost receipt cannot be recovered or resent by opening a page."""
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        context.close()
    request = 'legacy-consumed-input'
    target = {'source': 'agy', 'uid': uid, 'session_id': fixture.sid,
              'cwd': str(fixture.work), 'agent': ''}
    text = '全面审计代码、重复逻辑和性能；原生已消费的旧任务'
    draft = {'text': text, 'request_id': request, 'attachments': [], 'quotes': []}
    row = {'id': 'bridge:' + request, 'request_id': request, 'delivery_id': request,
           'operation': 'turn', 'target': target, 'draft': draft, 'input': draft,
           'state': original_state, 'consumption': 'observed', 'attempts': 0, 'actions': {},
           'result': {'accepted': True},
           'error': '服务已重启，原生投递结果未确认；请核对会话后明确重试或移除，系统不会自动重发。'}
    if original_state == 'failed':
        row['error'] = 'agy_sdk_native_ERROR: The stream was interrupted. Please continue the task you were working on.'
    scope = json.dumps(['agy', uid, ''], separators=(',', ':'))
    (fixture.root / 'state/native-sdk-inputs.json').write_text(json.dumps({scope: [row]}))
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture, accept_popups=False)
        try:
            gone(page, request)
            assert fixture.entry(request)['state'] == 'completed'
            assert fixture.entry(request)['runtime_error'] == row['error']
            assert fixture.entry(request)['error'] is None
            assert not fixture.inputs(text)
            follower = send(page, 'continue after checking legacy history')
            expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: continue after checking legacy history')
            gone(page, follower['request_id'])
            assert not fixture.inputs(text)
            assert len(fixture.inputs('continue after checking')) == 1
            pictures(page, uid, screenshots, 'legacy-delivered-' + original_state)
            print('Agy: legacy execution verdict migrates to completed delivery without resend or blocking', flush=True)
        finally:
            context.close()


def restart_conflicting_result(browser, fixture, screenshots=None, *, text='__RECOVERED_STREAM__'):
    conflict = text == '__RECOVERED_STREAM__'
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture, accept_popups=False)
        try:
            submitted = send(page, text)
            gone(page, submitted['request_id'])
            expect(page.locator('.sdk-live')).to_contain_text(
                '最终结果：所有指定检查通过' if conflict else 'agy_sdk_native_ERROR:')
            pid = fixture.native_pid()
            if not conflict:
                assert page.evaluate('NativeSdkView.raw.error_kind') == 'turn'
                pictures(page, uid, screenshots, 'agy-retained-error-' + text.strip('_').lower())
        finally:
            context.close()
    assert fixture.alive(pid), 'native ERROR receipt killed the persistent native runtime'
    with fixture.server() as base:
        context, page, _ = open_page(browser, base, fixture, accept_popups=False)
        try:
            gone(page, submitted['request_id'])
            assert fixture.native_pid() == pid and fixture.alive(pid)
            assert fixture.inputs(text) == [text]
            assert page.evaluate('NativeSdkView.raw.error_kind') == ('uncertain' if conflict else 'turn')
            queued = send(page, 'continue after result reconciliation')
            expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: continue after result reconciliation')
            gone(page, queued['request_id'])
            assert fixture.native_pid() == pid and fixture.inputs(text) == [text]
            assert len(fixture.inputs('continue after result reconciliation')) == 1
            print(f'Agy: {text} survives node restart; delivery remains completed, next input resumes the same PID once', flush=True)
        finally:
            context.close()


def closed_runtime_result(browser, fixture, screenshots, *, conflict=True, dismissed=False):
    """Replay a completed receipt from an exited host, as with pre-fix Agy bindings."""
    text = '__RECOVERED_STREAM__' if conflict else 'completed before runtime exit'
    with fixture.server() as base:
        context, page, _ = open_page(browser, base, fixture, accept_popups=False)
        try:
            submitted = send(page, text)
            request = submitted['request_id']
            if conflict:
                gone(page, request)
                expect(page.locator('.sdk-live')).to_contain_text('最终结果：所有指定检查通过')
            else:
                gone(page, request)
                expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: ' + text)
            pid = fixture.native_pid()
        finally:
            context.close()
    # Terminate only this private fixture's retained native process, while the
    # node is offline. Its original output and input receipts remain intact.
    path = next(path for path in fixture.host_sockets()
                if fixture.host_rpc(path, 'Status')['pid'] == pid)
    host_pid = int((path.parent / 'host.pid').read_text())
    fixture.host_rpc(path, {'Signal': {'signal': signal.SIGKILL}})
    fixture.wait_exited(pid)
    fixture.wait_exited(host_pid)
    for restart in range(2):
        with fixture.server() as base:
            context, page, uid = open_page(browser, base, fixture, accept_popups=False)
            try:
                page.wait_for_function("""NativeSdkView.raw?.diagnostics?.includes('agy_sdk_runtime_closed') ||
                    NativeSdkView.raw?.error?.startsWith('agy_sdk_runtime_closed:')""")
                raw = page.evaluate('NativeSdkView.raw')
                assert raw['error_kind'] == ('uncertain' if conflict else 'transport'), raw
                assert 'signal: 9' in raw['diagnostics'], raw['diagnostics']
                assert raw['error'].startswith('agy_sdk_result_conflict:' if conflict else 'agy_sdk_runtime_closed:'), raw['error']
                expect(page.locator('.native-sdk-pane .sdk-status')).to_have_text('结果待核对' if conflict else '错误')
                assert fixture.inputs(text) == [text], 'opening/restarting resent the old input'
                assert fixture.entry(request)['state'] == 'completed'
                assert fixture.entry(request)['consumption'] == 'observed'
                if restart == 1:
                    pictures(page, uid, screenshots, f'agy-closed-{conflict}-{dismissed}')
                    fresh = send(page, 'new input after closed runtime')
                    expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: new input after closed runtime')
                    gone(page, fresh['request_id'])
                    assert fixture.native_pid() != pid
                    assert fixture.inputs(text) == [text]
                    assert len(fixture.inputs('new input after closed runtime')) == 1
                    assert all(row['sid'] == fixture.sid for row in fixture.peer.inputs())
                    assert page.evaluate('NativeSdkView.raw.error') is None
            except Exception:
                if screenshots:
                    page.screenshot(path=str(screenshots / 'agy-closed-failure.png'), full_page=True)
                raise
            finally:
                context.close()
    print(f'Agy: exited host replay preserves result (conflict={conflict}, dismissed={dismissed}), exit diagnostic, and new-input-only recovery', flush=True)


def restart_approval(browser, fixture):
    with fixture.server() as base:
        context, page, _ = open_page(browser, base, fixture)
        try:
            first = send(page, 'approval')
            page.wait_for_function('NativeSdkView.raw?.requests?.length === 1')
            key = page.evaluate('NativeSdkView.raw.requests[0].key')
            pid = fixture.native_pid()
            before = len(fixture.inputs('approval'))
        finally:
            context.close()
    assert fixture.alive(pid), 'pending approval child died with the node'
    with fixture.server() as base:
        context, page, _ = open_page(browser, base, fixture)
        try:
            page.wait_for_function('key => NativeSdkView.raw?.requests?.[0]?.key === key', arg=key)
            assert fixture.native_pid() == pid
            if fixture.source == 'grok':
                page.get_by_role('button', name='Reject', exact=True).click()
                final = 'Grok final answer'
            else:
                page.locator('[data-sdk-request]').get_by_role('button', name='拒绝', exact=True).click()
                final = 'Claude synthetic final approval'
            expect(page.locator('.sdk-live')).to_contain_text(final)
            gone(page, first['request_id'])
            assert len(fixture.inputs('approval')) == before
            assert fixture.entry(first['request_id'])['state'] == 'completed'
            print(f'{fixture.source}: same pending approval and PID restored, real UI decision reaches native request once', flush=True)
        finally:
            context.close()


def stop_inherited_pipes(browser, fixture):
    """Native parent exit must not make its still-open process group unstoppable."""
    with fixture.server() as base:
        context, page, _ = open_page(browser, base, fixture)
        try:
            send(page, '__DESCENDANT_PIPE__')
            deadline = time.monotonic() + 5
            observed = None
            while time.monotonic() < deadline:
                children = [row for row in fixture.peer.events() if row['kind'] == 'descendant']
                if children:
                    child = children[-1]
                    for path in fixture.host_sockets():
                        try:
                            status = fixture.host_rpc(path, 'Status')
                        except (FileNotFoundError, ConnectionRefusedError):
                            continue
                        if status['pid'] == child['parent_pid'] and status['exit'] is not None:
                            observed = path, status, child
                            break
                if observed:
                    break
                page.wait_for_timeout(20)
            assert observed, 'host did not report native parent exit while descendant held pipes'
            path, status, child = observed
            assert status['exit'] == {'code': 0, 'signal': None}, status
            assert not status['stdout_eof'] and not status['stderr_eof'], status
            assert fixture.alive(child['pid'])
            host_pid = int((path.parent / 'host.pid').read_text())
            expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label', '停止会话')
            page.locator('#a-session-toggle').click()
            expect(page.locator('#sdk-composer')).to_be_visible()
            expect(page.locator('#sdk-send')).to_be_disabled()
            fixture.wait_exited(child['pid'])
            fixture.wait_exited(host_pid)
            if (path.parent / 'status.json').exists():
                final = json.loads((path.parent / 'status.json').read_text())
                assert final['stdout_eof'] and final['stderr_eof'], final
            print('Agy: exited native parent retains pipe-owning descendant until explicit UI stop closes group and host', flush=True)
        finally:
            context.close()


def restart_idle_registry(browser, fixture):
    """An idle live host must be rediscovered before any SDK view is opened."""
    text = 'idle host survives without queued work'
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            submitted = send(page, text)
            final = 'Claude synthetic final ' + text if fixture.source == 'claude' else 'Synthetic Agy reply: ' + text if fixture.source == 'agy' else 'Grok final answer'
            expect(page.locator('.sdk-live')).to_contain_text(final)
            page.wait_for_function('NativeSdkView.raw && !NativeSdkView.raw.running')
            gone(page, submitted['request_id'])
            settled(page, fixture, submitted['request_id'])
            assert fixture.entry(submitted['request_id'])['state'] == 'completed'
            assert not any(row['state'] in ('sending', 'accepted', 'queued') for row in fixture.journal())
            pid = fixture.native_pid()
            initializations = fixture.initialization_count()
            assert initializations > 0
            inputs = fixture.inputs()
        finally:
            context.close()
    assert fixture.alive(pid), 'idle native process died with SessionDock'
    with fixture.server() as base:
        context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
        context.route('**/*', lambda route: route.continue_()
                      if route.request.url.startswith(base + '/') else route.abort())
        page = context.new_page()
        page.set_default_timeout(20000)
        opens = []
        page.on('request', lambda request: opens.append(request.url)
                if request.url.endswith('/api/native-sdk/open') else None)
        try:
            page.goto(base, wait_until='domcontentloaded')
            page.wait_for_function('uid => S.sessions.some(row => row.uid === uid)', arg=uid)
            # A fresh browser context has no selected conversation. The list's
            # own live refresh must recognize the host without /open side effects.
            page.wait_for_function('uid => S.liveSdk.has(uid) && S.live.has(uid)', arg=uid)
            assert not opens, 'registry test opened an SDK before checking list activity'
            assert fixture.native_pid() == pid and fixture.alive(pid)
            assert fixture.initialization_count() == initializations
            assert fixture.inputs() == inputs
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            if not page.locator('#sdk-input').is_visible():
                page.locator('#a-sdk').click()
            expect(page.locator('#sdk-input')).to_be_visible()
            page.wait_for_function('NativeSdkView.connected')
            expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label', '停止会话')
            assert fixture.native_pid() == pid
            assert fixture.initialization_count() == initializations
            assert fixture.inputs() == inputs
            if fixture.source == 'agy':
                notices=page.locator('.sdk-live').inner_text().count('Native runtime restarted')
                second=send(page,'continue in the same native runtime')
                gone(page,second['request_id'])
                assert fixture.native_pid()==pid and fixture.inputs().count(text)==1
                assert page.locator('.sdk-live').inner_text().count('Native runtime restarted')==notices
            print(f'{fixture.source}: idle host rediscovered by list before SDK open, same PID and no repeated initialization/input', flush=True)
        finally:
            context.close()


def restart_historical_result(browser, fixture, screenshots=None, *, persisted_state='completed',
                              mismatched_delivery=False):
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture, accept_popups=False)
        try:
            old = send(page, '__RECOVERED_STREAM__')
            gone(page, old['request_id'])
            current = send(page, '__HISTORICAL_ERROR__')
            expect(page.locator('.sdk-live')).to_contain_text('历史错误未污染本轮')
            gone(page, current['request_id'])
            settled(page, fixture, current['request_id'])
            assert fixture.entry(current['request_id'])['state'] == 'completed'
            pid = fixture.native_pid()
        finally:
            context.close()
    # Reproduce an older bridge's durable verdict while retaining the real
    # synthetic host output/binding. Never alter production or native records.
    journal_path = fixture.root / 'state/native-sdk-inputs.json'
    journal = json.loads(journal_path.read_text())
    entry = next(row for rows in journal.values() for row in rows
                 if row['request_id'] == current['request_id'])
    entry['state'] = persisted_state
    if persisted_state != 'completed':
        entry['error'] = 'agy_sdk_native_ERROR: The stream was interrupted. Please continue the task you were working on.'
    if mismatched_delivery:
        entry['delivery_id'] = 'different-native-delivery'
    saved_entry = json.loads(json.dumps(entry))
    journal_path.write_text(json.dumps(journal))
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture, accept_popups=False)
        try:
            expect(page.locator('.sdk-live')).to_contain_text('历史错误未污染本轮')
            page.wait_for_function('NativeSdkView.raw?.running === false')
            assert not page.evaluate('NativeSdkView.raw.error')
            assert fixture.native_pid() == pid and fixture.alive(pid)
            assert fixture.inputs('__HISTORICAL_ERROR__') == ['__HISTORICAL_ERROR__']
            page.wait_for_function('request => !NativeSdkView.raw?.input_queue?.some(row => row.request_id === request)',
                                   arg=current['request_id'])
            assert fixture.entry(current['request_id'])['state'] == 'completed'
            final_entry = fixture.entry(current['request_id'])
            assert final_entry['error'] is None
            for key in ('request_id', 'delivery_id', 'draft', 'input', 'consumption', 'attempts', 'actions'):
                assert final_entry[key] == saved_entry[key], key
            assert fixture.entry(old['request_id'])['state'] == 'completed'
            replay(page, base, 'turn', current)
            assert fixture.inputs('__HISTORICAL_ERROR__') == ['__HISTORICAL_ERROR__']
            pictures(page, uid, screenshots, f'agy-reconciled-{persisted_state}', all_sizes=True)
            following = send(page, 'after historical result restart')
            expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: after historical result restart')
            gone(page, following['request_id'])
            assert fixture.native_pid() == pid
            print(f'Agy: {persisted_state} historical-error receipt reconciled on restart, same runtime and no resend', flush=True)
        finally:
            context.close()


def check_recovery_logs(fixtures):
    for source in ('grok', 'agy', 'claude'):
        records = [json.loads(line)
                   for path in (fixtures[source].root / 'state/runtime-logs').glob('*.jsonl')
                   for line in path.read_text().splitlines()]
        transitions = [row for row in records if row.get('event') == 'native_sdk.input_transition']
        assert any(row['reason'] == 'service_recovery' for row in transitions), (source, transitions)
        assert any(row['to'] == 'completed' for row in transitions), source
        assert all(row['request_id'] and row['delivery_id'] and row['uid'] for row in transitions)
        if source == 'agy':
            reused = [row for row in records if row.get('event') == 'agy.sdk.write_receipt' and row['reused']]
            assert reused and all(row['written'] for row in reused), 'Agy receipt reuse must be explicit'
            assert any(row.get('event') == 'agy.sdk.turn_started' and row['recovering'] for row in records)
    print('Native queue: persisted recovery and reused-receipt diagnostic correlation verified', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--host-binary', type=Path, default=REPO / 'target/debug/ptyhost')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-queue-') as temporary, sync_playwright() as pw:
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        fixtures = {}
        try:
            root = Path(temporary)
            for source in ('grok', 'agy', 'claude'):
                fixtures[source] = NativeQueueFixture(root / source, source, args.binary, args.host_binary)
            fixtures['legacy-agy'] = NativeQueueFixture(root / 'legacy-agy', 'agy', args.binary, args.host_binary)
            fixtures['legacy-failed-agy'] = NativeQueueFixture(root / 'legacy-failed-agy', 'agy', args.binary, args.host_binary)
            fixtures['conflict-agy'] = NativeQueueFixture(root / 'conflict-agy', 'agy', args.binary, args.host_binary)
            for source in ('grok', 'agy', 'claude'):
                fixture = fixtures[source]
                basic_queue(browser, fixture, args.screenshots)
            cancelled_draft(browser, fixtures['grok'], args.screenshots)
            failure_and_unknown(browser, fixtures['grok'], args.screenshots)
            restart_queue(browser, fixtures['claude'], args.screenshots)
            restart_output(browser, fixtures['agy'])
            legacy_unknown(browser, fixtures['legacy-agy'], args.screenshots)
            legacy_unknown(browser, fixtures['legacy-failed-agy'], args.screenshots, 'failed')
            restart_conflicting_result(browser, fixtures['conflict-agy'])
            for marker in ('__ERROR__', '__UNRECOVERED_STREAM__'):
                name = 'error-agy-' + marker
                fixtures[name] = NativeQueueFixture(root / name, 'agy', args.binary, args.host_binary)
                restart_conflicting_result(browser, fixtures[name], args.screenshots, text=marker)
            for conflict, dismissed in ((True, False), (False, False)):
                name = f'closed-agy-{conflict}-{dismissed}'
                fixtures[name] = NativeQueueFixture(root / name, 'agy', args.binary, args.host_binary)
                closed_runtime_result(browser, fixtures[name], args.screenshots, conflict=conflict, dismissed=dismissed)
            restart_approval(browser, fixtures['claude'])
            restart_approval(browser, fixtures['grok'])
            stop_inherited_pipes(browser, fixtures['agy'])
            restart_idle_registry(browser, fixtures['claude'])
            restart_idle_registry(browser, fixtures['grok'])
            restart_idle_registry(browser, fixtures['agy'])
            for persisted_state, mismatch in (('completed', False), ('unknown', False),
                                              ('failed', False), ('unknown', True)):
                name = f'historical-agy-{persisted_state}-{mismatch}'
                fixtures[name] = NativeQueueFixture(root / name, 'agy', args.binary, args.host_binary)
                restart_historical_result(browser, fixtures[name], args.screenshots,
                                          persisted_state=persisted_state, mismatched_delivery=mismatch)
            check_recovery_logs(fixtures)
        finally:
            browser.close()
            for fixture in fixtures.values():
                fixture.close()
    print('PASS native SDK durable queues: Grok/Agy/Claude real browser, FIFO admission, shared SSE/cancel, STOP, request/action dedup, execution errors isolated from delivery, attachment publication, editor preservation and service restart', flush=True)


if __name__ == '__main__':
    main()
