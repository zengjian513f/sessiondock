#!/usr/bin/env python3
"""Real console typing and private shell receipts measure bounded terminal logs."""
import argparse
import json
from pathlib import Path
import statistics
import tempfile
import time
import uuid

from playwright.sync_api import sync_playwright
from audit_browser import audit_lines
from history_fixtures import BINARY, Corpus, codex_message, codex_row, isolated_server
import terminal_input_browser as fixture


def run(browser, binary, output, enabled):
    with tempfile.TemporaryDirectory(prefix='sessiondock-log-volume-') as tmp:
        root = Path(tmp)
        for name in ['host', 'work', 'claude', 'codex', 'grok', 'audit']:
            (root/name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root/'work')}),
                                 codex_message('user', 'Synthetic console')], [])
        uid = corpus.uid(sid)
        native = corpus.paths[sid].read_bytes()
        with fixture.host(root, 'synthetic-'+uuid.uuid4().hex, uid), isolated_server(
                corpus, binary, host_dir=root/'host', audit_dir=root/'audit' if enabled else None) as (base, _):
            context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
            context.route(lambda url: not url.startswith(base+'/'), lambda r:r.abort())
            page = context.new_page()
            page.goto(base, wait_until='networkidle')
            fixture.open_console(page, uid)
            page.locator('#termpane canvas').first.click()
            times = []
            for i in range(20):
                start = time.perf_counter()
                page.keyboard.type(f'volume-{i:02d}')
                page.keyboard.press('Enter')
                fixture.xterm_contains(page, f'ACK_volume-{i:02d}')
                times.append((time.perf_counter()-start)*1000)
            assert (root/'work/input.log').read_text().splitlines() == [f'volume-{i:02d}' for i in range(20)]
            page.screenshot(path=str(output/f'console-{enabled}.png'))
            # Real navigation flushes the last partial summary via pagehide.
            page.goto(base, wait_until='networkidle')
            page.wait_for_timeout(6000)
            logs = audit_lines(root/'audit')
            selected = [r for r in logs if r['event'] in {'browser.terminal.input', 'browser.terminal.frame.sent'}]
            if enabled:
                assert selected and all(r['data'].get('summary') for r in selected), selected
                inputs = [r for r in selected if r['event']=='browser.terminal.input']
                frames = [r for r in selected if r['event']=='browser.terminal.frame.sent' and r['data']['kind']=='input']
                assert sum(r['data']['count'] for r in inputs) >= 200, inputs
                assert sum(r['data']['bytes'] for r in inputs) == 200, inputs
                assert sum(r['data']['bytes'] for r in frames) == 200, frames
                assert len(selected) < 20, len(selected)
                assert all(r['page_id'] and r['connection_id'] and r['data']['elapsed_ms'] >= 0 for r in selected)
                assert not any(r['event']=='browser.terminal.heartbeat.pong' and r['data'].get('matched') for r in logs)
                assert 'volume-00' not in json.dumps(logs) and 'ACK_volume' not in json.dumps(logs)
            else:
                assert not logs
            assert corpus.paths[sid].read_bytes() == native
            context.close()
            return {'samples':len(times), 'p50_ms':statistics.median(times),
                    'p95_ms':sorted(times)[18], 'events':len(logs),
                    'bytes':sum(p.stat().st_size for p in (root/'audit').glob('*.jsonl')),
                    'input_frame_summary_events':len(selected), 'verified_shell_commands':20}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--output',type=Path,default=Path('target/terminal-log-volume'))
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    fixture.SHELL = """stty -echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
 printf '%s\\n' "$command" >> input.log
 printf 'ACK_%s\\n' "$command"
done
"""
    with sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            result={str(enabled):run(browser,args.binary,args.output,enabled) for enabled in [False,True]}
            (args.output/'result.json').write_text(json.dumps(result,indent=2))
            print('PASS real terminal typing, independent shell receipts, pagehide summaries and on/off timings',result,flush=True)
        finally:browser.close()


if __name__=='__main__':main()
