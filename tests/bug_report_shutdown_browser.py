#!/usr/bin/env python3
"""Submit a real report while SIGTERM interrupts its diagnostic capture.

Private node, fake Claude and Chromium only; replay after restart must keep
the same report and worker. No model or production session is used.
"""
import json
import argparse
import os
from pathlib import Path
import shutil
import sys
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright

from history_fixtures import REPO, Corpus, isolated_server
from hub_send_browser import prepare, cleanup_hosts
from bug_report_node_browser import open_report, wait_drafts
from browser_runtime import wait_for_async


def run(browser, root, binary, disconnected=False):
    root.mkdir()
    config = prepare(root)
    git = shutil.which('git')
    shim = root / 'bin/git'
    marker = root / 'shutdown-during-capture'
    shim.write_text(f'''#!{sys.executable}
import os, signal, sys, time
from pathlib import Path
marker = Path({str(marker)!r})
if sys.argv[1:2] == ['rev-parse'] and not marker.exists():
    marker.touch()
    os.kill(os.getppid(), signal.SIGTERM)
    time.sleep(0.3)  # Give the reactor time to process the signal during capture.
os.execv({git!r}, [{git!r}, *sys.argv[1:]])
''')
    shim.chmod(0o700)
    env = {'SESSIONDOCK_BUG_REPORT_DIR': str(root / 'reports'),
           'SESSIONDOCK_BUG_REPORT_REPO': str(root / 'work')}
    previous_path = os.environ['PATH']
    os.environ['PATH'] = str(root / 'bin') + os.pathsep + previous_path
    result = body = None
    try:
        for restarted in (False, True):
            with isolated_server(Corpus(root), binary,
                    state_dir=root / 'state', host_dir=root / 'host',
                    lifecycle_dir=root / 'ledger', launcher_config=config,
                    audit_dir=root / 'audit', extra_env=env) as (base, _):
                context = browser.new_context(service_workers='block')
                try:
                    page = context.new_page()
                    page.goto(base, wait_until='domcontentloaded')
                    if not restarted:
                        def remember(request):
                            nonlocal body
                            if request.method == 'POST' and urlsplit(request.url).path == '/api/bug-report':
                                body = request.post_data_json
                        page.on('request', remember)
                        page.expose_function('captureInterrupted', lambda: marker.exists())
                        open_report(page)
                        page.locator('input[name="bug-report-source"][value="claude"]').check()
                        page.locator('#bug-report-description').fill('Report during deployment restart')
                        wait_drafts(page)
                        if disconnected:
                            page.locator('#bug-report-go').click()
                            wait_for_async(page, 'async () => await captureInterrupted()')
                            page.close()
                            assert body, 'Missing original report request'
                        else:
                            with page.expect_response(lambda response: urlsplit(response.url).path == '/api/bug-report') as sent:
                                page.locator('#bug-report-go').click()
                            assert sent.value.status == 202, sent.value.text()
                            result = sent.value.json()
                            assert result['worker']['record_id']
                        assert marker.exists(), 'Fixture did not interrupt the diagnostic capture'
                        print(f'PASS report capture during SIGTERM: browser {"disconnected" if disconnected else "received accepted worker"}', flush=True)
                    else:
                        replay = context.request.post(base + '/api/bug-report', data=body)
                        assert replay.status == 202, replay.text()
                        if result is None:
                            result = replay.json()
                        assert replay.json() == result, replay.text()
                        page.locator(f'#side .item[data-uid="tmux:{result["worker"]["name"]}"]').click()
                        page.wait_for_function('uid => S.sel === uid', arg='tmux:' + result['worker']['name'])
                        ledger = json.loads((root / 'ledger/lifecycle-ledger.json').read_text())
                        assert len(ledger['records']) == 1, ledger
                        assert len(list((root / 'reports').glob('BUG-*'))) == 1
                        draft = context.request.get(base + '/api/session/conversation', params={
                            'uid': 'tmux:' + result['worker']['name']}).json()['draft']['value']
                        assert draft['report_prompt'] and 'Report during deployment restart' in draft['text'], draft
                        print('PASS restart: open retained worker, original report prompt, identical replay, one bundle/launch', flush=True)
                finally:
                    context.close()
    finally:
        os.environ['PATH'] = previous_path
        cleanup_hosts(root)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default=str(REPO / 'target/release/sessiondock'))
    binary = Path(parser.parse_args().binary).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='sd-report-stop-') as tmp, sync_playwright() as pw:
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        try:
            for disconnected in (False, True):
                run(browser, Path(tmp) / ('d' if disconnected else 'c'), binary, disconnected)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
