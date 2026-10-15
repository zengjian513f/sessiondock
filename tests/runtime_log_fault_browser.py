#!/usr/bin/env python3
"""Real browser navigation remains usable with a blocked or failing stderr sink."""
import argparse
import json
import os
from pathlib import Path
import tempfile
import time
import threading
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, build_corpus, isolated_server, get_json
from diagnostic_retention_browser import until
from browser_evidence_fixtures import run_with_evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--mode', choices=['normal', 'failed', 'blocked', 'recovered'])
    parser.add_argument('--output', type=Path, default=Path('target/runtime-log-fault'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    evidence = {}
    with tempfile.TemporaryDirectory(prefix='sessiondock-runtime-log-') as tmp, sync_playwright() as pw:
        corpus = build_corpus(Path(tmp))
        browser = pw.chromium.launch(headless=True)
        try:
            for mode in ([args.mode] if args.mode else ['normal', 'failed', 'blocked', 'recovered']):
                read_fd = write_fd = None
                sink = None
                reader = None
                reader_stop = threading.Event()
                captured = bytearray()
                if mode == 'recovered':
                    fifo = corpus.root/'recoverable-stderr'
                    os.mkfifo(fifo)
                    read_fd = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
                    write_fd = os.open(fifo, os.O_WRONLY | os.O_NONBLOCK)
                    os.close(read_fd); read_fd = None
                    sink = write_fd
                elif mode == 'blocked':
                    import fcntl
                    read_fd, write_fd = os.pipe()
                    capacity = fcntl.fcntl(write_fd, fcntl.F_SETPIPE_SZ, 4096)
                    os.write(write_fd, b'x' * capacity)
                    sink = write_fd
                elif mode == 'failed':
                    sink = os.open('/dev/full', os.O_WRONLY)
                log = args.output/f'{mode}-server.jsonl'
                try:
                    with isolated_server(corpus, args.binary, log_path=log, stderr_sink=sink,
                            extra_env={'SESSIONDOCK_LOG_REQUESTS':'all'}) as (base, opener):
                        page = browser.new_page()
                        page.goto(base)
                        times = []
                        samples = 650 if mode == 'blocked' else 30
                        for i in range(samples):
                            sid = 'claude-branch' if i % 2 else 'codex-fork'
                            tick = time.monotonic()
                            page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                            expect(page.locator('#msgs')).to_contain_text('Claude selected answer' if i % 2 else 'Codex fork answer')
                            times.append((time.monotonic()-tick)*1000)
                        if mode == 'failed':
                            until(page, lambda:get_json(opener,base,'/api/health')['runtime_log']['write_errors'] > 0)
                        if mode == 'recovered':
                            # Leave the sink broken across the real 60s summary
                            # interval, then recover it without restarting the node.
                            page.wait_for_timeout(61000)
                            before = get_json(opener,base,'/api/health')['runtime_log']
                            assert before['write_errors'] > 0
                            read_fd = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
                            def drain():
                                while not reader_stop.wait(.01):
                                    try: captured.extend(os.read(read_fd, 65536))
                                    except BlockingIOError: pass
                            reader = threading.Thread(target=drain, daemon=True); reader.start()
                            page.locator(f'#side .item[data-uid="{corpus.uid("codex-fork")}"]').click()
                            expect(page.locator('#msgs')).to_contain_text('Codex fork answer')
                            def summaries():
                                return [r for line in bytes(captured).splitlines() if line.endswith(b'}')
                                    for r in [json.loads(line)] if r.get('event') == 'log.delivery']
                            until(page, lambda: bool(summaries()), seconds=70)
                            summary = summaries()[-1]
                            assert summary['write_errors'] >= before['write_errors'], (summary,before)
                            evidence['summary_recovery'] = {'before':before,'summary':summary,
                                'failed_summary_retained':True}
                        health = get_json(opener, base, '/api/health')['runtime_log']
                        assert health['queued_bytes'] <= 8*1024*1024
                        if mode == 'blocked':
                            assert health['queued_bytes'] > 0 and health['written'] == 0
                            assert health['dropped'] > 0, health
                        page.screenshot(path=str(args.output/f'{mode}.png'))
                        page.close()
                        stop = time.monotonic()
                    elapsed = time.monotonic()-stop
                    assert elapsed < 4, (mode, elapsed)
                    evidence[mode] = {'health':health,'shutdown_seconds':elapsed,
                        'p50_ms':sorted(times)[len(times)//2], 'p95_ms':sorted(times)[int(len(times)*.95)], 'samples':len(times)}
                    if mode == 'normal':
                        logs = [json.loads(line) for line in log.read_text().splitlines()]
                        assert logs[0]['event'] == 'server.listening'
                        assert any(r['event']=='http.request' and r.get('page') and r.get('trace') for r in logs)
                        assert all('ts' in r for r in logs)
                        evidence[mode].update(events=len(logs), bytes=log.stat().st_size, flush_verified=True)
                    print('PASS runtime log',mode,evidence[mode],flush=True)
                finally:
                    reader_stop.set()
                    if reader: reader.join(timeout=1)
                    if captured: log.write_bytes(captured)
                    if read_fd is not None: os.close(read_fd)
                    if write_fd is not None: os.close(write_fd)
                    elif sink is not None: os.close(sink)
        finally: browser.close()
    (args.output/'result.json').write_text(json.dumps(evidence,indent=2)+'\n')


if __name__ == '__main__':
    run_with_evidence(main, 'target/runtime-log-fault')
