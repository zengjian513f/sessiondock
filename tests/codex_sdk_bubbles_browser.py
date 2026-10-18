#!/usr/bin/env python3
"""Type-based bubbles and disclosure interaction during SDK streaming."""
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright, expect
from codex_sdk_browser import build, server, THREAD
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, codex_message

TEXT = '\n\n'.join(f'段落 {i}：原生补全需要保留路径、参数与光标位置。' for i in range(24)) + '\n\n完整结尾 END'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-bubbles-') as tmp, sync_playwright() as p:
        root = Path(tmp)
        corpus = build(root)
        with corpus.paths['sdk'].open('a') as f:
            f.write(json.dumps(codex_message('user', TEXT)) + '\n')
            f.write(json.dumps(codex_message('assistant', TEXT)) + '\n')
        with FakeAppServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake, server(corpus, args.binary, root/'app.sock') as base:
            browser = p.chromium.launch(headless=True)
            for theme in ('light', 'dark'):
                context = browser.new_context(viewport={'width': 1440, 'height': 900}, color_scheme=theme)
                page = context.new_page()
                page.goto(base, wait_until='networkidle')
                page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
                for width in (3406, 390):
                    page.set_viewport_size({'width': width, 'height': 900})
                    if not page.locator('#sdk-input').is_visible():
                        page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
                    answer = page.locator('.sdk-history .msg[data-role="assistant"]').last
                    expect(answer.locator('.mb.clip')).to_have_count(0)
                    assert answer.locator('.mb').evaluate('(b) => b.scrollHeight <= b.clientHeight + 1')
                    bubble = page.locator('.sdk-history .msg[data-role="user"]').last
                    expect(bubble).to_contain_text('完整结尾 END')
                    page.screenshot(path=str(args.screenshots/f'{theme}-{width}-before.png'))
                    expand = bubble.get_by_text('展开全文', exact=False)
                    expect(expand).to_be_visible()
                    expand.click()
                    assert bubble.locator('.mb').evaluate('(b) => b.scrollHeight <= b.clientHeight + 1')
                    bubble.get_by_text('收起', exact=True).click()
                    expect(expand).to_be_visible()
                    page.screenshot(path=str(args.screenshots/f'{theme}-{width}-collapsed.png'))
                    expand.click()
                    bubble.get_by_text('收起', exact=True).scroll_into_view_if_needed()
                    page.screenshot(path=str(args.screenshots/f'{theme}-{width}-expanded.png'))
                    bubble.get_by_text('收起', exact=True).click()
                page.locator('#sdk-input').fill('stream a long reply')
                page.locator('#sdk-send').click()
                expect(page.locator('.sdk-live')).to_contain_text('stream a long reply')
                def notify(method, raw):
                    asyncio.run_coroutine_threadsafe(fake.notify(method, {'threadId': THREAD, 'turnId': 'turn-1', **raw}), fake.loop).result(timeout=5)
                notify('item/started', {'item': {'id': 'long-reply', 'type': 'agentMessage', 'text': TEXT}})
                live = page.locator('[data-sdk-item="long-reply"]')
                expect(live.locator('.mb')).to_contain_text('完整结尾 END')
                expect(live.locator('.mb.clip')).to_have_count(0)
                # Beyond the old character threshold, all assistant prose still renders.
                notify('item/agentMessage/delta', {'itemId': 'long-reply', 'delta': TEXT * 8 + '\n最终完整结尾'})
                expect(live.locator('.mb')).to_contain_text('最终完整结尾')
                assert live.locator('.mb').evaluate('(b) => b.scrollHeight <= b.clientHeight + 1')
                notify('item/started', {'item': {'id': 'long-thought', 'type': 'reasoning', 'summary': [TEXT]}})
                thought = page.locator('[data-sdk-item="long-thought"]')
                more = thought.get_by_text('展开全文', exact=False)
                expect(more).to_be_visible()
                assert more.bounding_box()['y'] >= thought.locator('.mb').bounding_box()['y'] + thought.locator('.mb').bounding_box()['height'] - 1
                # An arriving delta between pointer down/up must not replace the button.
                more.scroll_into_view_if_needed()
                bounds = more.bounding_box()
                page.mouse.move(bounds['x'] + 10, bounds['y'] + 10)
                page.mouse.down()
                notify('item/reasoning/summaryTextDelta', {'itemId': 'long-thought', 'summaryIndex': 0, 'delta': '\n指针按下时新增'})
                expect(thought.locator('.mb')).to_contain_text('指针按下时新增')
                page.mouse.up()
                expect(thought.get_by_text('收起', exact=True)).to_be_visible()
                notify('item/reasoning/summaryTextDelta', {'itemId': 'long-thought', 'summaryIndex': 0, 'delta': '\n展开后新增'})
                expect(thought.locator('.mb')).to_contain_text('展开后新增')
                expect(thought.locator('.mb.clip')).to_have_count(0)
                thought.get_by_text('收起', exact=True).click()
                notify('item/reasoning/summaryTextDelta', {'itemId': 'long-thought', 'summaryIndex': 0, 'delta': '\n收起后新增'})
                expect(thought.locator('.mb')).to_contain_text('收起后新增')
                expect(thought.locator('.mb.clip')).to_have_count(1)
                more.click()
                thought.get_by_text('收起', exact=True).scroll_into_view_if_needed()
                page.screenshot(path=str(args.screenshots/f'{theme}-streaming-expanded.png'))
                page.locator('#sdk-stop').click()
                expect(page.locator('#sdk-stop')).to_be_hidden()
                page.locator('#a-chat').click()
                page.locator('#a-sdk').click()
                expect(page.locator('.sdk-history').get_by_text('展开全文', exact=False).last).to_be_visible()
                context.close()
                print(f'PASS {theme}: persisted/live expansion, resize, view reentry', flush=True)
            browser.close()


if __name__ == '__main__':
    main()
