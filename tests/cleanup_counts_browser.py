#!/usr/bin/env python3
"""Chromium cleanup badges: real node/Hub timers, fresh dialogs and color boundaries.

Native files and /proc are synthetic and private; no CLI is launched or stopped.
The timer override allows checking background updates with every page closed.
"""
import argparse
from contextlib import ExitStack
import os
import re
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
from hub_fixtures import Hub
from machine_controls_browser import launch_chromium
from node_auth_fixtures import node_env, TOKEN, free_port
from spawned_by_fixtures import proc_pid


def main(binary):
    with tempfile.TemporaryDirectory(prefix='sessiondock-cleanup-counts-') as temporary, ExitStack() as stack:
        root = Path(temporary)
        corpus = Corpus(root / 'node')
        corpus.put('old', 'codex', [codex_row('session_meta', {'id': 'old', 'cwd': '/synthetic/work'}),
                                  codex_message('user', 'Synthetic cleanup counter')], [])
        old = time.time() - 3 * 86400
        os.utime(corpus.paths['old'], (old, old))
        proc = corpus.root / 'proc'
        proc.mkdir()
        (proc / 'stat').write_text('btime 1700000000\n')
        proc_pid(proc, 100, 'codex', ['codex'], 1, fds={3: str(corpus.paths['old'])})
        (corpus.root / 'ids').mkdir()
        nid = 'a' * 32
        (corpus.root / 'ids/node-id').write_text(nid)
        node = SimpleNamespace(name='Synthetic', nid=nid, port=free_port(), token=TOKEN)
        env = node_env(corpus.root, node.port, '127.0.0.0/8')
        env.update(SESSIONDOCK_PROC_ROOT=str(proc), SESSIONDOCK_CLEANUP_INTERVAL_SECS='2')
        base, _ = stack.enter_context(isolated_server(corpus, binary, extra_env=env))
        hubroot = root / 'hub'
        hubroot.mkdir()
        hub = Hub(binary.resolve().with_name('sessiondock-hub'), hubroot, [node])
        hub.env['SESSIONDOCK_CLEANUP_INTERVAL_SECS'] = '2'
        hub.start()
        stack.callback(hub.stop)
        with sync_playwright() as pw:
            browser = launch_chromium(pw)
            urls = [base, f'http://127.0.0.1:{hub.port}']
            errors = []
            for url in urls:
                context = browser.new_context(viewport={'width': 1800, 'height': 900}, service_workers='block')
                page = context.new_page()
                page.on('pageerror', lambda e: errors.append(str(e)))
                page.goto(url, wait_until='networkidle')
                expect(page.locator('#session-cleanup-count')).to_have_text('1', timeout=15000)
                summary = page.request.get(url + '/api/sessions/cleanup-counts').json()
                assert summary['interval_seconds'] == 2 and summary['ready'], summary
                assert set(summary) == {'ready', 'checked_at', 'days', 'partial', 'error', 'interval_seconds', 'next_check_at'}, summary
                page.locator('#session-cleanup').click()
                expect(page.locator('#session-cleanup-status')).to_contain_text('找到 1 个')
                page.locator('#session-cleanup-close').click()
                expect(page.locator('#session-cleanup-list')).to_be_empty()
                context.close()

            # Both services keep checking after all browser pages have closed.
            os.utime(corpus.paths['old'], None)
            time.sleep(5)
            before_open = time.time() * 1000
            for url in urls:
                context = browser.new_context(viewport={'width': 1800, 'height': 900}, service_workers='block')
                page = context.new_page()
                page.on('pageerror', lambda e: errors.append(str(e)))
                summary = page.request.get(url + '/api/sessions/cleanup-counts').json()
                assert sum(summary['days'].values()) == 0, summary
                assert before_open - 5000 <= summary['checked_at'] <= time.time() * 1000, summary
                page.goto(url, wait_until='networkidle')
                expect(page.locator('#session-cleanup-count')).to_have_text('0')
                summary = page.request.get(url + '/api/sessions/cleanup-counts').json()
                assert sum(summary['days'].values()) == 0, summary
                # An open page picks up the next numeric summary without opening the dialog.
                os.utime(corpus.paths['old'], (old, old))
                expect(page.locator('#session-cleanup-count')).to_have_text('1', timeout=15000)
                os.utime(corpus.paths['old'], None)
                expect(page.locator('#session-cleanup-count')).to_have_text('0', timeout=15000)
                context.close()

            context = browser.new_context(viewport={'width': 1800, 'height': 900}, service_workers='block')
            page = context.new_page()
            page.on('pageerror', lambda e: errors.append(str(e)))
            count = 30
            requests = []
            def summary_response(route):
                requests.append(route.request.url)
                route.fulfill(json={'ready': True, 'days': {'2': count}, 'checked_at': int(time.time() * 1000),
                                    'next_check_at': int(time.time() * 1000) + 10800000, 'interval_seconds': 10800})
            context.route('**/api/sessions/cleanup-counts', summary_response)
            for count, color in [(30, ''), (31, 'warn'), (100, 'warn'), (101, 'danger')]:
                page.goto(base, wait_until='networkidle')
                badge = page.locator('#session-cleanup-count')
                expect(badge).to_have_text(str(count))
                button = page.locator('#session-cleanup')
                assert page.evaluate("document.querySelector('#session-cleanup').classList.contains('cleanup-warn')") == (color == 'warn')
                assert page.evaluate("document.querySelector('#session-cleanup').classList.contains('cleanup-danger')") == (color == 'danger')
                expect(button).to_have_attribute('aria-label', f'清扫会话，{count} 个')
            assert len(requests) == 4, requests
            page.locator('#session-cleanup').click()
            expect(page.locator('#session-cleanup-status')).to_contain_text('没有超过 2 天')
            expect(page.locator('#session-cleanup-count')).to_have_text('0')
            page.locator('#session-cleanup-close').click()
            # A late summary from before the dialog cannot overwrite its fresh result.
            context.unroute('**/api/sessions/cleanup-counts', summary_response)
            context.route('**/api/sessions/cleanup-counts', lambda route: route.fulfill(json={
                'ready': True, 'days': {'2': 101}, 'checked_at': 1, 'next_check_at': int(time.time() * 1000) + 10800000}))
            page.evaluate('refreshCleanupCount()')
            expect(page.locator('#session-cleanup-count')).to_have_text('0')
            context.unroute('**/api/sessions/cleanup-counts')
            context.route('**/api/sessions/cleanup-counts', lambda route: route.fulfill(status=503, json={'error': 'synthetic'}))
            page.reload(wait_until='networkidle')
            expect(page.locator('#session-cleanup-count')).to_have_text('?')
            expect(page.locator('#session-cleanup')).to_have_attribute('title', re.compile('更新失败'))
            assert not errors, errors
            context.close()
            browser.close()
    print('PASS cleanup counts: node/Hub background timers, numeric summaries, color boundaries, fresh dialog and failure state')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    main(parser.parse_args().binary)
