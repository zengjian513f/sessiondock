#!/usr/bin/env python3
"""Freeze/resume a real private fake CLI process tree through Chromium."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_parity import REPO, BINARY, Corpus, codex_row, codex_message, isolated_server
from session_stop_browser import CODEX_SID, session_action, wait_xterm
from popups import on_popup
from hub_http_suite import Hub, free_port, scoped

CLI = '''import os, subprocess, sys, threading, time
from pathlib import Path
root = Path(os.environ['FREEZE_FIXTURE'])
child = subprocess.Popen([sys.executable, str(root / 'bin/child.py'), str(root / 'child-tick')])
(root / 'pids').write_text(str(os.getpid()) + ' ' + str(child.pid))
def tick():
    while True:
        (root / 'main-tick').write_text(str(time.monotonic()))
        time.sleep(.03)
threading.Thread(target=tick, daemon=True).start()
print('FREEZE_READY', flush=True)
try:
    for line in sys.stdin:
        print('FREEZE_INPUT_' + line.strip(), flush=True)
finally:
    child.terminate()
    child.wait()
'''


def state(pid):
    return Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0]


def freeze_button(page):
    button = page.locator('#a-session-freeze')
    if not button.is_visible():
        page.locator('#a-more').click()
    expect(button).to_be_visible()
    return button


def main():
    if not sys.platform.startswith('linux'):
        raise SystemExit('Linux process freeze suite')
    with tempfile.TemporaryDirectory(prefix='sessiondock-freeze-') as directory:
        root = Path(directory)
        for folder in ['host', 'ledger', 'bin', 'work', 'audit', 'reports', 'hub']:
            (root / folder).mkdir(mode=0o700)
        corpus = Corpus(root)
        corpus.put(CODEX_SID, 'codex', [codex_row('session_meta', {'id': CODEX_SID, 'cwd': str(root / 'work')}),
                   codex_message('user', 'Freeze diagnostic fixture')], [])
        uid = corpus.uid(CODEX_SID)
        native = {name: path.read_bytes() for name, path in corpus.paths.items()}
        (root / 'bin/cli.py').write_text(CLI)
        (root / 'bin/child.py').write_text("import sys,time\nfrom pathlib import Path\np=Path(sys.argv[1])\nwhile True:\n p.write_text(str(time.monotonic()))\n time.sleep(.03)\n")
        executable = root / 'bin/fake-codex'
        executable.write_text('#!/bin/sh\ncase "$1" in resume) ;; *) exit 0 ;; esac\nexec "$FREEZE_PYTHON" "$FREEZE_SCRIPT" "$@"\n')
        executable.chmod(0o700)
        config = root / 'launcher.json'
        config.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
            'host_dir': str(root / 'host'), 'adapters': [], 'profiles': [{
                'id': 'codex-cli-v1', 'source': 'codex', 'executable': str(executable),
                'args': [], 'resume_args': ['resume', '{sid}'], 'env': {
                    'PATH': '/usr/bin:/bin', 'TERM': 'xterm-256color', 'FREEZE_PYTHON': sys.executable,
                    'FREEZE_SCRIPT': str(root / 'bin/cli.py'), 'FREEZE_FIXTURE': str(root)}}]}))
        config.chmod(0o600)
        subprocess.run([str(BINARY), '--initialize-lifecycle', str(root / 'ledger')],
                       cwd=REPO, check=True, capture_output=True, timeout=15)
        node = SimpleNamespace(nid='a' * 32, name='FreezeFixture', port=free_port(), token='f' * 64)
        token_file, id_file = root / 'node-token', root / 'node-id'
        for path, value in [(token_file, node.token), (id_file, node.nid)]:
            path.touch(mode=0o600)
            path.write_text(value)
        pids = []
        hub = None
        try:
            with isolated_server(corpus, BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                    launcher_config=config, audit_dir=root / 'audit', extra_env={
                        'SESSIONDOCK_BUG_REPORT_DIR': str(root / 'reports'),
                        'SESSIONDOCK_BUG_REPORT_REPO': str(root / 'work'),
                        'SESSIONDOCK_NODE_BIND': f'127.0.0.1:{node.port}',
                        'SESSIONDOCK_NODE_TOKEN_FILE': str(token_file), 'SESSIONDOCK_NODE_ID_FILE': str(id_file),
                        'SESSIONDOCK_NODE_PEERS': '127.0.0.0/8'}) as (base, opener):
                with sync_playwright() as playwright:
                    options = {'headless': True}
                    if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                        options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                    browser = playwright.chromium.launch(**options)
                    context = browser.new_context(service_workers='block')
                    context.route('**/*', lambda route: route.continue_()
                                  if route.request.url.startswith(base + '/') else route.abort())
                    page = context.new_page()
                    errors = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    on_popup(page, lambda dialog: dialog.accept())
                    page.goto(base, wait_until='networkidle')
                    assert page.evaluate('SessionDockCapabilities.config.session_freeze') is True
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator('#a-term')).to_have_attribute('data-unavailable', 'false')
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/takeover') as response:
                        page.locator('#a-term').click()
                    target = response.value.json()
                    wait_xterm(page, 'FREEZE_READY')
                    pids = list(map(int, (root / 'pids').read_text().split()))
                    page.wait_for_timeout(100)
                    assert (root / 'child-tick').exists()
                    page.wait_for_function('uid => T.list.some(row => row.uid === uid && row.frozen === false)', arg=uid)
                    button = freeze_button(page)
                    expect(button).to_have_attribute('aria-label', '冻结现场')
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/freeze') as response:
                        button.click()
                    answer = response.value.json()
                    assert response.value.status == 200 and answer['frozen'] and answer['process_count'] == 2, answer
                    expect(button).to_have_attribute('aria-label', '恢复运行')
                    assert all(state(pid) == 'T' for pid in pids), [(pid, state(pid)) for pid in pids]
                    ticks = [(root / name).read_text() for name in ['main-tick', 'child-tick']]
                    time.sleep(.2)
                    assert ticks == [(root / name).read_text() for name in ['main-tick', 'child-tick']]
                    # The PTY host stays alive, with the same exact session instance.
                    rows = context.request.get(base + '/api/term/list').json()['sessions']
                    assert any(row['instance_id'] == target['instance_id'] and row['frozen'] for row in rows)
                    repeated = context.request.post(base + '/api/session/freeze', data={
                        'uid': uid, 'instance_id': target['instance_id'], 'frozen': True})
                    assert repeated.status == 200 and repeated.json()['frozen']
                    refused = context.request.post(base + '/api/session/freeze', data={
                        'uid': uid, 'instance_id': 'replaced-instance', 'frozen': False})
                    assert refused.status == 409 and all(state(pid) == 'T' for pid in pids)
                    report = page.locator('.dhead [data-report-bug]')
                    if not report.is_visible():
                        page.locator('#a-more').click()
                    report.click()
                    expect(page.locator('#bug-report-dialog')).to_be_visible()
                    page.locator('#bug-report-dialog .modal-close').click()
                    page.reload(wait_until='networkidle')
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    page.wait_for_function('uid => T.list.some(row => row.uid === uid && row.frozen === true)', arg=uid)
                    button = freeze_button(page)
                    expect(button).to_have_attribute('aria-label', '恢复运行')
                    assert page.evaluate("browserStateSnapshot('fixture').data.terminal.frozen") is True
                    button = freeze_button(page)
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/freeze') as response:
                        button.click()
                    assert response.value.status == 200 and response.value.json()['frozen'] is False
                    expect(button).to_have_attribute('aria-label', '冻结现场')
                    deadline = time.monotonic() + 3
                    while time.monotonic() < deadline and any((root / name).read_text() == tick
                            for name, tick in zip(['main-tick', 'child-tick'], ticks)):
                        time.sleep(.03)
                    assert all((root / name).read_text() != tick
                               for name, tick in zip(['main-tick', 'child-tick'], ticks))
                    # Actual authenticated Hub routing and scoped UID, not a mock.
                    context.close()
                    hub = Hub(REPO / 'target/debug/sessiondock-hub', root / 'hub', [node])
                    hub.start()
                    base = f'http://127.0.0.1:{hub.port}'
                    uid = scoped(node.nid, uid)
                    context = browser.new_context(service_workers='block')
                    context.route('**/*', lambda route: route.continue_()
                                  if route.request.url.startswith(base + '/') else route.abort())
                    page = context.new_page()
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    on_popup(page, lambda dialog: dialog.accept())
                    page.goto(base, wait_until='networkidle')
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    page.wait_for_function('uid => T.list.some(row => row.uid === uid && row.frozen === false)', arg=uid)
                    button = freeze_button(page)
                    expect(button).to_have_attribute('aria-label', '冻结现场')
                    # Mobile action menu uses the same recovery control and fits.
                    page.set_viewport_size({'width': 390, 'height': 844})
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    button = freeze_button(page)
                    bounds = button.bounding_box()
                    assert bounds and bounds['x'] >= 0 and bounds['x'] + bounds['width'] <= 391, bounds
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/freeze'):
                        button.click()
                    expect(button).to_have_attribute('aria-label', '恢复运行')
                    assert page.evaluate("browserStateSnapshot('fixture').data.terminal.frozen") is True
                    # Stop directly while frozen: the server must recover the
                    # tree before EOF, so the child can exit with its parent.
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/stop') as response:
                        session_action(page).click()
                    assert response.value.status == 200 and response.value.json()['stage'] == 'graceful', response.value.text()
                    assert all(not Path(f'/proc/{pid}').exists() for pid in pids)
                    assert not errors, errors
                    assert {name: path.read_bytes() for name, path in corpus.paths.items()} == native
                    browser.close()
        finally:
            if hub:
                hub.stop()
            for pid in pids:
                try:
                    os.kill(pid, signal.SIGCONT)
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            # Only fixture hosts, guarded by their original launch identity.
            for path in (root / 'host').glob('*.json'):
                record = json.loads(path.read_text())
                meta = record['meta']
                try:
                    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                        stream.settimeout(2)
                        stream.connect(str(root / 'host' / (record['name'] + '.sock')))
                        stream.sendall(json.dumps({'op': 'launch_guard_v1', 'expected_source': meta['source'],
                            'expected_launch_id': meta['launch_id'], 'expected_instance_id': meta['instance_id'],
                            'request': {'op': 'kill', 'force': True}}).encode() + b'\n')
                except OSError:
                    pass
    print('PASS freeze browser: real parent/child stop and progress resume, idempotency, stale instance refusal, '
          'report dialog and frozen snapshot, reload recovery, authenticated Hub, 390px menu, ordinary stop, native files preserved')


if __name__ == '__main__':
    main()
