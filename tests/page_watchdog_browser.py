#!/usr/bin/env python3
"""Persist an unresponsive page's receipt before allowing its handler to return."""
from contextlib import ExitStack
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import tempfile
import threading
import time
from types import SimpleNamespace

from playwright.sync_api import sync_playwright, expect
from audit_browser import audit_lines
from history_fixtures import BINARY, build_corpus, isolated_server
from hub_fixtures import Hub, scoped
from node_auth_fixtures import TOKEN, free_port, node_env


def receipt(directory, page_id, suffix):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        for row in audit_lines(directory):
            if row['page_id'] == page_id and row['event'] == 'browser.watchdog.' + suffix:
                return row
        time.sleep(.05)
    raise AssertionError(f'missing {suffix} for {page_id}')


def main():
    with tempfile.TemporaryDirectory(prefix='sd-watchdog-') as tmp, ExitStack() as stack:
        root = Path(tmp)
        corpus = build_corpus(root)
        directory = root / 'audit'
        directory.mkdir()
        port = free_port()
        env = node_env(root, port, '127.0.0.1/32')
        base, _ = stack.enter_context(isolated_server(corpus, BINARY, audit_dir=directory, extra_env=env))
        hubroot = root / 'hub'
        hubroot.mkdir()
        hub = Hub(BINARY.resolve().with_name('sessiondock-hub'), hubroot,
                  [SimpleNamespace(name='Probe', token=TOKEN, port=port)])
        hub.start()
        stack.callback(hub.stop)
        hub_base = f'http://127.0.0.1:{hub.port}'
        release, entered = threading.Event(), threading.Event()

        class Gate(BaseHTTPRequestHandler):
            def do_GET(self):
                entered.set()
                release.wait(20)
                self.send_response(200)
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()

            def log_message(self, *args):
                pass

        gate = ThreadingHTTPServer(('127.0.0.1', 0), Gate)
        threading.Thread(target=gate.serve_forever, daemon=True).start()
        stack.callback(gate.server_close)
        stack.callback(gate.shutdown)
        stack.callback(release.set)
        pw = stack.enter_context(sync_playwright())
        browser = pw.chromium.launch(headless=True)
        stack.callback(browser.close)
        uid = corpus.uid('claude-branch')
        nid = (root / 'ids' / 'node-id').read_text().strip()
        for url, selected in [(base, uid), (hub_base, scoped(nid, uid))]:
            release.clear()
            entered.clear()
            page = browser.new_page()
            page.goto(url)
            page.locator(f'#side .item[data-uid="{selected}"]').click()
            expect(page.locator('#msgs')).to_be_visible()
            page_id = page.evaluate('window.__sessiondockPageId')
            page.locator('#settings').evaluate("""(button, url) => {
              button.addEventListener('click', () => setTimeout(() => {
                const request = new XMLHttpRequest();
                request.open('GET', url, false);
                request.send();
              }, 100), {once:true});
            }""", f'http://127.0.0.1:{gate.server_port}/hold')
            page.locator('#settings').click()
            assert entered.wait(3), 'click did not enter the blocked handler'
            try:
                stalled = receipt(directory, page_id, 'stalled')
                data = stalled['data']
                assert stalled['uid'] == uid and data['missed'] == 'task', stalled
                assert data['task_gap_ms'] >= 5000 and data['surface'] == 'index', data
                assert any(p['event'] == 'click' and p['control'] == 'settings'
                           and p['delay_ms'] >= 0 for p in data['points']), data
            finally:
                release.set()
            recovered = receipt(directory, page_id, 'recovered')
            assert recovered['data']['incident'] == data['incident']
            expect(page.locator('#settings-dialog')).to_be_visible()
            page.close()

        # A standalone Hub page with no selected UID still needs a storage destination.
        page = browser.new_page()
        page.goto(hub_base + '/quotas.html')
        page_id = page.evaluate('window.__sessiondockPageId')
        page.evaluate("""() => {
          const raf = window.requestAnimationFrame;
          window.requestAnimationFrame = callback => setTimeout(() => raf(callback), 8500);
        }""")
        stalled = receipt(directory, page_id, 'stalled')['data']
        assert stalled['surface'] == 'quotas' and stalled['missed'] == 'animation_frame', stalled
        assert stalled['task_gap_ms'] < 3000 and stalled['frame_gap_ms'] >= 5000, stalled
        assert receipt(directory, page_id, 'recovered')['data']['incident'] == stalled['incident']
        for name in ['file', 'files', 'session-info']:
            page.goto(base + '/' + name + '.html')
            assert page.evaluate('!!globalThis.SessionDockWatchdog'), name
        print('PASS page watchdog: node and Hub blocked handler, persisted before release, '
              'recovery, standalone Hub frame stall and all HTML entry points')


if __name__ == '__main__':
    main()
