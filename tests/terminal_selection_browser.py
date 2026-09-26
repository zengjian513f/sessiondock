#!/usr/bin/env python3
"""Real mouse selection/copy with and without CLI mouse capture, in isolated PTYs."""
import os
from pathlib import Path
import shlex
import tempfile
import uuid

from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
import terminal_input_browser as fixture

CLI = r'''
import os, tty
tty.setraw(0)
os.write(1, b'RS_SHELL_READY\r\n')
command = b''
while True:
 data = os.read(0, 1)
 with open('input.bin', 'ab') as log: log.write(data)
 command += data
 if data == b'\r':
  # Mouse reports from the previous check may precede the next command.
  mode = command.rsplit(b'mode:', 1)[-1].strip().decode()
  command = b''
  if mode not in ('none', '1000', '1002', '1003'): continue
  os.write(1, b'\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006h')
  if mode != 'none': os.write(1, ('\x1b[?' + mode + 'h').encode())
  os.write(1, '\x1b[?2004h\x1b[2J\x1b[HSELECT_FIRST second_word\r\nNEXT_LINE 中文查找 中文查找\r\n'.encode())
'''

# Only observe transport writes; all commands and selections use browser input.
OBSERVE = """window.selectionBytes = [];
const send = WebSocket.prototype.send;
WebSocket.prototype.send = function(data) {
  if (typeof data !== 'string') selectionBytes.push(Array.from(new Uint8Array(data)));
  return send.call(this, data);
};"""


def check_surface(pw, surface, mobile=False):
    with tempfile.TemporaryDirectory(prefix='sessiondock-selection-') as temporary:
        root = Path(temporary)
        for name in ['host', 'work', 'claude', 'codex', 'grok']:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
                                codex_message('user', 'Synthetic terminal selection')], [])
        uid = corpus.uid(sid)
        with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid), \
             isolated_server(corpus, BINARY, host_dir=root / 'host') as (base, _):
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(viewport={'width':390 if mobile else 1280,'height':900},
                                          has_touch=mobile, is_mobile=mobile, service_workers='block',
                                          permissions=['clipboard-read','clipboard-write'])
            context.add_init_script(OBSERVE)
            context.add_init_script("localStorage.setItem('sessiondock.consoleRenderer', JSON.stringify(" + repr(surface) + "))")
            context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda e: errors.append(str(e)))
            standalone = surface == 'standalone'
            page.goto(base + ('/grid.html' if standalone else ''), wait_until='networkidle')
            if standalone:
                page.wait_for_function("__grid.model && document.querySelector('#session').options.length > 0")
                page.locator('#connect').click()
                page.wait_for_function("__grid.state.connected && __gridText().includes('RS_SHELL_READY')")
                page.evaluate("""window.selectionTerm = {
                  getSelection() { const s = __grid.state.selection;
                    return s ? __grid.model.selectionText(s.start, s.end) : ''; },
                  get modes() { return {mouseTrackingMode: __grid.model.modes.mouse}; },
                  renderer: __grid.renderer,
                  _canvas: document.querySelector('#grid')
                };""")
                keyboard = page.locator('#keys')
            else:
                fixture.open_console(page, uid)
                page.evaluate('window.selectionTerm = [...T.views.values()][0].term')
                keyboard = page.locator('#termpane .xterm-helper-textarea')
            if mobile:
                check_touch(page, context, root, keyboard, surface)
                assert not errors, errors
                context.close()
                browser.close()
                return
            for mode in ['none', '1000', '1002', '1003']:
                keyboard.focus()
                page.keyboard.type('mode:' + mode)
                page.keyboard.press('Enter')
                expected = ({'none':'none','1000':'press_release','1002':'button_motion','1003':'any_motion'}
                            if standalone else {'none':'none','1000':'vt200','1002':'drag','1003':'any'})[mode]
                page.wait_for_function('mode => selectionTerm.modes.mouseTrackingMode === mode', arg=expected)
                page.wait_for_timeout(100)
                b = page.evaluate("""() => {
                  const t = selectionTerm;
                  if (t._canvas) { const r=t.renderer, b=t._canvas.getBoundingClientRect();
                    return {x:b.x,y:b.y,cw:r.cellWidth,ch:r.cellHeight}; }
                  const b = document.querySelector('.xterm-screen').getBoundingClientRect();
                  return {x:b.x,y:b.y,cw:b.width/t.cols,ch:b.height/t.rows};
                }""")
                # No capture: plain drag. Capture: Shift must bypass the CLI.
                page.evaluate("navigator.clipboard.writeText('selection-sentinel')")
                if mode != 'none': page.keyboard.down('Shift')
                page.mouse.move(b['x'] + .2*b['cw'], b['y'] + .5*b['ch'])
                # xterm's any-motion hover precedes the local drag. Start the
                # transport assertion at mousedown, after that hover is drained.
                page.wait_for_timeout(100)
                page.evaluate('selectionBytes = []')
                before = (root / 'work/input.bin').read_bytes()
                page.mouse.down()
                if surface != 'xterm' and mode == '1003':
                    # The selection owns the gesture until mouseup, even when
                    # the user lets go of Shift before finishing the drag.
                    page.keyboard.up('Shift')
                page.mouse.move(b['x'] + 12.8*b['cw'], b['y'] + .5*b['ch'], steps=12)
                assert page.evaluate('navigator.clipboard.readText()') == 'selection-sentinel'
                selected = page.evaluate('selectionTerm.getSelection()')
                assert selected.strip() == 'SELECT_FIRST', (surface, mode, selected, errors)
                page.mouse.up()
                if mode != 'none': page.keyboard.up('Shift')
                page.wait_for_function("selectionTerm.getSelection() === ''")
                page.wait_for_function("navigator.clipboard.readText().then(t => t.trim() === 'SELECT_FIRST')")
                assert not page.evaluate('selectionBytes'), (surface, mode, page.evaluate('selectionBytes'))
                assert (root / 'work/input.bin').read_bytes() == before, (surface, mode, 'selection reached CLI')
                page.keyboard.press('Control+c')
                page.wait_for_function('selectionBytes.some(bytes => bytes.length === 1 && bytes[0] === 3)')
                assert page.evaluate('navigator.clipboard.readText()').strip() == 'SELECT_FIRST'
                print('PASS', surface, mode, 'copy on mouse release, no PTY input', flush=True)
                # With capture, an ordinary drag must still reach the CLI.
                if mode != 'none':
                    page.evaluate("navigator.clipboard.writeText('remote-gesture-sentinel')")
                    page.evaluate('selectionBytes = []')
                    page.mouse.move(b['x'] + 15*b['cw'], b['y'] + .5*b['ch'])
                    page.mouse.down()
                    page.mouse.move(b['x'] + 20*b['cw'], b['y'] + .5*b['ch'], steps=4)
                    page.mouse.up()
                    page.wait_for_function('selectionBytes.length >= 2')
                    data = bytes(sum(page.evaluate('selectionBytes'), []))
                    assert b'\x1b[<' in data and data.endswith(b'm'), (surface, mode, data)
                    assert page.evaluate('navigator.clipboard.readText()') == 'remote-gesture-sentinel'
            if not standalone:
                keyboard.focus()
                page.keyboard.type('mode:none')
                page.keyboard.press('Enter')
                page.wait_for_function("selectionTerm.modes.mouseTrackingMode === 'none'")
                screen = page.locator('.grid-canvas' if surface == 'grid' else '.xterm-screen')
                def menu():
                    screen.click(button='right', position={'x': 30, 'y': 12})
                    page.locator('.term-context-menu:visible').wait_for()
                menu()
                assert page.locator('.term-context-menu:visible button').all_text_contents() == ['粘贴', '复制全部', '查找']
                # Reuse the existing menu surface, with menu rows rather than
                # individually bordered form buttons, in both themes.
                for theme in ['dark', 'light']:
                    page.evaluate("theme => document.documentElement.dataset.theme = theme", theme)
                    appearance = page.locator('.term-context-menu:visible').evaluate("""menu => {
                      const button = menu.querySelector('button');
                      const style = getComputedStyle(button);
                      return {shared: menu.classList.contains('ctx-menu'), border: style.borderTopWidth,
                        background: style.backgroundColor, size: style.fontSize,
                        width: menu.getBoundingClientRect().width};
                    }""")
                    assert appearance['shared'] and appearance['border'] == '0px', appearance
                    assert appearance['background'] == 'rgba(0, 0, 0, 0)', appearance
                    assert appearance['size'] == '13px' and appearance['width'] >= 150, appearance
                page.get_by_role('menuitem', name='粘贴', exact=True).press('ArrowDown')
                assert page.get_by_role('menuitem', name='复制全部', exact=True).evaluate('e => e === document.activeElement')
                page.get_by_role('menuitem', name='复制全部', exact=True).click()
                page.wait_for_function("navigator.clipboard.readText().then(t => t.includes('SELECT_FIRST second_word') && t.includes('NEXT_LINE'))")
                page.wait_for_function("selectionTerm.getSelection() === ''")
                menu()
                page.get_by_role('menuitem', name='查找', exact=True).click()
                page.get_by_role('searchbox', name='查找终端输出').fill('second_word')
                page.wait_for_function("selectionTerm.getSelection() === 'second_word'")
                page.get_by_role('searchbox', name='查找终端输出').fill('中文查找')
                page.wait_for_function("selectionTerm.getSelection() === '中文查找'")
                count = int(page.locator('.term-find [role=status]').inner_text().split('/')[1])
                assert count >= 2
                assert page.locator('.term-find [role=status]').inner_text() == f'1/{count}'
                page.get_by_role('button', name='下一个', exact=True).click()
                assert page.locator('.term-find [role=status]').inner_text() == f'2/{count}'
                page.get_by_role('button', name='上一个', exact=True).click()
                assert page.locator('.term-find [role=status]').inner_text() == f'1/{count}'
                page.get_by_role('searchbox', name='查找终端输出').fill('absent-term-query')
                assert page.locator('.term-find [role=status]').inner_text() == '无匹配'
                page.get_by_role('searchbox', name='查找终端输出').press('Escape')
                assert not page.locator('.term-find').is_visible()
                page.evaluate("navigator.clipboard.writeText('PASTE_MENU_SENTINEL')")
                before = (root / 'work/input.bin').read_bytes()
                menu()
                page.get_by_role('menuitem', name='粘贴', exact=True).click()
                page.wait_for_function("selectionBytes.flat().length > 0")
                import time
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and (root / 'work/input.bin').read_bytes() == before:
                    page.wait_for_timeout(50)
                assert (root / 'work/input.bin').read_bytes()[len(before):] == b'\x1b[200~PASTE_MENU_SENTINEL\x1b[201~'
                page.evaluate("[...T.views.values()][0].replay = true")
                menu()
                assert page.get_by_role('menuitem', name='粘贴', exact=True).is_disabled()
                page.get_by_role('menuitem', name='复制全部', exact=True).click()
                print('PASS', surface, 'context menu copy all / find / paste / read-only', flush=True)
            assert not errors, errors
            context.close()
            browser.close()


def check_touch(page, context, root, keyboard, surface):
    cdp = context.new_cdp_session(page)
    def touch(kind, x=0, y=0):
        cdp.send('Input.dispatchTouchEvent', {'type': kind, 'touchPoints':
                 [] if kind in ['touchEnd', 'touchCancel'] else [{'x': x, 'y': y, 'id': 1}]})
    for mode in ['none', '1003']:
        keyboard.focus()
        page.keyboard.type('mode:' + mode)
        page.keyboard.press('Enter')
        page.wait_for_function('m => selectionTerm.modes.mouseTrackingMode === m',
                               arg='none' if mode == 'none' else 'any')
        page.wait_for_timeout(150)
        b = page.evaluate("""() => {
          const t = selectionTerm;
          const screen = t._canvas || document.querySelector('.xterm-screen');
          const b = screen.getBoundingClientRect();
          return {x:b.x,y:b.y,cw:b.width/t.cols,ch:b.height/t.rows};
        }""")
        x, y = b['x'] + .2*b['cw'], b['y'] + .5*b['ch']
        page.evaluate("navigator.clipboard.writeText('touch-sentinel')")
        page.evaluate('selectionBytes = []')
        before = (root / 'work/input.bin').read_bytes()
        touch('touchStart', x, y)
        page.wait_for_timeout(650)
        page.wait_for_function("selectionTerm.getSelection() === 'S'")
        for col in [3, 6, 9, 11]:
            touch('touchMove', b['x'] + (col + .5)*b['cw'], y)
        assert page.evaluate('selectionTerm.getSelection()') == 'SELECT_FIRST'
        assert not page.locator('.term-context-menu').is_visible()
        assert page.evaluate('navigator.clipboard.readText()') == 'touch-sentinel'
        touch('touchEnd')
        page.wait_for_function("navigator.clipboard.readText().then(t => t === 'SELECT_FIRST')")
        page.wait_for_function("selectionTerm.getSelection() === ''")
        assert (root / 'work/input.bin').read_bytes() == before
        assert not page.evaluate('selectionBytes')
        # A cancelled gesture must not copy; a quick swipe must not select.
        touch('touchStart', x, y)
        page.wait_for_timeout(550)
        touch('touchCancel')
        page.wait_for_function("selectionTerm.getSelection() === ''")
        assert page.evaluate('navigator.clipboard.readText()') == 'SELECT_FIRST'
        touch('touchStart', x, y)
        touch('touchMove', x, y + 50)
        touch('touchEnd')
        page.wait_for_timeout(550)
        assert page.evaluate('selectionTerm.getSelection()') == ''
        print('PASS', surface, mode, 'mobile long press + drag + copy, cancel, swipe, no PTY input', flush=True)
    page.get_by_role('button', name='终端菜单', exact=True).tap()
    page.locator('.term-context-menu:visible').wait_for()
    page.get_by_role('menuitem', name='查找', exact=True).tap()
    page.get_by_role('searchbox', name='查找终端输出').fill('second_word')
    page.wait_for_function("selectionTerm.getSelection() === 'second_word'")
    page.get_by_role('button', name='关闭查找', exact=True).tap()
    print('PASS', surface, 'mobile menu and find via touch', flush=True)


def main():
    fixture.SHELL = 'exec python3 -u -c ' + shlex.quote(CLI)
    with sync_playwright() as pw:
        for surface in ['grid', 'standalone', 'xterm']:
            check_surface(pw, surface)
        for surface in ['grid', 'xterm']:
            check_surface(pw, surface, mobile=True)


if __name__ == '__main__':
    main()
