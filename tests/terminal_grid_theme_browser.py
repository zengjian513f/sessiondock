#!/usr/bin/env python3
"""Main console theme switching with real PTY colors and dark-only OSC replies."""
import json
import os
from pathlib import Path
import shlex
import tempfile
import uuid

from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
import terminal_input_browser as fixture

CLI = r'''
import os, select, termios, tty
old = termios.tcgetattr(0)
tty.setraw(0)
def emit(s): os.write(1, s.encode())
emit('RS_SHELL_READY\r\n')
command = b''
try:
 while True:
  c = os.read(0, 1)
  if c not in (b'\r', b'\n'):
   command += c
   continue
  if command == b'colors':
   emit('\x1b[0m\x1b[2J\x1b[H')
   for name, sgr in [('PROMPT', '38;2;220;220;220;48;2;30;30;30'),
                     ('GREEN', '38;2;150;235;160;48;2;30;58;40'),
                     ('RED', '38;2;250;155;170;48;2;72;32;26'),
                     ('CODE', '38;5;183;48;5;235'),
                     ('YELLOW', '93;40'), ('DIM', '2;38;2;180;180;180'),
                     ('INVERSE', '7;38;2;220;220;220;48;2;30;30;30')]:
    emit('\x1b[' + sgr + 'm' + name + '                    \x1b[0m\r\n')
  elif command == b'query':
   emit('\x1b]11;?\x07')
   answer = b''
   while select.select([0], [], [], 2)[0]:
    answer += os.read(0, 1024)
    if answer.endswith((b'\x07', b'\x1b\\')): break
   emit('QUERY_BG=' + answer.hex() + '\r\n')
  command = b''
finally:
 termios.tcsetattr(0, termios.TCSANOW, old)
'''

SAMPLE = """() => {
 const t = [...T.views.values()].find(v => v.grid).term, r = t.renderer;
 const canvas = t._canvas, ctx = canvas.getContext('2d');
 const labels = ['PROMPT','GREEN','RED','CODE','YELLOW','DIM','INVERSE'];
 const result = {};
 for (let slot = 0; slot < r._slots.length; slot++) {
  const row = t.model.rowAt(r._slots[slot].line);
  const cells = row ? t.model.cellsOf(row) : [];
  const text = cells.map(c => c.text).join('');
  const label = labels.find(l => text.startsWith(l));
  if (!label) continue;
  const y = slot * r.cellHeight + r.baseline;
  const paint = canvas._paints?.filter(p => p.x === 0 && Math.abs(p.y-y) < .01).at(-1);
  const bg = [...ctx.getImageData(Math.round(15*r.cellWidth*r.dpr),
     Math.round((slot+.5)*r.cellHeight*r.dpr), 1, 1).data].slice(0,3);
  result[label] = {bg, paint, raw: cells.slice(0,1)};
 }
 return result;
}"""


def lum(rgb):
    v = [x / 255 for x in rgb]
    return sum((x / 12.92 if x <= .04045 else ((x + .055) / 1.055) ** 2.4) * w
               for x, w in zip(v, [.2126, .7152, .0722]))


def main():
    fixture.SHELL = 'exec python3 -u -c ' + shlex.quote(CLI)
    with tempfile.TemporaryDirectory(prefix='sessiondock-grid-theme-') as temporary:
        root = Path(temporary)
        for name in ['host', 'work', 'claude', 'codex', 'grok']:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
                                codex_message('user', 'Synthetic terminal colors')], [])
        uid = corpus.uid(sid)
        with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid), \
             isolated_server(corpus, BINARY, host_dir=root / 'host') as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
            context.add_init_script("""
              localStorage.setItem('sessiondock.theme', JSON.stringify('dark'));
              localStorage.setItem('sessiondock.consoleRenderer', JSON.stringify('grid'));
              const fill = CanvasRenderingContext2D.prototype.fillText;
              CanvasRenderingContext2D.prototype.fillText = function(text,x,y,...rest) {
                (this.canvas._paints ||= []).push({text,x,y,fg:this.fillStyle,alpha:this.globalAlpha});
                return fill.call(this,text,x,y,...rest);
              };
            """)
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda e: errors.append(str(e)))
            page.goto(base, wait_until='networkidle')
            fixture.open_console(page, uid)
            assert page.evaluate('[...T.views.values()].every(v => v.grid)')
            keyboard = page.locator('#termpane .xterm-helper-textarea')

            def command(text):
                keyboard.focus()
                page.keyboard.type(text)
                page.keyboard.press('Enter')

            def sample():
                page.wait_for_function('(' + SAMPLE + ')().INVERSE?.paint != null')
                return page.evaluate(SAMPLE)

            command('colors')
            dark = sample()
            assert dark['PROMPT']['bg'] == [30, 30, 30], dark
            expected = 'QUERY_BG=' + b'\x1b]11;rgb:0000/0000/0000'.hex()
            command('query')
            fixture.xterm_contains(page, expected)
            for theme in ['light', 'dark', 'light']:
                page.locator('#settings').click()
                page.locator('#setting-theme').select_option(theme)
                page.keyboard.press('Escape')
                page.wait_for_function('theme => [...T.views.values()].every(v => !!v.term.renderer.theme.light === (theme === "light"))', arg=theme)
                command('colors')
                page.wait_for_timeout(250)
                data = sample()
                assert {k:v['raw'] for k,v in data.items()} == {k:v['raw'] for k,v in dark.items()}, data
                if theme == 'dark':
                    assert {k:(v['bg'],v['paint']['fg']) for k,v in data.items()} == {k:(v['bg'],v['paint']['fg']) for k,v in dark.items()}
                else:
                    for label, v in data.items():
                        if label != 'INVERSE':
                            assert (max(v['bg']) + min(v['bg'])) / 510 > .65, (label,v)
                        color = v['paint']['fg'].lstrip('#')
                        fg = [int(color[i:i+2],16) for i in (0,2,4)]
                        alpha = v['paint']['alpha']
                        ink = lum([x * alpha + b * (1-alpha) for x,b in zip(fg,v['bg'])])
                        bg = lum(v['bg'])
                        assert (max(ink,bg)+.05)/(min(ink,bg)+.05) >= 4.5, (label,v)
                command('query')
                fixture.xterm_contains(page, expected)
            assert not errors, errors
            browser.close()
    print('PASS terminal_grid_theme_browser: light code/diff/prompt, indexed/RGB/dim/inverse contrast, reversible themes, CLI always sees black')


if __name__ == '__main__':
    main()
