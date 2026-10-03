#!/usr/bin/env python3
"""Cetus-style late ConPTY redraw tail through a private PTY and real console.

BUG-20261003-110817-4e7c32: DEC 2026 end precedes the final cursor restore
by about 15 ms. No real CLI, production host, or native session is used.
"""
from browser_runtime import js
import json
import os
from pathlib import Path
import shlex
import sys
import tempfile
import uuid

from playwright.sync_api import sync_playwright

import bench_term_echo_browser as fixture
from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
from terminal_browser import stop


CLI = r'''
import os, sys, time, tty
tty.setraw(sys.stdin.fileno())
def emit(text):
    os.write(1, text.encode())
emit('\x1b[2J\x1b[HRS_SHELL_READY\x1b[11;3H')
while True:
    key = os.read(0, 1)
    if key == b'q' or not key:
        break
    if key in (b'r', b's'):
        begin = '\x1b[?2026h\x1b[?25l\x1b[5;40Hanimation\x1b[?25h'
        if key == b's':
            # Also split the marker itself across PTY/WebSocket packets.
            emit(begin[:5]); time.sleep(.005); emit(begin[5:])
            time.sleep(.005)
        else:
            emit(begin)
        emit('\x1b[0 q\x1b[?2026l')
        # Include the extra scheduling/transport gap seen under browser load.
        time.sleep(.035)
        emit('\x1b[?25l\x1b[6;1HRS_TAIL_' + key.decode() + '\x1b[11;3H\x1b[?25h')
    elif key == b'h':
        emit('\x1b[?2026h\x1b[7;1HRS_UNTERMINATED')
    elif key == b'e':
        emit('\x1b[11;3H\x1b[?2026l')
    else:
        emit(key.decode())
'''


def main():
    if os.name != 'posix':
        raise SystemExit('This private PTY fixture requires POSIX')
    with tempfile.TemporaryDirectory(prefix='sessiondock-sync-') as temporary:
        root = Path(temporary)
        for name in ('host', 'work', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        script = root / 'cli.py'
        script.write_text(CLI)
        fixture.SHELL = 'exec ' + shlex.quote(sys.executable) + ' ' + shlex.quote(str(script))
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [
            codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
            codex_message('user', 'Synthetic synchronized redraw'),
        ], [])
        uid = corpus.uid(sid)
        process = fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid)
        try:
            with isolated_server(corpus, BINARY, host_dir=root / 'host') as (base, _), sync_playwright() as p:
                browser = p.chromium.launch(headless=True)
                context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
                context.add_init_script("localStorage.setItem('sessiondock.consoleRenderer', '\"xterm\"')")
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base, wait_until='networkidle')
                fixture.open_console(page, uid)
                keyboard = page.locator('#termpane .xterm-helper-textarea')
                keyboard.focus()
                # The initial attach resizes/reflows the fixture's startup grid.
                keyboard.press('e')
                page.wait_for_function(js('T.term.buffer.active.cursorX === 2 && T.term.buffer.active.cursorY === 10', 'runtime.terminal.state.term.buffer.active.cursorX === 2 && runtime.terminal.state.term.buffer.active.cursorY === 10'))
                page.evaluate(js('''() => {
                    window.cursorMoves = [];
                    T.term.onCursorMove(() => cursorMoves.push([
                        T.term.buffer.active.cursorX, T.term.buffer.active.cursorY]));
                    window.parsedWrites = [];
                    window.writeTimes = [];
                    const view = T.views.get(T.name), write = view.term.write.bind(view.term);
                    view.term.write = (bytes, done) => {
                        parsedWrites.push(bytes); writeTimes.push(performance.now()); write(bytes, done);
                    };
                }''', """() => {
                    window.cursorMoves = [];
                    runtime.terminal.state.term.onCursorMove(() => cursorMoves.push([
                        runtime.terminal.state.term.buffer.active.cursorX, runtime.terminal.state.term.buffer.active.cursorY]));
                    window.parsedWrites = [];
                    window.writeTimes = [];
                    const view = runtime.terminal.state.views.get(runtime.terminal.state.name), write = view.term.write.bind(view.term);
                    view.term.write = (bytes, done) => {
                        parsedWrites.push(bytes); writeTimes.push(performance.now()); write(bytes, done);
                    };
                }"""))
                for key in ('r', 's'):
                    page.evaluate('cursorMoves.length = 0')
                    keyboard.press(key)
                    page.wait_for_function('needle => (' + fixture.XTERM_TEXT + ')().includes(needle)', arg='RS_TAIL_' + key)
                    moves = page.evaluate('cursorMoves')
                    diagnostic = page.evaluate(js('''({writes:parsedWrites,times:writeTimes,
                        settle:writeTermOutput.toString().includes('TERM_SYNC_SETTLE_MS')})''', """({writes:parsedWrites,times:writeTimes,
                        settle:runtime.terminal.writeTermOutput.toString().includes('TERM_SYNC_SETTLE_MS')})"""))
                    assert all(move == [2, 10] for move in moves), (
                        f'intermediate redraw cursor escaped: {moves}; diagnostic={diagnostic!r}')
                    print(f'PASS {key}: late restore committed with redraw; cursor events={moves}', flush=True)
                # Ordinary typing reaches the parser without entering either hold timer.
                keyboard.press('p')
                page.wait_for_function(js('T.term.buffer.active.cursorX === 3', 'runtime.terminal.state.term.buffer.active.cursorX === 3'))
                assert page.evaluate(js('T.views.get(T.name).syncHold === null', 'runtime.terminal.state.views.get(runtime.terminal.state.name).syncHold === null'))
                assert page.evaluate("parsedWrites.at(-1) === 'p'")
                # A broken CLI must still release the page-side buffer at the cap.
                keyboard.press('h')
                page.wait_for_function("parsedWrites.some(s => s.includes('RS_UNTERMINATED'))", timeout=2000)
                assert page.evaluate(js('T.views.get(T.name).syncHold === null', 'runtime.terminal.state.views.get(runtime.terminal.state.name).syncHold === null'))
                keyboard.press('e')
                page.wait_for_function(js('T.term.buffer.active.cursorX === 2 && T.term.buffer.active.cursorY === 10', 'runtime.terminal.state.term.buffer.active.cursorX === 2 && runtime.terminal.state.term.buffer.active.cursorY === 10'))
                assert not errors, errors
                print('PASS terminal_sync_browser: split/complete markers, late cursor restore, plain echo, bounded fallback', flush=True)
                browser.close()
        finally:
            stop(process)


if __name__ == '__main__':
    main()
