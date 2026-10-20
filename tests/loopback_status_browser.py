#!/usr/bin/env python3
"""Refresh live status with a refused TCP host beside a healthy delayed host.

Runs on Windows and Linux; only private records, loopback sockets and synthetic
native histories. Windows exercises its per-socket SYN retry configuration.
"""
import argparse
from contextlib import contextmanager, ExitStack
import json
import os
from pathlib import Path
import socket
import socketserver
import tempfile
import threading
import time

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server


@contextmanager
def endpoint(record):
    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            try:
                request = json.loads(self.rfile.readline())
                if request.get('token') != record['token']:
                    self.wfile.write(b'{"ok":false}\n')
                    return
                # A healthy peer may take time to answer Info. Connection tuning
                # must not shorten the established transport's operation budget.
                time.sleep(.3)
                response = dict(ok=True, info=record, exited=False)
                self.wfile.write(json.dumps(response).encode() + b'\n')
            except (OSError, ValueError):
                pass
    class Server(socketserver.ThreadingTCPServer):
        allow_reuse_address = True
        daemon_threads = True
    with Server(('127.0.0.1', record['port']), Handler) as server:
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            yield
        finally:
            server.shutdown()
            worker.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--report-only', action='store_true')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='loopback-status-') as temporary, ExitStack() as stack:
        root = Path(temporary)
        corpus = Corpus(root)
        hosts = root / 'hosts'
        hosts.mkdir()
        (root / 'state').mkdir()
        records = []
        originals = {}
        for sid in ('healthy', 'refused'):
            corpus.put(sid, 'codex', [codex_row('session_meta', dict(id=sid, cwd=str(root))),
                codex_message('user', f'Synthetic {sid} history')], [])
            with socket.socket() as reservation:
                reservation.bind(('127.0.0.1', 0))
                port = reservation.getsockname()[1]
            record = dict(name=sid, host_pid=os.getpid(), pid=os.getpid(), created=int(time.time()),
                cwd=str(root), cmd='synthetic-host', cols=80, rows=24, port=port,
                token='private-fixture-token', meta=dict(source='codex', sid=sid, uid=corpus.uid(sid),
                    instance_id='synthetic-instance-' + sid))
            path = hosts / (sid + '.json')
            path.write_text(json.dumps(record), encoding='utf-8')
            originals[path] = path.read_bytes()
            records.append(record)
        stack.enter_context(endpoint(records[0]))
        base, _ = stack.enter_context(isolated_server(corpus, args.binary, host_dir=hosts,
            state_dir=root / 'state'))
        pw = stack.enter_context(sync_playwright())
        options = dict(headless=True)
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        stack.callback(browser.close)
        page = browser.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.goto(base, wait_until='domcontentloaded')
        times = []
        for _ in range(2):
            page.reload(wait_until='domcontentloaded')
            page.locator(f'#side .item[data-uid="{corpus.uid("healthy")}"]').click()
            expect(page.locator('#msgs')).to_contain_text('Synthetic healthy history')
            result = page.evaluate('''async () => {
                const start = performance.now();
                const response = await fetch('api/live?force=1');
                if (!response.ok) throw Error(response.status);
                return {data: await response.json(), ms: performance.now() - start};
            }''')
            live = result['data']
            times.append(round(result['ms']))
            assert corpus.uid('healthy') in live['uids'], live
            assert corpus.uid('refused') not in live['uids'], live
            refused = next(h for h in live['managed']['hosts'] if h['summary']['name'] == 'refused')
            assert refused['known'] is False, refused
            assert all(path.read_bytes() == content for path, content in originals.items())
        print('Refreshed live query ms:', times, flush=True)
        # The same previously refused endpoint can recover without rewriting its
        # record or switching sessions; no negative cache or guessed exit.
        stack.enter_context(endpoint(records[1]))
        recovered = page.evaluate("async () => (await fetch('api/live?force=1')).json()")
        assert corpus.uid('refused') in recovered['uids'], recovered
        assert all(path.read_bytes() == content for path, content in originals.items())
        assert not errors, errors
        if not args.report_only:
            assert max(times) < 1500, times
        print('PASS refused host stays unknown, healthy delayed peer and recovered peer stay visible; records unchanged', flush=True)


if __name__ == '__main__':
    main()
