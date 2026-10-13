#!/usr/bin/env python3
"""OPEN-but-stalled terminal sockets recover without replaying ambiguous input."""

from contextlib import ExitStack
from types import SimpleNamespace
import argparse
import json
import os
from pathlib import Path
import time
import tempfile
import uuid

from playwright.sync_api import sync_playwright, expect
from audit_browser import audit_lines, wait_for_events
from hub_fixtures import Hub, free_port, scoped
from history_fixtures import BINARY, Corpus, codex_message, codex_row, isolated_server
import terminal_input_browser as fixture


def run(browser, binary, through_hub=False, screenshots=None):
    with tempfile.TemporaryDirectory(prefix='sessiondock-heartbeat-') as temporary:
        root = Path(temporary)
        for name in ['host', 'work', 'claude', 'codex', 'grok', 'audit', 'hub']:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
                                codex_message('user', 'Synthetic stalled terminal')], [])
        uid = corpus.uid(sid)
        native = corpus.paths[sid].read_bytes()
        node = SimpleNamespace(nid='b' * 32, name='HeartbeatFixture', token='f' * 64, port=free_port())
        env = {}
        if through_hub:
            for name, value in [('node-token', node.token), ('node-id', node.nid)]:
                path = root / name
                path.touch(mode=0o600)
                path.write_text(value)
            env = {'SESSIONDOCK_NODE_BIND': f'127.0.0.1:{node.port}',
                   'SESSIONDOCK_NODE_TOKEN_FILE': str(root / 'node-token'),
                   'SESSIONDOCK_NODE_ID_FILE': str(root / 'node-id'), 'SESSIONDOCK_NODE_PEERS': '127.0.0.0/8'}
        with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid) as (host, _), ExitStack() as stack:
            base, _ = stack.enter_context(isolated_server(corpus, binary, host_dir=root / 'host',
                                                         audit_dir=root / 'audit', extra_env=env))
            if through_hub:
                hub = Hub(binary.with_name('sessiondock-hub'), root / 'hub', [node])
                hub.start()
                stack.callback(hub.stop)
                base, uid = f'http://127.0.0.1:{hub.port}', scoped(node.nid, uid)
            context = browser.new_context(viewport={'width': 1000, 'height': 800}, service_workers='block')
            context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
            sockets, probes = [], []

            def route_socket(client):
                server = client.connect_to_server()
                state = {'block': 'up' if through_hub and not sockets else None}
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
            if through_hub:
                # Report 441af3: the first screen arrives but the very first
                # ping and typed bytes never cross the upstream boundary.
                cold_connection = page.evaluate('currentTermViewObject().auditConnectionId')
                keyboard.focus()
                page.keyboard.type('coldlost')
                page.keyboard.press('Enter')
                page.wait_for_function('currentTermViewObject().heartbeat === null', timeout=16000)
                page.wait_for_function('!!currentTermViewObject().heartbeat && T.ws.readyState === 1', timeout=10000)
                assert len(sockets) == 2 and not (root / 'work/input.log').exists()
                deadline = time.monotonic() + 15
                while True:
                    cold_rows = audit_lines(root / 'audit')
                    node_end = [r['data'] for r in cold_rows if r['event'] == 'terminal.transport.progress'
                                and r['data']['connection'] == cold_connection and r['data']['reason'] == 'closed']
                    hub_end = [r['data'] for r in cold_rows if r['event'] == 'browser.hub.terminal.transport'
                               and r['data']['connection'] == cold_connection and r['data']['reason'] in ('eof', 'io_error')]
                    browser_send = [r for r in cold_rows if r['event'] == 'browser.terminal.frame.sent'
                                    and r['connection_id'] == cold_connection and r['data']['kind'] == 'input']
                    if node_end and hub_end and browser_send:
                        break
                    assert time.monotonic() < deadline, (node_end, hub_end, browser_send)
                    page.wait_for_timeout(100)
                assert node_end[-1]['received_frames'] == node_end[-1]['written_frames'] == node_end[-1]['pings_received'] == 0, node_end
                stalled = [r['data'] for r in cold_rows if r['event'] == 'browser.hub.terminal.transport'
                           and r['data']['connection'] == cold_connection and r['data']['reason'] == 'periodic'
                           and r['data']['elapsed_ms'] >= 4000]
                assert stalled and all(r['browser_read']['bytes'] == r['node_written']['bytes'] == 0 for r in stalled), stalled
                # The browser's close control frame still crosses the route;
                # raw tunnel totals must not be mistaken for decoded PTY input.
                assert hub_end[-1]['browser_read']['bytes'] == hub_end[-1]['node_written']['bytes'], hub_end
                assert hub_end[-1]['node_read']['bytes'] == hub_end[-1]['browser_written']['bytes'] > 0, hub_end
                print('PASS hub cold upstream stall: browser queued input, Hub received zero, node received zero; initial screen returned; no replay', flush=True)
            # A quiet CLI must stay connected across multiple real probe rounds.
            page.wait_for_timeout(6500)
            assert len(probes) >= 3 and len(sockets) == 1 + int(through_hub), (probes, len(sockets))
            log = root / 'work/input.log'
            assert not log.exists(), 'heartbeat was written to the CLI'

            for direction, command in [('down', 'once'), ('up', 'lost')]:
                connection = page.evaluate('currentTermViewObject().auditConnectionId')
                before = len(log.read_text().splitlines()) if log.exists() else 0
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
                expect(page.locator('#term-output-notice')).to_contain_text('控制台连接无响应')
                expect(page.locator('#float-stack > :visible, dialog.app-popup')).to_have_count(0)
                page.wait_for_function('!!currentTermViewObject().heartbeat && T.ws.readyState === 1', timeout=10000)
                expect(page.locator('#term-output-notice')).to_be_hidden()
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
                # Even revocation/cancellation must preserve receive/write boundaries.
                deadline = time.monotonic() + 15
                while True:
                    rows = audit_lines(root / 'audit')
                    final = [r['data'] for r in rows if r['event'] == 'terminal.transport.progress'
                             and r['data']['connection'] == connection and r['data']['reason'] == 'closed']
                    hub_rows = [r for r in rows if r['event'] == 'browser.hub.terminal.transport'
                                and r['data']['connection'] == connection]
                    browser_sent = [r for r in rows if r['event'] == 'browser.terminal.frame.sent'
                                    and r['connection_id'] == connection and r['data']['kind'] == 'input']
                    if final and browser_sent and (not through_hub or any(r['data']['reason'] in ('eof', 'io_error') for r in hub_rows)):
                        break
                    assert time.monotonic() < deadline, (direction, final, hub_rows, browser_sent)
                    page.wait_for_timeout(100)
                final = final[-1]
                assert final['received_bytes'] == final['written_bytes'], final
                # First socket had no CLI input; the second received the recovery ping.
                expected_bytes = (0 if before == 0 else len('ping\r')) + (len(command + '\r') if direction == 'down' else 0)
                assert final['written_bytes'] == expected_bytes, (direction, final, expected_bytes)
                assert final['pings_received'] == final['pongs_sent'] and final['pings_received'] > 0, final
                assert final['input_stage'] == 'waiting_browser', final
                assert all(r['data']['sent_frames'] > 0 for r in browser_sent)
                assert all('data' not in r['data'] and 'text' not in r['data'] for r in browser_sent)
                assert any(r['event'] == 'browser.terminal.heartbeat.pong' and r['data']['matched'] for r in rows)
                if through_hub:
                    end = next(r['data'] for r in reversed(hub_rows) if r['data']['reason'] in ('eof', 'io_error'))
                    assert end['browser_read']['bytes'] == end['node_written']['bytes'] > 0, end
                    assert end['node_read']['bytes'] == end['browser_written']['bytes'] > 0, end
                    assert any(r['event'] == 'hub.terminal.transport' and r['data']['connection'] == connection for r in hub.audit_events())
                    assert all(r['client_ts'] and r['data']['observer'] == 'hub' for r in hub_rows)
                print('PASS', 'hub' if through_hub else 'node', direction,
                      'stalled OPEN socket recovered; correlated send/forward/receive/write/final receipts; no input replay', flush=True)

            wait_for_events(page, root / 'audit', {'browser.terminal.heartbeat_timeout'})
            deadline = time.monotonic() + 15
            while True:
                rows = [r for r in audit_lines(root / 'audit') if r['event'] == 'browser.terminal.heartbeat_timeout']
                if len(rows) >= 2 + int(through_hub) or time.monotonic() >= deadline: break
                page.wait_for_timeout(100)
            assert len(rows) == 2 + int(through_hub) and len({r['connection_id'] for r in rows}) == len(rows), rows
            assert all(r['data']['timeout_ms'] == 10000 for r in rows)
            all_receipts = json.dumps(audit_lines(root / 'audit'))
            assert node.token not in all_receipts and 'ECHO_once' not in all_receipts and 'RS_PING_OK' not in all_receipts
            if through_hub:
                assert node.token not in json.dumps(hub.audit_events())
            if through_hub and screenshots:
                screenshots.mkdir(parents=True, exist_ok=True)
                for theme in ('light', 'dark'):
                    page.set_viewport_size({'width': 1280, 'height': 844})
                    page.locator('#settings').click()
                    page.locator('#setting-theme').select_option(theme)
                    page.locator('#settings-dialog .modal-close').click()
                    fixture.open_console(page, uid)
                    page.wait_for_function('theme => currentTermViewObject().term.renderer.theme.light === (theme === "light")', arg=theme)
                    for width in (1280, 390):
                        page.set_viewport_size({'width': width, 'height': 844})
                        # Let the responsive media-query handler settle before
                        # deciding whether the list or detail is visible.
                        page.evaluate('() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
                        if page.locator('#side').is_visible():
                            fixture.open_console(page, uid)
                        expect(page.locator('#termpane')).to_be_in_viewport()
                        keyboard.focus()
                        page.keyboard.type('ping')
                        page.keyboard.press('Enter')
                        fixture.xterm_contains(page, 'RS_PING_OK')
                        page.screenshot(path=str(screenshots / f'pty-{width}-{theme}.png'))
            assert corpus.paths[sid].read_bytes() == native
            assert not errors, errors
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
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
            run(browser, args.binary)
            run(browser, args.binary, through_hub=True, screenshots=args.screenshots)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
