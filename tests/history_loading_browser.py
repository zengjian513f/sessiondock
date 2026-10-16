#!/usr/bin/env python3
"""History navigation stays inside the page; only explicit full loads show progress.

Real isolated Rust history, held network responses, Chromium clicks, retries,
cache reopen and batched renders at desktop/report/phone widths in both themes.
"""
import argparse
import os
import re
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from browser_race_assets import install_small_render_batches
from history_fixtures import BINARY, Corpus, codex_row, isolated_server
from media_browser import uid

WATCH = """() => {
  window.loadingOverlays = [];
  new MutationObserver(records => {
    const bar = document.querySelector('#prog');
    if (bar.classList.contains('on') || records.some(r => /\\bon\\b/.test(r.oldValue || '')))
      loadingOverlays.push(bar.textContent);
  }).observe(document.querySelector('#prog'),
    {attributes:true, attributeOldValue:true, attributeFilter:['class']});
}"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-history-loading-') as temporary:
        corpus = Corpus(Path(temporary))
        for source in ('claude', 'codex', 'grok'):
            (corpus.root / source).mkdir()
        for name, count in [('codex-loading-main', 720), ('codex-loading-other', 1)]:
            corpus.put(name, 'codex', [
                codex_row('session_meta', {'id': name, 'cwd': '/synthetic/history-loading'}),
                *[codex_row('response_item', {'type': 'message', 'role': 'user', 'content': [
                    {'type': 'input_text', 'text': f'{name} {i:04d} 加载历史与长标题 navigation'}
                ]}) for i in range(count)]], [])
        with isolated_server(corpus, args.binary) as (base, _), sync_playwright() as playwright:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = playwright.chromium.launch(**options)
            for width in (1280, 675, 390):
                for theme in ('light', 'dark'):
                    context = browser.new_context(viewport={'width': width, 'height': 846},
                                                  color_scheme=theme, service_workers='block')
                    page = context.new_page()
                    errors = []
                    page.on('pageerror', lambda e: errors.append(str(e)))
                    install_small_render_batches(page, context)
                    page.goto(base, wait_until='networkidle')
                    page.evaluate(WATCH)

                    def capture(state):
                        if args.screenshots:
                            page.screenshot(path=str(args.screenshots / f'{width}-{theme}-{state}.png'))

                    def click(name):
                        if page.locator('.mobile-back').is_visible():
                            page.locator('.mobile-back').click()
                        page.locator(f'#side .item[data-uid="{uid(corpus, name)}"]').click()

                    def quiet():
                        expect(page.locator('#prog .txt')).to_be_hidden()
                        assert page.evaluate('loadingOverlays') == [], page.evaluate('loadingOverlays')

                    held = []
                    page.route('**/api/messages/**', lambda route: held.append(route), times=1)
                    click('codex-loading-main')
                    expect(page.locator('#detail .spin')).to_have_text('正在读取会话…')
                    expect(page.locator('#a-term')).to_be_visible()
                    quiet()
                    capture('loading')
                    assert len(held) == 1
                    held.pop().continue_()
                    expect(page.locator('#msgs')).to_contain_text('codex-loading-main 0719')
                    quiet()
                    capture('loaded')

                    # A failed opening retains its inline explanation and clickable retry.
                    page.route('**/api/messages/**', lambda route: route.fulfill(
                        status=503, json={'error': 'Synthetic history read failure'}), times=1)
                    click('codex-loading-other')
                    expect(page.locator('#detail .empty')).to_contain_text('Synthetic history read failure')
                    quiet()
                    capture('failed')
                    page.get_by_role('button', name='重试读取', exact=True).click()
                    expect(page.locator('#msgs')).to_contain_text('codex-loading-other 0000')
                    click('codex-loading-main')
                    expect(page.locator('#msgs')).to_contain_text('codex-loading-main 0719')
                    quiet()
                    page.locator('.history-gap-load').click()
                    expect(page.locator('.history-gap')).to_have_count(0, timeout=20000)
                    expect(page.locator('#msgs')).to_contain_text('codex-loading-main 0250')
                    quiet()
                    capture('expanded')
                    assert not errors, errors
                    context.close()
                    print(f'PASS {width} {theme}: delayed open, inline retry, cached reopen, paged render', flush=True)

            # Old-node fallback is an explicit user request; retain its progress UI.
            context = browser.new_context(viewport={'width': 1280, 'height': 846}, service_workers='block')
            page = context.new_page()
            def old_node(route):
                response = route.fetch()
                body, count = re.subn(r'(&quot;history_pages&quot;\s*:\s*)true',
                                      r'\1false', response.text())
                assert count == 1
                route.fulfill(response=response, body=body)
            page.route(base + '/', old_node)
            page.goto(base, wait_until='networkidle')
            page.locator(f'#side .item[data-uid="{uid(corpus, "codex-loading-main")}"]').click()
            expect(page.locator('#msgs')).to_contain_text('codex-loading-main 0719')
            held = []
            page.route('**/api/messages/**', lambda route: held.append(route), times=1)
            page.locator('.history-gap-load').click()
            expect(page.locator('#prog .txt')).to_contain_text('下载完整历史')
            assert len(held) == 1
            held.pop().continue_()
            expect(page.locator('.history-gap')).to_have_count(0)
            expect(page.locator('#msgs')).to_contain_text('codex-loading-main 0250')
            expect(page.locator('#prog .txt')).to_be_hidden()
            context.close()
            browser.close()
            print('PASS explicit full-history download retains progress', flush=True)


if __name__ == '__main__':
    main()
