#!/usr/bin/env python3
"""Optional renderers load on demand; large code is highlighted off the UI thread."""
import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, claude_row, codex_row, codex_message, isolated_server


LIST_CASES = (
    ('loose', '1. **检查输入。** 确认路径和参数符合预期。\n\n'
     '2. **执行任务。** 保留现有工作目录和运行状态。\n\n'
     '3. **验证结果。** 使用实际浏览器打开页面并检查编号。\n\n'
     '4. **发布修复。** 完成部署与健康检查。', [('OL', 1, 4)]),
    ('repeated', '1. First\n\n1. Second\n   continuation\n\n\n1. Third', [('OL', 1, 3)]),
    ('start', '7) Seven\n8) Eight\n\n9) Nine', [('OL', 7, 3)]),
    ('boundaries', '1. First\n\nSeparate paragraph.\n\n4. Fourth\n\n'
     '## Heading\n\n8. Eighth\n- Bullet one\n\n- Bullet two\n2. Second',
     [('OL', 1, 1), ('OL', 4, 1), ('OL', 8, 1), ('UL', None, 2), ('OL', 2, 1)]),
    ('fence', '1. First\n\n```text\n2. Literal code\n```\n\n3. Third',
     [('OL', 1, 1), ('OL', 3, 1)]),
)


def check_lists(browser, base, corpus, screenshots):
    # BUG-20261012-054602-f0bb43: native numbered items separated by blank
    # lines became separate <ol>s, each displaying 1. Open native fixtures
    # through the sidebar so projection and the real message renderer run.
    for width in (1280, 675):
        for theme in ('light', 'dark'):
            page = browser.new_page(viewport={'width': width, 'height': 900}, color_scheme=theme)
            try:
                page.goto(base)
                expect(page.locator('html')).to_have_attribute('data-theme', theme)
                for name, body, expected in LIST_CASES:
                    if width < 720 and page.locator('.mobile-back').is_visible():
                        page.locator('.mobile-back').click()
                    page.locator(f'#side .item[data-uid="{corpus.uid("list-" + name)}"]').click()
                    message = page.locator('#msgs .msg[data-role="assistant"] .mb')
                    expect(message).to_be_visible()
                    if screenshots and name == 'loose':
                        screenshots.mkdir(parents=True, exist_ok=True)
                        message.scroll_into_view_if_needed()
                        page.screenshot(path=str(screenshots / f'lists-{width}-{theme}.png'))
                    actual = message.locator(':scope > ol, :scope > ul').evaluate_all(
                        'nodes => nodes.map(n => [n.tagName, n.tagName === "OL" ? n.start : null, n.children.length])')
                    assert actual == [list(row) for row in expected], (name, width, theme, actual)
                    if name == 'repeated':
                        expect(message.locator('li').nth(1)).to_contain_text('continuation')
                    if name == 'fence':
                        expect(message.locator('code')).to_have_text('2. Literal code')
                page.reload()
                expect(page.locator('#msgs ol').last).to_have_attribute('start', '3')
            finally:
                page.close()
    print('PASS Markdown lists: loose/compact, repeated markers, start, boundaries, reload, wide/narrow, light/dark')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-render-assets-') as temporary:
        corpus = Corpus(Path(temporary))
        for source in ('claude', 'codex', 'grok'):
            (corpus.root / source).mkdir()
        for name, body, _ in LIST_CASES:
            sid = 'list-' + name
            corpus.put(sid, 'codex', [
                codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/lists'}),
                codex_message('user', 'Markdown 编号 ' + name),
                codex_message('assistant', body, 2),
            ], [])
        code = ''.join(f'const value{i} = "line {i}";\n' for i in range(16000)).rstrip()
        for sid, title, body in (
            ('plain', 'Plain rendering', 'No optional renderer is needed here.'),
            ('formula', 'Formula rendering', 'An equation: $$x^2 + y^2 = z^2$$'),
            ('code', 'Large code rendering', '```javascript\n' + code + '\n```'),
        ):
            corpus.put(sid, 'claude', [
                claude_row(sid, 'user', 'u', None, title),
                claude_row(sid, 'user' if sid == 'code' else 'assistant', 'a', 'u', body),
            ], [])
        corpus.put('batched', 'claude', [
            claude_row('batched', 'user', f'u{i}', f'u{i-1}' if i else None,
                       '$$x^2$$\n```python\nprint("batched")\n```' if i == 0 else f'Batch row {i}')
            for i in range(600)
        ], [])
        with isolated_server(corpus, args.binary) as (base, _), sync_playwright() as p:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = p.chromium.launch(**options)
            try:
                check_lists(browser, base, corpus, args.screenshots)
                page = browser.new_page(viewport={'width': 1280, 'height': 900})
                requests, errors = [], []
                page.on('request', lambda request: requests.append(request.url))
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base)

                def open_session(sid):
                    page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()

                open_session('plain')
                expect(page.locator('#msgs')).to_contain_text('No optional renderer')
                assert not any('/vendor/katex/' in url for url in requests), requests
                open_session('formula')
                expect(page.locator('#msgs .katex')).to_be_visible()
                assert any('/vendor/katex/katex.min.js' in url for url in requests)
                open_session('code')
                block = page.locator('#msgs .mb code.code-block')
                expect(block).to_have_attribute('data-syntax-done', '1')
                page.evaluate('''() => {
                  window.syntaxTicks = 0;
                  window.syntaxClock = setInterval(() => syntaxTicks++, 10);
                }''')
                page.locator('#msgs .msg[data-role="user"] button').filter(has_text='展开全文').click()
                expect(block).to_have_attribute('data-syntax-done', '1', timeout=15000)
                assert block.text_content() == code
                assert block.locator('.hljs-keyword').count() == 16000
                assert page.evaluate('syntaxTicks') > 0
                page.evaluate('clearInterval(syntaxClock)')
                # Rapid preview changes must not receive a stale full-code reply.
                page.locator('#msgs .msg[data-role="user"] button').filter(has_text='收起').click()
                expect(block).to_have_attribute('data-syntax-done', '1')
                assert len(block.text_content()) < 5000
                page.locator('#msgs .msg[data-role="user"] button').filter(has_text='展开全文').click()
                page.locator('#msgs .msg[data-role="user"] button').filter(has_text='收起').click()
                expect(block).to_have_attribute('data-syntax-done', '1')
                assert len(block.text_content()) < 5000
                open_session('batched')
                # The first render batch lives in a detached fragment while
                # later batches yield; its queued syntax work must survive.
                expect(page.locator('#msgs .code-block')).to_have_attribute('data-syntax-done', '1')
                expect(page.locator('#msgs .code-block')).to_contain_text('print("batched")')
                cold = browser.new_page(viewport={'width': 1280, 'height': 900})
                cold.on('pageerror', lambda error: errors.append(str(error)))
                cold.goto(base)
                cold.locator(f'#side .item[data-uid="{corpus.uid("batched")}"]').click()
                expect(cold.locator('#msgs .katex')).to_have_count(1)
                expect(cold.locator('#msgs .code-block')).to_have_attribute('data-syntax-done', '1')
                cold.close()
                assert not errors, errors
                print('PASS render assets: lazy libraries, worker highlighting, exact large code, responsive UI, stale preview protection')
            finally:
                browser.close()


if __name__ == '__main__':
    main()
