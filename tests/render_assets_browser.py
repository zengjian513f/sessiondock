#!/usr/bin/env python3
"""Optional renderers load on demand; large code is highlighted off the UI thread."""
import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, claude_row, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-render-assets-') as temporary:
        corpus = Corpus(Path(temporary))
        for source in ('claude', 'codex', 'grok'):
            (corpus.root / source).mkdir()
        code = ''.join(f'const value{i} = "line {i}";\n' for i in range(16000)).rstrip()
        for sid, title, body in (
            ('plain', 'Plain rendering', 'No optional renderer is needed here.'),
            ('formula', 'Formula rendering', 'An equation: $$x^2 + y^2 = z^2$$'),
            ('code', 'Large code rendering', '```javascript\n' + code + '\n```'),
        ):
            corpus.put(sid, 'claude', [
                claude_row(sid, 'user', 'u', None, title),
                claude_row(sid, 'assistant', 'a', 'u', body),
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
                page.locator('#msgs .msg[data-role="assistant"] button').filter(has_text='展开全文').click()
                expect(block).to_have_attribute('data-syntax-done', '1', timeout=15000)
                assert block.text_content() == code
                assert block.locator('.hljs-keyword').count() == 16000
                assert page.evaluate('syntaxTicks') > 0
                page.evaluate('clearInterval(syntaxClock)')
                # Rapid preview changes must not receive a stale full-code reply.
                page.locator('#msgs .msg[data-role="assistant"] button').filter(has_text='收起').click()
                expect(block).to_have_attribute('data-syntax-done', '1')
                assert len(block.text_content()) < 5000
                page.locator('#msgs .msg[data-role="assistant"] button').filter(has_text='展开全文').click()
                page.locator('#msgs .msg[data-role="assistant"] button').filter(has_text='收起').click()
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
