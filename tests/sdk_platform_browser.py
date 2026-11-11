#!/usr/bin/env python3
"""Exercise native SDK adapters on the selected OS through the production page.

SDK_PLATFORM_WORKER is a JSON argv array running sdk_platform_fixtures.py,
locally or over SSH. SDK_PLATFORM_BASE optionally names an existing loopback
SSH forward. The worker creates all native stores and service state privately.
"""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO
from popups import on_popup
from sdk_ui_fixtures import start_sdk


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', default='claude', choices=('agy', 'claude', 'codebuddy', 'droid', 'grok', 'kiro'))
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--driver', type=Path, default=REPO / 'target/debug/examples/platform_fixture')
    args = parser.parse_args()
    command = json.loads(os.environ['SDK_PLATFORM_WORKER']) if 'SDK_PLATFORM_WORKER' in os.environ else [
        sys.executable, str(REPO / 'tests/sdk_platform_fixtures.py'), '--source', args.source,
        '--binary', str(args.binary), '--driver', str(args.driver), '--web', str(REPO / 'web')]
    with tempfile.TemporaryDirectory(prefix='sdk-platform-browser-') as tmp:
        log = (Path(tmp) / 'worker.log').open('w+')
        worker = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True)
        def receive():
            line = worker.stdout.readline()
            if not line:
                log.flush(); log.seek(0)
                raise AssertionError(('platform worker exited', worker.poll(), log.read()))
            return json.loads(line)
        def control(action):
            worker.stdin.write(json.dumps({'action': action}) + '\n'); worker.stdin.flush()
            reply = receive()
            assert reply['ok'], reply
            return reply['value']
        try:
            info = receive()
            print('PRIVATE', args.source, info, flush=True)
            base = os.environ.get('SDK_PLATFORM_BASE', info['base'])
            with sync_playwright() as pw:
                browser = pw.chromium.launch(headless=True)
                context = browser.new_context(viewport={'width': 1280, 'height': 900})
                page = context.new_page(); page.set_default_timeout(30000)
                page.on('pageerror', lambda error: print('PAGE ERROR', error, flush=True))
                def report_error(response):
                    if response.status < 400 or not any(response.url.endswith('/api/native-sdk/' + action)
                            for action in ('open', 'turn', 'respond', 'interrupt', 'stop')):
                        return
                    try: print('HTTP ERROR', response.url, response.status, response.text()[:1800], flush=True)
                    except Exception: pass  # Navigation may discard a diagnostic response body.
                page.on('response', report_error)
                on_popup(page, lambda dialog: dialog.accept())
                def open_sdk():
                    page.goto(base, wait_until='domcontentloaded')
                    page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=info['sid'])
                    uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', info['sid'])
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    if not page.locator('#sdk-input').is_visible(): page.locator('#a-sdk').click()
                    start_sdk(page)
                    return uid
                def send(text):
                    page.locator('#sdk-input').fill(text)
                    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/turn')) as reply:
                        page.locator('#sdk-send').click()
                    assert reply.value.ok, reply.value.text()
                    return reply.value.request.post_data_json['request_id']
                def settled(request):
                    deadline = time.monotonic() + 30
                    while time.monotonic() < deadline:
                        rows = [row for group in control('evidence')['inputs'].values() for row in group]
                        matched = [row for row in rows if row.get('request_id') == request]
                        if matched and matched[0]['state'] == 'completed':
                            page.wait_for_function('NativeSdkView.raw?.running === false')
                            return
                        page.wait_for_timeout(100)
                    raise AssertionError(('input did not complete', request, rows))
                def native_pids():
                    return {row['pid'] for row in control('evidence')['hosts'] if row['exit'] is None}
                try:
                    uid = open_sdk()
                    first = send('platform first input'); settled(first)
                    initial = native_pids(); assert len(initial) == 1, initial
                    control('restart')
                    assert native_pids() == initial, 'service restart killed the native peer'
                    open_sdk()
                    second = send('platform second input'); settled(second)
                    assert native_pids() == initial, 'restart spawned a replacement native peer'
                    evidence = control('evidence')
                    serialized = [json.dumps(row) for row in evidence['events']]
                    for text in ('platform first input', 'platform second input'):
                        assert sum(text in row for row in serialized) == 1, (text, evidence)
                    # Pending native permission must remain actionable after a
                    # second service restart, without replaying the user input.
                    if args.source in ('claude', 'codebuddy', 'droid'):
                        request = send('approval')
                        page.wait_for_function('NativeSdkView.raw?.requests?.length > 0')
                        key = page.evaluate('NativeSdkView.raw.requests[0].key')
                        control('restart'); open_sdk()
                        deny = page.locator(f'[data-sdk-request="{key}"]').get_by_role('button', name='拒绝', exact=True)
                        with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/respond')) as reply:
                            deny.click()
                        assert reply.value.ok, reply.value.text()
                        settled(request)
                        assert native_pids() == initial
                    hold = {'agy': 'hold:platform', 'grok': 'wait-for-cancel', 'kiro': 'wait-for-cancel'}.get(args.source, 'hold')
                    request = send(hold)
                    page.wait_for_function('NativeSdkView.raw?.running === true')
                    control('restart'); open_sdk()
                    expect(page.locator('#sdk-stop')).to_be_visible()
                    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/interrupt')) as reply:
                        page.locator('#sdk-stop').click()
                    assert reply.value.ok, reply.value.text()
                    page.wait_for_function('NativeSdkView.raw?.running === false')
                    assert native_pids() == initial, 'interrupt must retain the native runtime'
                    if os.environ.get('SDK_PLATFORM_SCREENSHOTS'):
                        directory = Path(os.environ['SDK_PLATFORM_SCREENSHOTS']); directory.mkdir(parents=True, exist_ok=True)
                        page.screenshot(path=str(directory / (args.source + '.png')), full_page=True)
                    stop = page.locator('#a-session-toggle')
                    if not stop.is_visible(): page.locator('#a-more').click()
                    expect(stop).to_have_attribute('aria-label', '停止会话')
                    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/stop')) as reply:
                        stop.click()
                    assert reply.value.ok, reply.value.text()
                    page.wait_for_function('SdkView.readOnly === true && !ConsoleUI.busy.has(S.sel)')
                    deadline = time.monotonic() + 10
                    while native_pids() and time.monotonic() < deadline: page.wait_for_timeout(50)
                    assert not native_pids(), 'explicit stop left the private native peer running'
                    print('PASS', args.source, 'native platform: resume, sends once, persistent PID, active restart, interrupt, stop'
                          + ('; permission recovery' if args.source in ('claude', 'codebuddy', 'droid') else ''), flush=True)
                except BaseException:
                    print('DIAGNOSTIC', control('diagnostic'), flush=True)
                    print('EVIDENCE', control('evidence'), flush=True)
                    print('SDK STATE', page.evaluate('({connected:SdkView.connected,readOnly:SdkView.readOnly,uid:NativeSdkView.uid,raw:NativeSdkView.raw})'), flush=True)
                    print('PAGE', page.locator('body').inner_text()[-8000:], flush=True)
                    raise
                finally: browser.close()
        finally:
            if worker.poll() is None:
                worker.stdin.write('{"action":"close"}\n'); worker.stdin.flush()
                worker.wait(timeout=45)
            log.close()
            assert worker.returncode == 0, ('platform worker cleanup failed', worker.returncode)


if __name__ == '__main__': main()
