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
from hub_send_browser import cleanup_hosts
from popups import on_popup  # noqa: E402

CODEX_SID = "6a7b8c9d-0e1f-4a2b-9c3d-4e5f6a7b8c9d"


def resume_codex(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.wait_for_function("uid => S.sel === uid", arg=uid, timeout=20000)
    expect(page.locator("#msgs")).to_contain_text("Synthetic codex prompt")
    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover") as taken:
        page.locator("#a-term").click()
    resumed = taken.value.json()
    assert taken.value.status == 200 and resumed["launch_kind"] == "resume", resumed
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    xterm_includes(page, "FAKE_CODEX_TUI sid=[%s]" % CODEX_SID)
    page.wait_for_function("uid => (T.list || []).some(row => row.uid === uid && row.instance_id)", arg=uid, timeout=15000)
    return resumed


def user_records(path):
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    return [row["payload"] for row in rows
            if row.get("type") == "response_item" and row["payload"].get("type") == "message"
            and row["payload"].get("role") == "user"]


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
                    'SESSIONDOCK_TEST_QUOTED_READY': '1',
                    'SESSIONDOCK_TEST_BUSY_WARNING': '1',
                    'SESSIONDOCK_TEST_CODEX_ROOT': str(root / 'codex')}}]}))
        initialize('--initialize-lifecycle', root / 'ledger')
        try:
            with isolated_server(corpus, BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                    launcher_config=launcher, state_dir=root / 'state',
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
                on_popup(page, lambda dialog: (errors.append(dialog.message), dialog.accept()))
                page.goto(base, wait_until='domcontentloaded')
                resume_codex(page, uid)
                xterm_includes(page, 'Context 32% used · Ready')
                xterm_includes(page, 'Context 27% used · Working')
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
