#!/usr/bin/env python3
"""Claude workspace trust before hooks/history, through shared composer cards."""

import json
import os
from pathlib import Path
import select
import sys
import tempfile
import termios
import time
import tty
from urllib.parse import urlsplit

DISCLOSURE = ("Quick safety check: Is this a project you created or one you trust?\n"
              "If not, take a moment to review what's in this folder first.\n\n"
              "Claude Code'll be able to read, edit, and execute files here.\n\nSecurity guide")


def frame(folder, state):
    selected = 1 if state == 'yes' else 0
    labels = ['No, exit', 'Yes, I trust this folder']
    if state == 'reverse':
        labels.reverse()
        selected = 1
    rows = [(' ❯ ' if i == selected else '   ') + label for i, label in enumerate(labels)]
    return ('\n' + '─' * 70 + f'\n Accessing workspace:\n\n {folder}\n\n'
            + DISCLOSURE + '\n\n' + '\n'.join(rows) + '\n\n Enter to confirm · Esc to cancel\n')


def fake():
    """Two-row menu with a movable selection; no transcript until trust."""
    from fake_claude_cli import Fake, parse
    screen = Path(os.environ['SESSIONDOCK_TEST_SCREEN'])
    original = termios.tcgetattr(0)
    tty.setraw(0)
    previous, selected, labels, pending = None, 0, [], b''
    trusted = False
    try:
        while True:
            state = screen.read_text()
            if state != previous:
                labels = ['No, exit', 'Yes, I trust this folder']
                selected = 1 if state == 'yes' else 0
                if state == 'reverse':
                    labels.reverse()
                    selected = 1
                text = frame(Path.cwd(), state) if state in ('no', 'yes', 'reverse') else state
                sys.stdout.write('\x1b[2J\x1b[H' + text.replace('\n', '\r\n'))
                sys.stdout.flush()
                previous = state
            if not select.select([0], [], [], .02)[0]:
                continue
            data = os.read(0, 4096)
            with screen.with_suffix('.trace').open('ab') as trace:
                trace.write(data)
            pending += data
            while pending:
                if pending.startswith((b'\x1b[A', b'\x1bOA')):
                    selected = max(0, selected - 1)
                    pending = pending[3:]
                elif pending.startswith((b'\x1b[B', b'\x1bOB')):
                    selected = min(1, selected + 1)
                    pending = pending[3:]
                elif pending == b'\x1b' and select.select([0], [], [], .05)[0]:
                    break
                elif pending.startswith((b'\x1b[', b'\x1bO')) and len(pending) < 3:
                    break
                elif pending[0] == 27 or pending[0] == 13:
                    trusted = pending[0] == 13 and labels[selected] == 'Yes, I trust this folder'
                    screen.with_suffix('.outcome').write_text('trust' if trusted else 'quit')
                    return trusted
                else:
                    pending = pending[1:]
    finally:
        termios.tcsetattr(0, termios.TCSANOW, original)
        if trusted:
            Fake(parse(sys.argv[1:])).run()


def main():
    import argparse
    from playwright.sync_api import expect, sync_playwright
    from history_parity import REPO, BINARY, Corpus, isolated_server
    from send_browser import initialize, create_claude, wait_history
    from popups import on_popup
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, default=BINARY)
    binary = parser.parse_args().binary.resolve()
    with tempfile.TemporaryDirectory(prefix='sessiondock-startup-claude-') as temporary:
        root = Path(temporary).resolve()
        for name in ['host', 'work', 'work/claude-area', 'ledger', 'state', 'home', 'claude', 'codex', 'grok']:
            (root / name).mkdir(mode=0o700)
        screen = root / 'screen'
        screen.write_text('no')
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'claude-cli-v1', 'source': 'claude',
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(Path(__file__).resolve()), '--fake', '--reply'],
                'new_args': ['--session-id', '{session_id}'], 'resume_args': ['--resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                        'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                        'SESSIONDOCK_TEST_SCREEN': str(screen),
                        'SESSIONDOCK_TEST_CLAUDE_ROOT': str(root / 'claude')}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger')
        with isolated_server(Corpus(root), binary, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
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
                card = page.locator('#composer-question')

                def launch(state):
                    screen.write_text(state)
                    page.set_viewport_size({'width': 1280, 'height': 720})
                    page.evaluate('showMobileList()')
                    receipt = create_claude(page, base, root / 'work', open_terminal=False)
                    expect(card).to_be_visible(timeout=15000)
                    assert not (root / 'claude/project-history' / (receipt['declared_sid'] + '.jsonl')).exists()
                    expect(card.locator('.question-text')).to_contain_text(str(root / 'work/claude-area'))
                    expect(card.locator('.question-text')).to_contain_text(DISCLOSURE)
                    expect(card.locator('.question-options > button > span')).to_have_text(['1', '2'])
                    expect(card.locator('.question-options button b')).to_have_text(['信任并继续', '退出'])
                    expect(page.locator('#csend')).to_be_disabled()
                    page.locator('#cinput').fill('keep Claude draft')
                    page.evaluate('async () => await composerDraftWrites')
                    return receipt

                def trust(keys, mobile=False):
                    if mobile:
                        page.set_viewport_size({'width': 390, 'height': 844})
                        page.evaluate('showMobileDetail()')
                    card.locator('[data-question-option="0"]').click()
                    expect(card).to_be_hidden(timeout=10000)
                    expect(page.locator('#csend')).to_be_enabled(timeout=10000)
                    expect(page.locator('#cinput')).to_have_value('keep Claude draft')
                    assert screen.with_suffix('.outcome').read_text() == 'trust'
                    assert writes[-1]['keys'] == keys and writes[-1]['token'] == ''

                launch('no')
                page.locator('#cinput').press('Enter')
                page.wait_for_timeout(1600)
                assert not writes and not screen.with_suffix('.trace').exists()
                screen.write_text(frame(root / 'work/claude-area', 'no') + '❯ ordinary editor below quoted menu')
                expect(card).to_be_hidden(timeout=10000)
                screen.write_text('no')
                expect(card).to_be_visible(timeout=10000)
                # Fresh CHECK must use a cursor changed since the card rendered.
                screen.write_text('yes')
                page.wait_for_timeout(80)
                trust(['Enter'], mobile=True)
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                    page.locator('#csend').click()
                assert sent.value.status == 200, sent.value.text()
                wait_history(page, 'keep Claude draft')
                # Default exit selection and reversed order both navigate correctly.
                launch('no')
                trust(['Down', 'Enter'])
                launch('reverse')
                trust(['Up', 'Enter'])
                # Both explicit exit and the shared card's Cancel retain the draft.
                for selector in ['[data-question-option="1"]', '.question-cancel']:
                    launch('yes')
                    card.locator(selector).click()
                    page.wait_for_function('sessionComposerEnded(S.sel)', timeout=15000)
                    assert screen.with_suffix('.outcome').read_text() == 'quit'
                    assert writes[-1]['keys'] == ['Escape']
                    assert page.evaluate('composerDraft().text') == 'keep Claude draft'
                assert not errors and not dialogs, (errors, dialogs)
            finally:
                browser.close()
    print('PASS startup_claude_browser: pre-history shared card, current cursor, default/reversed options, phone, SEND, exit/cancel and draft retention')


if __name__ == "__main__":
    if '--fake' in sys.argv:
        fake()
    else:
        main()
