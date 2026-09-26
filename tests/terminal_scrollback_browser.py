#!/usr/bin/env python3
"""PTY history is scrolled locally by real wheel input in both console renderers."""
import json
import os
from pathlib import Path
import tempfile
import uuid

from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
import terminal_input_browser as fixture

SHELL = """stty -echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  case "$command" in
    history) seq 1 150 | sed 's/^/SCROLL_ROW_/' ;;
    ping) printf 'SCROLL_PING_OK\\n' ;;
    later) sleep 1; seq 151 180 | sed 's/^/SCROLL_ROW_/' ;;
  esac
done
"""


def check(pw, renderer):
    with tempfile.TemporaryDirectory(prefix='sessiondock-scrollback-') as tmp:
        root = Path(tmp)
        for name in ['host', 'work', 'claude', 'codex', 'grok']:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
                                  codex_message('user', 'Synthetic scrollback')], [])
        uid = corpus.uid(sid)
        fixture.SHELL = SHELL
        with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid), \
             isolated_server(corpus, BINARY, host_dir=root / 'host') as (base, _):
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
            context.add_init_script("localStorage.setItem('sessiondock.consoleRenderer', JSON.stringify(" + json.dumps(renderer) + "))")
            context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
            scrolls, errors = [], []
            context.on('request', lambda r: scrolls.append(r.url) if '/api/term/scroll' in r.url else None)
            page = context.new_page()
            page.on('pageerror', lambda e: errors.append(str(e)))
            try:
                page.goto(base, wait_until='networkidle')
                fixture.open_console(page, uid)
                page.evaluate('window.scrollTerm = [...T.views.values()][0].term')
                assert page.evaluate("T.list.find(r => r.name === T.name).server") == 'ptyhost'
                keyboard = page.locator('#termpane .xterm-helper-textarea')
                keyboard.press('h')
                page.keyboard.type('istory')
                page.keyboard.press('Enter')
                fixture.xterm_contains(page, 'SCROLL_ROW_150')
                page.wait_for_function('scrollTerm.buffer.active.baseY > 50')
                canvas = page.locator('.grid-canvas' if renderer == 'grid' else '.xterm-screen')
                canvas.hover()
                before = page.evaluate('scrollTerm.buffer.active.viewportY')
                page.mouse.wheel(0, -300)
                page.wait_for_function('before => scrollTerm.buffer.active.viewportY < before', arg=before, timeout=3000)
                top = page.evaluate('scrollTerm.buffer.active.viewportY')
                # Idle repaint must keep the chosen history, and downward wheel returns to live output.
                page.wait_for_timeout(600)
                assert page.evaluate('scrollTerm.buffer.active.viewportY') == top
                page.mouse.wheel(0, 300)
                page.wait_for_function('scrollTerm.buffer.active.viewportY === scrollTerm.buffer.active.baseY')
                if renderer == 'grid':
                    bar = page.get_by_role('scrollbar', name='终端历史')
                    thumb = page.locator('.grid-scrollbar-thumb')
                    bar.wait_for(state='visible')
                    page.evaluate('window.scrollBytes = []; scrollTerm.onData(d => scrollBytes.push(d))')
                    def drag_to(fraction):
                        track_box, thumb_box = bar.bounding_box(), thumb.bounding_box()
                        x = thumb_box['x'] + thumb_box['width'] / 2
                        page.mouse.move(x, thumb_box['y'] + thumb_box['height'] / 2)
                        page.mouse.down()
                        page.mouse.move(x, track_box['y'] + thumb_box['height'] / 2
                                        + (track_box['height'] - thumb_box['height']) * fraction, steps=10)
                        page.mouse.up()
                    drag_to(0)
                    page.wait_for_function('scrollTerm.buffer.active.viewportY === 0')
                    assert bar.get_attribute('aria-valuenow') == '0'
                    track_box = bar.bounding_box()
                    bar.click(position={'x': track_box['width'] / 2, 'y': track_box['height'] * .75})
                    page.wait_for_function('scrollTerm.buffer.active.viewportY > scrollTerm.buffer.active.baseY / 2')
                    bar.press('Home')
                    page.wait_for_function('scrollTerm.buffer.active.viewportY === 0')
                    bar.press('PageDown')
                    page.wait_for_function('scrollTerm.buffer.active.viewportY === scrollTerm.rows')
                    bar.press('End')
                    page.wait_for_function('scrollTerm.buffer.active.viewportY === scrollTerm.buffer.active.baseY')
                    assert not page.evaluate('scrollBytes'), 'scrollbar interaction reached the PTY'
                    keyboard.press_sequentially('later')
                    keyboard.press('Enter')
                    drag_to(.25)
                    anchored = page.evaluate('scrollTerm.buffer.active.viewportY')
                    fixture.xterm_contains(page, 'SCROLL_ROW_180')
                    assert page.evaluate('scrollTerm.buffer.active.viewportY') == anchored
                    drag_to(1)
                    page.wait_for_function('scrollTerm.buffer.active.viewportY === scrollTerm.buffer.active.baseY')
                    page.set_viewport_size({'width': 900, 'height': 650})
                    page.wait_for_function('scrollTerm._canvas.getBoundingClientRect().right <= scrollTerm._scrollbar.getBoundingClientRect().left')
                    print('PASS grid scrollbar drag, keyboard, output anchoring, bottom follow, resize gutter', flush=True)
                keyboard.press('p')
                page.keyboard.type('ing')
                page.keyboard.press('Enter')
                fixture.xterm_contains(page, 'SCROLL_PING_OK')
                assert not scrolls, scrolls
                assert not errors, errors
                print('PASS', renderer, 'PTY history wheel up/down, stable repaint, live input, no scroll HTTP', flush=True)
            finally:
                context.close()
                browser.close()


def main():
    with sync_playwright() as pw:
        for renderer in ['grid', 'xterm']:
            check(pw, renderer)


if __name__ == '__main__':
    main()
