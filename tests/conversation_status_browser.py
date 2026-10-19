"""Conversation footer status and cached usage through real page interactions."""
import argparse
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
            for width, theme in [(1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark')]:
                page = browser.new_page(viewport={'width': width, 'height': 800}, color_scheme=theme)
                state = {'value': 'working'}
                errors = []
                page.on('pageerror', lambda e: errors.append(str(e)))
                uid = corpus.uid('active')
                page.route('**/api/live*', lambda r: r.fulfill(json={'known': True, 'uids': [uid], 'working_uids': [uid], 'tmux_uids': []}))
                page.route('**/api/quotas', lambda r: r.fulfill(json={'agents': [{'source': 'codex', 'captured_at': '2026-10-19T03:00:00Z', 'buckets': [
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
                    expect(page.locator('#msgs #activity')).to_have_count(0)
                    expect(page.locator('#msgs')).to_contain_text('Existing conversation content')
                    assert footer.evaluate('(e) => e.scrollWidth <= e.clientWidth + 1')
                    page.screenshot(path=str(args.screenshots / f'{width}-{theme}-{status}.png'))
                page.unroute('**/api/quotas')
                page.route('**/api/quotas', lambda r: r.fulfill(json={'agents': []}))
                page.reload()
                expect(page.locator('#conversation-status .sdk-gauges')).to_have_text('用量暂无数据')
                assert not errors, errors
                page.close()
                print('PASS conversation footer', width, theme, flush=True)
            browser.close()

if __name__ == '__main__':
    main()
