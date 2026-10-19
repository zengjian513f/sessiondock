#!/usr/bin/env python3
"""Durable native SDK queues via Chromium, synthetic peers and real node restarts.

No native account, model, production directory or CLI default is used.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from native_sdk_queue_fixtures import NativeQueueFixture



def debug(page):
    page.on('pageerror', lambda error: print('queue pageerror', error, flush=True))
    page.on('requestfailed', lambda request: print('queue requestfailed', request.url, request.failure, flush=True))
    page.on('console', lambda message: print('queue console', message.type, message.text, flush=True) if message.type == 'error' else None)
    page.on('request', lambda request: print('queue action request', request.url, request.post_data_json, flush=True)
            if '/api/native-sdk/queue_' in request.url else None)


def open_page(browser, base, fixture):
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(20000)
    debug(page)
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=fixture.sid)
    uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', fixture.sid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    if not page.locator('#sdk-input').is_visible():
        page.locator('#a-sdk').click()
    expect(page.locator('#sdk-input')).to_be_enabled()
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
        page.locator('.sdk-queue').scroll_into_view_if_needed()
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
                expect(page.locator('#sdk-input')).to_be_visible()
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
            state(page, first_id, 'accepted')
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
            state(page, first['request_id'], 'accepted')
            attach(page)
            original = send(page, 'lost ACK attachment 中文', lose_ack=True)
            original_id = original['request_id']
            state(page, original_id, 'queued')
            expect(page.locator('#sdk-input')).to_have_value('lost ACK attachment 中文')
            click_action(page, original_id, 'queue_cancel', '撤销')
            gone(page, original_id)
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
        context, page, uid = open_page(browser, base, fixture)
        try:
            print('Grok: explicit failure blocks later input; retry action replay is idempotent', flush=True)
            failed = send(page, 'native-error')
            request = failed['request_id']
            state(page, request, 'failed')
            blocked = send(page, 'blocked behind failure')
            state(page, blocked['request_id'], 'queued')
            assert not fixture.inputs('blocked behind failure')
            retry = click_action(page, request, 'queue_retry', '重试')
            state(page, request, 'failed', attempts=1)
            assert len(fixture.inputs('native-error')) == 2
            replay(page, base, 'queue_retry', retry)
            assert fixture.entry(request)['attempts'] == 1
            assert len(fixture.inputs('native-error')) == 2
            pictures(page, uid, screenshots, 'failed')
            click_action(page, request, 'queue_cancel', '撤销')
            gone(page, blocked['request_id'])
            assert len(fixture.inputs('blocked behind failure')) == 1

            print('Grok: uncertain transport does not replay; explicit confirmed retry uses a fresh delivery ID', flush=True)
            unknown = send(page, 'disconnect')
            request = unknown['request_id']
            state(page, request, 'unknown')
            blocked = send(page, 'blocked behind unknown')
            state(page, blocked['request_id'], 'queued')
            card(page, request).get_by_role('button', name='重试', exact=True).click()
            page.get_by_role('button', name='取消', exact=True).click()
            state(page, request, 'unknown')
            assert len(fixture.inputs('disconnect')) == 1
            card(page, request).get_by_role('button', name='重试', exact=True).click()
            with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/queue_retry')) as response:
                page.get_by_role('button', name='重新发送', exact=True).click()
            assert response.value.ok, response.value.text()
            retry = response.value.request.post_data_json
            state(page, request, 'unknown', attempts=1)
            assert len(fixture.inputs('disconnect')) == 2
            replay(page, base, 'queue_retry', retry)
            assert fixture.entry(request)['attempts'] == 1 and len(fixture.inputs('disconnect')) == 2
            assert not fixture.inputs('blocked behind unknown')
            card(page, request).get_by_role('button', name='移除记录', exact=True).click()
            page.get_by_role('button', name='移除记录', exact=True).last.click()
            gone(page, blocked['request_id'])
            assert len(fixture.inputs('blocked behind unknown')) == 1
            print('Grok: failed/unknown pause, explicit retry, action receipt replay and local dismissal passed', flush=True)
        finally:
            context.close()


def restart_queue(browser, fixture, screenshots):
    print('Claude: persist accepted/queued inputs and newer editor, restart the real isolated node', flush=True)
    holds_before = len(fixture.inputs('hold'))
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        first = send(page, fixture.blocker)
        state(page, first['request_id'], 'accepted')
        attach(page)
        queued = send(page, 'durable queued attachment after restart')
        state(page, queued['request_id'], 'queued')
        with page.expect_response(lambda response: '/api/session/conversation' in response.url and response.request.method == 'POST' and response.ok):
            page.locator('#sdk-input').fill('newer draft survives service restart 中文')
        expect(page.locator('#sdk-input')).to_have_value('newer draft survives service restart 中文')
        assert len(fixture.inputs('hold')) == holds_before + 1 and not fixture.inputs('durable queued attachment')
        paths = fixture.entry(queued['request_id'])['input']['attachments']
        context.close()
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            state(page, first['request_id'], 'unknown')
            state(page, queued['request_id'], 'queued')
            expect(page.locator('#sdk-input')).to_have_value('newer draft survives service restart 中文')
            assert len(fixture.inputs('hold')) == holds_before + 1 and not fixture.inputs('durable queued attachment')
            pictures(page, uid, screenshots, 'restart-unknown', all_sizes=True)
            card(page, first['request_id']).get_by_role('button', name='恢复文字', exact=True).click()
            expect(page.locator('#sdk-input')).to_have_value('newer draft survives service restart 中文\nhold')
            card(page, first['request_id']).get_by_role('button', name='移除记录', exact=True).click()
            page.get_by_role('button', name='移除记录', exact=True).last.click()
            gone(page, queued['request_id'])
            assert len(fixture.inputs('hold')) == holds_before + 1 and len(fixture.inputs('durable queued attachment')) == 1
            expect(page.locator('#sdk-input')).to_have_value('newer draft survives service restart 中文\nhold')
            entry = fixture.entry(queued['request_id'])
            assert entry['state'] == 'completed' and entry['consumption'] == 'observed', entry
            assert entry['input']['attachments'] == paths
            repeat = replay(page, base, 'turn', queued)
            assert repeat['state'] == 'completed' and len(fixture.inputs('durable queued attachment')) == 1
            assert fixture.entry(first['request_id'])['state'] == 'cancelled'
            print('Claude: restart unknown blocks, no automatic replay, confirmed dismissal resumes safe queued input, newer draft and publication retained passed', flush=True)
        finally:
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--host-binary', type=Path, default=BINARY.parent / 'ptyhost')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-queue-') as temporary, sync_playwright() as pw:
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        try:
            root = Path(temporary)
            fixtures = {source: NativeQueueFixture(root / source, source, args.binary, args.host_binary)
                        for source in ('grok', 'agy', 'claude')}
            for fixture in fixtures.values():
                basic_queue(browser, fixture, args.screenshots)
            cancelled_draft(browser, fixtures['grok'], args.screenshots)
            failure_and_unknown(browser, fixtures['grok'], args.screenshots)
            restart_queue(browser, fixtures['claude'], args.screenshots)
        finally:
            browser.close()
    print('PASS native SDK durable queues: Grok/Agy/Claude real browser, FIFO admission, shared SSE/cancel, STOP, request/action dedup, failed/unknown retry, attachment publication, editor preservation and service restart', flush=True)


if __name__ == '__main__':
    main()
