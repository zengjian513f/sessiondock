#!/usr/bin/env python3
"""Native SDK accepted receipts stay compact without duplicating sent input."""
import argparse
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from native_sdk_footer_browser import provider
from sdk_selection_fixtures import snapshots

TEXT = '全面审计sessiondock的代码，找出所有的dead code，重复逻辑，还有性能优化点，等等。可开多个subagent去做。'


def exercise(browser, base, source, events, width, theme, screenshots):
    context = browser.new_context(viewport={'width': width, 'height': 900},
        color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base+'/') else route.abort())
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    raw = {'running': True, 'items': [], 'input_queue': [], 'capabilities': {}}
    events.packet = {'type': 'snapshot', 'seq': 1, 'raw': raw}
    context.route('**/api/native-sdk/open', lambda route: route.fulfill(json={'ok': True}))
    context.route('**/api/native-sdk/events?*', lambda route: route.continue_(url=events.url))
    context.route('**/api/native-sdk/usage', lambda route: route.fulfill(json={'ok': True}))
    context.route('**/api/native-sdk/models', lambda route: route.fulfill(json={'ok': True, 'raw': []}))
    def publish():
        events.publish()
        page.wait_for_function('seq=>NativeSdkView.seq===seq', arg=events.packet['seq'])
    def submit(route):
        data = route.request.post_data_json
        raw['items'] = [{'id': 'sent', 'role': 'user', 'text': data['text']}]
        raw['input_queue'] = [{'id': 'bridge:sent', 'state': 'accepted', 'text': data['text']}]
        route.fulfill(json={'ok': True, 'accepted': True})
    context.route('**/api/native-sdk/turn', submit)
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    row = page.locator(f'#side .item[data-uid^="{source}:"]').first
    if not row.is_visible(): page.locator(f'#chips button[data-source="{source}"]').click()
    row.click()
    page.locator('#a-sdk').click()
    power = page.locator('#a-session-toggle')
    if not power.is_visible(): page.locator('#a-more').click()
    power.click()
    expect(page.locator('#sdk-input')).to_be_visible()
    expect(page.locator('#sdk-input')).to_be_enabled()
    page.locator('#sdk-input').fill(TEXT)
    with page.expect_response('**/api/native-sdk/turn'): page.locator('#sdk-send').click()
    publish()
    receipt = page.locator('.sdk-queue [data-queue-id="bridge:sent"]')
    expect(receipt).to_have_text('原生已接收')
    expect(page.locator('.sdk-msgs').get_by_text(TEXT, exact=True)).to_have_count(1)
    expect(page.locator('.sdk-queue .msg')).to_have_count(0)
    if screenshots: page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}-accepted.png'), full_page=True)
    # A later failure must still expose recovery actions and preserve editor text.
    raw['input_queue'][0].update(state='failed', error='Synthetic delivery failed')
    publish()
    page.locator('#sdk-input').fill('后续草稿')
    receipt.get_by_role('button', name='恢复文字', exact=True).click()
    expect(page.locator('#sdk-input')).to_have_value('后续草稿\n'+TEXT)
    expect(receipt.get_by_role('button', name='重试', exact=True)).to_be_visible()
    def cancel(route):
        raw['input_queue'] = []
        route.fulfill(json={'ok': True})
    context.route('**/api/native-sdk/queue_cancel', cancel)
    with page.expect_response('**/api/native-sdk/queue_cancel'):
        receipt.get_by_role('button', name='撤销', exact=True).click()
    publish()
    expect(receipt).to_have_count(0)
    expect(page.locator('#sdk-input')).to_have_value('后续草稿\n'+TEXT)
    assert not errors, errors
    context.close()
    print(f'PASS {source} {width} {theme}: send, compact receipt, failure recovery and cancel', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-native-receipts-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in ('agy', 'grok', 'claude', 'opencode'):
                root = Path(temporary)/source; root.mkdir()
                with provider(source, root, args.binary) as base, snapshots() as events:
                    for width, theme in ((1440, 'dark'), (390, 'light')):
                        exercise(browser, base, source, events, width, theme, args.screenshots)
        finally: browser.close()


if __name__ == '__main__': main()
