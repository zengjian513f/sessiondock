#!/usr/bin/env python3
"""Freeze/resume a real private fake CLI process tree through Chromium."""
import json
import os
import re
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
from history_parity import REPO, BINARY, Corpus, claude_row, codex_row, codex_message, isolated_server
from session_stop_browser import CODEX_SID, session_action, wait_xterm
from popups import on_popup
from hub_http_suite import Hub, free_port, scoped

CLI = '''import os, subprocess, sys, threading, time, tty
from pathlib import Path
root = Path(os.environ['FREEZE_FIXTURE'])
child = subprocess.Popen([sys.executable, str(root / 'bin/child.py'), str(root / 'child-tick')])
(root / 'pids').write_text(str(os.getpid()) + ' ' + str(child.pid))
def tick():
    while True:
        (root / 'main-tick').write_text(str(time.monotonic()))
        time.sleep(.03)
threading.Thread(target=tick, daemon=True).start()
tty.setraw(0)
print('FREEZE_READY', flush=True)
try:
    while True:
        data = os.read(0, 1024)
        if not data or b'\\x04' in data:
            break
        print('FREEZE_INPUT_' + data.decode(errors='replace').strip(), flush=True)
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
    report = page.locator('.dhead [data-report-bug]')
    expect(report).to_be_visible()
    assert button.evaluate("button => button.nextElementSibling?.hasAttribute('data-report-bug')")
    assert button.evaluate("button => button.parentElement.classList.contains('session-menu-diagnostics')")
    return button


def check_unavailable_pause(page, reason):
    requests = []
    def record(request):
        if urlsplit(request.url).path == '/api/session/freeze':
            requests.append(request)
    page.on('request', record)
    for width in [1698, 390]:
        page.set_viewport_size({'width': width, 'height': 900})
        page.wait_for_timeout(80)
        if width == 390 and page.locator('#side').is_visible():
            page.locator(f'#side .item[data-uid="{page.evaluate("S.sel")}"]').click()
        button = freeze_button(page)
        expect(button.locator('use')).to_have_attribute('href', '#i-pause')
        # SVG geometry catches an empty/invisible glyph, not just button layout.
        assert button.locator('use').evaluate('e => e.getBBox().height') > 0
        expect(button).to_have_attribute('aria-disabled', 'true')
        expect(button).to_have_attribute('aria-label', '冻结现场')
        expect(button).to_have_attribute('title', re.compile(reason))
        for theme in ['light', 'dark']:
            page.evaluate('theme => applyTheme(theme)', theme)
            button.hover()
            expect(button).to_have_css('color', 'rgb(107, 114, 128)' if theme == 'light' else 'rgb(139, 147, 161)')
            expect(button).to_have_css('opacity', '0.55')
        bounds = button.bounding_box()
        page.mouse.click(bounds['x'] + bounds['width']/2, bounds['y'] + bounds['height']/2)
        button.focus()
        page.keyboard.press('Enter')
        page.keyboard.press('Escape')
    assert not requests, 'Unavailable pause must not submit a freeze request'
    page.remove_listener('request', record)
    page.evaluate("applyTheme('light')")
    page.set_viewport_size({'width': 1280, 'height': 900})
    print(f'PASS unavailable pause: {reason}, desktop/mobile, both themes, mouse/keyboard', flush=True)


def check_pause_badges(page, uid):
    side = page.locator(f'#side .item[data-uid="{uid}"] > .ico > .item-status')
    header = page.locator('#dlive')
    for badge in [side, header]:
        expect(badge).to_have_class(re.compile(r'\bfrozen\b'))
        expect(badge).to_have_text('')
        expect(badge.locator('use')).to_have_attribute('href', '#i-pause')
        expect(badge).not_to_have_class(re.compile(r'\binput-question\b'))
        expect(badge).to_have_css('background-color', 'rgb(251, 191, 36)')
        expect(badge).to_have_attribute('aria-label', re.compile('已暂停'))
        expect(badge).to_have_css('animation-name', 'none')
    expect(header).to_be_visible()
    badge_box = header.bounding_box()
    icon_box = header.locator('..').bounding_box()
    assert badge_box['x'] >= icon_box['x'] + icon_box['width'] / 2
    assert badge_box['y'] < icon_box['y'] + icon_box['height'] / 2


def check_freeze_overlay(page):
    overlay = page.locator('#session-freeze-overlay')
    expect(overlay).to_be_visible()
    expect(overlay).to_have_attribute('data-uid', page.evaluate('S.sel'))
    pane = page.locator('#right').bounding_box()
    card = overlay.locator('.session-freeze-line').bounding_box()
    assert abs(card['x'] + card['width'] / 2 - pane['x'] - pane['width'] / 2) < 2
    assert abs(card['y'] + card['height'] / 2 - pane['y'] - pane['height'] / 2) < 2
    assert card['x'] >= pane['x'] and card['width'] <= pane['width']
    expect(overlay.locator('.session-freeze-line')).to_have_text('会话已暂停')
    expect(overlay.locator('button')).to_have_count(1)
    expect(overlay.locator('[data-freeze-resume]')).to_have_attribute('aria-label', '恢复运行')
    expect(overlay.locator('[data-freeze-resume] use')).to_have_attribute('href', '#i-play')
    label_box = overlay.locator('span').bounding_box()
    play_box = overlay.locator('button').bounding_box()
    assert play_box['x'] > label_box['x'] + label_box['width']
    assert abs(play_box['y'] + play_box['height'] / 2 - label_box['y'] - label_box['height'] / 2) < 2
    shades = []
    for theme in ['light', 'dark']:
        page.evaluate('theme => applyTheme(theme)', theme)
        shades.append(overlay.evaluate('node => getComputedStyle(node).backgroundColor'))
        card_colors = overlay.locator('.session-freeze-line').evaluate(
            'node => [getComputedStyle(node).backgroundColor, getComputedStyle(node).color]')
        assert card_colors == (['rgb(255, 255, 255)', 'rgb(28, 32, 36)'] if theme == 'light'
                               else ['rgb(28, 31, 38)', 'rgb(223, 227, 234)']), card_colors
        expect(overlay.locator('[data-freeze-resume]')).to_be_visible()
    assert shades == ['rgba(15, 23, 42, 0.24)', 'rgba(0, 0, 0, 0.52)'], shades
    page.evaluate("applyTheme('light')")


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
        other_sid = 'freeze-unrelated-session'
        corpus.put(other_sid, 'claude', [claude_row(other_sid, 'user', 'u0', None,
                   'Unrelated session without a paused process')], [])
        other_uid = corpus.uid(other_sid)
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
                    # Windows/macOS standalone capability: no Linux freeze support.
                    def unsupported_page(route):
                        response = route.fetch()
                        body = response.text()
                        assert '&quot;session_freeze&quot;:true' in body
                        route.fulfill(response=response, body=body.replace(
                            '&quot;session_freeze&quot;:true', '&quot;session_freeze&quot;:false'))
                    page.route(base + '/', unsupported_page)
                    page.goto(base, wait_until='networkidle')
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    check_unavailable_pause(page, '当前节点不支持冻结现场')
                    page.unroute(base + '/', unsupported_page)
                    page.goto(base, wait_until='networkidle')
                    assert page.evaluate('SessionDockCapabilities.config.session_freeze') is True
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    check_unavailable_pause(page, '没有可验证的运行实例')
                    expect(page.locator('#a-term')).to_have_attribute('data-unavailable', 'false')
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/takeover') as response:
                        page.locator('#a-term').click()
                    target = response.value.json()
                    wait_xterm(page, 'FREEZE_READY')
                    pids = list(map(int, (root / 'pids').read_text().split()))
                    page.wait_for_timeout(100)
                    assert (root / 'child-tick').exists()
                    page.wait_for_function('uid => T.list.some(row => row.uid === uid && row.frozen === false)', arg=uid)
                    for width in [1698, 1400, 1200, 1000, 800, 721]:
                        page.set_viewport_size({'width': width, 'height': 900})
                        page.wait_for_timeout(80)
                        freeze_button(page)
                        page.keyboard.press('Escape')
                    button = freeze_button(page)
                    expect(button).to_have_attribute('aria-label', '冻结现场')
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/freeze') as response:
                        button.click()
                    answer = response.value.json()
                    assert response.value.status == 200 and answer['frozen'] and answer['process_count'] == 2, answer
                    expect(button).to_have_attribute('aria-label', '恢复运行')
                    check_pause_badges(page, uid)
                    check_freeze_overlay(page)
                    expect(page.locator('#session-stop-notice')).to_be_hidden()
                    page.locator(f'#side .item[data-uid="{other_uid}"]').click()
                    page.wait_for_function('uid => S.sel === uid', arg=other_uid)
                    expect(page.locator('#session-stop-notice')).to_be_hidden()
                    expect(page.locator('#session-freeze-overlay')).to_be_hidden()
                    expect(page.locator('#dlive.frozen')).to_have_count(0)
                    # The paused session retains its own marker and unread count.
                    page.evaluate('uid => addUnread(uid, 3)', uid)
                    paused_side = page.locator(f'#side .item[data-uid="{uid}"] > .ico > .item-status')
                    expect(paused_side).to_have_class(re.compile(r'\bfrozen\b'))
                    expect(paused_side).to_have_text('')
                    expect(paused_side.locator('use')).to_have_attribute('href', '#i-pause')
                    expect(paused_side).to_have_css('background-color', 'rgb(251, 191, 36)')
                    expect(paused_side).to_have_attribute('title', re.compile('3 条新内容.*已暂停'))
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    check_pause_badges(page, uid)
                    expect(page.locator('#session-stop-notice')).to_be_hidden()
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
                    check_freeze_overlay(page)
                    freeze_button(page)
                    page.locator('.dhead [data-report-bug]').click()
                    expect(page.locator('#bug-report-dialog')).to_be_visible()
                    page.locator('#bug-report-dialog .modal-close').click()
                    page.reload(wait_until='networkidle')
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    page.wait_for_function('uid => T.list.some(row => row.uid === uid && row.frozen === true)', arg=uid)
                    button = freeze_button(page)
                    expect(button).to_have_attribute('aria-label', '恢复运行')
                    check_pause_badges(page, uid)
                    assert page.evaluate("browserStateSnapshot('fixture').data.terminal.frozen") is True
                    check_freeze_overlay(page)
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/freeze') as response:
                        page.locator('[data-freeze-resume]').click()
                    assert response.value.status == 200 and response.value.json()['frozen'] is False
                    expect(button).to_have_attribute('aria-label', '冻结现场')
                    expect(page.locator('#session-freeze-overlay')).to_be_hidden()
                    expect(page.locator('#dlive.frozen')).to_have_count(0)
                    expect(page.locator(f'#side .item[data-uid="{uid}"] .item-status.frozen')).to_have_count(0)
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
                    # Hub keeps its own capability; Windows node rows omit frozen.
                    def unsupported_rows(route):
                        headers = {key: value for key, value in route.request.headers.items()
                                   if key.lower() != 'x-sessiondock-list'}
                        response = route.fetch(headers=headers)
                        data = response.json()
                        assert 'sessions' in data, (response.status, data)
                        for row in data['sessions']:
                            row.pop('frozen', None)
                        route.fulfill(response=response, json=data)
                    page.route('**/api/term/list', unsupported_rows)
                    page.goto(base, wait_until='networkidle')
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    check_unavailable_pause(page, '当前节点不支持冻结现场')
                    page.unroute('**/api/term/list', unsupported_rows)
                    page.goto(base, wait_until='networkidle')
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    page.wait_for_function('uid => T.list.some(row => row.uid === uid && row.frozen === false)', arg=uid)
                    button = freeze_button(page)
                    expect(button).to_have_attribute('aria-label', '冻结现场')
                    expect(button).not_to_have_attribute('aria-disabled', 'true')
                    expect(button).to_have_attribute('title', '冻结现场')
                    # Mobile action menu uses the same recovery control and fits.
                    page.set_viewport_size({'width': 390, 'height': 844})
                    page.wait_for_timeout(80)
                    if page.locator('#side').is_visible():
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                    button = freeze_button(page)
                    bounds = button.bounding_box()
                    assert bounds and bounds['x'] >= 0 and bounds['x'] + bounds['width'] <= 391, bounds
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/freeze'):
                        button.click()
                    expect(button).to_have_attribute('aria-label', '恢复运行')
                    check_pause_badges(page, uid)
                    assert page.evaluate("browserStateSnapshot('fixture').data.terminal.frozen") is True
                    page.keyboard.press('Escape')
                    check_freeze_overlay(page)
                    page.locator('.dhead .mobile-back').click()
                    expect(page.locator('#session-freeze-overlay')).to_be_hidden()
                    expect(page.locator('#side')).to_be_visible()
                    expect(page.locator('#right')).to_be_hidden()
                    page.set_viewport_size({'width': 1000, 'height': 900})
                    check_freeze_overlay(page)
                    page.set_viewport_size({'width': 390, 'height': 844})
                    expect(page.locator('#session-freeze-overlay')).to_be_hidden()
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    check_freeze_overlay(page)
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
    print('PASS freeze browser: unsupported Node/Hub and absent instance retain gray pause glyph and hover reason, '
          'mouse/keyboard cannot freeze unavailable sessions, real parent/child stop and progress resume, idempotency, stale instance refusal, '
          'session-specific pause badges and unread counts, centered themed session overlay, mobile list hides overlay, single-line pause message and play control, report dialog and frozen snapshot, reload recovery, authenticated Hub, 390px menu, ordinary stop, native files preserved')


if __name__ == '__main__':
    main()
