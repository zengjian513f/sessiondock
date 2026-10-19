#!/usr/bin/env python3
"""SDK async questions: native item/history recovery, real choices and delivery."""
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright, expect
from codex_sdk_browser import build, server, THREAD
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, codex_row

QUESTIONS = [
    {'title': '气泡折叠规则：助手正文完整显示，用户长输入和思考保留折叠。你希望采用哪种？',
     'options': ['采用上述规则', '用户输入也始终完整显示', '只让最终回答完整显示，过程说明仍折叠']},
    {'title': '补充说明 / Additional details'},
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        for theme, width in [(theme, width) for theme in ('light', 'dark') for width in (1440, 390)]:
            with tempfile.TemporaryDirectory(prefix='sd-sdk-question-') as tmp:
                root = Path(tmp)
                corpus = build(root)
                with FakeAppServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake, server(corpus, args.binary, root/'app.sock') as base:
                    context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
                    page = context.new_page()
                    errors = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    page.goto(base, wait_until='networkidle')
                    page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
                    expect(page.locator('#sdk-input')).to_be_visible()
                    page.locator('#sdk-input').fill('question scenario')
                    page.locator('#sdk-send').click()
                    fake.wait_for(lambda: fake.turn)
                    expect(page.locator('.sdk-live')).to_contain_text('question scenario')
                    def notify(method, raw):
                        asyncio.run_coroutine_threadsafe(fake.notify(method, {'turnId': fake.turn, **raw}), fake.loop).result(timeout=5)
                    item = {'id': 'async-call-1', 'type': 'agentMessage', 'text': 'Question text fallback',
                            'delivery': 'async', 'phase': 'final_answer', 'questions': QUESTIONS}
                    notify('item/completed', {'item': item})
                    card = page.locator('[data-sdk-async-question="async-call-1"]')
                    expect(card).to_contain_text(QUESTIONS[0]['title'])
                    expect(card.locator('input:checked')).to_have_count(0)
                    card.get_by_role('button', name='提交回答').click()
                    expect(page.locator('#sdk-status-menu')).to_contain_text('请回答每个问题')
                    assert not fake.methods('turn/steer')
                    card.get_by_label('只让最终回答完整显示，过程说明仍折叠', exact=True).check()
                    card.get_by_placeholder('输入答案', exact=True).fill('保留手动展开状态')
                    # Continued work must preserve selected answers and typed text.
                    notify('item/started', {'item': {'id': 'progress', 'type': 'agentMessage', 'text': 'Continuing work'}})
                    expect(card.locator('input:checked')).to_have_count(1)
                    expect(card.get_by_placeholder('输入答案', exact=True)).to_have_value('保留手动展开状态')
                    card.scroll_into_view_if_needed()
                    page.screenshot(path=str(args.screenshots/f'{theme}-{width}-selected.png'))
                    assert card.evaluate('(e) => e.scrollWidth <= e.clientWidth + 1')
                    # Snapshot recovery must restore the pending question.
                    page.reload(wait_until='networkidle')
                    expect(card).to_contain_text(QUESTIONS[0]['title'])
                    card.get_by_placeholder('其他答案').fill('自定义选择')
                    card.get_by_placeholder('输入答案', exact=True).fill('保留手动展开状态')
                    page.locator('#sdk-input').fill('草稿不应被回答覆盖')
                    # A failed HTTP submit retains answers and can be retried.
                    page.route('**/api/codex-sdk/turn', lambda route: route.fulfill(status=503, content_type='application/json', body='{"error":"synthetic failure"}'), times=1)
                    card.get_by_role('button', name='提交回答').click()
                    expect(page.locator('#sdk-status-menu')).to_contain_text('回答失败')
                    expect(card.get_by_placeholder('其他答案')).to_have_value('自定义选择')
                    card.get_by_role('button', name='提交回答').click()
                    steer = fake.wait_for(lambda: fake.methods('turn/steer'))[-1]['params']
                    assert steer['threadId'] == THREAD and steer['expectedTurnId'] == fake.turn, steer
                    assert steer['input'][0]['text'] == QUESTIONS[0]['title']+'\n• 自定义选择\n\n'+QUESTIONS[1]['title']+'\n• 保留手动展开状态', steer
                    expect(page.locator('#sdk-input')).to_have_value('草稿不应被回答覆盖')
                    expect(card).to_have_count(0)
                    assert not fake.responses(), 'async question must not send a JSON-RPC server-request result'
                    # Cold history recovery, without an app-server item/snapshot.
                    page.locator('#sdk-stop').click()
                    expect(page.locator('#sdk-stop')).to_be_hidden()
                    with corpus.paths['sdk'].open('a') as f:
                        f.write(json.dumps(codex_row('response_item', {'type': 'function_call', 'name': 'request_user_input_async',
                            'call_id': 'async-history', 'arguments': json.dumps({'questions': QUESTIONS[:1]}, ensure_ascii=False)}))+'\n')
                        f.write(json.dumps(codex_row('response_item', {'type': 'function_call_output', 'call_id': 'async-history', 'output': '{"accepted":true}'}))+'\n')
                    page.reload(wait_until='networkidle')
                    history_card = page.locator('[data-sdk-async-question="async-history"]')
                    expect(history_card).to_be_visible()
                    history_card.get_by_label('采用上述规则', exact=True).check()
                    history_card.scroll_into_view_if_needed()
                    page.screenshot(path=str(args.screenshots/f'{theme}-{width}-history.png'))
                    history_card.get_by_role('button', name='提交回答').click()
                    start = fake.wait_for(lambda: fake.methods('turn/start') if len(fake.methods('turn/start')) == 2 else None)[-1]['params']
                    assert start['input'][0]['text'] == QUESTIONS[0]['title']+'\n• 采用上述规则', start
                    assert len(fake.methods('turn/steer')) == 1
                    assert not errors, errors
                    context.close()
                    print(f'PASS {theme}/{width}: live and history questions, snapshot, selection/free text, retry, steer/start, draft preserved', flush=True)
        browser.close()


if __name__ == '__main__':
    main()
