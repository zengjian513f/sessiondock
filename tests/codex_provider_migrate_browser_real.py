#!/usr/bin/env python3
"""Real Codex provider migration in a temporary home, through Chromium.

No authentication is copied and no model input is submitted. Direct invocation
only, or the real-suite validation flags. Uses gpt-5.6-luna at low effort.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import queue
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

from playwright.sync_api import expect, sync_playwright
from codex_sdk_actions_browser import environment
from codex_sdk_browser import server
from history_fixtures import BINARY, REPO, Corpus

sys.path.insert(0, str(REPO/'deploy'))
from codex_provider_migrate import migrate

SID = '01a11d3f-7510-7d20-8f67-546a6975ebef'
MODEL = 'gpt-5.6-luna'


@contextmanager
def native(executable, home, cwd, provider, socket=None):
    args = [str(executable), '-c', f'model_provider="{provider}"', '-c', f'model="{MODEL}"',
            '-c', 'model_reasoning_effort="low"', 'app-server']
    if socket:
        args += ['--listen', 'unix://'+str(socket)]
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen(args, cwd=cwd, env={**os.environ, 'HOME': str(home.parent), 'CODEX_HOME': str(home)},
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True)
        try:
            yield proc
        except BaseException:
            log.seek(0); print(log.read().decode(errors='replace'), file=sys.stderr)
            raise
        finally:
            proc.terminate(); proc.wait(timeout=15)


def seed(executable, home, cwd, provider="openai", label="provider migration history"):
    folder = home/'sessions/2026/10/09'; folder.mkdir(parents=True, exist_ok=True)
    path = folder/f'rollout-2026-10-09T04-41-00-{SID}.jsonl'
    rows = [('session_meta', {'id': SID, 'timestamp': '2026-10-08T20:41:00.441Z', 'cwd': str(cwd),
             'originator': 'codex_cli_rs', 'cli_version': '0.161.0', 'source': 'cli', 'model_provider': provider, 'history_mode': 'paginated'})]
    for i in range(2):
        turn = f'01a11d3f-7510-7d20-8f67-546a6975ebe{i}'
        rows += [('event_msg', {'type': 'task_started', 'turn_id': turn}),
                 ('response_item', {'type': 'message', 'role': 'user', 'content': [{'type': 'input_text', 'text': f'{label} {i}'}]}),
                 ('response_item', {'type': 'message', 'role': 'assistant', 'content': [{'type': 'output_text', 'text': f'preserved answer {i}'}]}),
                 ('event_msg', {'type': 'task_complete', 'turn_id': turn, 'last_agent_message': f'preserved answer {i}'})]
    path.write_text(''.join(json.dumps({'timestamp': '2026-10-08T20:41:00.441Z', 'type': t, 'payload': p, 'ordinal': n}, ensure_ascii=False)+'\n' for n,(t,p) in enumerate(rows)))
    with native(executable, home, cwd, provider) as proc:
        messages = queue.Queue()
        def reader():
            for line in proc.stdout:
                messages.put(json.loads(line))
        threading.Thread(target=reader, daemon=True).start()
        def call(mid, method, params):
            proc.stdin.write(json.dumps({'id': mid, 'method': method, 'params': params})+'\n'); proc.stdin.flush()
            while True:
                msg = messages.get(timeout=20)
                if msg.get('id') == mid:
                    assert 'error' not in msg, msg
                    return msg['result']
        call(1, 'initialize', {'clientInfo': {'name': 'sessiondock', 'version': '1'}, 'capabilities': {'experimentalApi': True}})
        proc.stdin.write('{"method":"initialized"}\n'); proc.stdin.flush()
        result = call(2, 'thread/resume', {'threadId': SID, 'excludeTurns': True})
        assert result['model'] == MODEL, result
        call(3, 'thread/settings/update', {'threadId': SID, 'effort': 'low'})
        settings = call(5, 'thread/resume', {'threadId': SID, 'excludeTurns': True})
        assert settings['reasoningEffort'] == 'low', settings
        call(4, 'thread/turns/list', {'threadId': SID, 'limit': 10})
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', type=Path, default=Path(shutil.which('codex') or 'codex'))
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sd-provider-native-') as tmp, sync_playwright() as pw:
        root = Path(tmp); home = root/'codex'; home.mkdir(); cwd = root/'work'; cwd.mkdir()
        for name in ('claude', 'grok'): (root/name).mkdir()
        (home/'config.toml').write_text('''model_provider = "openai_catalog_cache"
[model_providers.openai_catalog_cache]
name = "Synthetic catalog provider"
wire_api = "responses"
base_url = "http://127.0.0.1:9/v1"
''')
        path = seed(args.codex, home, cwd)
        corpus = Corpus(root)
        socket = root/'native.sock'
        with native(args.codex, home, cwd, 'openai_catalog_cache', socket) as proc:
            deadline = time.monotonic()+15
            while not socket.exists():
                assert proc.poll() is None and time.monotonic() < deadline
                time.sleep(.05)
            env = {**environment(root, args.binary), 'SESSIONDOCK_CODEX_ROOT': str(home/'sessions')}
            with server(corpus, args.binary, socket, env) as base:
                browser = pw.chromium.launch(headless=True)
                try:
                    page = browser.new_page(viewport={'width': 1280, 'height': 844})
                    page.goto(base, wait_until='networkidle')
                    page.locator('#side .item').filter(has_text='provider migration history').first.click()
                    expect(page.locator('.sdk-history')).to_contain_text('preserved answer 1')
                    page.get_by_title('启动 SDK会话', exact=True).click()
                    expect(page.locator('#sdk-status-menu')).to_contain_text('current provider is openai_catalog_cache', timeout=15000)
                    plan = migrate(home, SID, 'openai', 'openai_catalog_cache', root/'backup')
                    assert not plan['applied']
                    applied = migrate(home, SID, 'openai', 'openai_catalog_cache', root/'backup', True)
                    assert applied['applied'] and applied['indexed_turns'] == 2, applied
                    # Every indexed offset must still land on the original record boundary.
                    data = path.read_bytes()
                    db = sqlite3.connect(home/'thread_history_1.sqlite')
                    for offset, end in db.execute('select rollout_byte_offset,rollout_end_byte_offset from thread_turns where thread_id=?', (SID,)):
                        assert json.loads(data[offset:].splitlines()[0])['payload']['type'] == 'task_started'
                        assert end is None or end == len(data) or data[end-1:end] == b'\n'
                    db.close()
                    page.get_by_title('启动 SDK会话', exact=True).click()
                    page.wait_for_function('CodexSdkView.connected', timeout=15000)
                    page.wait_for_function('CodexSdkView.summary?.settings?.model === "gpt-5.6-luna"')
                    expect(page.locator('.sdk-history')).to_contain_text('preserved answer 1')
                    try:
                        migrate(home, SID, 'openai_catalog_cache', 'openai', root/'active-backup', True)
                    except BlockingIOError:
                        pass
                    else:
                        raise AssertionError('migration admitted a native active writer')
                    page.reload(wait_until='networkidle')
                    page.wait_for_function('CodexSdkView.connected')
                    expect(page.locator('.sdk-history')).to_contain_text('preserved answer 0')
                    if args.screenshots:
                        args.screenshots.mkdir(parents=True, exist_ok=True)
                        page.screenshot(path=str(args.screenshots/'provider-migrated.png'), full_page=True)
                    print('PASS real native provider refusal → explicit metadata migration → browser resume/reload; two turn offsets preserved; no model input', flush=True)
                finally:
                    browser.close()


if __name__ == '__main__':
    main()
