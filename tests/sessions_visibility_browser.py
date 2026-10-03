#!/usr/bin/env python3
"""Chromium: obsolete test registries/URLs never hide ordinary node or Hub sessions.

All histories and state are synthetic and temporary; no CLI is launched.
"""
from browser_runtime import js
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode, urlsplit

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, claude_row, isolated_server
from hub_http_suite import Hub, free_port
from node_auth_suite import TOKEN, node_env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-visibility-') as tmp, sync_playwright() as pw:
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**launch)
        try:
            for hub_mode in (False, True):
                with ExitStack() as stack:
                    root = Path(tmp) / ('hub' if hub_mode else 'node')
                    corpus = Corpus(root / 'data')
                    for source in ('claude', 'codex', 'grok'):
                        (corpus.root / source).mkdir(parents=True)
                    # Both former matching paths (uid and cwd) plus an ordinary row.
                    for sid in ('by-id', 'by-cwd', 'ordinary'):
                        cwd = '/synthetic/old-run/work' if sid == 'by-cwd' else '/synthetic/plain'
                        corpus.put(sid, 'claude', [
                            claude_row(sid, 'user', 'u0', None, f'visibilityneedle {sid}', cwd=cwd),
                            claude_row(sid, 'assistant', 'a0', 'u0', f'answer {sid}', cwd=cwd),
                        ], [])
                    state = corpus.root / 'state'; state.mkdir(mode=0o700)
                    registry = state / 'debug-runs.json'
                    registry.write_text(json.dumps({'version': 1, 'runs': {'old-run': {
                        'root': '/synthetic/old-run', 'sessions': [{'uid': corpus.uid('by-id')}],
                    }}}))
                    registry.chmod(0o600)
                    original = registry.read_bytes()
                    nid = 'a' * 32
                    ids = corpus.root / 'ids'; ids.mkdir()
                    (ids / 'node-id').write_text(nid + '\n')
                    port = free_port()
                    base, _ = stack.enter_context(isolated_server(corpus, args.binary, state_dir=state,
                        extra_env=node_env(corpus.root, port, '127.0.0.0/8')))
                    if hub_mode:
                        hubroot = root / 'hub'; hubroot.mkdir()
                        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot,
                            [SimpleNamespace(nid=nid, name='synthetic', port=port, token=TOKEN)])
                        hub.start(); stack.callback(hub.stop)
                        base = f'http://127.0.0.1:{hub.port}'
                    context = browser.new_context(viewport={'width': 1280, 'height': 900})
                    stack.callback(context.close)
                    context.route('**/*', lambda route: route.continue_()
                        if route.request.url.startswith(base + '/') else route.abort())
                    page = context.new_page(); errors = []; requests = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    page.on('request', lambda request: requests.append(request.url))
                    def document(path):
                        response = context.request.get(base + path)
                        assert response.ok, (path, response.status, response.text())
                        return response.json()
                    baseline = document('/api/sessions')
                    uids = {row['uid'] for row in baseline['sessions']}
                    assert len(uids) == 3, baseline
                    for value in ('', 'old-run', 'unknown', '../bad', 'x' * 65):
                        baseline = document('/api/sessions')
                        suffix = '?' + urlencode({'debug_run': value}) if value else ''
                        result = document('/api/sessions' + suffix)
                        assert {row['uid'] for row in result['sessions']} == uids
                        assert result['sig'] == baseline['sig']
                        unchanged = document('/api/sessions?' + urlencode({'debug_run': value, 'sig': baseline['sig']}))
                        assert unchanged.get('unchanged') is True, unchanged
                        found = document('/api/search?' + urlencode({'q': 'visibilityneedle', 'debug_run': value}))
                        assert {row['uid'] for row in found['results']} == uids, found
                        assert found['total_pool'] == 3, found
                        page.goto(base + '/' + suffix, wait_until='domcontentloaded')
                        expect(page.locator('#side .item[data-uid]')).to_have_count(3)
                        page.wait_for_function(js('uiEventsReady', 'runtime.core.events.ready'), timeout=30000)
                        for sid in ('by-id', 'by-cwd', 'ordinary'):
                            page.locator('#side .item[data-uid]').filter(has_text=f'visibilityneedle {sid}').click()
                            expect(page.locator('#msgs')).to_contain_text(f'answer {sid}')
                        old = page.locator('#stat').get_attribute('data-seq') or ''
                        page.locator('#q').fill('visibilityneedle')
                        page.locator('#q').press('Enter')
                        page.wait_for_function("old => String(document.querySelector('#stat').dataset.seq || '') !== old", arg=old)
                        expect(page.locator('#search-progress')).not_to_be_visible()
                        expect(page.locator('#side .item[data-uid]')).to_have_count(3)
                        page.locator('#side .item[data-uid]').filter(has_text='by-id').click()
                        expect(page.locator('#msgs')).to_contain_text('answer by-id')
                    assert not any('debug_run=' in urlsplit(url).query for url in requests
                        if urlsplit(url).path.startswith('/api/')), requests
                    assert registry.read_bytes() == original
                    # Registry changes must not change list contents or signature.
                    baseline = document('/api/sessions')
                    registry.write_text('{obsolete and damaged')
                    after = document('/api/sessions?force=1&debug_run=old-run')
                    assert after['sig'] == baseline['sig']
                    assert {row['uid'] for row in after['sessions']} == uids
                    page.reload(wait_until='domcontentloaded')
                    expect(page.locator('#side .item[data-uid]')).to_have_count(3)
                    page.wait_for_function(js('uiEventsReady', 'runtime.core.events.ready'), timeout=30000)
                    corpus.put('added', 'claude', [claude_row('added', 'user', 'u0', None,
                        'visibilityneedle added', cwd='/synthetic/old-run/work')], [])
                    expect(page.locator('#side .item[data-uid]')).to_have_count(4, timeout=15000)
                    page.locator('#side .item[data-uid]').filter(has_text='visibilityneedle added').click()
                    expect(page.locator('#msgs')).to_contain_text('visibilityneedle added')
                    assert not errors, errors
                    print(f'PASS {"hub" if hub_mode else "node"}: registry and obsolete URLs preserve list, signature, opens, search and SSE', flush=True)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
