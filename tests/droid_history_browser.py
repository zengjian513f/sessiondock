#!/usr/bin/env python3
"""Droid internal records must not flash as messages before SDK replay connects."""
import argparse
import json
import tempfile
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import quote, urlencode

from playwright.sync_api import expect, sync_playwright
from claude_sdk_browser import open_sdk
from droid_sdk_fixtures import DroidFixture
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN, free_port, node_env


def reminder(suffix):
    return {'type': 'message', 'id': '__internal__system-reminder__' + suffix,
            'timestamp': '2026-11-09T01:00:01Z',
            'message': {'role': 'user', 'visibility': 'llm_only',
                        'content': [{'type': 'text', 'text':
                                     '<system-reminder>InternalReminderOnlyNeedle</system-reminder>'}]}}


def seed(f):
    rows = list(map(json.loads, f.path.read_text().splitlines()))
    rows[-1]['message']['openaiPhase'] = 'final_answer'
    # Unknown records still have a visible fallback, before the replay boundary.
    rows.insert(1, {'type': 'future_native_record', 'text': 'UnknownRecordPreserved'})
    rows.insert(3, {'type': 'todo_state', 'id': 'todo-1',
                    'todos': {'todos': [{'id': '1', 'status': 'completed',
                                         'content': 'BookkeepingOnlyNeedle'}]}, 'messageIndex': 1})
    rows.append({'type': 'agent_turn_outcome', 'turnId': 'turn-1',
                 'reason': 'completed', 'resultKind': 'text'})
    rows.append(reminder('seed'))
    for mid, visibility, text in [
        ('context', 'llm_only', 'OrdinaryContextPreserved'),
        ('literal', 'visible', 'Quoted <system-reminder>LiteralReminderPreserved</system-reminder>'),
        ('__internal__system-reminder__literal', 'visible', 'VisibleIdentityPreserved'),
    ]:
        rows.append({'type': 'message', 'id': mid, 'timestamp': '2026-11-09T01:00:01Z',
                     'message': {'role': 'user', 'visibility': visibility,
                                 'content': [{'type': 'text', 'text': text}]}})
    f.path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
    (f.root / 'replay-history').touch()


def check(browser, base, f, label, shots):
    page = browser.new_page(viewport={'width': 1280, 'height': 900}, color_scheme='light')
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    card = page.locator('#side .item[data-uid^="droid:"]').first
    expect(card).to_be_visible()
    uid = card.get_attribute('data-uid')
    card.click()
    pane = page.locator('.native-sdk-pane')
    expect(pane).to_contain_text('earlier Droid answer')
    page.locator('#a-turns').click()

    def assert_history():
        expect(pane).not_to_contain_text('agent_turn_outcome')
        expect(pane).not_to_contain_text('todo_state')
        expect(pane).not_to_contain_text('InternalReminderOnlyNeedle')
        for text in ('OrdinaryContextPreserved', 'LiteralReminderPreserved', 'VisibleIdentityPreserved'):
            expect(pane).to_contain_text(text)
        expect(pane).to_contain_text('UnknownRecordPreserved')
        expect(pane.get_by_text('earlier Droid answer', exact=True)).to_have_count(1)
        expect(pane.get_by_text('earlier Droid answer', exact=True)).to_be_visible()

    assert_history()
    endpoint = '/api/messages/' + quote(uid, safe='')
    history = page.request.get(base + endpoint).json()
    assert [m['role'] for m in history['messages']] == ['system', 'user', 'assistant', 'system', 'user', 'user'], history
    # Internal-only appends advance the physical cursor without new content.
    with f.path.open('a') as stream:
        stream.write(json.dumps({'type': 'agent_turn_outcome', 'turnId': 'turn-2',
                                 'reason': 'cancelled', 'resultKind': 'text'}) + '\n')
        stream.write(json.dumps(reminder('append')) + '\n')
    query = urlencode({'start': history['end'], 'head': history['version']['head'],
                       'anchor': history['anchor'], 'window': 1})
    delta = page.request.get(base + endpoint + '?' + query).json()
    assert not delta['reset'] and not delta['messages'] and delta['end'] > history['end'], delta
    hits = page.request.get(base + '/api/search?q=BookkeepingOnlyNeedle').json()
    assert not hits['results'], hits
    hits = page.request.get(base + '/api/search?q=InternalReminderOnlyNeedle').json()
    assert not hits['results'], hits
    open_sdk(page, f, expect, uid=uid)
    page.wait_for_function('NativeSdkView.items.some(i => i.role === "assistant")')
    assert not page.evaluate('NativeSdkView.raw.items.some(i => (i.text || "").includes("InternalReminderOnlyNeedle"))')
    assert_history()

    # Hold the transport on both cold reload and warm cached re-open. History
    # must remain useful during connection, rather than hiding the entire pane.
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        page.set_viewport_size({'width': width, 'height': 900 if width == 1280 else 844})
        page.emulate_media(color_scheme=theme)
        held = []
        page.route('**/api/native-sdk/events?*', lambda route: held.append(route))
        for mode in ('reload', 'cached'):
            if mode == 'reload':
                page.reload(wait_until='domcontentloaded')
            else:
                if width == 390:
                    page.locator('#detail .mobile-back').click()
                page.locator(f'#side .item[data-uid="{uid}"]').click()
            page.wait_for_function('NativeSdkView.historyData?.messages?.length > 0 && !NativeSdkView.connected')
            expect(pane).to_contain_text('earlier Droid answer')
            page.wait_for_timeout(150)
            assert held, 'SDK transport was not held'
            assert_history()
            page.screenshot(path=str(shots / f'{label}-{width}-{mode}-connecting.png'), full_page=True)
            for route in held:
                route.continue_()
            held.clear()
            page.wait_for_function('NativeSdkView.connected && NativeSdkView.items.some(i => i.role === "assistant")')
            assert_history()
            page.screenshot(path=str(shots / f'{label}-{width}-{mode}-connected.png'), full_page=True)
        page.unroute('**/api/native-sdk/events?*')
    assert not errors, errors
    page.close()
    print(f'PASS {label}: history/replay, cached re-open, internal reminder append/search, literal/context preservation, wide/light and narrow/dark', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--screenshots', type=Path, default=Path(tempfile.gettempdir()) / 'sd-droid-history')
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-droid-history-') as root, sync_playwright() as pw:
        f = DroidFixture(root)
        seed(f)
        port = free_port()
        with f.server(BINARY, node_env(Path(root), port, '127.0.0.0/8')) as base:
            browser = pw.chromium.launch(headless=True)
            check(browser, base, f, 'node', args.screenshots)
            hubroot = Path(root) / 'hub'
            hubroot.mkdir()
            hub = Hub(BINARY.with_name('sessiondock-hub'), hubroot,
                      [SimpleNamespace(name='DroidNode', token=TOKEN, port=port)])
            hub.start()
            try:
                check(browser, f'http://127.0.0.1:{hub.port}', f, 'hub', args.screenshots)
            finally:
                hub.stop()
                browser.close()


if __name__ == '__main__':
    main()
