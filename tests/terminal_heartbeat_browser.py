#!/usr/bin/env python3
"""OPEN-but-stalled terminal sockets recover without replaying ambiguous input."""
import argparse
import json
import os
from pathlib import Path
import tempfile
import uuid

from playwright.sync_api import sync_playwright
from audit_browser import audit_lines, wait_for_events
from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
import terminal_input_browser as fixture


def run(browser, binary, renderer):
    with tempfile.TemporaryDirectory(prefix='sessiondock-heartbeat-') as temporary:
        root = Path(temporary)
        for name in ['host', 'work', 'claude', 'codex', 'grok', 'audit']:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
                                codex_message('user', 'Synthetic stalled terminal')], [])
        uid = corpus.uid(sid)
        native = corpus.paths[sid].read_bytes()
        with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid) as (host, _), \
                isolated_server(corpus, binary, host_dir=root / 'host', audit_dir=root / 'audit') as (base, _):
            context = browser.new_context(viewport={'width': 1000, 'height': 800}, service_workers='block')
            context.add_init_script('localStorage.setItem("sessiondock.consoleRenderer", JSON.stringify(%s))' % json.dumps(renderer))
            context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
            sockets, probes = [], []

            def route_socket(client):
                server = client.connect_to_server()
                state = {'block': None}
                sockets.append(state)

                def upstream(message):
                    if isinstance(message, str) and json.loads(message).get('t') == 'ping':
                        probes.append(message)
                    if state['block'] != 'up':
                        server.send(message)

                def downstream(message):
                    if state['block'] != 'down':
                        client.send(message)

                client.on_message(upstream)
                server.on_message(downstream)

            context.route_web_socket('**/api/term/attach?*', route_socket)
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda e: errors.append(str(e)))
            page.goto(base, wait_until='networkidle')
            fixture.open_console(page, uid)
            keyboard = page.locator('#termpane .xterm-helper-textarea')
            page.wait_for_function('!!currentTermViewObject().heartbeat')
            # A quiet CLI must stay connected across multiple real probe rounds.
            page.wait_for_timeout(6500)
            assert len(probes) >= 3 and len(sockets) == 1, (probes, len(sockets))
            log = root / 'work/input.log'
            assert not log.exists(), 'heartbeat was written to the CLI'

            for direction, command in [('down', 'once'), ('up', 'lost')]:
                old = sockets[-1]
                old['block'] = direction
                count = len(sockets)
                keyboard.focus()
                page.keyboard.type(command)
                page.keyboard.press('Enter')
                assert page.evaluate('T.ws.readyState') == 1
                # HTTP/UI remain alive while only this WebSocket direction stalls.
                assert page.request.get(base + '/api/meta').ok
                page.wait_for_function('currentTermViewObject().heartbeat === null', timeout=16000)
                page.wait_for_function('!!currentTermViewObject().heartbeat && T.ws.readyState === 1', timeout=10000)
                assert len(sockets) == count + 1, (direction, len(sockets))
                assert host.poll() is None, 'recovery restarted/stopped the host'
                if direction == 'down':
                    fixture.xterm_contains(page, 'ECHO_once')  # recovered snapshot, no replay
                keyboard.focus()
                page.keyboard.type('ping')
                page.keyboard.press('Enter')
                fixture.xterm_contains(page, 'RS_PING_OK')
                page.wait_for_timeout(200)
                expected = ['once', 'ping'] if direction == 'down' else ['once', 'ping', 'ping']
                assert log.read_text().splitlines() == expected, (direction, log.read_text())
                print('PASS', renderer, direction, 'stalled OPEN socket recovered; no input replay', flush=True)

            wait_for_events(page, root / 'audit', {'browser.terminal.heartbeat_timeout'})
            rows = [r for r in audit_lines(root / 'audit') if r['event'] == 'browser.terminal.heartbeat_timeout']
            assert len(rows) == 2 and len({r['connection_id'] for r in rows}) == 2, rows
            assert all(r['data']['timeout_ms'] == 10000 for r in rows)
            assert corpus.paths[sid].read_bytes() == native
            assert not errors, errors
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    fixture.SHELL = """stty -echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  printf '%s\\n' "$command" >> input.log
  case "$command" in
    ping) printf 'RS_PING_OK\\n' ;;
    *) printf 'ECHO_%s\\n' "$command" ;;
  esac
done
"""
    with sync_playwright() as pw:
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        try:
            for renderer in ['grid', 'xterm']:
                run(browser, args.binary, renderer)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
