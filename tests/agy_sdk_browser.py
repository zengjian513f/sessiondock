#!/usr/bin/env python3
"""Native Agy SDK through generic SessionDock HTTP/UI and a private fake CLI.

Requires the reviewer's integrated native-sdk routes/UI and a prebuilt binary.
No build, real CLI, credential, model, production home, or PTY is used here.
The test clicks SDK/SEND/STOP, types real input, reloads during accepted work,
and checks the native fixture's independently recorded input/argv/signal log.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

sys.dont_write_bytecode = True
from playwright.sync_api import expect, sync_playwright
from agy_sdk_fixtures import AgyFixture, MODEL, SID, TITLE
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from new_session_model_browser import open_dialog
from send_browser import initialize


WATCH = r'''uid => {
  window.__agyEvents?.close();
  window.__agySnapshots = [];
  window.__agyEvents = new EventSource('/api/native-sdk/events?uid='+encodeURIComponent(uid));
  window.__agyEvents.addEventListener('sdk', event => {
    const snapshot=JSON.parse(event.data);
    if(snapshot.type==='snapshot') {
      window.__agySnapshots.push(snapshot);
      window.__agySnapshot=snapshot;
    }
  });
}'''


def watch(page, uid):
    page.evaluate(WATCH, uid)
    page.wait_for_function('window.__agySnapshots.length > 0')


def idle(page):
    page.wait_for_function('window.__agySnapshot?.raw?.running === false')


def post(page, base, method, body, *, ok=True):
    response = page.request.post(base + '/api/native-sdk/' + method, data=body)
    assert response.ok == ok, (method, response.status, response.text())
    return response.json()


def select_sdk(page, uid=None):
    row = page.locator('#side .item[data-uid]').filter(has_text=TITLE).first if uid is None else page.locator(f'#side .item[data-uid="{uid}"]')
    expect(row).to_be_visible()
    uid = row.get_attribute('data-uid')
    row.click()
    expect(page.locator('#a-sdk')).to_be_visible()
    page.locator('#a-sdk').click()
    expect(page.locator('#sdk-input')).to_be_visible()
    return uid


def send(page, text):
    page.locator('#sdk-input').fill(text)
    with page.expect_response(lambda response: '/api/native-sdk/turn' in response.url and response.request.method == 'POST') as event:
        page.locator('#sdk-send').click()
    assert event.value.ok, event.value.text()
    return event.value.json()


def exercise(browser, base, fixture, screenshots=None):
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.on('response', lambda response: print('SDK HTTP error', response.status, response.url, response.text(), flush=True) if '/api/native-sdk/' in response.url and response.status >= 400 else None)
    page.set_default_timeout(15000)
    errors = []
    page.on('pageerror', lambda error: errors.append(error.stack))
    try:
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        uid = select_sdk(page)
        opened = post(page, base, 'open', {'uid': uid})
        assert opened['session_id'] == SID and opened['cwd'] == str(fixture.work)
        assert opened['capabilities']['send'] and opened['capabilities']['models']
        assert opened['capabilities']['respond'] is False
        watch(page, uid)
        models = post(page, base, 'models', {'uid': uid})
        assert MODEL in [row['id'] for row in models['models']], models
        assert not fixture.inputs(), 'open/model discovery must not submit a prompt'
        page.locator('#sdk-model').select_option(MODEL)
        expect(page.locator('#sdk-effort')).to_be_visible()
        page.locator('#sdk-effort').select_option('high')
        page.locator('#sdk-more').click()
        page.locator('#native-sdk-mode').select_option('plan')
        page.locator('#sdk-more').click()
        print('Agy: click SDK, type/send, sparse tool result, idempotent retry', flush=True)
        receipt = send(page, 'SDK 中文\nsecond line')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: SDK 中文')
        expect(page.locator('.sdk-live')).to_contain_text('synthetic tool output')
        idle(page)
        post(page, base, 'turn', {'uid': uid, 'text': 'SDK 中文\nsecond line', 'request_id': receipt['request_id']})
        assert len(fixture.inputs('SDK 中文\nsecond line')) == 1
        assert fixture.inputs('SDK 中文\nsecond line')[0]['model'] == MODEL
        assert fixture.inputs('SDK 中文\nsecond line')[0]['effort'] == 'high'
        assert fixture.inputs('SDK 中文\nsecond line')[0]['mode'] == 'plan'
        page.locator('#sdk-usage-toggle').click()
        expect(page.locator('#native-usage-detail')).to_contain_text('12,800')
        # Native quota may arrive after the menu opens, with a long diagnostic.
        # Reposition the growing popover so the actual refresh button is usable.
        def expanded_usage(route):
            response = route.fetch()
            data = response.json()
            payload = data.get('raw', data)
            payload.setdefault('quota', {})['error'] = 'Delayed native quota diagnostic. ' * 80
            route.fulfill(response=response, json=data)
        page.route('**/api/native-sdk/usage', expanded_usage, times=1)
        page.locator('#native-usage-refresh').click()
        expect(page.locator('#native-usage-detail')).to_contain_text('72%')
        expect(page.locator('#native-usage-detail')).to_contain_text('Delayed native quota diagnostic.')
        menu_box = page.locator('#sdk-usage-menu').bounding_box()
        assert menu_box['y'] >= 0 and menu_box['y'] + menu_box['height'] <= 900, menu_box
        page.locator('#native-usage-refresh').click()
        expect(page.locator('#native-usage-detail')).not_to_contain_text('Delayed native quota diagnostic.')
        page.locator('#sdk-usage-toggle').click()
        before = len(fixture.inputs())
        page.locator('#sdk-input').fill('/usage')
        page.locator('#sdk-send').click()
        expect(page.locator('#sdk-input')).to_have_value('')
        assert len(fixture.inputs()) == before, 'slash query must never reach NDJSON user input'
        page.locator('#sdk-usage-toggle').click()
        page.locator('#sdk-input').fill('/unknown-native-command')
        page.locator('#sdk-send').click()
        expect(page.locator('.sdk-status')).to_contain_text('尚未映射')
        expect(page.locator('#sdk-input')).to_have_value('/unknown-native-command')
        assert len(fixture.inputs()) == before
        page.locator('#sdk-input').fill('')
        snapshot = page.evaluate('window.__agySnapshot')
        tool = next(item for item in snapshot['raw']['items'] if item['role'] == 'tool')
        assert tool['input']['AbsolutePath'] == str(fixture.work / 'synthetic.txt')
        assert tool['output'] == 'synthetic tool output'
        ids = [item['id'] for item in snapshot['raw']['items']]
        assert len(ids) == len(set(ids)), ids

        print('Agy: running request retry, FIFO, detach/reopen without cancellation', flush=True)
        held = send(page, 'hold:detached')
        expect(page.locator('.sdk-live')).to_contain_text('Agy running detached')
        post(page, base, 'turn', {'uid': uid, 'request_id': held['request_id'], 'text': 'hold:detached'})
        post(page, base, 'turn', {'uid': uid, 'request_id': 'queued-once', 'text': 'queued after detach'})
        # Closing this document also closes every EventSource; the accepted turn must survive.
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        fixture.release('detached')
        select_sdk(page, uid)
        watch(page, uid)
        expect(page.locator('.sdk-live')).to_contain_text('finished detached')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: queued after detach')
        idle(page)
        assert len(fixture.inputs('hold:detached')) == len(fixture.inputs('queued after detach')) == 1
        assert all(row['sid'] == SID for row in fixture.inputs())

        print('Agy: native SIGINT then resume same SID', flush=True)
        send(page, 'hold:interrupt')
        expect(page.locator('.sdk-live')).to_contain_text('Agy running interrupt')
        page.locator('#sdk-stop').click()
        idle(page)
        assert any(event['kind'] == 'signal' and event['signal'] == 'SIGINT' for event in fixture.events())
        send(page, 'after native interrupt')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: after native interrupt')
        idle(page)

        print('Agy: unsupported responses, stderr redaction, independent writer rejection', flush=True)
        post(page, base, 'respond', {'uid': uid, 'key': 'not-a-native-request', 'result': 'approve'}, ok=False)
        send(page, '__ERROR__')
        idle(page)
        expect(page.locator('.sdk-live')).to_contain_text('synthetic native error')
        diagnostic = page.evaluate('window.__agySnapshot.raw.diagnostics')
        assert 'synthetic transport diagnostic' in diagnostic
        assert fixture.env['AGY_SDK_FAKE_KEY'] not in diagnostic and '[redacted]' in diagnostic
        page.wait_for_function("NativeSdkView.raw.input_queue?.some(row=>row.text==='__ERROR__' && row.state==='failed')")
        failed = page.evaluate("NativeSdkView.raw.input_queue.find(row=>row.text==='__ERROR__')")
        page.locator(f'[data-queue-id="{failed["id"]}"]').get_by_role('button', name='撤销', exact=True).click()
        expect(page.locator(f'[data-queue-id="{failed["id"]}"]')).to_have_count(0)
        fixture.occupy()
        before = len(fixture.inputs())
        send(page, 'must not take over')
        page.wait_for_function("NativeSdkView.raw.input_queue?.some(row=>row.text==='must not take over' && ['failed','unknown'].includes(row.state))")
        refused = page.evaluate("NativeSdkView.raw.input_queue.find(row=>row.text==='must not take over')")
        assert 'busy' in refused['error'] or '占用' in refused['error'], refused
        assert len(fixture.inputs()) == before
        fixture.unoccupy()
        page.locator(f'[data-queue-id="{refused["id"]}"]').get_by_role('button', name='移除记录' if refused['state']=='unknown' else '撤销', exact=True).click()
        if refused['state']=='unknown':
            page.get_by_role('button', name='移除记录', exact=True).last.click()
        expect(page.locator(f'[data-queue-id="{refused["id"]}"]')).to_have_count(0)
        send(page, 'after external writer exits')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: after external writer exits')
        idle(page)

        print('Agy: create through the real picker, native SID receipt and first SEND', flush=True)
        open_dialog(page)
        page.locator('input[name="new-source"][value="agy"]').check()
        page.locator('#new-cwd').fill(str(fixture.work))
        with page.expect_response(lambda response: '/api/native-sdk/create' in response.url) as created_event:
            page.locator('#new-session-go').click()
        assert created_event.value.ok, created_event.value.text()
        created = created_event.value.json()
        assert created['session_id'] != SID
        assert (fixture.native / 'conversations' / (created['session_id'] + '.db')).is_file()
        assert created['session']['source'] == 'agy'
        assert created['uid'].startswith('agy:')
        assert not [row for row in fixture.inputs() if row['sid'] == created['session_id']]
        expect(page.locator('#sdk-input')).to_be_visible()
        watch(page, created['uid'])
        send(page, 'created native conversation')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: created native conversation')
        expect(page.locator('#sdk-input')).to_have_value('')
        idle(page)
        assert fixture.inputs('created native conversation')[0]['sid'] == created['session_id']
        page.reload(wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        select_sdk(page, created['uid'])
        expect(page.locator('#sdk-input')).to_have_value('')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: created native conversation')
        assert not list((fixture.root / 'host').glob('sessiondock-*.json')), 'SDK must not create a PTY'
        if screenshots:
            screenshots.mkdir(parents=True, exist_ok=True)
            for width in (1280, 390, 320):
                page.set_viewport_size({'width': width, 'height': 900})
                if width < 500 and not page.locator('#sdk-input').is_visible():
                    selected = page.evaluate('S.sel')
                    page.locator(f'#side .item[data-uid="{selected}"]').click()
                expect(page.locator('#sdk-input')).to_be_visible()
                for theme in ('light', 'dark'):
                    page.emulate_media(color_scheme=theme)
                    page.screenshot(path=str(screenshots / f'agy-sdk-{width}-{theme}.png'), full_page=True)
                    page.locator('#sdk-usage-toggle').click()
                    expect(page.locator('#native-usage-detail')).to_contain_text('12,800')
                    page.screenshot(path=str(screenshots / f'agy-sdk-usage-{width}-{theme}.png'), full_page=True)
                    page.locator('#sdk-usage-toggle').click()
                    page.locator('#sdk-more').click()
                    page.screenshot(path=str(screenshots / f'agy-sdk-settings-{width}-{theme}.png'), full_page=True)
                    page.locator('#sdk-more').click()
        assert not errors, errors
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--ptyhost', type=Path, default=REPO / 'target/debug/ptyhost')
    parser.add_argument('--frontend', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    os.environ.setdefault('PLAYWRIGHT_BROWSERS_PATH', str(Path.home() / '.cache/ms-playwright'))
    if args.frontend:
        os.environ['SESSIONDOCK_TEST_WEB_DIR'] = str(args.frontend.resolve())
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-sdk-') as tmp:
        root = Path(tmp).resolve()
        fixture = AgyFixture(root, args.ptyhost.resolve(strict=True))
        with patch.dict(os.environ, fixture.env):
            initialize('--initialize-lifecycle', root / 'ledger', args.binary.resolve(strict=True))
            with isolated_server(Corpus(root), args.binary, host_dir=root / 'host',
                    lifecycle_dir=root / 'ledger', state_dir=root / 'state',
                    launcher_config=fixture.launcher, extra_env=fixture.extra_env) as (base, _), sync_playwright() as pw:
                options = {'headless': True}
                if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                    options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                browser = pw.chromium.launch(**options)
                try:
                    exercise(browser, base, fixture, args.screenshots)
                finally:
                    browser.close()
    print('PASS Agy native SDK browser', flush=True)


if __name__ == '__main__':
    main()
