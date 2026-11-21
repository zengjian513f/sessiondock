#!/usr/bin/env python3
"""Cold/hot Chromium search over tool-heavy synthetic native history.

All data and caches are private. Timings are reported, not tied to a machine's
speed. Search must ignore tool output, preserve semantic hits on cold/hot and
append paths, and leave a subsequent ordinary history open fully functional.
"""
import argparse
import json
import os
from pathlib import Path
import tempfile
import time

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_message, codex_row, encoded, isolated_server


def search(page, query, count):
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


def cache_backfill(binary, browser, root):
    data = Corpus(root)
    for source in ('claude', 'codex', 'grok'):
        (root / source).mkdir(parents=True)
    first = 'Kelvin ' * 210 + 'first record'
    # The first byte of a Kelvin sign falls at the end of a 1 MiB read.
    # Its simple case fold is one ASCII byte, so the raw 4 MiB body fits
    # after folding into a 2 MiB tier.
    boundary = 1024**2 - len(first.encode()) - 2
    tail = 'x' * (boundary % 3) + 'K' * 1_400_000 + 'end'
    data.put('fold-backfill', 'codex', [
        codex_row('session_meta', {'id': 'fold-backfill', 'cwd': '/synthetic/fold-backfill'}),
        codex_message('user', first),
        codex_message('assistant', tail),
    ], [])
    cache = root / 'search-cache'
    reference = None
    for fold_bytes in (0, 64 * 1024, 1024**2, 2 * 1024**2):
        # The first process seeds the persistent body. Later processes must
        # match this exact entry with an empty in-memory folded tier.
        with isolated_server(data, binary, extra_env={
            'SESSIONDOCK_SEARCH_CACHE_DIR': str(cache),
            'SESSIONDOCK_SEARCH_FOLD_BYTES': str(fold_bytes),
        }) as (base, opener):
            page = browser.new_page(service_workers='block')
            try:
                page.route('**/*', lambda route: route.continue_()
                           if route.request.url.startswith(base + '/') else route.abort())
                page.goto(base, wait_until='networkidle')
                expect(page.locator('#side .item[data-uid]')).to_have_count(1)
                proc_io = Path(f'/proc/{opener.sessiondock_pid}/io')

                def bytes_read():
                    if proc_io.exists():
                        return int(dict(line.split(':', 1) for line in proc_io.read_text().splitlines())['rchar'])
                    return 0

                before = bytes_read()
                _, result = search(page, 'kelvin', 1)
                assert result[0]['hits'] == 200 and result[0]['hits_capped']
                assert 'Kelvin' in result[0]['snippet']
                if reference is None:
                    reference = result
                    # With folding disabled, a second search hits the cache.
                    before = bytes_read()
                    _, result = search(page, 'kelvin', 1)
                assert result == reference
                if fold_bytes <= 64 * 1024:
                    assert bytes_read() - before < 2 * 1024**2, 'fold fallback read beyond the first matching chunk'
                before = bytes_read()
                search(page, 'AbsentNeedle', 0)
                if fold_bytes == 2 * 1024**2:
                    assert bytes_read() - before < 2 * 1024**2, 'shrinking Unicode body did not backfill the prefilter'
            finally:
                page.close()
    print('PASS persistent fold backfill: disabled, oversized and shrinking Unicode bodies', flush=True)


def kiro_cache_restart(binary, browser, root):
    data = Corpus(root)
    for name in ('claude', 'codex', 'grok', 'kiro-native', 'proc'):
        (root / name).mkdir(parents=True)
    sid = '019a0000-0000-7000-8000-000000000301'
    uid = 'kiro:0123456789abcdef'
    projection = root / 'kiro-projection'
    session = projection / 'synthetic-account' / sid
    session.mkdir(parents=True)
    timestamp = '2026-09-11T10:00:00Z'
    projection_format = 'sessiondock-kiro-projection-v1'
    (session / 'summary.json').write_text(json.dumps({
        'format': projection_format, 'uid': uid, 'history_synced': True,
        'session': {'id': sid, 'title': 'Synthetic Kiro session', 'cwd': '/synthetic/kiro',
                    'created': timestamp, 'updated': timestamp},
    }), encoding='utf-8')
    text = 'Kiro 持久缓存中的助手答复'
    (session / 'messages.jsonl').write_bytes(encoded({
        'format': projection_format,
        'item': {'id': 'reply', 'role': 'assistant', 'text': text, 'timestamp': timestamp},
    }))
    cache = root / 'search-cache'
    env = {
        'SESSIONDOCK_KIRO_ROOT': str(root / 'kiro-native'),
        'SESSIONDOCK_KIRO_PROJECTION_ROOT': str(projection),
        'SESSIONDOCK_PROC_ROOT': str(root / 'proc'),
        'SESSIONDOCK_GROK_ACTIVE': str(root / 'grok' / 'active.json'),
        'SESSIONDOCK_SEARCH_CACHE_DIR': str(cache),
        'SESSIONDOCK_SEARCH_FOLD_BYTES': '0',
    }
    entry = cache / uid.replace(':', '-')
    written = None
    for phase in ('cold', 'restart'):
        with isolated_server(data, binary, extra_env=env) as (base, _):
            page = browser.new_page(service_workers='block')
            try:
                page.route('**/*', lambda route: route.continue_()
                           if route.request.url.startswith(base + '/') else route.abort())
                page.goto(base, wait_until='networkidle')
                expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_be_visible()
                if phase == 'cold':
                    assert not entry.exists(), 'List opening prebuilt Kiro search text'
                _, result = search(page, '持久缓存中的助手答复', 1)
                assert result[0]['uid'] == uid and result[0]['hits'] == 1, result
                assert text in result[0]['snippet'], result
                assert entry.is_file(), f'Kiro search body was not persisted: {list(cache.iterdir())}'
                if phase == 'cold':
                    written = entry.stat().st_mtime_ns
                else:
                    assert entry.stat().st_mtime_ns == written, 'Restart search rewrote the Kiro cache entry'
            finally:
                page.close()
    print('PASS Kiro persistent search: cold result persisted and reused after restart', flush=True)


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

                cold, first = search(page, 'SemanticNeedle', 4)
                cached = {p.name: p.stat().st_mtime_ns for p in cache.iterdir()}
                hot, second = search(page, 'SemanticNeedle', 4)
                assert first == second
                assert cached == {p.name: p.stat().st_mtime_ns for p in cache.iterdir()}
                for row in first:
                    assert row['hits'] == 1 and 'SemanticNeedle' in row['snippet'], row
                search(page, 'ToolOnlyNeedle', 0)
                with data.paths['cold-search-0'].open('ab') as stream:
                    stream.write(encoded(codex_message('assistant', 'AppendNeedle visible answer')))
                search(page, 'AppendNeedle', 1)
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
                cache_backfill(args.binary, browser, data.root / 'backfill')
                kiro_cache_restart(args.binary, browser, data.root / 'kiro')
            finally:
                browser.close()


if __name__ == '__main__':
    main()
