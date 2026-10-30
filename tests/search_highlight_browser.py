#!/usr/bin/env python3
"""Search annotations in ordinary and seven SDK views, using private fixtures."""
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_mode_fixtures import provider
from sdk_selection_fixtures import snapshots
from sdk_ui_fixtures import start_sdk

SOURCES = ('codex', 'claude', 'grok', 'agy', 'opencode', 'kiro', 'codebuddy')


def exercise(browser, base, source, events, width, theme, screenshots):
    context = browser.new_context(viewport={'width': width, 'height': 948},
                                  color_scheme=theme, service_workers='block')
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    history = [{'id': f'{role}-{turn}', 'role': role, 'turn_id': f'past-{turn}',
                'text': f'搜索检查 {turn}：provider 配置与 SDK 会话的显示需要保持一致。'}
               for turn in range(55) for role in ('user', 'assistant')]
    # A folded process and a long user bubble exercise disclosures separately
    # from the many short bubbles that must not consume an expansion budget.
    history.extend([
        {'role': 'user', 'turn_id': 'fold', 'text': '请检查折叠内容'},
        {'role': 'thinking', 'turn_id': 'fold', 'text': '过程中的 HiddenNeedle。'},
        {'role': 'assistant', 'turn_id': 'fold', 'text': '检查结束。', 'phase': 'final'},
        {'role': 'user', 'turn_id': 'long', 'text': '长内容说明。' * 1000 + ' TailNeedle'},
        {'role': 'assistant', 'turn_id': 'long', 'text': '不匹配的结论。', 'phase': 'final'},
    ])
    raw = {'items': [], 'requests': [], 'running': False, 'input_queue': [],
           'summary': {'settings': {}, 'queue': [], 'status': {'type': 'idle'}}}
    events.packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
    api = 'codex-sdk' if source == 'codex' else 'native-sdk'
    context.route(f'**/api/{api}/open', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
    context.route(f'**/api/{api}/events?*', lambda r: r.continue_(url=events.url))
    for endpoint in ('models', 'limits', 'usage', 'agents'):
        context.route(f'**/api/{api}/{endpoint}*', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))

    def messages(route):
        response = route.fetch()
        data = response.json()
        data.update(messages=history, reset=True, start=0, end=len(history),
                    message_total=len(history), partial=None, activity={'state': 'idle'})
        route.fulfill(response=response, json=data)
    context.route('**/api/messages/**', messages)
    context.route('**/api/watch?*', lambda r: r.fulfill(
        content_type='text/event-stream', body=': synthetic history\n\n'))
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    row = page.locator(f'#side .item[data-uid^="{source}:"]').first
    if not row.is_visible(): page.locator(f'#chips button[data-source="{source}"]').click()
    uid = row.get_attribute('data-uid')
    row.click()
    page.locator('#a-sdk').click()
    expect(page.locator('.sdk-history')).to_contain_text('搜索检查 54')
    start_sdk(page)
    expect(page.locator('#sdk-input')).to_be_enabled()
    page.keyboard.press('Escape')

    def query(text):
        if width < 600 and not page.locator('#q').is_visible():
            page.locator('.mobile-back').click()
        page.locator('#q').fill(text)
        # The provider itself stays in the sidebar even for body-only terms.
        # Enter uses a routed search result, keeping the actual user navigation.
        page.locator('#q').press('Enter')
        expect(page.locator('#side-search-label')).to_have_text('搜索结果')
        if width < 600:
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            start_sdk(page)
        expect(page.locator('.sdk-history')).to_contain_text('搜索检查 54')

    # Search routing only supplies the matching row; message rendering and the
    # worker use the real frontend, not injected application state or DOM.
    def search(route):
        response = route.fetch(url=base + '/api/sessions')
        sessions = response.json()['sessions']
        match = next(row for row in sessions if row['uid'] == uid)
        route.fulfill(json={'results': [{**match, 'hits': 110}], 'scanned': 1, 'total_pool': 1})
    context.route('**/api/search?*', search)

    def capture(label):
        if screenshots:
            page.screenshot(path=str(screenshots / f'{source}-{width}-{theme}-{label}.png'), full_page=True)

    def menu_click(selector):
        button = page.locator(selector)
        if not button.is_visible(): page.locator('#a-more').click()
        button.click()
        page.keyboard.press('Escape')

    query('sdk')
    expect(page.locator('.sdk-history mark')).to_have_count(110)
    expect(page.locator('.sdk-history .hashit')).to_have_count(0)
    page.locator('.sdk-history .msg[data-role="assistant"]').nth(54).click()
    capture('literal')
    menu_click('#m-next')
    expect(page.locator('.sdk-msgs mark.cur')).to_have_count(1)
    expect(page.locator('#msgs mark.cur')).to_have_count(0)

    # Changing options must update an already-open pane, including async regex.
    if width < 600: page.locator('.mobile-back').click()
    page.locator('#opts button[data-o="case"]').click()
    if width < 600: page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator('.sdk-history mark')).to_have_count(0)
    query('SDK')
    expect(page.locator('.sdk-history mark')).to_have_count(110)
    if width < 600: page.locator('.mobile-back').click()
    page.locator('#opts button[data-o="regex"]').click()
    if width < 600: page.locator(f'#side .item[data-uid="{uid}"]').click()
    query('S.K')
    expect(page.locator('.sdk-history mark')).to_have_count(110)
    expect(page.locator('.sdk-history .hashit')).to_have_count(0)
    capture('regex')

    # The live-only match must be highlighted, counted and navigable, even as
    # repeated snapshots replace history and streaming deltas repaint prose.
    for index in range(3):
        text = f'实时生成 {index}：SDK LiveNeedle 与 provider。'
        if source == 'codex':
            raw['items'] = [{'id': 'live-search', 'type': 'agentMessage', 'text': text}]
            raw['turn'] = {'id': 'live-turn', 'status': 'inProgress'}
        else:
            raw['items'] = [{'id': 'live-search', 'role': 'assistant', 'text': text}]
            raw['running'] = True
        events.publish()
        expect(page.locator('[data-sdk-item="live-search"]')).to_contain_text(f'实时生成 {index}')
        expect(page.locator('[data-sdk-item="live-search"] mark')).to_have_text('SDK')
        expect(page.locator('.sdk-msgs .hashit')).to_have_count(0)
    query('LiveNeedle')
    expect(page.locator('.sdk-msgs mark')).to_have_text('LiveNeedle')
    menu_click('#m-next')
    expect(page.locator('[data-sdk-item="live-search"] mark.cur')).to_have_text('LiveNeedle')
    expect(page.locator('#mcount')).to_have_text('1/1 处匹配')
    capture('live')

    query('HiddenNeedle')
    process = page.locator('.sdk-history .turn-process[data-turn-id="fold"]')
    if process.locator('.turn-process-body').is_hidden(): process.locator('.fold-toggle').first.click()
    expect(process.locator('mark')).to_have_text('HiddenNeedle')
    query('TailNeedle')
    long = page.locator('.sdk-history .msg[data-role="user"]').last
    if long.get_by_role('button', name='展开全文', exact=False).is_visible(): long.get_by_role('button', name='展开全文', exact=False).click()
    expect(long.locator('mark')).to_have_text('TailNeedle')

    if source not in ('kiro', 'codebuddy'):
        page.locator('#a-chat').click()
        ordinary_long = page.locator('#msgs .msg[data-role="user"]').last
        disclosure = ordinary_long.get_by_role('button', name='展开全文', exact=False)
        if disclosure.is_visible(): disclosure.click()
        expect(page.locator('#msgs mark')).to_have_text('TailNeedle')
        menu_click('#m-next')
        expect(page.locator('#msgs mark.cur')).to_have_text('TailNeedle')
        capture('chat')
        page.locator('#a-sdk').click()
    if width < 600: page.locator('.mobile-back').click()
    page.locator('#q').press('Escape')
    if width < 600: page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator('.sdk-msgs mark, .sdk-msgs .hashit, #msgs mark, #msgs .hashit')).to_have_count(0)
    expect(page.locator('#mcount')).to_be_hidden()
    capture('cleared')
    if source == 'codex' and width > 1000:
        # Ordinary history renders before SDK history on entry. Each must get
        # its own 40 long-message expansion slots, regardless of short hits.
        history.extend({'role': 'user', 'turn_id': f'budget-{i}',
                        'text': '预算检查。' * 900 + f' BudgetNeedle {i}'} for i in range(45))
        page.reload(wait_until='domcontentloaded')
        expect(page.locator('.sdk-history')).to_contain_text('预算检查')
        page.locator('#opts button[data-o="regex"]').click()
        query('BudgetNeedle')
        page.locator(f'#side .item[data-uid="{uid}"]').click()
        expect(page.locator('.sdk-history .hashit')).to_have_count(5)
        expect(page.locator('.sdk-history mark')).to_have_count(40)
        last = page.locator('.sdk-history .msg[data-role="user"]').last
        last.get_by_role('button', name='展开全文', exact=False).click()
        expect(last.locator('mark')).to_have_text('BudgetNeedle')
        expect(page.locator('.sdk-history .hashit')).to_have_count(4)
        capture('expansion-budget')
    assert not errors, errors
    context.unroute_all(behavior='wait')
    context.close()
    print(f'PASS {source} {width}/{theme}: history/live, options, regex, navigation, disclosures, clear', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', nargs='+', default=SOURCES, choices=SOURCES)
    parser.add_argument('--screenshots', type=Path)
    parser.add_argument('--hub', action='store_true', help='Exercise the federated Hub entry')
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-search-highlight-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources:
                root = Path(tmp)/source
                root.mkdir()
                with provider(source, root, args.binary) as (base, _, extra), snapshots() as events:
                    hub = None
                    if args.hub:
                        hub_root = root/'hub'
                        hub_root.mkdir()
                        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                                  [SimpleNamespace(name='SearchNode', token=TOKEN,
                                   port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                        hub.start()
                        base = f'http://127.0.0.1:{hub.port}'
                    try:
                        for width, theme in ((1912, 'light'), (390, 'dark')):
                            exercise(browser, base, source, events, width, theme, args.screenshots)
                    finally:
                        if hub: hub.stop()
        finally:
            browser.close()


if __name__ == '__main__':
    main()
