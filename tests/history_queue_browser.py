#!/usr/bin/env python3
"""History queue recovery must not probe terminals for unrelated queued sends."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import socket
import tempfile
import threading
from urllib.parse import unquote, urlsplit

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO, Corpus, claude_row, isolated_server
from send_browser import initialize


@contextmanager
def observed_host(root):
    """A private unreachable host: count probes without providing authority."""
    path = root / 'host' / 'queue-fixture.sock'
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(path))
    listener.listen()
    listener.settimeout(.1)
    stopped = threading.Event()
    probes = []

    def serve():
        while not stopped.is_set():
            try:
                conn, _ = listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            probes.append(True)
            conn.close()

    record = root / 'host' / 'queue-fixture.json'
    record.write_text(json.dumps({'name': 'queue-fixture', 'host_pid': os.getpid(),
        'pid': os.getpid(), 'cols': 80, 'rows': 24, 'sock': str(path)}))
    record.chmod(0o600)
    worker = threading.Thread(target=serve, daemon=True)
    worker.start()
    try:
        yield probes
    finally:
        stopped.set()
        listener.close()
        worker.join(timeout=2)


def queued(text):
    # Existing SEND receipt format, not a new integrity check.
    echo = hashlib.sha256(json.dumps(text, ensure_ascii=False).encode()).hexdigest()
    return {'request_id': text, 'text': text, 'echo_hash': echo, 'sent_at': 0,
            'state': 'queued', 'cli_queued_at': None}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-history-queue-') as temporary:
        root = Path(temporary)
        for name in ('host', 'work', 'ledger', 'state', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        for sid, text in [('unrelated', 'Unrelated history'), ('own', 'Own accepted input'),
                          ('alias', 'Alias accepted input'), ('pending', 'Pending history'),
                          ('candidate', 'Unknown queued input')]:
            corpus.put(sid, 'claude', [claude_row(sid, 'user', 'u0', None, text)], [])
        native = {sid: path.read_bytes() for sid, path in corpus.paths.items()}
        config = root / 'launcher.json'
        config.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
            'host_dir': str(root / 'host'), 'adapters': [], 'profiles': [{
                'id': 'claude-cli-v1', 'source': 'claude', 'executable': '/bin/sh',
                'args': [], 'new_args': [], 'resume_args': []}]}))
        config.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger', args.binary)
        directory = root / 'state' / 'conversations'
        directory.mkdir(mode=0o700)
        ledger = directory / 'conversation-ledger.json'
        ledger.write_text(json.dumps({'aliases': {corpus.uid('alias'): 'launch:alias'}, 'queued': {
            'launch:unknown': [queued('Unknown queued input')],
            corpus.uid('own'): [queued('Own accepted input')],
            'launch:alias': [queued('Alias accepted input')],
            corpus.uid('pending'): [queued('Still waiting for CLI')],
        }}))
        ledger.chmod(0o600)
        with observed_host(root) as probes, isolated_server(corpus, args.binary,
                state_dir=root / 'state', host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=config, file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            try:
                page = browser.new_page()
                # Isolate the history route from independent status/composer
                # observations; selection and rendering still use real UI clicks.
                requested = []
                def route(request):
                    path = urlsplit(request.request.url).path
                    requested.append(path)
                    if path.startswith(('/api/live', '/api/term/', '/api/watch', '/api/events',
                                        '/api/session/conversation', '/api/process-links')):
                        request.abort()
                    else:
                        request.continue_()
                page.route('**/api/**', route)
                page.goto(base, wait_until='domcontentloaded')
                # The independent startup cleanup count performs one host scan.
                # Wait for its published completion before measuring UI opens.
                page.wait_for_function("async () => Boolean((await (await fetch('api/sessions/cleanup-counts')).json())?.next_check_at)")
                probes.clear()
                for sid, text in [('unrelated', 'Unrelated history'), ('own', 'Own accepted input'),
                                  ('alias', 'Alias accepted input'), ('pending', 'Pending history')]:
                    uid = corpus.uid(sid)
                    with page.expect_response(lambda r: unquote(urlsplit(r.url).path) == f'/api/messages/{uid}') as reply:
                        page.locator(f'#side .item[data-uid="{uid}"] .t').click()
                    body = reply.value.json()
                    expect(page.locator('#msgs')).to_contain_text(text)
                    assert not probes, f'{sid} history needlessly probed a terminal: {requested}'
                    if sid == 'pending':
                        assert body['cli']['queued'][0]['text'] == 'Still waiting for CLI'
                saved = json.loads(ledger.read_text())['queued']
                assert not saved.get(corpus.uid('own')) and not saved.get('launch:alias')
                assert saved['launch:unknown'][0]['text'] == 'Unknown queued input'
                # A possible echo with no durable identity still uses the
                # original resolver; a failed probe cannot retire another queue.
                page.locator(f'#side .item[data-uid="{corpus.uid("candidate")}"] .t').click()
                expect(page.locator('#msgs')).to_contain_text('Unknown queued input')
                assert probes, 'unknown matching echo bypassed identity resolution'
                assert json.loads(ledger.read_text())['queued']['launch:unknown']
                assert all(path.read_bytes() == native[sid] for sid, path in corpus.paths.items())
            finally:
                browser.close()
    print('PASS history queue: unrelated opens make no host probe; durable native/alias queues recover, pending sends survive, unknown candidates still resolve', flush=True)


if __name__ == '__main__':
    main()
