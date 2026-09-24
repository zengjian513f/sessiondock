#!/usr/bin/env python3
"""Server drafts across two pages of one person: a second page opens the saved
text without error, a refused save rebases instead of dying (the typing page
wins), an idle page follows within the poll both ways, a staged attachment
added on one page is sent from the other, that SEND empties the first page, an
image staged on one page is previewed on the other from the server's staged
bytes, and a console command sent from the composer clears the server draft. Runs the real
legacy composer against the fake Claude CLI plus a synthetic shell.
"""
import base64
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect

from history_parity import REPO, BINARY, Corpus, isolated_server
from send_browser import SETTINGS, initialize, create_claude
from lifecycle_http_suite import SHELL


# 1x1 opaque PNG: the smallest real image an <img> will decode.
PNG = base64.b64decode(
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx'
    '0gAAAABJRU5ErkJggg==')


def draft(context, base, uid):
    row = context.request.get(base + '/api/session/conversation?uid=' + uid).json()['draft']
    return row['revision'], row['value']


def wait_server_text(context, base, uid, text, timeout=10.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        revision, value = draft(context, base, uid)
        if (value or {}).get('text') == text:
            return revision
        time.sleep(0.1)
    raise AssertionError('server draft text is %r, wanted %r' % ((value or {}).get('text'), text))


def open_session(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').first.click()
    page.wait_for_function('uid => composerUid === uid && !composerDraft().loading && takenOver(uid)', arg=uid)


def main():
    if os.name != 'posix':
        raise SystemExit('Draft sync browser acceptance currently requires POSIX.')
    with tempfile.TemporaryDirectory(prefix='sessiondock-draft-sync-') as temporary:
        root = Path(temporary).resolve()
        for name in ['host', 'work', 'work/claude-area', 'ledger', 'delivery', 'state', 'bin', 'home', 'claude', 'codex', 'grok']:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        python = Path(subprocess.check_output(['/bin/sh', '-c', 'command -v python3']).decode().strip()).resolve()
        wrapper = root / 'bin/fake-claude'
        wrapper.write_text('#!/bin/sh\nexec %s %s "$@"\n' % (python, REPO / 'tests/fake_conversation_cli.py'))
        wrapper.chmod(0o700)
        configuration = root / 'launcher.json'
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
            'host_dir': str(root / 'host'), 'adapters': [
                {'id': 'synthetic-shell-v1', 'source': 'shell', 'executable': str(Path('/bin/sh').resolve()),
                 'args': ['-c', SHELL], 'env': {'PATH': '/usr/bin:/bin', 'TERM': 'xterm-256color'}}], 'profiles': [
                {'id': 'claude-cli-v1', 'source': 'claude', 'executable': str(wrapper),
                 'args': ['--settings', SETTINGS, '--reply', '--delay', '500'], 'new_args': ['--session-id', '{session_id}'],
                 'resume_args': ['--resume', '{sid}'],
                 'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TERM': 'xterm-256color',
                         'LANG': 'C.UTF-8', 'SESSIONDOCK_TEST_CLAUDE_ROOT': str(root / 'claude'),
                         'SESSIONDOCK_TEST_GATE': str(root / 'gate'), 'SESSIONDOCK_TEST_GATE_TRACE': str(root / 'gate.trace')}}]}))
        initialize('--initialize-lifecycle', root / 'ledger')
        initialize('--initialize-delivery', root / 'delivery')
        with sync_playwright() as playwright:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                                     launcher_config=configuration, delivery_dir=root / 'delivery', state_dir=root / 'state',
                                     file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _):
                    errors, sends = [], []

                    def open_page(context, draft_route=None):
                        context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
                        page = context.new_page()
                        page.on('pageerror', lambda error: errors.append(str(error)))
                        page.on('dialog', lambda dialog: dialog.accept())
                        if draft_route:
                            page.route('**/api/session/conversation?*', draft_route)
                        page.goto(base, wait_until='networkidle')
                        return page

                    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
                    context.on('request', lambda request: sends.append(request.post_data_json)
                        if urlsplit(request.url).path == '/api/session/conversation/send' else None)
                    a = open_page(context)
                    receipt = create_claude(a, base, root / 'work', open_terminal=False)
                    uid = 'tmux:' + receipt['name']
                    a.wait_for_function('composerUid && !composerDraft().loading && takenOver(composerUid)')

                    # 1. A second page opens the same session: it reads the saved
                    #    text, adopts the server revision and reports no error.
                    #    (The composer is disabled until that read returns, so
                    #    typing cannot race the first read through the UI.)
                    a.fill('#cinput', 'from A')
                    wait_server_text(context, base, uid, 'from A')
                    b = open_page(context)
                    open_session(b, uid)
                    expect(b.locator('#cinput')).to_have_value('from A')
                    assert not b.locator('.draft-save-error').count(), b.locator('.draft-save-error').all_text_contents()
                    assert b.evaluate('composerDraft().revision') == draft(context, base, uid)[0]
                    b.type('#cinput', ' late B')
                    b.evaluate('async () => await composerDraftWrites')
                    assert not b.locator('.draft-save-error').count(), b.locator('.draft-save-error').all_text_contents()
                    wait_server_text(context, base, uid, 'from A late B')

                    # 2. A refused save rebases: another writer bumps the revision
                    #    behind page A; A's next keystroke still lands, A wins.
                    a.wait_for_function("document.querySelector('#cinput').value === 'from A late B'", timeout=6000)
                    revision, value = draft(context, base, uid)
                    bumped = context.request.post(base + '/api/session/conversation',
                        data={'uid': uid, 'revision': revision, 'value': {**value, 'text': 'phone wrote this'}})
                    assert bumped.status == 200, bumped.text()
                    a.type('#cinput', ' +A')
                    a.evaluate('async () => await composerDraftWrites')
                    assert not a.locator('.draft-save-error').count(), a.locator('.draft-save-error').all_text_contents()
                    expect(a.locator('#cinput')).to_have_value('from A late B +A')
                    wait_server_text(context, base, uid, 'from A late B +A')

                    # 3. The idle page follows within the poll, then typing there
                    #    continues from the followed text and A follows back.
                    b.wait_for_function("document.querySelector('#cinput').value === 'from A late B +A'", timeout=6000)
                    assert not b.locator('.draft-save-error').count()
                    b.type('#cinput', ' +B')
                    b.evaluate('async () => await composerDraftWrites')
                    wait_server_text(context, base, uid, 'from A late B +A +B')
                    a.wait_for_function("document.querySelector('#cinput').value === 'from A late B +A +B'", timeout=6000)

                    # 4. An attachment staged on A is listed and sent from B; the
                    #    SEND on B empties A.
                    a.locator('#cadd').click()
                    with a.expect_file_chooser() as chooser:
                        a.locator('#attach-menu [data-attach=file]').click()
                    with a.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/attachment') as staged:
                        chooser.value.set_files([{'name': 'shared.txt', 'mimeType': 'text/plain', 'buffer': b'staged on A'}])
                    assert staged.value.status == 200, staged.value.text()
                    a.evaluate('async () => await composerDraftWrites')
                    b.wait_for_function("composerDraft().attachments.length === 1 && composerDraft().attachments[0].uploaded?.upload_id", timeout=6000)
                    assert not b.evaluate('composerDraft().attachments[0].file instanceof File')
                    expect(b.locator('#compose-items .draft-card')).to_have_count(1)
                    with b.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send', timeout=20000) as sent:
                        b.locator('#csend').click()
                    assert sent.value.status == 200 and sent.value.json()['state'] == 'sent', sent.value.text()
                    assert sends[-1]['attachments'][0]['upload_id'] == staged.value.json()['upload_id']
                    published = list((root / 'work/claude-area/sessiondock_attachments').glob('*/shared.txt'))
                    assert len(published) == 1 and published[0].read_bytes() == b'staged on A'
                    expect(b.locator('#cinput')).to_have_value('')
                    a.wait_for_function("document.querySelector('#cinput').value === '' && composerDraft().attachments.length === 0", timeout=6000)
                    assert not a.locator('.draft-save-error').count() and not b.locator('.draft-save-error').count()

                    # 4b. An image staged on A is previewed on B from the
                    #     server's staged bytes: a card that never held the File
                    #     still shows a thumbnail instead of a broken image.
                    a.locator('#cadd').click()
                    with a.expect_file_chooser() as chooser:
                        a.locator('#attach-menu [data-attach=image]').click()
                    with a.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/attachment') as picture:
                        chooser.value.set_files([{'name': 'shot.png', 'mimeType': 'image/png', 'buffer': PNG}])
                    assert picture.value.status == 200, picture.value.text()
                    upload_id = picture.value.json()['upload_id']
                    a.evaluate('async () => await composerDraftWrites')
                    b.wait_for_function("composerDraft().attachments.length === 1 && !!composerDraft().attachments[0].uploaded?.upload_id", timeout=6000)
                    assert not b.evaluate('composerDraft().attachments[0].file instanceof File')
                    b.wait_for_function("document.querySelector('#compose-items .draft-card .draft-thumb img')?.src.startsWith('blob:') || false", timeout=6000)
                    served = context.request.get(base + '/api/session/conversation/attachment?uid=' + uid + '&id=' + upload_id)
                    assert served.status == 200 and served.body() == PNG, served.status
                    assert served.headers['content-type'] == 'image/png', served.headers
                    assert served.headers['x-content-type-options'] == 'nosniff', served.headers
                    assert 'sandbox' in served.headers['content-security-policy'], served.headers
                    # Staged bytes of an unknown id are not served.
                    assert context.request.get(base + '/api/session/conversation/attachment?uid=' + uid + '&id=absent').status == 404
                    b.evaluate('removeComposerAttachment(composerDraft().attachments[0].id)')
                    b.wait_for_function('composerDraft().attachments.length === 0', timeout=6000)
                    a.wait_for_function('composerDraft().attachments.length === 0', timeout=6000)

                    # Failed writes recover without another keystroke or SEND.
                    def fail_draft(route):
                        route.abort('failed')
                    a.route('**/api/session/conversation', fail_draft)
                    a.fill('#cinput', 'offline draft')
                    a.wait_for_function('!!composerDraft().storageError')
                    expect(a.locator('#cinput')).to_have_value('offline draft')
                    a.unroute('**/api/session/conversation', fail_draft)
                    a.wait_for_function('!composerDraft().storageError && composerDraft().savedVersion === composerDraft().editVersion', timeout=10000)
                    wait_server_text(context, base, uid, 'offline draft')
                    assert len(sends) == 1, sends

                    # A new page fails its first read, keeps edits, then merges
                    # the existing server text once networking returns.
                    c = open_page(context, fail_draft)
                    open_session(c, a.evaluate("S.sel"))
                    c.wait_for_function('composerDraft().loadFailed === true')
                    c.fill('#cinput', 'early offline edit')
                    c.evaluate('async () => await composerDraftWrites')
                    c.type('#cinput', ' retained')
                    c.evaluate('async () => await composerDraftWrites')
                    error_text = c.locator('.draft-save-error').inner_text()
                    assert error_text.count('服务端草稿读取失败') == 1, error_text
                    assert '服务端草稿保存失败' not in error_text, error_text
                    c.unroute('**/api/session/conversation?*', fail_draft)
                    c.wait_for_function('!composerDraft().loadFailed && !composerDraft().storageError && composerDraft().savedVersion === composerDraft().editVersion', timeout=10000)
                    expect(c.locator('#cinput')).to_have_value('offline draft\nearly offline edit retained')
                    wait_server_text(context, base, uid, 'offline draft\nearly offline edit retained')
                    assert len(sends) == 1, sends
                    c.close()

                    # 5. A console (SSH/shell) command sent from the composer clears
                    #    the server draft; the exited console leaves no
                    #    "retained draft" row behind.
                    shell = context.request.post(base + '/api/term/create', data={'source': 'shell', 'cwd': str(root / 'work'), 'request_id': 'shell-composer-send'})
                    assert shell.status == 200 and shell.json()['running'], shell.text()
                    shell = shell.json()
                    shell_uid = 'tmux:' + shell['name']
                    a.evaluate('info => openPendingSession(info)', shell)
                    expect(a.locator('#composer')).to_be_visible()
                    a.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                    a.fill('#cinput', 'echo composer-sent')
                    wait_server_text(context, base, shell_uid, 'echo composer-sent')
                    a.locator('#csend').click()
                    wait_server_text(context, base, shell_uid, '')
                    expect(a.locator('#cinput')).to_have_value('')
                    # `quit` ends the synthetic shell; sent from the composer as well.
                    a.fill('#cinput', 'quit')
                    wait_server_text(context, base, shell_uid, 'quit')
                    a.locator('#csend').click()
                    wait_server_text(context, base, shell_uid, '')
                    # The session list is the index of recordings: the exited console stays
                    # listed (not running, with its recording) but leaves no server draft.
                    a.wait_for_function('async id => { await loadTermList(); return T.pending.some(row => row.record_id === id && row.running === false && row.recording?.id); }', arg=shell['record_id'], timeout=30000)
                    expect(a.locator(f'#side .item[data-uid="{shell_uid}"]')).to_have_count(1)
                    listed = context.request.get(base + '/api/session/conversation/drafts').json()['drafts']
                    assert not any(row['uid'] == shell_uid for row in listed), listed
                    assert not errors, errors
                    context.request.post(base + '/api/term/kill', data={'record_id': receipt['record_id'], 'instance_id': receipt['instance_id']})
                    context.close()
            finally:
                browser.close()
    print('PASS draft_sync_browser: second page reads without error, 409 rebase, idle follow both ways, cross-page staged attachment send, SEND empties the other page, staged image preview on a page without the File, console composer send clears the server draft, failed writes and initial reads recover without SEND')


if __name__ == '__main__':
    main()
