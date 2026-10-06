#!/usr/bin/env python3
"""Native echoes arriving during CLI observation must still reach the browser.

BUG-20261005-151728-007777: the Claude rewind probe consumed the watch
notification without sending history or settling SEND. Delay only this
fixture's host to put a native append inside the in-flight screen read.
"""

import argparse
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import time
import uuid
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from history_fixtures import REPO, BINARY, Corpus, isolated_server
from private_hosts import private_hosts
from send_browser import SETTINGS, create_claude, initialize, wait_history


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    binary = parser.parse_args().binary.resolve()
    with tempfile.TemporaryDirectory(prefix='sessiondock-send-echo-') as temporary, private_hosts(Path(temporary)):
        root = Path(temporary).resolve()
        for name in ['host', 'work', 'work/claude-area', 'ledger', 'state', 'home', 'claude', 'codex', 'grok']:
            (root / name).mkdir(mode=0o700)
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'claude-cli-v1', 'source': 'claude',
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(REPO / 'tests/fake_claude_cli.py'), '--settings', SETTINGS, '--reply', '--swallow', '2'],
                'new_args': ['--session-id', '{session_id}'], 'resume_args': ['--resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                        'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                        'SESSIONDOCK_TEST_CLAUDE_ROOT': str(root / 'claude')}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger', binary)
        with isolated_server(Corpus(root), binary, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, state_dir=root / 'state',
                file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(service_workers='block', viewport={'width': 1280, 'height': 900})
            context.route('**/*', lambda route: route.continue_()
                          if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page()
            errors, watches = [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.on('request', lambda request: watches.append(request.url)
                    if urlsplit(request.url).path == '/api/watch' else None)
            try:
                page.goto(base, wait_until='networkidle')
                receipt = create_claude(page, base, root / 'work', open_terminal=False)
                page.wait_for_function('composerUid && !composerDraft().loading && takenOver(composerUid)')

                def send(text):
                    page.locator('#cinput').fill(text)
                    expect(page.locator('#csend')).to_be_enabled()
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as response:
                        page.locator('#csend').click()
                    assert response.value.status == 200, response.value.text()
                    return response.value.json()['request_id']

                send('Bootstrap native echo observation')
                wait_history(page, 'OK: Bootstrap native echo observation')
                page.wait_for_function("S.sel && !S.sel.startsWith('tmux:')")
                uid = page.evaluate('S.sel')
                text = 'Accepted native input before any model response'
                request_id = send(text)
                queued = page.locator('#queued-sends .queued-send').filter(has_text=text)
                expect(queued).to_have_count(1)
                expect(queued.locator('.queued-send-state')).to_have_text('已发送，等待 CLI 处理')
                # Keep the HTTP fallback at its pre-append version. Only the
                # existing SSE connection can deliver the regression's echo.
                snapshot = context.request.get(base + '/api/messages/' + uid).json()
                context.route('**/api/messages/**', lambda route: route.fulfill(json=snapshot))
                watch_count = len(watches)
                native = root / 'claude/project-history' / (receipt['declared_sid'] + '.jsonl')
                rows = [json.loads(line) for line in native.read_text().splitlines()]
                host_record = json.loads((root / 'host' / (receipt['name'] + '.json')).read_text())
                host_pid = host_record['host_pid']
                parent = rows[-1]['uuid']

                def append_during_observation(role, content):
                    nonlocal parent
                    # This is the host launched above in the private fixture,
                    # never the service or any production CLI process.
                    os.kill(host_pid, signal.SIGSTOP)
                    try:
                        page.wait_for_timeout(2200)  # CLI poll now waits on its host.
                        row = {'type': role, 'uuid': str(uuid.uuid4()), 'parentUuid': parent,
                               'sessionId': receipt['declared_sid'], 'cwd': str(root / 'work/claude-area'),
                               'timestamp': time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime()) + '.000Z',
                               'message': {'role': role, 'content': content}}
                        with native.open('a') as stream:
                            stream.write(json.dumps(row) + '\n')
                            stream.flush()
                            os.fsync(stream.fileno())
                        parent = row['uuid']
                        page.wait_for_timeout(1200)  # Publisher sees the append during the read.
                    finally:
                        os.kill(host_pid, signal.SIGCONT)

                append_during_observation('user', text)
                expect(page.locator('#msgs .msg[data-role=user]:not(.queued-send)').filter(has_text=text)).to_have_count(1, timeout=5000)
                expect(queued).to_have_count(0)
                ledger = json.loads((root / 'state/conversations/conversation-ledger.json').read_text())
                assert not any(row['request_id'] == request_id for queue in ledger['queued'].values() for row in queue)
                assert not any(row['type'] == 'assistant' and text in str(row['message'])
                               for row in map(json.loads, native.read_text().splitlines()))
                print('PASS native user echo settles SEND over SSE before any assistant response', flush=True)

                append_during_observation('assistant', 'Delayed assistant response after echo settlement')
                wait_history(page, 'Delayed assistant response after echo settlement', timeout=5000)
                assert len(watches) == watch_count, 'reconnect masked a lost watch notification'
                assert not errors, errors
                print('PASS later history append survives the same CLI observation race without reconnect', flush=True)
            finally:
                context.close()
                browser.close()


if __name__ == '__main__':
    main()
