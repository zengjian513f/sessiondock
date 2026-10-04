#!/usr/bin/env python3
"""Cetus-style late ConPTY redraw tail through a private PTY and real console.

BUG-20261003-110817-4e7c32: DEC 2026 end precedes the final cursor restore
by about 15 ms. No real CLI, production host, or native session is used.
"""
from browser_runtime import js, scoped_frontend
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
from host_identity import request as host_request
from terminal_browser import stop


CLI = r'''
import os, sys, time, tty
tty.setraw(sys.stdin.fileno())
input_log = open(os.path.join(os.path.dirname(__file__), 'input.log'), 'ab', buffering=0)
def emit(text):
    os.write(1, text.encode())
emit('\x1b[2J\x1b[HRS_SHELL_READY\x1b[11;3H')
while True:
    key = os.read(0, 1)
    input_log.write(key)
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
    elif key == b'd':
        emit('\x1b[?2026h\x1b[8;1HRS_DISPOSE_QUEUED\x1b[?2026l')
    else:
        emit(key.decode())
'''


def verify_root_disposal(page, keyboard, root, process):
    if not scoped_frontend():
        return
    requests, sockets = [], []
    page.on('request', lambda request: requests.append(request.url)
            if '/api/term/' in request.url else None)
    page.on('websocket', lambda socket: sockets.append(socket.url))
    page.evaluate("""() => {
        const state = window.SessionDockRuntime.terminal.state;
        window.__termLifetime = {
            view: state.views.get(state.name), socket: state.ws,
            json: Response.prototype.json, timeout: window.setTimeout, held: [],
        };
        // Hold the next real polling response after its body has been read.
        // The fallback keeps this test delay bounded even if an assertion fails.
        Response.prototype.json = async function(...args) {
            const data = await __termLifetime.json.apply(this, args);
            if (!new URL(this.url, location.href).pathname.endsWith('/api/term/list')) return data;
            return new Promise(done => {
                const release = () => { clearTimeout(timer); done(data); };
                const timer = __termLifetime.timeout.call(window, release, 5000);
                __termLifetime.held.push(release);
            });
        };
    }""")
    page.wait_for_function('__termLifetime.held.length > 0', timeout=6000)
    page.evaluate("""() => {
        // Delay the existing 50/100 ms sync windows long enough to observe both
        // queued timers, without replacing terminal input or output operations.
        window.setTimeout = (callback, delay, ...args) => __termLifetime.timeout.call(
            window, callback, delay === 50 || delay === 100 ? 2000 : delay, ...args);
    }""")
    keyboard.press('d')
    page.wait_for_function("""() => {
        const view = __termLifetime.view;
        return view.syncHold?.includes('RS_DISPOSE_QUEUED')
            && !!view.syncHoldTimer && !!view.syncSettleTimer;
    }""")
    delivered = (root / 'input.log').read_bytes()
    assert delivered.count(b'd') == 1, delivered
    page.evaluate("""() => {
        __termLifetime.list = window.SessionDockRuntime.terminal.state.list;
        __termLifetime.writes = parsedWrites.length;
        document.querySelector('#app').__vue_app__.unmount();
        Response.prototype.json = __termLifetime.json;
        window.setTimeout = __termLifetime.timeout;
        __termLifetime.held.splice(0).forEach(release => release());
    }""")
    count = len(requests), len(sockets)
    page.clock.fast_forward(30000)
    page.evaluate("""async () => {
        dispatchEvent(new Event('online'));
        dispatchEvent(new Event('pageshow'));
        dispatchEvent(new Event('resize'));
        document.dispatchEvent(new Event('visibilitychange'));
        await new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)));
    }""")
    page.wait_for_function('__termLifetime.socket.readyState === WebSocket.CLOSED')
    assert page.locator('#termpane, #xterm, .xterm').count() == 0
    assert page.evaluate("""() => {
        const state = window.SessionDockRuntime.terminal.state, view = __termLifetime.view;
        return state.views.size === 0 && !state.ws && !state.term
            && state.list === __termLifetime.list && !view.host.isConnected
            && !view.ws && !view.inputLease && !view.heartbeat
            && !view.reconnectTimer && !view.connectTimer
            && view.syncHold === null && view.syncHoldTimer === null
            && view.syncSettleTimer === null && parsedWrites.length === __termLifetime.writes;
    }""")
    assert (len(requests), len(sockets)) == count, (requests, sockets)
    assert (root / 'input.log').read_bytes() == delivered, 'disposed client resent CLI input'
    record = json.loads((root / 'host/bench-echo-host.json').read_text())
    assert record['host_pid'] == process.pid and process.poll() is None
    os.kill(record['pid'], 0)  # Only the child of this newly created private host.
    assert host_request(record, {'op': 'info'})['exited'] is False
    assert 'RS_DISPOSE_QUEUED' in host_request(record, {'op': 'capture', 'styled': False})['text']
    print('PASS Vue terminal root disposal: late list, queued sync, sockets/timers, host/input preserved', flush=True)


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
                if scoped_frontend():
                    page.clock.install()
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
                verify_root_disposal(page, keyboard, root, process)
                assert not errors, errors
                print('PASS terminal_sync_browser: split/complete markers, late cursor restore, plain echo, bounded fallback', flush=True)
                browser.close()
        finally:
            stop(process)


if __name__ == '__main__':
    main()
