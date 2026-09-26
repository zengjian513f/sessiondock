#!/usr/bin/env python3
"""Conversation SEND against a resumed native Codex session, using a private fake CLI."""
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
from history_parity import REPO, BINARY, Corpus, codex_message, codex_row, isolated_server
from send_browser import initialize, xterm_includes
from send_codex_browser import CODEX_SID, resume_codex, user_records
from hub_send_browser import cleanup_hosts


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-send-') as temporary:
        root = Path(temporary).resolve()
        for name in ('host', 'work', 'ledger', 'delivery', 'state', 'home', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        rollout = corpus.put(CODEX_SID, 'codex', [
            codex_row('session_meta', {'id': CODEX_SID, 'cwd': str(root / 'work')}),
            codex_message('user', 'Synthetic codex prompt')], [])
        uid = corpus.uid(CODEX_SID)
        launcher = root / 'launcher.json'
        launcher.touch(mode=0o600)
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'codex-cli-v1', 'source': 'codex',
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(REPO / 'tests/fake_codex_cli.py'), '--model', 'gpt-5.6-luna', '--reply'],
                'new_args': [], 'resume_args': ['resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                    'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                    'SESSIONDOCK_TEST_CODEX_ROOT': str(root / 'codex')}}]}))
        initialize('--initialize-lifecycle', root / 'ledger')
        initialize('--initialize-delivery', root / 'delivery')
        try:
            with isolated_server(corpus, BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                    launcher_config=launcher, delivery_dir=root / 'delivery', state_dir=root / 'state',
                    file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
                options = {'headless': True}
                if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                    options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                browser = pw.chromium.launch(**options)
                context = browser.new_context(service_workers='block')
                context.route('**/*', lambda route: route.continue_()
                    if route.request.url.startswith(base + '/') else route.abort())
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.on('dialog', lambda dialog: (errors.append(dialog.message), dialog.accept()))
                page.goto(base, wait_until='domcontentloaded')
                resume_codex(page, uid)
                page.locator('#a-term').click()
                for number in range(2):
                    page.wait_for_function("uid => composerUid === uid && composerDraft()?.inputStatus?.state === 'ready'", arg=uid)
                    text = f'native conversation send {number}'
                    page.locator('#cinput').fill(text)
                    page.evaluate('async () => await composerDraftWrites')
                    draft = context.request.get(base + '/api/session/conversation', params={'uid': uid}).json()['draft']
                    assert draft['value']['session']['cwd'] == str(root / 'work'), draft
                    started = time.monotonic()
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200, sent.value.text()
                    body = sent.value.request.post_data_json
                    assert body['uid'] == uid and sent.value.json()['state'] == 'sent', body
                    page.wait_for_function('() => !composerSending')
                    expect(page.locator('#cinput')).to_have_value('')
                    print(f'native Codex click-to-clear: {(time.monotonic() - started)*1000:.0f} ms', flush=True)
                    page.locator('#a-term').click()
                    xterm_includes(page, '> ' + text)
                    page.locator('#a-term').click()
                    users = [r['content'][0]['text'] for r in user_records(rollout)]
                    assert users.count(text) == 1, users
                    # Replaying a completed request only reads its receipt.
                    replay = context.request.post(base + '/api/session/conversation/send', data=body)
                    assert replay.status == 200 and replay.json()['state'] == 'sent', replay.text()
                    assert len(user_records(rollout)) == number + 2
                assert not errors, errors
                context.close()
                browser.close()
        finally:
            cleanup_hosts(root)
    print('PASS native Codex browser: resumed identity, cwd, repeated composer sends, exact native records, replay without writes')


if __name__ == '__main__':
    main()
