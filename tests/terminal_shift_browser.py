#!/usr/bin/env python3
"""Touch Shift shortcuts reach a fake CLI's queued question through the real PTY."""

import shlex
import sys
import tempfile
import uuid
from pathlib import Path

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_message, codex_row, isolated_server
import terminal_input_browser as fixture

CLI = r'''
import os, tty
tty.setraw(0)
os.write(1, b'RS_SHELL_READY\r\nQueued follow-up inputs\r\n? 1 question\r\nshift+left to answer\r\n')
received = b''
opened = False
while True:
    data = os.read(0, 1024)
    received += data
    with open('input.bin', 'ab') as log:
        log.write(data)
    if not opened and b'\x1b[1;2D' in received:
        opened = True
        os.write(1, b'QUESTION_OPEN: Choose a follow-up\r\n')
    if opened and b'\r' in data:
        os.write(1, b'ANSWER_ACCEPTED\r\n')
'''


def run(browser):
    with tempfile.TemporaryDirectory(prefix='sessiondock-shift-') as tmp:
        root = Path(tmp)
        for name in ('host', 'work', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
                                codex_message('user', 'Synthetic queued question')], [])
        uid = corpus.uid(sid)
        with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid), \
                isolated_server(corpus, BINARY, host_dir=root / 'host') as (base, _):
            context = browser.new_context(viewport={'width': 608, 'height': 761},
                                          has_touch=True, is_mobile=True, service_workers='block')
            page = context.new_page()
            errors, sends = [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            context.on('request', lambda request: sends.append(request.post_data_json)
                       if request.url.endswith('/api/term/send') else None)
            page.goto(base, wait_until='networkidle')
            fixture.open_console(page, uid)
            shift = page.locator('[data-term-modifier="shift"]')
            keyboard = page.locator('#termpane .xterm-helper-textarea')
            received = b''

            def key(name, data, modifiers=()):
                nonlocal received
                for modifier in modifiers:
                    page.locator(f'[data-term-modifier="{modifier}"]').tap()
                with page.expect_response(lambda r: r.url.endswith('/api/term/send')) as response:
                    page.locator(f'[data-term-key="{name}"]').tap()
                assert response.value.ok, response.value.text()
                received += data
                # HTTP acknowledgment follows the host write; let the fake CLI consume it.
                for _ in range(100):
                    if (root / 'work/input.bin').exists() and (root / 'work/input.bin').read_bytes() == received:
                        break
                    page.wait_for_timeout(20)
                assert (root / 'work/input.bin').read_bytes() == received
                for modifier in ('shift', 'alt', 'ctrl'):
                    expect(page.locator(f'[data-term-modifier="{modifier}"]')).to_have_attribute('aria-pressed', 'false')

            # Regression: tapping Shift then Left must open the queued question,
            # including when the software keyboard changes the viewport height.
            shift.tap()
            expect(shift).to_have_attribute('aria-pressed', 'true')
            assert not keyboard.evaluate('e => e === document.activeElement')
            page.set_viewport_size({'width': 608, 'height': 493})
            key('Left', b'\x1b[1;2D')
            fixture.xterm_contains(page, 'QUESTION_OPEN: Choose a follow-up')
            keyboard.press('Enter')
            received += b'\r'
            fixture.xterm_contains(page, 'ANSWER_ACCEPTED')
            key('Left', b'\x1b[D')
            key('Tab', b'\x1b[Z', ('shift',))
            for name, final in [('Up', 'A'), ('Down', 'B'), ('Right', 'C')]:
                key(name, ('\x1b[1;2' + final).encode(), ('shift',))
            key('PPage', b'\x1b[5;2~', ('shift',))
            key('NPage', b'\x1b[6;2~', ('shift',))
            key('Left', b'\x1b[1;4D', ('shift', 'alt'))
            key('Left', b'\x1b[1;6D', ('shift', 'ctrl'))
            key('Left', b'\x1b[1;8D', ('shift', 'alt', 'ctrl'))
            shift.tap()
            shift.tap()
            key('Left', b'\x1b[D')
            shift.tap()
            page.locator('#a-term').tap()
            page.locator('#a-term').tap()
            expect(shift).to_have_attribute('aria-pressed', 'false')
            assert len(sends) == 12, sends
            assert not errors, errors
            context.close()
            print('PASS touch Shift+Left opens question, answer, one-shot, arrows, Tab, pages, modifiers, cancel, close', flush=True)


def main():
    fixture.SHELL = 'exec ' + shlex.quote(sys.executable) + ' -u -c ' + shlex.quote(CLI)
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        run(browser)
        browser.close()


if __name__ == '__main__':
    main()
