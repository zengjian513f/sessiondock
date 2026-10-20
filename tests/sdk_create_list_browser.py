#!/usr/bin/env python3
"""SDK creation stays in the sidebar before first input: five CLIs, node and Hub."""
import argparse
from datetime import datetime
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub
from new_session_model_browser import open_dialog
from node_auth_fixtures import TOKEN
from sdk_start_stop_fixtures import provider


def exercise(browser, base, source, peer, root, screenshots, label):
    context = browser.new_context(viewport={'width': 1440, 'height': 900},
                                  service_workers='block')
    page = context.new_page()
    page.set_default_timeout(20000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    page.wait_for_function('T.listLoaded')
    cwd = getattr(peer, 'work', getattr(peer, 'cwd', root))
    if source == 'agy':
        (root/'empty-time').touch()
    open_dialog(page)
    page.locator(f'input[name=new-source][value={source}]').check()
    page.locator('#new-cwd').fill(str(cwd))
    endpoint = 'codex-sdk' if source == 'codex' else 'native-sdk'
    with page.expect_response(lambda r: f'/api/{endpoint}/create' in r.url) as event:
        page.locator('#new-session-go').click()
    assert event.value.ok, event.value.text()
    receipt = event.value.json()
    uid = receipt['uid']
    expect(page.locator('#sdk-input')).to_be_visible()
    page.screenshot(path=str(screenshots/f'{label}-{source}-created.png'), full_page=True)
    for field in ('created', 'updated'):
        assert datetime.fromisoformat(receipt['session'][field]).year > 1, receipt
    replay = page.request.post(base+f'/api/{endpoint}/create',
                               data=event.value.request.post_data_json)
    assert replay.ok, replay.text()
    assert replay.json()['uid'] == uid
    assert replay.json()['session']['created'] == receipt['session']['created']
    row = page.locator(f'#side .item[data-uid="{uid}"]')
    expect(row).to_be_visible()
    # Pending receipts survive a reload without changing UID or their timestamp.
    page.reload(wait_until='networkidle')
    page.wait_for_function('T.listLoaded')
    expect(row).to_be_visible()
    row.click()
    expect(page.locator('#sdk-input')).to_be_visible()
    if source == 'agy':
        page.wait_for_function('uid => S.sessions.some(s => s.uid === uid)', arg=uid)
        expect(page.locator('#detail')).not_to_contain_text('仅找到 Agy 的会话索引')
        projected = page.evaluate('uid => S.sessions.find(s => s.uid === uid)', uid)
        assert datetime.fromisoformat(projected['updated']).year > 1, projected
        assert page.locator('#side .item[data-uid^="agy:"]').first.get_attribute('data-uid') == uid
        page.locator('#a-chat').click()
        expect(page.locator('#detail')).not_to_contain_text('仅找到 Agy 的会话索引')
        page.screenshot(path=str(screenshots/f'{label}-agy-empty-chat.png'), full_page=True)
        page.locator('#a-sdk').click()
        # Opening an empty native DB can change its mtime. Re-exporting its
        # zero-time catalog entry must preserve the first observed timestamp.
        native = peer.native/'conversations'/f'{receipt["session_id"]}.db'
        native.touch()
        with sqlite3.connect(peer.native/'conversation_summaries.db') as db:
            db.execute('UPDATE conversation_summaries SET title=? WHERE conversation_id=?',
                       ('Empty Agy retained', receipt['session_id']))
        page.reload(wait_until='networkidle')
        page.wait_for_function('uid => S.sessions.find(s => s.uid === uid)?.title === "Empty Agy retained"', arg=uid)
        assert page.evaluate('uid => S.sessions.find(s => s.uid === uid).updated', uid) == projected['updated']
        expect(row).to_be_visible()
        row.click()
    for width, theme in [(1440, 'light'), (390, 'dark')]:
        page.set_viewport_size({'width': width, 'height': 900})
        page.emulate_media(color_scheme=theme)
        if page.locator('#detail .mobile-back').is_visible():
            page.locator('#detail .mobile-back').click()
        expect(row).to_be_visible()
        page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-{theme}.png'), full_page=True)
        row.click()
        expect(page.locator('#sdk-input')).to_be_visible()
    page.set_viewport_size({'width': 1440, 'height': 900})
    text = f'first input {source} {label}'
    page.locator('#sdk-input').fill(text)
    expect(page.locator('#sdk-send')).to_be_enabled()
    with page.expect_response(lambda r: f'/api/{endpoint}/' in r.url
                              and r.request.method == 'POST' and
                              r.url.split('?')[0].endswith(('/turn', '/send'))) as sent:
        page.locator('#sdk-send').click()
    assert sent.value.ok, sent.value.text()
    expect(page.locator('#sdk-input')).to_have_value('')
    assert page.evaluate('S.sel') == uid
    expect(row).to_be_visible()
    assert not errors, errors
    print(f'PASS {label} {source}: create, sidebar, reload, first send', flush=True)
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='agy,claude,grok,opencode,codex')
    parser.add_argument('--screenshots', type=Path,
                        default=Path(tempfile.gettempdir())/'sessiondock-sdk-create-list')
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-create-list-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(tmp)/source
                root.mkdir()
                with provider(source, root, args.binary) as (base, peer, extra):
                    exercise(browser, base, source, peer, root, args.screenshots, 'node')
                    hub_root = root/'hub'
                    hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                              [SimpleNamespace(name='SdkNode', token=TOKEN,
                               port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', source, peer,
                                 root, args.screenshots, 'hub')
                    finally:
                        hub.stop()
        finally:
            browser.close()


if __name__ == '__main__':
    main()
