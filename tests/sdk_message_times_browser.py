#!/usr/bin/env python3
"""Backend message receipts, late Hub attachment and offline sdkhost output in Chromium."""
import argparse
import asyncio
from datetime import datetime
from pathlib import Path
import re
import struct
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_mode_fixtures import provider
from sdk_ui_fixtures import start_sdk
from sdkhost_fixtures import stop_private_sdkhosts
from native_sdk_queue_fixtures import NativeQueueFixture
from native_sdk_queue_browser import open_page, send, gone


def open_view(browser, base, source, narrow=False, sid=None):
    context = browser.new_context(viewport={'width': 390 if narrow else 1280, 'height': 900},
        color_scheme='dark' if narrow else 'light', timezone_id='Asia/Shanghai', service_workers='block')
    page = context.new_page()
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    if sid:
        page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=sid)
        uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', sid)
        row = page.locator(f'#side .item[data-uid="{uid}"]')
    else:
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
    if not row.is_visible(): page.locator(f'#chips button[data-source="{source}"]').click()
    row.click()
    page.locator('#a-sdk').click()
    start_sdk(page)
    return context, page


def raw_items(page, source):
    return page.evaluate('[...CodexSdkView.items.values()]' if source == 'codex' else 'NativeSdkView.raw.items')


def instant(value):
    return datetime.fromisoformat(value.replace('Z', '+00:00')).timestamp()


def receipts(root):
    values = set()
    for path in root.rglob('stdout.time'):
        data = path.read_bytes()
        assert len(data) % 16 == 0
        values.update(ms for _, ms in struct.iter_unpack('<QQ', data))
    return values


def exercise(browser, base, hub_base, source, peer, root, screenshots):
    context, page = open_view(browser, base, source)
    try:
        sid = page.evaluate('S.sessions.find(row => row.uid === S.sel).sid')
        before = {row['id'] for row in raw_items(page, source)}
        sent_at = time.time()
        text = f'Check new message time {source}'
        page.locator('#sdk-input').fill(text)
        page.locator('#sdk-send').click()
        expect(page.locator('.sdk-live')).to_contain_text(text)
        if source == 'codex':
            # Genuine app-server packets pass through the backend reducer;
            # no HTTP/SSE projection or timestamps are supplied by the page.
            asyncio.run_coroutine_threadsafe(peer._agent(peer.turn, 'time-answer',
                'New streamed reply with no native timestamp', 'commentary'), peer.loop).result(10)
        page.wait_for_function('''([source, before]) => {
            const items = source === 'codex' ? [...CodexSdkView.items.values()] : NativeSdkView.raw.items;
            return items.some(m => !before.includes(m.id) && (m.role === 'assistant' || m.type === 'agentMessage'));
        }''', arg=[source, list(before)])
        page.wait_for_timeout(500)
        fresh = [row for row in raw_items(page, source) if row['id'] not in before]
        assert fresh
        for row in fresh:
            assert row.get('ts') or row.get('timestamp') or row.get('received_at'), (source, row)
            if row.get('received_at'):
                assert sent_at - 1 <= instant(row['received_at']) <= time.time(), row
            card = page.locator(f'.sdk-live [data-sdk-item="{row["id"]}"]')
            if card.count():
                expect(card.locator('time')).to_have_attribute('datetime', re.compile(r'^\d{4}-'))
        if source in ('claude', 'grok', 'kiro', 'codebuddy', 'agy'):
            recorded = receipts(root)
            assert recorded, 'new sdkhost did not persist output receipt times'
            for row in fresh:
                if row.get('role') in ('assistant', 'thinking', 'tool') and row.get('received_at'):
                    assert round(instant(row['received_at']) * 1000) in recorded, (source, row)
        expect(page.locator('.sdk-pane')).not_to_contain_text('时间未提供')
        page.screenshot(path=str(screenshots / f'{source}-node-new.png'), full_page=True)
        original = {row['id']: row['received_at'] for row in fresh if row.get('received_at')}
        assert original
        # A late viewer must receive the backend instant, not its own load time.
        page.wait_for_timeout(3500)
        second, hub_page = open_view(browser, hub_base, source, narrow=True, sid=sid)
        try:
            for row in raw_items(hub_page, source):
                if row['id'] in original:
                    assert row.get('received_at') == original[row['id']], row
            assert set(original) <= {row['id'] for row in raw_items(hub_page, source)}
            hub_page.reload(wait_until='domcontentloaded')
            hub_page.wait_for_function('SdkView.connected')
            for row in raw_items(hub_page, source):
                if row['id'] in original: assert row['received_at'] == original[row['id']]
            expect(hub_page.locator('.sdk-pane')).not_to_contain_text('时间未提供')
            hub_page.screenshot(path=str(screenshots / f'{source}-hub-reloaded.png'), full_page=True)
        finally:
            second.close()
        print(f'PASS {source}: actual send/stream, backend receipt, late Hub attach and reload', flush=True)
    finally:
        context.close()


def offline_output(browser, root, binary, screenshots):
    fixture = NativeQueueFixture(root, 'agy', binary, REPO / 'target/debug/ptyhost')
    try:
        with fixture.server() as base:
            context, page, _ = open_page(browser, base, fixture)
            first = send(page, 'hold:offline-times')
            gone(page, first['request_id'])
            expect(page.locator('.sdk-live')).to_contain_text('Agy running offline-times')
            pid = fixture.native_pid()
            initial = next(row['received_at'] for row in raw_items(page, 'agy') if row.get('role') == 'assistant')
            context.close()
        assert fixture.alive(pid)
        fixture.peer.release('offline-times')
        fixture.wait_native_text('Agy running offline-times finished offline-times')
        # Let the independent host drain the new frame while the node is stopped.
        deadline = time.monotonic() + 5
        while not any(b'New message produced while node is offline' in path.read_bytes()
                      for path in (root / 'state/sdkhost').glob('*/stdout')):
            assert time.monotonic() < deadline
            time.sleep(.02)
        offline_end = time.time()
        with fixture.server() as base:
            context, page, _ = open_page(browser, base, fixture)
            try:
                expect(page.locator('.sdk-live')).to_contain_text('New message produced while node is offline')
                items = raw_items(page, 'agy')
                running = next(row for row in items if str(row.get('content', '')).startswith('Agy running offline-times'))
                new = next(row for row in items if row.get('content') == 'New message produced while node is offline')
                assert running['received_at'] == initial
                assert instant(new['received_at']) <= offline_end
                assert round(instant(new['received_at']) * 1000) in receipts(root)
                card = page.locator(f'.sdk-live [data-sdk-item="{new["id"]}"]')
                expect(card.locator('time')).to_have_attribute('datetime', new['received_at'])
                assert fixture.native_pid() == pid and fixture.inputs('hold:offline-times') == ['hold:offline-times']
                page.screenshot(path=str(screenshots / 'agy-offline-recovered.png'), full_page=True)
            finally:
                context.close()
        print('PASS sdkhost: new message produced while node offline retains original receipt after restart', flush=True)
    finally:
        fixture.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='codex,claude,grok,agy,opencode,kiro,codebuddy')
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-sdk-times'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-message-times-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(tmp) / source; root.mkdir()
                try:
                    with provider(source, root, args.binary) as (base, peer, extra):
                        hub_root = root / 'hub'; hub_root.mkdir()
                        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                            [SimpleNamespace(name='TimesNode', token=TOKEN,
                                port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                        hub.start()
                        try: exercise(browser, base, f'http://127.0.0.1:{hub.port}', source, peer, root, args.screenshots)
                        finally: hub.stop()
                finally:
                    if source in ('grok', 'agy'):
                        stop_private_sdkhosts(root)
            if 'agy' in args.sources.split(','):
                offline_output(browser, Path(tmp) / 'offline', args.binary, args.screenshots)
        finally:
            browser.close()


if __name__ == '__main__': main()
