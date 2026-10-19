"""Conversation footer status and cached usage through real page interactions."""
import argparse
import html
import json
import re
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright, expect
from history_fixtures import Corpus, codex_row, codex_message, isolated_server


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-footer-') as temp, sync_playwright() as pw:
        corpus = Corpus(Path(temp))
        for sid in ('active', 'inactive'):
            corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/conversation-footer'}),
                codex_message('user', sid + ' 会话状态和用量显示检查 · long title'),
                codex_message('assistant', 'Existing conversation content', 2)], [])
        with isolated_server(corpus) as (base, _):
            browser = pw.chromium.launch()
            for width, theme in [(1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark'), (320, 'light'), (320, 'dark')]:
                page = browser.new_page(viewport={'width': width, 'height': 800}, color_scheme=theme)
                state = {'value': 'working'}
                errors = []
                page.on('pageerror', lambda e: errors.append(str(e)))
                uid = corpus.uid('active')
                def document(route):
                    response = route.fetch()
                    body = response.text()
                    match = re.search(r'(<meta name="sessiondock-capabilities" content=")(.*?)(">)', body)
                    config = json.loads(html.unescape(match[2]))
                    config.update(conversation_send=True, terminal=True)
                    body = body[:match.start(2)] + html.escape(json.dumps(config), quote=True) + body[match.end(2):]
                    route.fulfill(response=response, body=body)
                page.route(re.compile(re.escape(base) + r'/(?:\?.*)?$'), document)
                page.route('**/api/term/list*', lambda r: r.fulfill(json={'enabled': True,
                    'sources': {'codex': True}, 'pending': [], 'sessions': [{'name': 'footer-fixture',
                    'uid': uid, 'sid': 'active', 'source': 'codex', 'instance_id': '1' * 32}]}))
                page.route('**/api/session/conversation?*', lambda r: r.fulfill(json={'draft': {'revision': 0, 'value': None}}))
                page.route('**/api/session/conversation/check', lambda r: r.fulfill(json={'ok': True, 'input': {'state': 'ready', 'code': '', 'message': ''}, 'prompt': None}))
                page.route('**/api/live*', lambda r: r.fulfill(json={'known': True, 'uids': [uid], 'working_uids': [uid], 'tmux_uids': []}))
                page.route('**/api/quotas', lambda r: r.fulfill(json={'agents': [{'source': 'codex', 'captured_at': '2026-10-19T03:00:00Z', 'buckets': [
                    {'id':'codex:credits','group':'Codex credits','remaining_text':'1234.6'},
                    {'id':'reset-credits','group':'重置卡','remaining_text':'3 张','expires_at':'2026-11-01T00:00:00Z'},
                    {'window': 300, 'remaining_percent': 74}, {'window': 10080, 'remaining_percent': 91}]}]}))
                def history(route):
                    response = route.fetch()
                    data = response.json()
                    data['activity'] = {'state': state['value']}
                    route.fulfill(response=response, json=data)
                page.route('**/api/messages/**', history)
                for status in ('working', 'waiting', 'failed', 'idle'):
                    state['value'] = status
                    page.goto(base)
                    card = page.locator(f'#side .item[data-uid="{uid}"]')
                    expect(card).to_be_attached()
                    if width < 720 and not card.is_visible():
                        page.locator('#detail .mobile-back').click()
                    card.click()
                    footer = page.locator('#conversation-status')
                    expect(footer).to_be_visible()
                    expect(footer.locator('.sdk-status')).to_have_text({'working': 'Working…', 'waiting': '等待回答', 'failed': '执行失败', 'idle': '就绪'}[status])
                    expect(footer.locator('.sdk-gauges')).to_contain_text('26%')
                    expect(footer.locator('.sdk-gauges')).to_contain_text('9%')
                    expect(footer.locator('[data-balance="codex:credits"]')).to_have_text('🪙 1,235')
                    expect(footer.locator('[data-balance="reset-credits"]')).to_have_text('重置卡 3 张')
                    expect(footer.locator('[data-balance="reset-credits"]')).to_have_attribute('title', re.compile('最近到期'))
                    expect(page.locator('#cinput')).to_be_visible()
                    expect(page.locator('#msgs #activity')).to_have_count(0)
                    expect(page.locator('#msgs')).to_contain_text('Existing conversation content')
                    assert footer.evaluate('(e) => e.scrollWidth <= e.clientWidth + 1')
                    page.screenshot(path=str(args.screenshots / f'{width}-{theme}-{status}.png'))
                page.unroute('**/api/session/conversation/check')
                page.route('**/api/session/conversation/check', lambda r: r.fulfill(json={'ok': False, 'input': {'state': 'blocked', 'code': 'cli_input_present', 'message': '终端已有输入；草稿已保留，请先处理终端输入'}}))
                page.reload()
                page.wait_for_function('T.listLoaded')
                page.get_by_role('button', name='对话', exact=True).click()
                notice = page.locator('#composer-input-status')
                expect(notice).to_be_visible()
                footer = page.locator('#conversation-status')
                expect(footer.locator('[data-balance="codex:credits"]')).to_be_visible()
                assert footer.bounding_box()['y'] + footer.bounding_box()['height'] <= notice.bounding_box()['y'] + 1
                assert notice.bounding_box()['y'] + notice.bounding_box()['height'] <= page.locator('#cinput').bounding_box()['y'] + 1
                page.screenshot(path=str(args.screenshots / f'{width}-{theme}-blocked.png'))
                for balance in ('0', '不限量', None):
                    page.unroute('**/api/quotas')
                    page.route('**/api/quotas', lambda r, *, balance=balance: r.fulfill(json={'agents': [{'source': 'codex', 'buckets': [
                        {'id': 'codex:credits', 'remaining_text': balance},
                        {'id': 'reset-credits', 'remaining_text': '0 张'}]}]}))
                    page.reload()
                    page.wait_for_function('T.listLoaded')
                    page.get_by_role('button', name='对话', exact=True).click()
                    credit = page.locator('#conversation-status [data-balance="codex:credits"]')
                    if balance is None:
                        expect(credit).to_have_count(0)
                    else:
                        expect(credit).to_have_text('🪙 ' + ('无限' if balance == '不限量' else '0'))
                    expect(page.locator('#conversation-status [data-balance="reset-credits"]')).to_have_text('重置卡 0 张')
                page.unroute('**/api/quotas')
                page.route('**/api/quotas', lambda r: r.fulfill(json={'agents': []}))
                page.reload()
                page.wait_for_function('T.listLoaded')
                page.get_by_role('button', name='对话', exact=True).click()
                expect(page.locator('#conversation-status .sdk-gauges')).to_have_text('用量暂无数据')
                assert not errors, errors
                page.close()
                print('PASS conversation footer', width, theme, flush=True)
            browser.close()

if __name__ == '__main__':
    main()
