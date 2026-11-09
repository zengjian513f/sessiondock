#!/usr/bin/env python3
"""Droid running-turn steering and durable FIFO through Node/Hub Chromium UI."""
import argparse
from contextlib import ExitStack
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import sync_playwright, expect
from claude_sdk_browser import open_sdk, send, wait_snapshot
from droid_sdk_fixtures import DroidFixture
from droid_actions_browser import calls, capture
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import node_env, free_port, TOKEN
from sdk_send_gesture_fixtures import exercise_idle_send_gestures, exercise_send_gestures


def check(page, base, fixture, shots, label):
    errors = []
    page.on('pageerror', lambda error: errors.append(error.stack or str(error)))
    page.goto(base, wait_until='networkidle')
    row = page.locator('#side .item[data-uid^="droid:"]').first
    expect(row).to_be_visible()
    uid = row.get_attribute('data-uid')
    assert ('~' in uid) == (label == 'hub')
    open_sdk(page, fixture, expect, uid=uid)
    box = page.locator('#sdk-input')
    button = page.locator('#sdk-send')
    def sent(text):
        return [frame for frame in calls(fixture, 'droid.add_user_message') if frame['params']['text'] == text]
    def idle(text):
        expect(page.locator('.sdk-live')).to_contain_text('Droid final ' + text)
        wait_snapshot(page, uid, lambda raw: not raw['running'])
        assert len(sent(text)) == 1
    exercise_idle_send_gestures(page, 'droid', label, idle)
    send(page, 'hold')
    wait_snapshot(page, uid, lambda raw: raw['running'])
    page.set_viewport_size({'width':390, 'height':900})
    open_sdk(page, fixture, expect, uid=uid)
    exercise_send_gestures(page, 'droid', label, shots)
    page.set_viewport_size({'width':1280, 'height':900})
    open_sdk(page, fixture, expect, uid=uid)

    send(page, 'ordinary FIFO ' + label)
    fifo = page.locator('.sdk-queued').filter(has_text='ordinary FIFO ' + label)
    expect(fifo).to_be_visible()
    assert not sent('ordinary FIFO ' + label)
    send(page, 'converted steer ' + label)
    converted = page.locator('.sdk-queued').filter(has_text='converted steer ' + label)
    expect(converted).to_be_visible()
    queue_id = converted.get_attribute('data-queue-id')
    box.fill('preserve unrelated draft')
    gate = fixture.root / 'steer-delay'; gate.touch()
    with page.expect_request(lambda request: request.url.endswith('/api/native-sdk/queue_steer')):
        converted.get_by_role('button', name='立即引导', exact=True).click()
    expect(converted.get_by_role('button', name='立即引导', exact=True)).to_be_disabled()
    gate.unlink()
    expect(converted).to_have_count(0)
    expect(box).to_have_value('preserve unrelated draft')
    expect(page.locator('.sdk-live')).to_contain_text('converted steer ' + label)
    repeated = page.request.post(base + '/api/native-sdk/queue_steer', data={'uid':uid, 'id':queue_id, 'request_id':'repeat-conversion'})
    assert repeated.ok, repeated.text()
    assert len(sent('converted steer ' + label)) == 1
    expect(fifo).to_be_visible()
    assert not sent('ordinary FIFO ' + label)

    def steer(text):
        box.fill(text)
        with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/steer')) as response:
            box.press('Control+Enter')
        expect(button).to_be_enabled()
        return response.value
    accepted = steer('direct steering ' + label)
    assert accepted.ok, accepted.text()
    expect(box).to_have_value('')
    expect(page.locator('.sdk-live')).to_contain_text('direct steering ' + label)
    repeat = page.request.post(accepted.url, data=accepted.request.post_data_json)
    assert repeat.ok, repeat.text()
    assert len(sent('direct steering ' + label)) == 1
    assert page.evaluate('NativeSdkView.running')
    assert page.evaluate("NativeSdkView.items.some(item => item.text?.includes('Droid working continuing after steer'))")

    # Droid accepts the same prepared file references as ordinary composer input.
    page.locator('#sdk-file').set_input_files({'name':'droid-steer-context.txt','mimeType':'text/plain','buffer':b'isolated steer context'})
    response = steer('attachment steering ' + label)
    assert response.ok, response.text()
    expect(box).to_have_value('')
    expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(0)
    assert any('attachment steering ' + label in frame['params']['text'] and 'droid-steer-context.txt' in frame['params']['text'] for frame in calls(fixture, 'droid.add_user_message'))

    page.locator('#sdk-file').set_input_files({'name':'droid-queued-context.txt','mimeType':'text/plain','buffer':b'queued steer context'})
    send(page, 'queued attachment ' + label)
    attached = page.locator('.sdk-queued').filter(has_text='queued attachment ' + label)
    attached.get_by_role('button', name='立即引导', exact=True).click()
    expect(attached).to_have_count(0)
    expect(page.locator('.sdk-live')).to_contain_text('queued attachment ' + label)
    assert any('queued attachment ' + label in frame['params']['text'] and 'droid-queued-context.txt' in frame['params']['text'] for frame in calls(fixture, 'droid.add_user_message'))

    # Native acknowledgement is distinct from consumption; show one body and a receipt.
    consume_gate = fixture.root / 'steer-consume-delay'; consume_gate.touch()
    acknowledged = steer('ack pending ' + label)
    assert acknowledged.ok, acknowledged.text()
    expect(box).to_have_value('')
    wait_snapshot(page, uid, lambda raw: any(item.get('status') == 'native_queued' and item.get('text') == 'ack pending ' + label for item in raw.get('steers', [])))
    expect(page.locator('.native-sdk-pane').get_by_text('ack pending ' + label, exact=True)).to_have_count(1)
    expect(page.locator('.sdk-queue .native-message-state').filter(has_text='原生已接收')).to_be_visible()
    assert page.evaluate('NativeSdkView.running')
    capture(page, shots, label + '-ack-pending-wide-light')
    consume_gate.unlink()
    wait_snapshot(page, uid, lambda raw: any(item.get('native_echo') and item.get('text') == 'ack pending ' + label for item in raw['items']) and not any(item.get('text') == 'ack pending ' + label for item in raw.get('steers', [])))
    expect(page.locator('.native-sdk-pane').get_by_text('ack pending ' + label, exact=True)).to_have_count(1)

    rejected = steer('reject steer')
    expect(box).to_have_value('reject steer')
    failure = page.locator('.sdk-queue [data-queue-origin="bridge"]').filter(has_text='reject steer')
    expect(failure).to_contain_text('Synthetic steer rejected')
    count = len(sent('reject steer'))
    steer('reject steer')
    assert len(sent('reject steer')) == count == 1
    failure.get_by_role('button', name='撤销', exact=True).click()
    expect(failure).to_have_count(0)
    expect(page.locator('.native-sdk-pane')).not_to_contain_text('reject steer')
    assert page.evaluate('NativeSdkView.running')

    gate.touch(); box.fill('delayed steer ' + label)
    with page.expect_request(lambda request: request.url.endswith('/api/native-sdk/steer')):
        box.press('Control+Enter')
    pending = page.locator('.sdk-live .msg[data-role="user"]').filter(has_text='delayed steer ' + label)
    expect(pending).to_be_visible()
    expect(pending).to_have_count(1)
    expect(page.locator('.native-sdk-pane').get_by_text('delayed steer ' + label, exact=True)).to_have_count(1)
    expect(page.locator('.sdk-queue .native-message-state').filter(has_text='正在发送')).to_be_visible()
    for width, theme in ((1280,'light'), (390,'dark')):
        page.set_viewport_size({'width':width, 'height':900})
        page.emulate_media(color_scheme=theme)
        expect(pending).to_be_visible()
        capture(page, shots, f'{label}-pending-{width}-{theme}')
    box.fill('newer draft ' + label)
    gate.unlink()
    expect(button).to_be_enabled()
    expect(box).to_have_value('newer draft ' + label)
    wait_snapshot(page, uid, lambda raw: any(item.get('native_echo') and item.get('text') == 'delayed steer ' + label for item in raw['items']))
    expect(page.locator('.native-sdk-pane').get_by_text('delayed steer ' + label, exact=True)).to_have_count(1)
    page.reload(wait_until='networkidle')
    open_sdk(page, fixture, expect, uid=uid)
    expect(box).to_have_value('newer draft ' + label)
    assert len(sent('delayed steer ' + label)) == 1
    expect(page.locator('.native-sdk-pane').get_by_text('direct steering ' + label, exact=True)).to_have_count(1)
    assert page.evaluate('NativeSdkView.running')
    expect(fifo).to_be_visible()
    assert not sent('ordinary FIFO ' + label)
    fifo.get_by_role('button', name='撤销', exact=True).click()
    expect(fifo).to_have_count(0)
    for frame in calls(fixture, 'droid.add_user_message'):
        assert frame['params']['queuePlacement'] == 'end_of_turn', frame
    assert not calls(fixture, 'droid.interrupt_session')
    page.locator('#sdk-stop').click()
    wait_snapshot(page, uid, lambda raw: not raw['running'])

    # Transport loss before native acknowledgement is retained, never replayed on refresh.
    send(page, 'hold'); wait_snapshot(page, uid, lambda raw: raw['running'])
    uncertain = steer('unknown steer')
    expect(box).to_have_value('unknown steer')
    unknown = page.locator('.sdk-queue [data-queue-origin="bridge"]').filter(has_text='unknown steer')
    expect(unknown).to_be_visible()
    wait_snapshot(page, uid, lambda raw: any(item.get('state') == 'unknown' for item in raw.get('input_queue', []) + raw.get('steers', [])))
    page.request.post(uncertain.url, data=uncertain.request.post_data_json)
    assert len(sent('unknown steer')) == 1
    delayed_terminal = []
    def release_terminal_after_restore(route):
        page.wait_for_function("typeof S !== 'undefined' && !!S.sel && !!document.querySelector('.dhead h2')")
        assert page.evaluate('typeof takenOver') == 'undefined'
        delayed_terminal.append(True)
        route.continue_()
    page.route('**/term.js?*', release_terminal_after_restore)
    page.reload(wait_until='networkidle')
    page.unroute('**/term.js?*', release_terminal_after_restore)
    assert delayed_terminal, 'terminal script delay did not exercise the restored page'
    expect(page.locator('.sdk-queue [data-queue-origin="bridge"]').filter(has_text='unknown steer')).to_be_visible()
    expect(box).to_have_value('unknown steer')
    assert len(sent('unknown steer')) == 1
    capture(page, shots, label + '-unknown')
    assert not errors, errors


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-droid-steer-browser'))
    args = parser.parse_args()
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        try:
            for hub_mode in (False, True):
                with tempfile.TemporaryDirectory(prefix='sessiondock-droid-steer-') as directory, ExitStack() as stack:
                    root = Path(directory); fixture = DroidFixture(root); port = free_port()
                    base = stack.enter_context(fixture.server(BINARY, node_env(root, port, '127.0.0.0/8') if hub_mode else None))
                    if hub_mode:
                        hub_root = root / 'hub'; hub_root.mkdir()
                        hub = Hub(BINARY.with_name('sessiondock-hub'), hub_root, [SimpleNamespace(name='DroidSteerNode', token=TOKEN, port=port)])
                        hub.start(); stack.callback(hub.stop); base = f'http://127.0.0.1:{hub.port}'
                    context = browser.new_context(viewport={'width':1280, 'height':900}, color_scheme='light', service_workers='block')
                    try:
                        label = 'hub' if hub_mode else 'node'; shots = args.screenshots / label; shots.mkdir(parents=True, exist_ok=True)
                        check(context.new_page(), base, fixture, shots, label)
                    finally: context.close()
        finally: browser.close()
    print('Droid steer Chromium Node + Hub: PASS')


if __name__ == '__main__': main()
