#!/usr/bin/env python3
"""Exercise lazy tool rendering, live append and cancellable regex with Chromium."""
import argparse
import os
import re
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, codex_message, codex_row, encoded, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-conversation-perf-') as temporary:
        root = Path(temporary)
        for source in ('claude', 'codex', 'grok'):
            (root / source).mkdir()
        corpus = Corpus(root)
        sid = 'conversation-performance'
        rows = [codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/performance'}),
                codex_message('user', 'Performance conversation'),
                codex_row('response_item', {'type': 'reasoning', 'summary': [
                    {'type': 'summary_text', 'text': 'Planning the tool sequence.'}]})]
        for index in range(180):
            rows.append(codex_row('response_item', {'type': 'function_call', 'name': 'shell_command',
                'call_id': f'call-{index}', 'arguments': '{"command":"echo perf"}'}))
        rows.append(codex_message('assistant', 'Tool sequence complete.'))
        path = corpus.put(sid, 'codex', rows, [])
        corpus.put('regex-slow', 'codex', [
            codex_row('session_meta', {'id': 'regex-slow', 'cwd': '/synthetic/regex'}),
            codex_message('user', 'a' * 32 + '!'),
            codex_message('assistant', 'Search responsiveness fixture')], [])
        with isolated_server(corpus, args.binary) as (base, _opener), sync_playwright() as playwright:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
                context.route('**/*', lambda route: route.continue_()
                              if route.request.url.startswith(base + '/') else route.abort())
                context.add_init_script("""(() => {
                  const Native = window.EventSource;
                  window.__performancePackets = [];
                  window.EventSource = class extends Native {
                    constructor(url, options) {
                      super(url, options);
                      this.addEventListener('message', event => {
                        try { const packet = JSON.parse(event.data);
                          window.__performancePackets.push({reset: !!packet.reset,
                            messages: (packet.messages || []).map(message => message.text)});
                        } catch {}
                      });
                    }
                  };
                })();""")
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base, wait_until='networkidle')
                page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                expect(page.locator('#msgs')).to_contain_text('Tool sequence complete.')
                # The header toggles existing process state, retaining the
                # mounted process and lazy tool group through fold/reopen.
                process = page.locator('#msgs > .turn-process')
                expect(process).to_have_count(1)
                process_node = process.element_handle()
                toggle = page.locator('#a-turns')
                for expanded in (True, False, True):
                    if not toggle.is_visible():
                        page.locator('#a-more').click()
                    toggle.click()
                    expect(toggle).to_have_attribute('aria-pressed', str(expanded).lower())
                    expect(process.locator('.turn-toolbar .fold-toggle')).to_have_attribute(
                        'aria-expanded', str(expanded).lower())
                    assert process_node.evaluate('node => node === document.querySelector("#msgs > .turn-process")')
                process_node.dispose()
                group = page.locator('#msgs .grp').first
                expect(group).to_have_class(re.compile(r'\bfolded\b'))
                expect(group.locator(':scope > .tool-entry')).to_have_count(0)
                group.locator(':scope > .fold-preview .fold-toggle').click()
                expect(group.locator(':scope > .tool-entry')).to_have_count(180)
                # Clicking real disclosure controls preserves content on reopening.
                group.locator(':scope > .disclosure').click()
                group.locator(':scope > .fold-preview .fold-toggle').click()
                expect(group.locator(':scope > .tool-entry')).to_have_count(180)
                with path.open('ab') as stream:
                    stream.write(encoded(codex_message('assistant', 'Live performance append sentinel')))
                expect(page.locator('#msgs')).to_contain_text('Live performance append sentinel', timeout=15000)
                # The first growth can warm an index/view checkpoint through a
                # reset. Verify stable nodes on the following hot append.
                page.locator('#msgs .msg[data-role="user"]').first.evaluate('(node) => window.__firstBubble = node')
                with path.open('ab') as stream:
                    stream.write(encoded(codex_message('assistant', 'Second live append sentinel')))
                expect(page.locator('#msgs')).to_contain_text('Second live append sentinel', timeout=15000)
                assert page.locator('#msgs .msg[data-role="user"]').first.evaluate('(node) => node === window.__firstBubble'), page.evaluate('window.__performancePackets')
                # A valid but pathological expression must not stop keyboard input.
                page.locator('#opts button[data-o="regex"]').click()
                page.locator('#q').fill('(a+)+$')
                page.wait_for_timeout(250)
                page.locator('#q').press('Escape', timeout=2500)
                expect(page.locator('#q')).to_have_value('')
                expect(page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]')).to_be_visible()
                # Normal regex semantics and highlights still work after termination.
                page.locator('#q').fill('Performance|conversation')
                expect(page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"] .t mark')).to_have_count(2)
                page.locator('#q').press('Enter')
                expect(page.locator('#search-progress')).not_to_be_visible(timeout=15000)
                assert not errors, errors
                context.close()
            finally:
                browser.close()
    print('PASS conversation performance browser', flush=True)


if __name__ == '__main__':
    main()
