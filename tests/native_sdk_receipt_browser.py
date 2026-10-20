#!/usr/bin/env python3
"""Native SDK submissions stay user bubbles through queue and delivery states."""
import argparse
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from native_sdk_footer_browser import provider
from sdk_selection_fixtures import snapshots

TEXT = '全面审计sessiondock的代码，找出所有的dead code，重复逻辑，还有性能优化点，等等。可开多个subagent去做。'


def exercise(browser, base, source, events, width, theme, screenshots):
    context = browser.new_context(viewport={'width': width, 'height': 846 if width == 675 else 900},
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
        raw['input_queue'] = [{'id': 'bridge:sent', 'state': 'preparing', 'text': data['text']}]
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
    for state, label in (('preparing', '准备输入'), ('queued', '排队中'), ('sending', '正在发送…')):
        raw['input_queue'][0]['state'] = state
        publish()
        if screenshots: page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}-{state}.png'), full_page=True)
        expect(receipt).to_have_attribute('data-role', 'user')
        expect(receipt.locator('.mb')).to_have_text(TEXT)
        expect(receipt.locator('.queued-send-state')).to_have_text(label)
        expect(receipt.locator('.sdk-request-title')).to_have_count(0)
        expect(page.locator('.sdk-msgs').get_by_text(TEXT, exact=True)).to_have_count(1)
        expect(receipt.get_by_role('button', name='撤销', exact=True)).to_have_count(1 if state == 'queued' else 0)
    raw['items'] = [{'id': 'sent', 'role': 'user', 'text': TEXT}]
    raw['input_queue'][0]['state'] = 'accepted'
    publish()
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

    # Lost result receipts are not a known send failure. In particular, the
    # report's legacy Agy input already has native consumption evidence.
    retried = []
    def retry(route):
        retried.append(route.request.post_data_json)
        raw['input_queue'][0]['state'] = 'queued'
        route.fulfill(json={'ok': True})
    context.route('**/api/native-sdk/queue_retry', retry)
    for consumed in ((False,) if source == 'grok' else (False, True)):
        raw['running'] = False
        raw['input_queue'] = [{'id': 'bridge:sent', 'state': 'unknown', 'text': TEXT,
            'consumption': 'observed' if consumed else 'unverified',
            'observed_consumed': consumed,
            'error': '服务已重启，原生投递结果未确认；不会自动重发输入。'}]
        publish()
        expect(receipt).to_contain_text('处理结果待核对')
        reason = '原生已读取这条输入，但执行结果尚未确认。' if consumed else '尚未确认原生是否收到或处理了这条输入。'
        expect(receipt).to_contain_text(reason)
        expect(receipt.get_by_role('button', name='重试', exact=True)).to_have_count(0)
        resend = receipt.get_by_role('button', name='重新发送', exact=True)
        before = len(retried)
        if screenshots: page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}-unknown-{consumed}.png'), full_page=True)
        resend.click()
        dialog = page.get_by_role('dialog')
        expect(dialog).to_have_accessible_name('重新发送这条输入？')
        expect(dialog).to_contain_text(reason)
        expect(dialog).to_contain_text('再次提交整条输入，可能重复执行')
        assert len(retried) == before
        if screenshots: page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}-resend-{consumed}.png'), full_page=True)
        dialog.get_by_role('button', name='取消', exact=True).click()
        expect(dialog).to_have_count(0)
        assert len(retried) == before
        resend.click()
        with page.expect_response('**/api/native-sdk/queue_retry'):
            dialog.get_by_role('button', name='重新发送', exact=True).click()
        assert len(retried) == before + 1 and retried[-1]['confirmed'] is True
        assert retried[-1]['id'] == 'bridge:sent' and retried[-1]['request_id']
        publish()
        expect(receipt).to_contain_text('排队中')
        with page.expect_response('**/api/native-sdk/queue_cancel'):
            receipt.get_by_role('button', name='撤销', exact=True).click()
        publish()
        expect(receipt).to_have_count(0)
        expect(page.locator('#sdk-input')).to_have_value('后续草稿\n'+TEXT)
    if source == 'opencode':
        raw['capabilities'].update(queue_cancel=True, queue_update=True)
        def inbox(route):
            raw['queue'] = [{'id': 'inbox:sent', 'text': route.request.post_data_json['text']}]
            route.fulfill(json={'ok': True, 'accepted': True})
        def steer(route):
            assert route.request.post_data_json['id'] == 'inbox:sent'
            raw['queue'][0]['delivery'] = 'steer'
            route.fulfill(json={'ok': True})
        def cancel_inbox(route):
            assert route.request.post_data_json['id'] == 'inbox:sent'
            raw['queue'] = []
            route.fulfill(json={'ok': True})
        context.route('**/api/native-sdk/turn', inbox)
        context.route('**/api/native-sdk/queue_update', steer)
        context.route('**/api/native-sdk/queue_cancel', cancel_inbox)
        page.locator('#sdk-input').fill(TEXT)
        with page.expect_response('**/api/native-sdk/turn'): page.locator('#sdk-send').click()
        publish()
        native = page.locator('.sdk-queue [data-queue-id="inbox:sent"]')
        expect(native).to_have_attribute('data-role', 'user')
        expect(native.locator('.mb')).to_have_text(TEXT)
        if screenshots: page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}-inbox.png'), full_page=True)
        with page.expect_response('**/api/native-sdk/queue_update'):
            native.get_by_role('button', name='立即引导', exact=True).click()
        publish()
        expect(native.locator('.queued-send-state')).to_have_text('等待引导')
        expect(native.get_by_role('button', name='立即引导', exact=True)).to_have_count(0)
        with page.expect_response('**/api/native-sdk/queue_cancel'):
            native.get_by_role('button', name='撤销', exact=True).click()
        publish()
        expect(native).to_have_count(0)
    assert not errors, errors
    context.close()
    print(f'PASS {source} {width} {theme}: user submission states, acceptance, failure recovery, explicit resend/cancel and draft preserved', flush=True)


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
                    for width, theme in ((1440, 'dark'), (390, 'light'), (675, 'light')):
                        exercise(browser, base, source, events, width, theme, args.screenshots)
        finally: browser.close()


if __name__ == '__main__': main()
