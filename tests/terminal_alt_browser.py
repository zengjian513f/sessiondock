#!/usr/bin/env python3
"""Mobile Alt reaches the PTY with physical-key bytes in both renderers."""
import json
import shlex
import sys
import tempfile
import uuid
from pathlib import Path

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
import terminal_input_browser as fixture

CLI = r'''
import os, tty
tty.setraw(0)
os.write(1, b'RS_SHELL_READY\r\n')
received = b''
while True:
    received += os.read(0, 1024)
    os.write(1, ('\r\nBYTES:' + received.hex() + '\r\n').encode())
'''


def run(browser, renderer):
    with tempfile.TemporaryDirectory(prefix='sessiondock-alt-') as tmp:
        root = Path(tmp)
        for name in ('host', 'work', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
                                codex_message('user', 'Synthetic Alt input')], [])
        uid = corpus.uid(sid)
        with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid), \
                isolated_server(corpus, BINARY, host_dir=root / 'host') as (base, _):
            context = browser.new_context(viewport={'width': 390, 'height': 844}, service_workers='block')
            context.add_init_script('localStorage.setItem("sessiondock.consoleRenderer", JSON.stringify(%s))' % json.dumps(renderer))
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda e: errors.append(str(e)))
            page.goto(base, wait_until='networkidle')
            fixture.open_console(page, uid)
            assert page.evaluate('!!currentTermViewObject().grid') == (renderer == 'grid')
            alt = page.locator('[data-term-modifier="alt"]')
            ctrl = page.locator('[data-term-modifier="ctrl"]')
            up = page.locator('[data-term-key="Up"]')
            keyboard = page.locator('#termpane .xterm-helper-textarea')
            received = b''

            def delivered(data):
                nonlocal received
                received += data
                page.wait_for_function("needle => (" + fixture.XTERM_TEXT +
                                       ")().replaceAll('\\n', '').includes(needle)",
                                       arg='BYTES:' + received.hex())
                expect(alt).to_have_attribute('aria-pressed', 'false')

            alt.click()
            expect(alt).to_have_attribute('aria-pressed', 'true')
            up.click()
            delivered(b'\x1b[1;3A')
            up.click()
            delivered(b'\x1b[A')
            alt.click()
            keyboard.press('x')
            delivered(b'\x1bx')
            alt.click()
            alt.click()
            keyboard.press('y')
            delivered(b'y')
            alt.click()
            keyboard.press('ArrowUp')
            delivered(b'\x1b[1;3A')
            alt.click()
            ctrl.click()
            up.click()
            delivered(b'\x1b[1;7A')
            expect(ctrl).to_have_attribute('aria-pressed', 'false')
            alt.click()
            page.locator('[data-term-key="PPage"]').click()
            delivered(b'\x1b[5;3~')
            alt.click()
            page.locator('#a-term').click()
            page.locator('#a-term').click()
            expect(alt).to_have_attribute('aria-pressed', 'false')
            assert not errors, errors
            context.close()
            print('PASS', renderer, 'Alt+Up PTY bytes, one-shot, character, cancel, physical arrow, Ctrl+Alt, page, close reset', flush=True)


def main():
    fixture.SHELL = 'exec ' + shlex.quote(sys.executable) + ' -u -c ' + shlex.quote(CLI)
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        for renderer in ('grid', 'xterm'):
            run(browser, renderer)
        browser.close()


if __name__ == '__main__':
    main()
