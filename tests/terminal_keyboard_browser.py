#!/usr/bin/env python3
"""Keyboard inset keeps short prompts and bottom editors visible without SIGWINCH."""
import json
import shlex
import sys
import tempfile
import uuid
from pathlib import Path

from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
import terminal_input_browser as fixture

CLI = r'''
import os, tty
tty.setraw(0)
os.write(1, b'RS_SHELL_READY\r\n')
while True:
    key = os.read(0, 1)
    rows = os.get_terminal_size(0).lines
    if key == b't':
        screen = '\x1b[2J\x1b[HWORKSPACE TRUST\r\n' + '\r\n'.join(
            ['Review this workspace'] * 10) + '\r\nNo, exit\r\nYes, trust\r\nEnter to confirm'
    elif key == b'b':
        screen = f'\x1b[2J\x1b[H\x1b[{rows-2};1HEDITOR >\r\nSTATUS FOOTER'
    elif key == b'c':
        screen = f'\x1b[2J\x1b[H\x1b[{rows};1HFOOTER\x1b[2;1HCURSOR >'
    else:
        continue
    os.write(1, screen.encode())
'''

GEOMETRY = """() => {
  const v=currentTermViewObject(), t=v.term, b=t.buffer.active;
  const host=v.host.getBoundingClientRect();
  const screen=v.host.querySelector(v.grid ? 'canvas' : '.xterm-screen').getBoundingClientRect();
  const ch=screen.height/t.rows;
  return {top:screen.top-host.top, bottom:screen.bottom-host.top, height:host.height,
    cursor:screen.top-host.top+(b.cursorY+1)*ch, ch, rows:t.rows, cols:t.cols};
}"""


def run(browser, renderer, scale=100):
    with tempfile.TemporaryDirectory(prefix='sessiondock-keyboard-') as tmp:
        root = Path(tmp)
        for name in ('host', 'work', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
                                codex_message('user', 'Synthetic keyboard viewport')], [])
        uid = corpus.uid(sid)
        with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid), \
                isolated_server(corpus, BINARY, host_dir=root / 'host') as (base, _):
            context = browser.new_context(viewport={'width': 608, 'height': 761}, service_workers='block')
            context.add_init_script('localStorage.setItem("sessiondock.consoleRenderer", JSON.stringify(%s))' % json.dumps(renderer))
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda e: errors.append(str(e)))
            context.add_init_script(f'localStorage.setItem("sessiondock.interfaceScale", "{scale}")')
            page.goto(base, wait_until='networkidle')
            fixture.open_console(page, uid)
            page.wait_for_timeout(300)
            pane = page.locator('#termpane').bounding_box()
            heading = page.locator('#detail > .dhead').bounding_box()
            assert abs(pane['y'] - heading['y'] - heading['height']) <= 1, (scale, pane, heading)
            keys = page.locator('#termpane .xterm-helper-textarea')
            keys.press('t')
            fixture.xterm_contains(page, 'Enter to confirm')
            before = page.evaluate(GEOMETRY)
            assert before['rows'] > 16, before
            page.evaluate("""() => {
              window.keyboardResizes=[];
              const ws=T.ws, send=ws.send.bind(ws);
              ws.send=data => {
                if(typeof data==='string' && JSON.parse(data).t==='resize') keyboardResizes.push(data);
                return send(data);
              };
            }""")
            page.set_viewport_size({'width': 608, 'height': 530})
            page.wait_for_function('visualKeyboardOpen()')
            page.wait_for_timeout(300)
            short = page.evaluate(GEOMETRY)
            assert abs(short['top']) < 1, (renderer, 'short prompt clipped', short)
            assert short['cursor'] <= short['height'] + 1, short
            keys.press('b')
            fixture.xterm_contains(page, 'STATUS FOOTER')
            page.wait_for_timeout(150)
            bottom = page.evaluate(GEOMETRY)
            assert bottom['top'] < -100 and 0 < bottom['cursor'] <= bottom['height'] + 1, bottom
            # A footer below a cursor near the top must not hide the cursor.
            keys.press('c')
            fixture.xterm_contains(page, 'CURSOR >')
            page.wait_for_timeout(150)
            cursor = page.evaluate(GEOMETRY)
            assert cursor['ch'] - 1 <= cursor['cursor'] <= cursor['height'] + 1, cursor
            keys.press('t')
            fixture.xterm_contains(page, 'Enter to confirm')
            page.wait_for_timeout(150)
            assert abs(page.evaluate(GEOMETRY)['top']) < 1
            assert page.evaluate('keyboardResizes') == []
            after = page.evaluate(GEOMETRY)
            assert (after['rows'], after['cols']) == (before['rows'], before['cols'])
            # Reopen with the keyboard already up and restore the active screen.
            page.locator('#a-term').click()
            page.locator('#a-term').click()
            page.wait_for_timeout(300)
            reopened = page.evaluate(GEOMETRY)
            assert abs(reopened['top']) < 1, (renderer, 'reopen', reopened)
            page.set_viewport_size({'width': 608, 'height': 761})
            page.wait_for_function('!visualKeyboardOpen()')
            page.wait_for_timeout(200)
            assert abs(page.evaluate(GEOMETRY)['top']) < 1
            # Safari/Android can resize only visualViewport, without window.resize.
            keys.press('b')
            fixture.xterm_contains(page, 'STATUS FOOTER')
            page.evaluate("""() => {
              Object.defineProperty(visualViewport, 'height', {configurable:true, get:() => 493});
              visualViewport.dispatchEvent(new Event('resize'));
            }""")
            page.wait_for_timeout(300)
            visual = page.evaluate(GEOMETRY)
            assert visual['top'] < -100 and 0 < visual['cursor'] <= visual['height'] + 1, visual
            page.evaluate("""() => {
              delete visualViewport.height;
              visualViewport.dispatchEvent(new Event('resize'));
            }""")
            page.wait_for_timeout(200)
            assert abs(page.evaluate(GEOMETRY)['top']) < 1
            page.set_viewport_size({'width': 1280, 'height': 900})
            page.wait_for_timeout(300)
            assert abs(page.evaluate(GEOMETRY)['top']) < 1
            assert not errors, errors
            context.close()
            print('PASS', renderer, scale, 'short menu, bottom editor, cursor, keyboard resize, reopen, desktop', flush=True)


def main():
    fixture.SHELL = 'exec ' + shlex.quote(sys.executable) + ' -u -c ' + shlex.quote(CLI)
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        for renderer in ('grid', 'xterm'):
            for scale in (75, 100, 150):
                run(browser, renderer, scale)
        browser.close()


if __name__ == '__main__':
    main()
