#!/usr/bin/env python3
"""A Codex rollout rotation must not trap its composer in draft CAS conflicts."""
import json
import os
from pathlib import Path
import shutil
import tempfile
from types import SimpleNamespace
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY, REPO, Corpus, codex_message, codex_row, isolated_server
from hub_fixtures import Hub, free_port, scoped
from private_hosts import private_hosts
from send_browser import initialize
from send_native_codex_browser import CODEX_SID, resume_codex, user_records


def rotation(browser, hub_mode):
    mode = 'hub' if hub_mode else 'node'
    screenshots = Path(os.environ.get('SESSIONDOCK_SCREENSHOTS', '/tmp/sessiondock-draft-rotation'))
    screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-draft-rotation-') as temporary, private_hosts(Path(temporary)):
        root = Path(temporary)
        for name in ('host', 'work', 'ledger', 'state', 'home', 'claude', 'codex', 'grok', 'hub', 'bin'):
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        original = corpus.put(CODEX_SID, 'codex', [
            codex_row('session_meta', {'id': CODEX_SID, 'cwd': str(root / 'work'),
                                     'timestamp': '2026-09-11T08:00:00Z'}),
            codex_message('user', 'Synthetic codex prompt')], [])
        marker = root / 'current-rollout'
        marker.write_text(str(original))
        executable = root / 'bin/codex'
        shutil.copy2(Path('/usr/bin/python3').resolve(), executable)
        # The private CLI switches its native output file just as a rewind
        # does, retaining the process, thread and terminal instance.
        cli = root / 'rotating-codex-cli.py'
        cli.write_text('import os, sys\nfrom pathlib import Path\n'
            f'sys.path.insert(0, {str(REPO / "tests")!r})\n'
            'from fake_codex_cli import Fake, parse\n'
            'class RotatingFake(Fake):\n'
            '    def render(self, working=False):\n'
            f'        path = Path({str(marker)!r}).read_text()\n'
            '        if not hasattr(self, "native_hold") or self.path != path:\n'
            '            if hasattr(self, "native_hold"): self.native_hold.close()\n'
            '            self.path = path\n'
            '            self.native_hold = open(path, "a")\n'
            '        if os.get_terminal_size().columns < 80:\n'
            '            shown = ("[Pasted Content %d chars]" % len(self.buffer)) if self.buffer else "\\x1b[2mAsk Codex to do anything\\x1b[0m"\n'
            '            self.write("\\x1b[2J\\x1b[H› " + shown + "\\r\\n\\r\\ngpt-5.6-luna low · /test\\x1b[1;3H")\n'
            '            return\n'
            '        return super().render(working)\n'
            'RotatingFake(parse(sys.argv[1:])).run()\n')
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'codex-cli-v1', 'source': 'codex',
                'executable': str(executable),
                'args': [str(cli), '--model', 'gpt-5.6-luna', '--reply'],
                'new_args': [], 'resume_args': ['resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                        'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                        'SESSIONDOCK_TEST_STARTUP_DELAY': '0',
                        'SESSIONDOCK_TEST_CODEX_ROOT': str(root / 'codex')}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger')
        node = SimpleNamespace(nid='a' * 32, name='RotationNode', port=free_port(), token='f' * 64)
        extra = {}
        if hub_mode:
            for name, value in [('node-token', node.token), ('node-id', node.nid)]:
                path = root / name
                path.touch(mode=0o600)
                path.write_text(value)
            extra = {'SESSIONDOCK_NODE_BIND': f'127.0.0.1:{node.port}',
                     'SESSIONDOCK_NODE_TOKEN_FILE': str(root / 'node-token'),
                     'SESSIONDOCK_NODE_ID_FILE': str(root / 'node-id'),
                     'SESSIONDOCK_NODE_PEERS': '127.0.0.0/8'}
        view_uid = lambda uid: scoped(node.nid, uid) if hub_mode else uid
        with isolated_server(corpus, BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, state_dir=root / 'state', extra_env=extra,
                file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _):
            hub = Hub(REPO / 'target/debug/sessiondock-hub', root / 'hub', [node]) if hub_mode else None
            if hub:
                hub.start()
                base = f'http://127.0.0.1:{hub.port}'
            context = browser.new_context(service_workers='block', color_scheme='dark' if hub_mode else 'light',
                viewport={'width': 1280, 'height': 900})
            context.route('**/*', lambda route: route.continue_()
                          if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            try:
                page.goto(base, wait_until='networkidle')
                uid = view_uid(corpus.uid(CODEX_SID))
                resume_codex(page, uid)
                instance = page.evaluate('T.views.get(T.name).instanceId')
                page.locator('#a-chat').click()
                if hub_mode:
                    page.set_viewport_size({'width': 390, 'height': 900})
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'")
                for generation in (1, 2):
                    # Both populated and empty migrated drafts carry revisions;
                    # neither revision belongs to the next native file's draft.
                    text = f'回退后继续处理第 {generation} 页 / preserve this draft'
                    page.locator('#cinput').fill(text)
                    page.evaluate('async () => await composerDraftWrites')
                    if generation == 2:
                        page.locator('#cinput').fill('')
                        page.evaluate('async () => await composerDraftWrites')
                    previous_revision = page.evaluate('composerDraft().revision')
                    assert previous_revision > 0
                    page.screenshot(path=str(screenshots / f'{mode}-{generation}-before.png'))
                    page.locator('#a-term').click()
                    native = original.read_bytes()
                    key = f'rotation-{generation}'
                    rotated = corpus.put(key, 'codex', [
                        codex_row('session_meta', {'id': CODEX_SID, 'cwd': str(root / 'work'),
                            'timestamp': f'2026-09-11T{8 + generation:02}:00:00Z',
                            'history_base': {'thread_id': CODEX_SID, 'end_byte_offset': len(native),
                                             'end_ordinal_exclusive': 2}}, 2),
                        codex_message('user', f'Rotated conversation {generation}', 3)], [])
                    marker.write_text(str(rotated))
                    uid = view_uid(corpus.uid(key))
                    page.evaluate('async () => { await loadSessions(true); await loadTermList(); }')
                    try:
                        page.wait_for_function('uid => S.sel === uid && T.uid === uid', arg=uid, timeout=10000)
                    except Exception:
                        print('rotation state', page.evaluate('({sel:S.sel, term:T.uid, rows:T.list, sessions:S.sessions})'), flush=True)
                        raise
                    row = context.request.get(base + '/api/session/conversation', params={'uid': uid}).json()['draft']
                    assert row['revision'] < previous_revision, row
                    assert page.evaluate('uid => T.list.find(row => row.current_uid === uid)?.instance_id', uid) == instance
                    if page.locator('#termpane').is_visible():
                        page.locator('#a-chat').click()
                    expect(page.locator('#cinput')).to_have_value(text if generation == 1 else '')
                    text += '，现在发送'
                    page.locator('#cinput').fill(text)
                    page.evaluate('async () => await composerDraftWrites')
                    page.screenshot(path=str(screenshots / f'{mode}-{generation}-saved.png'))
                    expect(page.locator('.draft-save-error')).to_have_count(0)
                    saved = context.request.get(base + '/api/session/conversation', params={'uid': uid}).json()['draft']
                    assert saved['value']['text'] == text, saved
                    assert page.evaluate('composerDraft().revision') == saved['revision']
                    page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=5000)
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200 and sent.value.json()['state'] == 'sent', sent.value.text()
                    expect(page.locator('#cinput')).to_have_value('')
                    expect(page.locator('#msgs')).to_contain_text('OK: ' + text)
                    assert [r['content'][0]['text'] for r in user_records(rotated)].count(text) == 1
                    page.screenshot(path=str(screenshots / f'{mode}-{generation}-sent.png'))
                assert not errors, errors
                print(f'PASS draft rotation {mode}: populated/empty drafts, lower revision, save, SEND, single native echo', flush=True)
            except Exception:
                print('failed composer', page.evaluate('({sel:S.sel, rows:T.list, draft:composerDraft()})'), flush=True)
                raise
            finally:
                context.close()
                if hub:
                    hub.stop()


def main():
    with sync_playwright() as pw:
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        try:
            rotation(browser, False)
            rotation(browser, True)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
