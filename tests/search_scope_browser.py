#!/usr/bin/env python3
"""Fulltext from live-only scope: all 165 historical hits, node and Hub."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from frontend_framework_browser import launch_chromium
from history_fixtures import BINARY, Corpus, codex_message, codex_row, isolated_server
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-search-scope-') as tmp, ExitStack() as stack:
        root = Path(tmp)
        data = Corpus(root / 'node')
        for index in range(165):
            sid = f'scope-{index:03d}'
            data.put(sid, 'codex', [
                codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/search-scope'}),
                codex_message('user', f'Historical session {index:03d}'),
                codex_message('assistant', 'ScopeNeedle historical answer'),
            ], [])
        node = SimpleNamespace(name='Synthetic', nid='a' * 32, port=free_port(), token=TOKEN)
        (data.root / 'ids').mkdir()
        (data.root / 'ids/node-id').write_text(node.nid)
        state = data.root / 'state'; state.mkdir()
        base, _ = stack.enter_context(isolated_server(data, args.binary, state_dir=state,
            audit_dir=data.root / 'audit', extra_env=node_env(data.root, node.port, '127.0.0.0/8')))
        hubroot = root / 'hub'; hubroot.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [node])
        hub.start(); stack.callback(hub.stop)
        pw = stack.enter_context(sync_playwright())
        browser = launch_chromium(pw); stack.callback(browser.close)
        for name, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
            for width, height, theme in [(1280, 900, 'light'), (675, 515, 'dark')]:
                context = browser.new_context(viewport={'width': width, 'height': height}, color_scheme=theme)
                context.add_init_script("""const originalFetch = window.fetch;
                  window.fetch = async (...args) => {
                    const response = await originalFetch(...args);
                    if (String(args[0]).includes('api/search?')) {
                      window.searchWire = response.clone().text();
                      window.searchTraceHeader = new Headers(args[1].headers).get('X-SessionDock-Trace');
                    }
                    return response;
                  };""")
                page = context.new_page(); errors = []; receipts = []
                page.on('request', lambda r, target=receipts: target.extend((r.post_data_json or {}).get('events', [])) if '/api/audit/browser' in r.url else None)
                page.on('pageerror', lambda e: errors.append(str(e)))
                page.goto(url, wait_until='networkidle')
                page.wait_for_function('S.sessions.length === 165')
                page.locator('#livecount').click()
                expect(page.locator('#side .item')).to_have_count(0)
                page.locator('#q').fill('ScopeNeedle')
                page.locator('#q').press('Enter')
                expect(page.locator('#stat')).to_contain_text('全文命中 165 个会话', timeout=30000)
                assert page.evaluate('S.activeOnly') is False
                expect(page.locator('#side .item[data-uid]')).to_have_count(165)
                expect(page.locator('#side-search-count')).to_have_text('165 条')
                stream = [json.loads(line) for line in page.evaluate('window.searchWire').splitlines()]
                result = next(e['data'] for e in stream if e['type'] == 'result')
                assert len(result['results']) == 165 and not result['truncated']
                assert len([e for e in stream if e['type'] == 'progress']) < 165
                assert page.evaluate('window.searchTraceHeader')
                page.evaluate('flushBrowserAudit()')
                search_receipts = [e for e in receipts if e.get('event', '').startswith('search.')]
                completed = next(e for e in search_receipts if e['event'] == 'search.completed')
                assert completed['data']['returned'] == completed['data']['visible'] == 165
                assert completed['trace_id'] == page.evaluate('window.searchTraceHeader')
                assert 'ScopeNeedle' not in json.dumps(search_receipts)
                if args.screenshots:
                    args.screenshots.mkdir(parents=True, exist_ok=True)
                    page.screenshot(path=str(args.screenshots / f'{name}-{width}-{theme}.png'), full_page=True)
                last = page.locator('#side .item[data-uid]').last
                last.locator('.t').click()
                expect(page.locator('#msgs')).to_contain_text('ScopeNeedle historical answer')
                assert not errors, errors
                print(f'PASS {name} {width} {theme}: 165 historical hits, no truncation, progress coalesced, open last hit', flush=True)
                context.close()


if __name__ == '__main__':
    main()
