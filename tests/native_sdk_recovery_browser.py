#!/usr/bin/env python3
"""Native SDK usage retry, stale-read isolation and pending queue actions in Chromium."""
import argparse
from pathlib import Path
import tempfile
import re
import time

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY
from native_sdk_footer_browser import native_events, provider
from sdk_ui_fixtures import start_sdk
from sdk_usage_fixtures import assert_usage_outside_menu, refresh_usage


def exercise(browser, base, source, events, screenshots):
    context = browser.new_context(viewport={'width': 1280, 'height': 900},
                                  service_workers='block', permissions=['local-network-access'])
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    state = {'failures': 1, 'total': 4000, 'billing_failures': 0, 'hold': False}
    reads, held, actions = [], [], []
    context.route('**/api/native-sdk/open', lambda r: r.fulfill(json={
        'ok': True, 'capabilities': {'models': False, 'billing': True, 'queue_cancel': True}}))
    context.route('**/api/native-sdk/events?*', lambda r: r.continue_(url=events.url))
    events.packet = {'type': 'snapshot', 'seq': 1, 'raw': {'running': True}}

    def usage(route):
        reads.append(time.monotonic())
        if state['hold']:
            held.append(route)
        elif state['failures']:
            state['failures'] -= 1
            route.fulfill(status=503, json={'ok': False, 'error': 'usage temporarily unavailable'})
        else:
            route.fulfill(json={'ok': True, 'raw': {'usage': {'tokens': {'total_tokens': state['total']}}}})

    def billing(route):
        if state['billing_failures']:
            state['billing_failures'] -= 1
            route.fulfill(status=503, json={'ok': False, 'error': 'billing temporarily unavailable'})
        else:
            route.fulfill(json={'ok': True, 'raw': {'billing': {'config': {'creditUsagePercent': 42.5}}}})

    context.route('**/api/native-sdk/usage', usage)
    context.route('**/api/native-sdk/billing', billing)
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    row = page.locator(f'#side .item[data-uid^="{source}:"]').first
    if not row.is_visible():
        page.locator(f'#chips button[data-source="{source}"]').click()
    row.click()
    if not page.locator('#sdk-input').is_visible():
        page.locator('#a-sdk').click()
    start_sdk(page)
    # First read fails. Recovery must happen without a model turn or refresh click.
    page.wait_for_function('NativeSdkView.usageError?.includes("temporarily unavailable")')
    page.wait_for_function('NativeSdkView.usageData?.usage?.tokens?.total_tokens === 4000')
    assert reads[1] - reads[0] >= .8, reads
    page.wait_for_function('!NativeSdkView.loadingUsage && !NativeSdkView.usageError')
    assert page.evaluate('NativeSdkView.usageRetryDelay') == 1000
    detail = page.locator('.native-sdk-pane .sdk-status-row > .sdk-gauges')
    expect(detail).to_have_attribute('title', re.compile('4,000'))
    expect(page.locator('.native-sdk-pane .sdk-gauges')).to_have_attribute('title', re.compile('42.5%'))
    assert_usage_outside_menu(page)

    # Repeated failures retain known values, then recover with exponential backoff.
    state.update(failures=2, total=4567)
    start = len(reads)
    refresh_usage(page)
    expect(detail).to_have_attribute('title', re.compile('temporarily unavailable'))
    expect(detail).to_have_attribute('title', re.compile('4,000'))
    expect(page.locator('.native-sdk-pane .sdk-gauges')).to_have_attribute('title', re.compile('42.5%'))
    page.screenshot(path=str(screenshots/f'{source}-usage-failed-wide-light.png'))
    page.wait_for_function('NativeSdkView.usageData?.usage?.tokens?.total_tokens === 4567')
    assert reads[start+1] - reads[start] >= .8, reads[start:]
    assert reads[start+2] - reads[start+1] >= 1.8, reads[start:]
    page.wait_for_function('!NativeSdkView.loadingUsage && !NativeSdkView.usageError')
    expect(detail).not_to_have_attribute('title', re.compile('temporarily unavailable'))
    assert page.evaluate('NativeSdkView.usageRetryDelay') == 1000

    state['billing_failures'] = 1
    refresh_usage(page)
    expect(detail).to_have_attribute('title', re.compile('billing temporarily unavailable'))
    expect(page.locator('.native-sdk-pane .sdk-gauges')).to_have_attribute('title', re.compile('42.5%'))
    page.wait_for_function('!NativeSdkView.loadingUsage && !NativeSdkView.usageError')
    expect(page.locator('.native-sdk-pane .sdk-gauges')).to_have_attribute('title', re.compile('42.5%'))
    page.set_viewport_size({'width': 390, 'height': 900})
    page.emulate_media(color_scheme='dark')
    if not page.locator('#sdk-input').is_visible():
        row.click()
        page.locator('#a-sdk').click()
    expect(page.locator('.native-sdk-pane .sdk-model')).to_be_hidden()
    expect(page.locator('.native-sdk-pane .sdk-effort')).to_be_hidden()
    expect(detail).to_be_visible()
    expect(detail).to_have_attribute('title', re.compile('4,567'))
    assert_usage_outside_menu(page)
    page.screenshot(path=str(screenshots/f'{source}-usage-recovered-mobile-dark.png'))

    # A real click starts a mutation. A subsequent SSE snapshot must not replace
    # the disabled buttons or permit a second operation while the first is pending.
    events.packet = {'type': 'snapshot', 'seq': 2, 'raw': {'running': True,
        'input_queue': [{'id': 'bridge:pending', 'state': 'queued', 'text': 'pending input'}]}}
    page.reload(wait_until='domcontentloaded')
    page.locator('#a-sdk').click()
    start_sdk(page)
    button = page.locator('[data-queue-id="bridge:pending"]').get_by_role('button', name='撤销', exact=True)
    expect(button).to_be_visible()
    context.route('**/api/native-sdk/queue_cancel', lambda route: actions.append(route))
    button.click()
    expect(button).to_be_disabled()
    # Deliver through the actual EventSource listener instead of calling the renderer.
    # native_events holds one packet per connection, so close it and let reconnect
    # attach to the updated snapshot while the HTTP action is still pending.
    events.packet = {'type': 'snapshot', 'seq': 3, 'raw': {'running': True,
        'input_queue': [{'id': 'bridge:pending', 'state': 'queued', 'text': 'updated pending input'},
                        {'id': 'bridge:next', 'state': 'queued', 'text': 'next input'}]}}
    page.evaluate('NativeSdkView.reconnect()')
    page.wait_for_function('NativeSdkView.raw?.input_queue?.length === 2')
    expect(button).to_be_disabled()
    assert len(actions) == 1
    expect(page.locator('[data-queue-id="bridge:next"]')).to_have_count(0)
    actions[0].fulfill(status=503, json={'ok': False, 'error': 'queue temporarily unavailable'})
    expect(button).to_be_enabled()
    expect(page.locator('[data-queue-id="bridge:next"]')).to_contain_text('next input')

    # Switching away cancels the pending read; its response cannot update a new view.
    state['hold'] = True
    refresh_usage(page)
    page.wait_for_function('!!NativeSdkView.usageRead')
    with page.expect_event('requestfailed', predicate=lambda r: r.url.endswith('/api/native-sdk/usage')):
        page.locator('#a-chat').click()
    assert held, 'usage request did not reach the fixture'
    assert page.evaluate('NativeSdkView.usageRead === null && NativeSdkView.usageRetryTimer === 0')
    state['hold'] = False
    state['total'] = 9999
    opens=[]
    context.route('**/api/native-sdk/open',lambda route: opens.append(route))
    page.locator('#a-sdk').click()
    expect(page.locator('.native-sdk-pane .sdk-gauges')).to_contain_text('4,567')
    expect(page.locator('.native-sdk-pane .sdk-gauges')).to_have_attribute('title',re.compile('42.5%'))
    page.locator('#sdk-input').click()
    page.wait_for_function('!!NativeSdkView.openRead')
    assert opens
    opens[0].fulfill(json={'ok':True,'capabilities':{'models':False,'billing':True}})
    page.wait_for_function('NativeSdkView.usageData?.usage?.tokens?.total_tokens === 9999')
    for route in held:
        route.fulfill(json={'ok': True, 'raw': {'usage': {'tokens': {'total_tokens': 1}}}})
    page.wait_for_function('!NativeSdkView.loadingUsage')
    assert page.evaluate('NativeSdkView.usageData.usage.tokens.total_tokens') == 9999

    # Close with a scheduled retry and prove there is no background usage request.
    state['failures'] = 10
    refresh_usage(page)
    page.wait_for_function('!!NativeSdkView.usageRetryTimer')
    page.locator('#a-chat').click()
    count = len(reads)
    page.evaluate('() => new Promise(resolve => setTimeout(resolve, 1300))')
    assert len(reads) == count, (count, len(reads))
    assert not errors, errors
    context.close()
    print(f'PASS {source}: automatic retry/backoff, retained usage/billing, abort, late read, pending queue action', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-native-recovery-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in ('grok', 'agy', 'opencode', 'claude'):
                root = Path(temporary)/source
                root.mkdir()
                with provider(source, root, args.binary) as base, native_events() as events:
                    exercise(browser, base, source, events, args.screenshots)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
