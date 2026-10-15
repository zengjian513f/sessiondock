#!/usr/bin/env python3
"""A blocked private audit FIFO cannot hold shutdown or kill an independent PTY."""
import argparse
import json
import os
from pathlib import Path
import tempfile
import time
import uuid
from playwright.sync_api import sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server, get_json
from diagnostic_retention_browser import until
from browser_evidence_fixtures import run_with_evidence
import terminal_input_browser as fixture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('target/audit-shutdown'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    fixture.SHELL = """stty -echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
 printf '%s\\n' "$command" >> input.log
 printf 'ACK_%s\\n' "$command"
done
"""
    with tempfile.TemporaryDirectory(prefix='sessiondock-audit-shutdown-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        for name in ['host', 'work', 'claude', 'codex', 'grok', 'audit']:
            (root/name).mkdir(mode=0o700)
        from datetime import datetime, timezone
        fifo = root/'audit'/('browser-'+datetime.now(timezone.utc).strftime('%Y-%m-%d')+'.jsonl')
        os.mkfifo(fifo, mode=0o600)
        corpus = Corpus(root)
        corpus.put('synthetic-native-sid', 'codex', [codex_row('session_meta', {'id':'synthetic-native-sid','cwd':str(root/'work')}),
            codex_message('user', 'Private restart fixture')], [])
        uid = corpus.uid('synthetic-native-sid')
        browser = pw.chromium.launch(headless=True)
        try:
            with fixture.host(root, 'synthetic-'+uuid.uuid4().hex, uid):
                with isolated_server(corpus, args.binary, host_dir=root/'host', audit_dir=root/'audit') as (base, opener):
                    page = browser.new_page()
                    page.goto(base)
                    fixture.open_console(page, uid)
                    page.locator('#termpane canvas').first.click()
                    page.keyboard.type('before-restart'); page.keyboard.press('Enter')
                    fixture.xterm_contains(page, 'ACK_before-restart')
                    until(page, lambda:get_json(opener,base,'/api/health')['audit']['accepted_events'] > 0)
                    health = get_json(opener,base,'/api/health')['audit']
                    assert health['written_events'] == 0
                    page.close()
                    stopped = time.monotonic()
                elapsed = time.monotonic()-stopped
                assert elapsed < 4, elapsed
                with isolated_server(corpus, args.binary, host_dir=root/'host') as (base, _):
                    page = browser.new_page()
                    page.goto(base)
                    fixture.open_console(page, uid)
                    page.locator('#termpane canvas').first.click()
                    page.keyboard.type('after-restart'); page.keyboard.press('Enter')
                    fixture.xterm_contains(page, 'ACK_after-restart')
                    assert (root/'work/input.log').read_text().splitlines() == ['before-restart','after-restart']
                    page.screenshot(path=str(args.output/'reconnected.png'))
                    page.close()
            evidence = {'passed':['actual console input before shutdown','blocked audit FIFO bounded shutdown',
                'same independent shell reconnects and accepts second input','independent shell ledger contains both commands'],
                'shutdown_seconds':elapsed,'audit':health}
            (args.output/'result.json').write_text(json.dumps(evidence,indent=2)+'\n')
            print('PASS blocked audit shutdown and detached PTY survival',evidence,flush=True)
        finally: browser.close()


if __name__ == '__main__':
    run_with_evidence(main, 'target/audit-shutdown')
