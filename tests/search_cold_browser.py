#!/usr/bin/env python3
"""Cold/hot Chromium search over tool-heavy synthetic native history.

All data and caches are private. Timings are reported, not tied to a machine's
speed. Search must ignore tool output, preserve semantic hits on cold/hot and
append paths, and leave a subsequent ordinary history open fully functional.
"""
import argparse
import os
from pathlib import Path
import tempfile
import time

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_message, codex_row, encoded, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-search-cold-') as temporary:
        data = Corpus(Path(temporary))
        for source in ('claude', 'codex', 'grok'):
            (data.root / source).mkdir()
        tool_text = 'ToolOnlyNeedle /synthetic/output.png ' + 'tool output text ' * 2048
        for index in range(4):
            sid = f'cold-search-{index}'
            path = data.put(sid, 'codex', [
                codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/search-cold'}),
                codex_message('user', f'Cold search fixture {index}'),
            ], [])
            with path.open('ab') as stream:
                for call in range(512):
                    stream.write(encoded(codex_row('response_item', {
                        'type': 'function_call', 'name': 'exec_command',
                        'call_id': f'call-{call}', 'arguments': '{"cmd":"synthetic"}',
                    })))
                    stream.write(encoded(codex_row('response_item', {
                        'type': 'function_call_output', 'call_id': f'call-{call}', 'output': tool_text,
                    })))
                stream.write(encoded(codex_message('assistant', f'SemanticNeedle answer {index}')))
        cache = data.root / 'search-cache'
        native = {path: (path.stat().st_mtime_ns, path.read_bytes()) for path in data.paths.values()}
        with isolated_server(data, args.binary, extra_env={
            'SESSIONDOCK_SEARCH_CACHE_DIR': str(cache),
        }) as (base, _), sync_playwright() as pw:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**launch)
            try:
                page = browser.new_page(service_workers='block')
                page.route('**/*', lambda route: route.continue_()
                           if route.request.url.startswith(base + '/') else route.abort())
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base, wait_until='networkidle')
                expect(page.locator('#side .item[data-uid]')).to_have_count(4)
                assert not list(cache.iterdir()), 'List opening prebuilt search text'

                def search(query, count):
                    old = page.locator('#stat').get_attribute('data-seq') or ''
                    page.locator('#q').fill(query)
                    started = time.monotonic()
                    page.locator('#q').press('Enter')
                    page.wait_for_function("old => (document.querySelector('#stat').dataset.seq || '') !== old",
                                           arg=old, timeout=30000)
                    elapsed = time.monotonic() - started
                    expect(page.locator('#search-progress')).to_be_hidden()
                    expect(page.locator('#side-search-count')).to_have_text(f'{count} 条')
                    expect(page.locator('#stat')).to_contain_text(f'全文命中 {count} 个会话')
                    expect(page.locator('#stat')).not_to_have_class('err')
                    result = page.evaluate('S.results')
                    assert len(result) == count, result
                    return elapsed, result

                cold, first = search('SemanticNeedle', 4)
                cached = {p.name: p.stat().st_mtime_ns for p in cache.iterdir()}
                hot, second = search('SemanticNeedle', 4)
                assert first == second
                assert cached == {p.name: p.stat().st_mtime_ns for p in cache.iterdir()}
                for row in first:
                    assert row['hits'] == 1 and 'SemanticNeedle' in row['snippet'], row
                search('ToolOnlyNeedle', 0)
                with data.paths['cold-search-0'].open('ab') as stream:
                    stream.write(encoded(codex_message('assistant', 'AppendNeedle visible answer')))
                search('AppendNeedle', 1)
                page.locator(f'#side .item[data-uid="{data.uid("cold-search-0")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('AppendNeedle visible answer', timeout=15000)
                # Opening history after a search must construct its own complete
                # encoding/checkpoint; a search-only parse cannot enter the view LRU.
                assert page.evaluate("cache.get(viewKey(S.sel, S.agent)).anchor")
                page.locator('#q').press('Escape')
                expect(page.locator('#side .item[data-uid]')).to_have_count(4)
                assert not errors, errors
                for path, (mtime, content) in native.items():
                    if path == data.paths['cold-search-0']:
                        assert path.read_bytes().startswith(content)
                    else:
                        assert (path.stat().st_mtime_ns, path.read_bytes()) == (mtime, content)
                print(f'PASS cold search: {sum(len(v[1]) for v in native.values()) / 1024**2:.1f} MiB native; '
                      f'cold={cold:.3f}s hot={hot:.3f}s; hits, ignored tools, append, history and native preservation',
                      flush=True)
            finally:
                browser.close()


if __name__ == '__main__':
    main()
