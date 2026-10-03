#!/usr/bin/env python3
"""Grok SEND retires the queued bubble once the user line is on screen.

Grok ``chat_history.jsonl`` user records have no ``timestamp``. The sent
text used to keep the composer waiting after that line was already visible
(BUG-20260927-211850-1fa082). An older identical line must not retire a later
send. Private fake CLI, loopback server, temporary directories only.
"""
from browser_runtime import js
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, REPO, Corpus, isolated_server
from send_browser import initialize
from popups import on_popup  # noqa: E402


def uid_for(path):
    return 'grok:' + hashlib.sha1(str(path).encode()).hexdigest()[:16]


def user_line(text, prompt_index):
    body = '<user_query>\n' + text + '\n</user_query>'
    return json.dumps({
        'type': 'user',
        'prompt_index': prompt_index,
        'content': [{'type': 'text', 'text': body}],
    }, ensure_ascii=False).encode() + b'\n'


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-grok-echo-') as temporary:
        root = Path(temporary).resolve()
        for name in ('host', 'work', 'ledger', 'delivery', 'state', 'home', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'grok-cli-v1', 'source': 'grok',
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(REPO / 'tests/fake_grok_echo.py')],
                'new_args': ['--session-id', '{session_id}'],
                'resume_args': ['--resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                    'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                    'SESSIONDOCK_GROK_ROOT': str(root / 'grok'),
                    'SESSIONDOCK_TEST_CWD': str(root / 'work')}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger')
        with isolated_server(Corpus(root), BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, state_dir=root / 'state',
                file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(viewport={'width': 390, 'height': 844}, service_workers='block')
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page()
            errors, dialogs = [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
            try:
                page.goto(base, wait_until='networkidle')
                if not page.locator('#new-session').is_visible():
                    page.locator('#header-more-btn').click()
                page.locator('#new-session').click()
                page.locator('input[name="new-source"][value="grok"]').check()
                page.locator('#new-cwd').fill(str(root / 'work'))
                with page.expect_response(lambda response: urlsplit(response.url).path == '/api/term/create') as created:
                    page.locator('#new-session-go').click()
                assert created.value.status == 200, created.value.text()
                receipt = created.value.json()
                if not page.locator('#composer').is_visible():
                    page.evaluate(js('showMobileDetail()', 'runtime.shell.showMobileDetail()'))
                expect(page.locator('#composer')).to_be_visible()
                sid = receipt['declared_sid']
                chat = root / 'grok' / 'echo' / sid / 'chat_history.jsonl'
                native = uid_for(chat.parent)
                page.wait_for_function(js('uid => S.sel === uid', 'uid => runtime.core.state.selection.sel === uid'), arg=native, timeout=20000)
                page.wait_for_function(js("() => composerDraft()?.inputStatus?.state === 'ready'", "() => runtime.composer.composerDraft()?.inputStatus?.state === 'ready'"), timeout=15000)
                expect(page.locator('#csend')).to_be_enabled()

                def append(text, prompt_index):
                    with chat.open('ab') as handle:
                        handle.write(user_line(text, prompt_index))

                def user_count(text):
                    return page.locator('#msgs .msg[data-role=user]:not(.queued-send)').filter(has_text=text).count()

                append('claude我已经卸载。cygnus上有', 0)
                page.wait_for_function(
                    "text => [...document.querySelectorAll('#msgs .msg[data-role=user]:not(.queued-send)')].some(n => n.textContent.includes(text))",
                    arg='claude我已经卸载。cygnus上有', timeout=15000)
                stamps = page.evaluate(js("() => (cache.get(S.sel)?.msgs || []).filter(m => m.role === 'user').map(m => m.ts)", "() => (runtime.core.cache.cache.get(runtime.core.state.selection.sel)?.msgs || []).filter(m => m.role === 'user').map(m => m.ts)"))
                assert stamps == [None], stamps

                def send(text):
                    page.locator('#cinput').fill(text)
                    with page.expect_response(lambda response: urlsplit(response.url).path == '/api/session/conversation/send', timeout=20000) as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200, sent.value.text()
                    assert sent.value.json()['state'] == 'sent', sent.value.text()
                    expect(page.locator('#cinput')).to_have_value('')
                    expect(page.locator('#csend')).to_have_attribute('aria-busy', 'false')
                    expect(page.locator('#queued-sends .msg.queued-send[data-role=user]').filter(has_text=text)).to_have_count(1)

                send('新的一句')
                assert user_count('新的一句') == 0
                # The queue is server state (docs/cli-state.md): a reloaded page
                # shows the bubble again before any echo exists.
                page.evaluate(js('async () => await composerDraftWrites', 'async () => await runtime.composer.composerDraftWrites'))
                page.reload(wait_until='networkidle')
                page.wait_for_function(js("composerUid && !composerDraft().loading", 'runtime.composer.composerUid && !runtime.composer.composerDraft().loading'), timeout=15000)
                expect(page.locator('#queued-sends .msg.queued-send[data-role=user]').filter(has_text='新的一句')).to_have_count(1, timeout=15000)
                expect(page.locator('#csend')).to_have_attribute('aria-busy', 'false')
                append('新的一句', 1)
                page.wait_for_function(
                    "() => [...document.querySelectorAll('#msgs .msg[data-role=user]:not(.queued-send)')].filter(n => n.textContent.includes('新的一句')).length === 1",
                    timeout=15000)
                expect(page.locator('#queued-sends')).to_have_count(0)
                expect(page.locator('#csend')).to_have_attribute('aria-busy', 'false')

                send('claude我已经卸载。cygnus上有')
                assert user_count('claude我已经卸载。cygnus上有') == 1
                expect(page.locator('#queued-sends .msg.queued-send')).to_have_count(1)
                append('claude我已经卸载。cygnus上有', 2)
                page.wait_for_function(
                    "() => [...document.querySelectorAll('#msgs .msg[data-role=user]:not(.queued-send)')].filter(n => n.textContent.includes('claude我已经卸载。cygnus上有')).length === 2",
                    timeout=15000)
                expect(page.locator('#queued-sends')).to_have_count(0)
                expect(page.locator('#csend')).to_have_attribute('aria-busy', 'false')
                assert not dialogs, dialogs
                assert not errors, errors
            finally:
                context.close()
                browser.close()


if __name__ == '__main__':
    main()
