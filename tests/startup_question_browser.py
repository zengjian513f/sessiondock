#!/usr/bin/env python3
"""Pre-rollout Codex folder trust answered in the composer (free fake CLI)."""

import json
import os
from pathlib import Path
import select
import sys
import tempfile
import termios
import textwrap
import tty
from urllib.parse import urlsplit


DISCLOSURE = ("Trust this folder? Codex can read, edit, and run files here, subject to your permission settings. "
              "Folder settings can run code automatically, even without a model request. "
              "Continue only if you trust these files. Your trust decision will be saved.")


def frame(folder, selected=2):
    return (f"  Folder access\n  {folder}\n\n" + "\n".join(textwrap.wrap(DISCLOSURE, 70)) + "\n\n"
            + ("›" if selected == 1 else " ") + " 1. Trust and continue\n"
            + ("›" if selected == 2 else " ") + " 2. Quit\n\n  enter continue · esc quit\n")


def fake():
    """Codex protected trust semantics: 1 highlights, Enter confirms; 2 quits."""
    root = Path(os.environ['SESSIONDOCK_TEST_SCREEN'])
    original = termios.tcgetattr(0)
    tty.setraw(0)
    previous, selected, trusted, editor = None, 2, False, b''
    try:
        while True:
            state = root.read_text()
            if state != previous:
                text = frame(Path.cwd(), selected) if state == 'trust' else (
                    '› \n\n  gpt-5.6-luna · test' if state == 'ready' else state)
                sys.stdout.write('\x1b[2J\x1b[H' + text.replace('\n', '\r\n')
                                 + ('\x1b[1;3H' if state == 'ready' else ''))
                sys.stdout.flush()
                previous = state
            if not select.select([0], [], [], .02)[0]:
                continue
            data = os.read(0, 4096)
            with root.with_suffix('.trace').open('ab') as trace:
                trace.write(data)
            if state != 'ready':
                for byte in data:
                    if byte == ord('1'):
                        selected = 1
                    elif byte in (ord('2'), 27):
                        root.with_suffix('.outcome').write_text('quit')
                        return
                    elif byte == 13:
                        root.with_suffix('.outcome').write_text('trust' if selected == 1 else 'quit')
                        if selected != 1:
                            return
                        trusted = True
                        root.write_text('ready')
            else:
                assert trusted
                editor += data
                # Enough native editor feedback for SEND's post-paste checkpoint.
                text = editor.replace(b'\x1b[200~', b'').replace(b'\x1b[201~', b'').decode()
                sys.stdout.write('\x1b[2J\x1b[H› ' + text + '\r\n\r\n  gpt-5.6-luna · test')
                sys.stdout.flush()
    finally:
        termios.tcsetattr(0, termios.TCSANOW, original)


def main():
    from playwright.sync_api import expect, sync_playwright
    from history_parity import REPO, BINARY, Corpus, isolated_server
    from private_hosts import private_hosts
    from send_browser import initialize
    from popups import on_popup
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=BINARY)
    BINARY = parser.parse_args().binary.resolve()
    with tempfile.TemporaryDirectory(prefix='sessiondock-startup-question-') as temporary, private_hosts(Path(temporary)):
        root = Path(temporary).resolve()
        for name in ['host', 'work', 'ledger', 'state', 'home', 'claude', 'codex', 'grok']:
            (root / name).mkdir(mode=0o700)
        screen = root / 'screen'
        screen.write_text('trust')
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'codex-cli-v1', 'source': 'codex',
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(Path(__file__).resolve()), '--fake'],
                'new_args': [], 'resume_args': ['resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                        'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                        'SESSIONDOCK_TEST_SCREEN': str(screen)}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger')
        with isolated_server(Corpus(root), BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, state_dir=root / 'state',
                file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(service_workers='block')
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page()
            errors, dialogs, writes = [], [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
            page.on('request', lambda request: writes.append(request.post_data_json)
                    if urlsplit(request.url).path == '/api/term/send' else None)
            try:
                page.goto(base, wait_until='networkidle')

                def create():
                    if not page.locator('#new-session').is_visible():
                        page.locator('#header-more-btn').click()
                    page.locator('#new-session').click()
                    page.locator('input[name="new-source"][value="codex"]').check()
                    page.locator('#new-cwd').fill(str(root / 'work'))
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/create') as created:
                        page.locator('#new-session-go').click()
                    assert created.value.status == 200, created.value.text()
                    receipt = created.value.json()
                    assert receipt['launch_kind'] == 'new_pending', receipt
                    return receipt

                receipt = create()
                card = page.locator('#composer-question')
                expect(card).to_be_visible(timeout=15000)
                expect(card).to_contain_text(str(root / 'work'))
                expect(card).to_contain_text(DISCLOSURE)
                expect(card.locator('.msg.question.live-question > .question-body')).to_have_count(1)
                expect(card.locator('.question-item > .question-header')).to_have_text('目录信任确认')
                expect(card.locator('.question-options > button > span')).to_have_text(['1', '2'])
                expect(card.locator('.question-options > button b')).to_have_text(['信任并继续', '退出'])
                expect(card.locator('.question-actions button')).to_have_text(['打开终端', '取消'])
                expect(page.locator('#termpane')).to_be_hidden()
                assert page.evaluate('T.views.size === 0')
                assert not list((root / 'codex').rglob('*.jsonl'))
                expect(page.locator('#csend')).to_be_disabled()
                page.locator('#cinput').fill('keep this draft')
                page.evaluate('async () => await composerDraftWrites')
                page.locator('#cinput').press('Enter')
                page.wait_for_timeout(1800)
                assert not writes and not screen.with_suffix('.trace').exists()
                expect(page.locator('#cinput')).to_have_value('keep this draft')
                assert page.evaluate("document.activeElement.id === 'cinput'")
                # Quoted or partial menus must not remain actionable.
                screen.write_text(frame(root / 'work') + '› ordinary editor after quoted dialog')
                expect(card).to_be_hidden(timeout=10000)
                screen.write_text(frame(root / 'work').replace('enter continue · esc quit', ''))
                expect(card).to_be_hidden()
                page.wait_for_timeout(1800)
                assert not writes
                screen.write_text('trust')
                expect(card).to_be_visible(timeout=10000)
                # Recheck catches a menu changed between rendering and clicking.
                screen.write_text('login screen')
                page.wait_for_timeout(80)
                card.locator('[data-question-option="0"]').click()
                expect(card).to_be_hidden(timeout=10000)
                assert not writes
                screen.write_text('trust')
                expect(card).to_be_visible(timeout=10000)
                # Both desktop and phone keep the disclosure, choices and draft.
                page.set_viewport_size({'width': 390, 'height': 844})
                page.evaluate('showMobileDetail()')
                expect(card).to_be_visible()
                buttons = card.locator('.question-options > button').all()
                first, second = [button.bounding_box() for button in buttons]
                assert first and second and second['y'] >= first['y'] + first['height'], (first, second)
                assert abs(first['width'] - second['width']) < 1, (first, second)
                assert first['x'] >= 0 and first['x'] + first['width'] <= 390, first
                card.locator('[data-question-option="0"]').click()
                expect(card).to_be_hidden(timeout=10000)
                expect(page.locator('#csend')).to_be_enabled(timeout=10000)
                expect(page.locator('#cinput')).to_have_value('keep this draft')
                assert screen.with_suffix('.outcome').read_text() == 'trust'
                assert screen.with_suffix('.trace').read_bytes() == b'1\r'
                assert writes[-1]['keys'] == ['1', 'Enter'] and writes[-1]['token'] == ''
                assert writes[-1]['record_id'] == receipt['record_id']
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                    page.locator('#csend').click()
                assert sent.value.status == 200, sent.value.text()
                assert b'keep this draft' in screen.with_suffix('.trace').read_bytes()
                # A second untouched launch takes the negative path explicitly.
                screen.write_text('trust')
                page.set_viewport_size({'width': 1280, 'height': 720})
                page.evaluate('showMobileList()')
                create()
                expect(card).to_be_visible(timeout=10000)
                page.locator('#cinput').fill('retain after quit')
                page.evaluate('async () => await composerDraftWrites')
                card.locator('.question-cancel').click()
                page.wait_for_function("sessionComposerEnded(S.sel)", timeout=15000)
                assert screen.with_suffix('.outcome').read_text() == 'quit'
                assert writes[-1]['keys'] == ['2']
                assert page.evaluate('composerDraft().text') == 'retain after quit'
                assert not errors and not dialogs, (errors, dialogs)
            finally:
                browser.close()
    print('PASS startup_question_browser: pre-rollout trust/quit, disclosure, draft, readiness, stale menu, phone and guarded keys')


if __name__ == "__main__":
    if '--fake' in sys.argv:
        fake()
    else:
        main()
