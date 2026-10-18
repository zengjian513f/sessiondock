#!/usr/bin/env python3
"""Switch small histories while a private collector returns boot-long SSH evidence."""
# run_validation: serial
import argparse
from contextlib import contextmanager
import json
from pathlib import Path
import socketserver
import tempfile
import threading
import time

import psutil
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
from node_auth_fixtures import node_env, free_port

NODE = 'a' * 32
COUNT = 100_000


@contextmanager
def collector(root):
    session = dict(node_id=NODE, source='codex', sid='small-0', title='Synthetic owner ' * 12)
    record = dict(process=dict(pid=200, start=20000),
        connection=dict(client_ip='10.0.0.1', client_port=50000, server_ip='10.0.0.2', server_port=50022),
        opened_at=1., closed_at=2., ssh=True, shared=False, session=session,
        launch_chain=[dict(process=dict(node_id=NODE, boot_id='private-boot', process=dict(pid=100, start=10000)),
                           session=session)])
    binding = dict(process=dict(pid=100, start=10000), session=session, first_observed_at=1., launch_chain=[])
    report = dict(version=1, node_id=NODE, boot_id='private-boot', supported=True,
                  sampled_at=3., outgoing=[], incoming=[], bindings=[binding], connections=[])
    # The real report was 71.8 MB / 103k records; keep this shape synthetic.
    prefix = json.dumps(dict(ok=True, result=report), separators=(',', ':')).encode()
    payload = prefix.replace(b'"connections":[]', b'"connections":[' +
        b','.join([json.dumps(record, separators=(',', ':')).encode()] * COUNT) + b']') + b'\n'
    calls = []
    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            try:
                request = json.loads(self.rfile.readline())
                calls.append(request['op'])
                if request['op'] in ('report', 'catalog', 'publish'):
                    self.wfile.write(payload)
                else:
                    self.wfile.write(json.dumps(dict(ok=True, result=dict(version=1, node_id=NODE,
                        boot_id='private-boot', availability='unavailable', samples=[]))).encode() + b'\n')
            except (BrokenPipeError, ConnectionResetError):
                pass
    class Server(socketserver.ThreadingUnixStreamServer):
        daemon_threads = True
    with Server(str(root / 'collector.sock'), Handler) as server:
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            yield len(payload), calls
        finally:
            server.shutdown()
            worker.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--report-only', action='store_true')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='history-collector-') as temporary:
        root = Path(temporary)
        corpus = Corpus(root)
        for i in range(4):
            sid = f'small-{i}'
            corpus.put(sid, 'codex', [codex_row('session_meta', dict(id=sid, cwd='/synthetic/collector')),
                codex_message('user', f'History {i} question'), codex_message('assistant', f'History {i} answer')], [])
        proc = root / 'proc'
        (proc / 'sys/kernel/random').mkdir(parents=True)
        (proc / 'sys/kernel/random/boot_id').write_text('private-boot')
        (proc / 'stat').write_text('btime 1700000000\n')
        (root / 'ids').mkdir()
        (root / 'ids/node-id').write_text(NODE)
        env = node_env(root, free_port(), '127.0.0.0/8')
        env.update(SESSIONDOCK_PROC_ROOT=str(proc), SESSIONDOCK_RESOURCE_AGENT_SOCKET=str(root / 'collector.sock'),
                   SESSIONDOCK_LOG_REQUESTS='all')
        with collector(root) as (size, calls), isolated_server(corpus, args.binary, extra_env=env) as (base, _):
            print(f'Collector response: {size} bytes; opening page', flush=True)
            process = next(p for p in psutil.Process().children() if p.cmdline() == [str(args.binary.resolve())])
            with sync_playwright() as pw:
                browser = pw.chromium.launch(headless=True)
                try:
                    page = browser.new_page()
                    errors = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    page.goto(base, wait_until='domcontentloaded')
                    expect(page.locator('#side .item')).to_have_count(4)
                    times = []
                    for i in range(4):
                        # Three ordinary liveness consumers overlap a user opening
                        # a cold history. Force bypasses the unchanged-list cache.
                        page.evaluate('''() => { window.liveReads = Promise.all(Array.from({length:3},
                            () => fetch('api/live?force=1').then(r => {if(!r.ok) throw Error(r.status); return r.json()}))); }''')
                        started = time.monotonic()
                        page.locator(f'#side .item[data-uid="{corpus.uid(f"small-{i}")}"]').click()
                        expect(page.locator('#msgs')).to_contain_text(f'History {i} answer')
                        times.append(round((time.monotonic() - started) * 1000))
                        page.evaluate('() => window.liveReads')
                        print(f'Cold history {i}: {times[-1]} ms', flush=True)
                    # The coordinator path must still receive all old evidence;
                    # optimizing liveness cannot prune or hide connection records.
                    full = page.evaluate('''async () => {const r=await fetch('api/process-links');
                        if(!r.ok) throw Error(r.status); const v=await r.json();
                        return {count:v.connections.length, first:v.connections[0].opened_at,
                            last:v.connections.at(-1).session.sid, bindings:v.bindings.length};}''')
                    assert full == dict(count=COUNT, first=1, last='small-0', bindings=1), full
                    hwm = next(int(line.split()[1]) for line in Path(f'/proc/{process.pid}/status').read_text().splitlines()
                               if line.startswith('VmHWM:')) / 1024
                    print(json.dumps(dict(collector_bytes=size, cold_open_ms=times, peak_rss_mib=round(hwm),
                                          requests=len(calls), complete_connections=full['count'])), flush=True)
                    assert calls.count('catalog') >= 1 and calls.count('report') >= 3, calls
                    assert not errors, errors
                    if not args.report_only:
                        assert hwm < 800, f'Collector JSON expanded into excessive heaps: {hwm:.0f} MiB'
                        assert max(times) < 2000, times
                finally:
                    browser.close()
    print('PASS history opens under boot-long collector reports; full connection evidence retained', flush=True)


if __name__ == '__main__':
    main()
