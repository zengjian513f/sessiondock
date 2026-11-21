#!/usr/bin/env python3
"""Exercise hosted SDK pipe backpressure and incomplete offline exit in Chromium."""
import argparse
import sys
import uuid
from pathlib import Path

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from sdk_hosted_recovery_fixtures import (
    BLOCKED_TEXT, OFFLINE, OFFLINE_TEXT, PAUSE, STDERR_TEXT, STDOUT_TEXT,
    alive, blocked_receipt, checkpoint, fixture, host_status, lose_private_host, until,
)
from sdk_ui_fixtures import start_sdk
from popups import on_popup


def select_session(page, worker):
    on_popup(page, lambda dialog: dialog.accept())
    page.goto(worker.base, wait_until='domcontentloaded')
    page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=worker.peer.sid)
    uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', worker.peer.sid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    if not page.locator('#sdk-input').is_visible():
        page.locator('#a-sdk').click()
    return uid


def submit(page, text):
    page.locator('#sdk-input').fill(text)
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/turn')) as accepted:
        page.locator('#sdk-send').click()
    assert accepted.value.ok, accepted.value.text()


def stop(page):
    button = page.locator('#a-session-toggle')
    if not button.is_visible():
        page.locator('#a-more').click()
    expect(button).to_have_attribute('aria-label', '停止会话')
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/stop'), timeout=8000) as stopped:
        button.click()
    assert stopped.value.ok, stopped.value.text()


def pressure(browser, source, binary, restart=False):
    with fixture(source, binary, PAUSE) as worker:
        page = browser.new_page(viewport={'width': 1280, 'height': 900}, service_workers='block')
        uid = select_session(page, worker)
        start_sdk(page)
        submit(page, PAUSE)
        until(lambda: (worker.root / 'gate-ready').exists(), 'stdin gate was not armed')
        expect(page.locator('.sdk-live')).to_contain_text('final ' + PAUSE)
        page.wait_for_function('NativeSdkView.raw?.running === false')
        response = page.request.post(worker.base + '/api/native-sdk/turn', data={
            'uid': uid, 'text': BLOCKED_TEXT, 'request_id': str(uuid.uuid4()),
            'attachments': [], 'quotes': [],
        })
        assert response.ok, response.text()
        host, initial = host_status(worker)
        entry_id = until(lambda: blocked_receipt(host, source), 'no in-flight pipe write K')
        if restart:
            page.close()
            worker.stop_service()
        (worker.root / 'request-output').touch()
        if restart:
            # Buffered stdout must be recovered despite the existing unknown
            # receipt and the still blocked original pipe write.
            worker.start()
            page = browser.new_page(viewport={'width': 1280, 'height': 900}, service_workers='block')
            select_session(page, worker)
            start_sdk(page)
        expect(page.locator('.sdk-live')).to_contain_text(STDOUT_TEXT)
        assert blocked_receipt(host, source) == entry_id
        assert host_status(worker)[1]['pid'] == initial['pid'] and alive(initial['pid'])
        if restart:
            # Allow the original write to finish. Neither the recovery worker
            # nor the page should submit a second native input.
            (worker.root / 'release-fixture').touch()
            def inputs():
                return [row for row in worker.peer.frames('user')
                        if row['message']['content'] == BLOCKED_TEXT]
            until(inputs, 'original blocked input did not reach the fake CLI')
            page.wait_for_function('NativeSdkView.raw?.running === false')
            assert len(inputs()) == 1
        stop(page)
        until(lambda: not alive(initial['pid']), 'explicit stop left the native child running')
        saved = checkpoint(worker)
        assert saved['host_key'] is None and STDERR_TEXT in saved['stderr']
        if not restart:
            assert blocked_receipt(host, source) == entry_id
        page.close()
    print(f'PASS {source}: {"restart preserves the original uncertain write" if restart else "read and stop while stdin is blocked"}', flush=True)


def lost_host(browser, binary):
    with fixture('codebuddy', binary, OFFLINE) as worker:
        page = browser.new_page(viewport={'width': 1280, 'height': 900}, service_workers='block')
        uid = select_session(page, worker)
        start_sdk(page)
        submit(page, OFFLINE)
        until(lambda: (worker.root / 'gate-ready').exists(), 'offline exit was not armed')
        page.wait_for_function('NativeSdkView.raw?.running === true')
        page.close()
        worker.stop_service()
        host, _ = host_status(worker)
        (worker.root / 'exit-leader').touch()
        until(lambda: host_status(worker)[1]['exit'] is not None, 'native leader exit not persisted')
        status = host_status(worker)[1]
        assert not status['stdout_eof'] and not status['stderr_eof'], status
        lose_private_host(host, worker.root)
        (worker.root / 'release-descendant').touch()
        worker.start()

        def retired():
            saved = checkpoint(worker)
            return saved if saved['host_key'] is None else None
        saved = until(retired, 'dead host was restored as a live runtime')
        assert saved['runtime_error'] and 'before both output streams reached EOF' in saved['runtime_error']
        assert not saved['turns'] and not saved['native_running']
        assert any(OFFLINE_TEXT in item.get('text', '') for item in saved['items'])
        assert saved['stdout_offset'] == host_status(worker)[1]['stdout_len']
        page = browser.new_page(viewport={'width': 1280, 'height': 900}, service_workers='block')
        reply = page.request.get(worker.base + '/api/live').json()
        assert uid not in reply['sdk_uids']
        assert len(list((worker.state / 'sdkhost').glob('codebuddy-*/status.json'))) == 1
        select_session(page, worker)
        start_sdk(page)
        page.wait_for_function('NativeSdkView.raw?.running === false')
        assert not any(row.get('message', {}).get('content') == OFFLINE for row in worker.peer.frames('user')[1:])
        page.close()
    print('PASS CodeBuddy: loss before EOF recovers output, retires the old runtime and does not replay input', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    if sys.platform != 'linux':
        print('SKIP private Linux pipe fixture')
        return
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        for source in ('claude', 'codebuddy', 'droid'):
            pressure(browser, source, args.binary.resolve())
        pressure(browser, 'codebuddy', args.binary.resolve(), restart=True)
        lost_host(browser, args.binary.resolve())
        browser.close()


if __name__ == '__main__':
    main()
