#!/usr/bin/env python3
"""Agy launcher: real picker, pending PTY, reload, discard and phone geometry.

Uses only a private fake CLI and loopback service. No native history, model
discovery or composer SEND is assumed. The unavailable-CLI case explicitly
overrides the capability response; it does not test installation probing.
"""

from contextlib import closing
import argparse
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time
from unittest.mock import patch
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
from agy_history_browser import SCHEMA, record, write_transcript
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from private_hosts import private_hosts
from popups import on_popup
from send_browser import initialize

RESUME_SID = '60651e8e-37aa-4e49-bc34-000000000001'
RESUME_TITLE = 'Agy 续接模型保持'
RESUME_MODEL = 'Gemini 4 Carbon (High)'
GLOBAL_MODEL = 'Gemini 4 Argon (Medium)'

FAKE = r'''
import json, os, sys, termios
from pathlib import Path
args = sys.argv[1:]
def opt(flag):
    for i, a in enumerate(args):
        if a == flag and i + 1 < len(args): return args[i + 1]
    return None
settings_file = Path(os.environ['HOME']) / '.gemini/antigravity-cli/settings.json'
saved = json.loads(settings_file.read_text()) if settings_file.is_file() else {}
session_settings = json.loads(opt('--settings')) if opt('--settings') else {}
active_model = opt('--model') or session_settings.get('model') or saved.get('model') or ''
conv = opt('--conversation')
held_db = (Path(os.environ['HOME']) / '.gemini/antigravity-cli/conversations' / f'{conv}.db').open('rb') if conv else None
with Path(os.environ['AGY_TEST_ARGV']).open('a') as log:
    log.write(json.dumps({'argv': args, 'cwd': os.getcwd(), 'pid': os.getpid(), 'active_model': active_model}) + '\n')
if '--version' in args:
    print('1.2.16', flush=True)
    sys.exit(0)
if not sys.stdin.isatty():
    sys.exit(0)
settings = termios.tcgetattr(0)
settings[3] &= ~termios.ECHO
termios.tcsetattr(0, termios.TCSANOW, settings)
print('AGY_PTY_READY model=' + active_model, flush=True)
for line in sys.stdin:
    print('AGY_CONFIRMED:' + line.rstrip('\r\n'), flush=True)
'''


def open_picker(page):
    # Let media-query and ResizeObserver work settle before choosing a control.
    # A click retry keeps its attached element even after header relayout moves
    # New into the hidden overflow menu.
    page.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
    page.locator('#new-session:visible, #header-more-btn:visible').first.click()
    if not page.locator('#new-session-dialog').is_visible():
        page.locator('#new-session').click()
    expect(page.locator('#new-session-dialog')).to_be_visible()


def wait_screen(page, text):
    read = """text => { const b = TERM?.buffer?.active; if (!b) return false;
        for (let i = 0; i < b.length; i++)
          if ((b.getLine(i)?.translateToString(true) || '').includes(text)) return true;
        return false; }"""
    page.wait_for_function((read.replace('TERM', 'T.term')), arg=text, timeout=15000)


def pending(page, base, record_id):
    return [r for r in page.request.get(base + '/api/term/list?force=1').json()['pending']
            if r['record_id'] == record_id]


def process_alive(pid):
    # Linux zombies have already exited; kill(pid, 0) alone counts them as alive.
    stat = Path(f'/proc/{pid}/stat')
    if stat.exists():
        return stat.read_text().rsplit(')', 1)[1].split()[0] != 'Z'
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def check_phone(page):
    page.set_viewport_size({'width': 390, 'height': 844})
    back = page.locator('#detail .mobile-back:visible')
    if back.count():
        back.click()
    for theme in ('light', 'dark'):
        page.emulate_media(color_scheme=theme)
        open_picker(page)
        labels = page.locator('#new-session-form .new-source label')
        assert labels.count() > 0
        for label in labels.all():
            expect(label).to_be_visible()
        geometry = page.evaluate("""() => {
          const dialog = document.querySelector('#new-session-dialog');
          const rect = e => { const r=e.getBoundingClientRect();
            return {left:r.left, right:r.right, top:r.top, bottom:r.bottom,
              width:r.width, height:r.height}; };
          const source=document.querySelector('#new-session-form .new-source');
          return {dialog:rect(dialog), width:innerWidth, height:innerHeight,
            overflow:dialog.scrollWidth > dialog.clientWidth + 1,
            source:rect(source), sourceOverflow:source.scrollWidth > source.clientWidth + 1,
            buttons:[...dialog.querySelectorAll(`.new-source label, #new-model, #new-effort,
              .modal-close, .modal-cancel, #new-session-go`)].map(rect)};
        }""")
        assert not geometry['overflow'] and not geometry['sourceOverflow'], (theme, geometry)
        for box in [geometry['dialog'], *geometry['buttons']]:
            assert box['width'] > 0 and box['height'] > 0, (theme, box)
            assert -1 <= box['left'] < box['right'] <= geometry['width'] + 1, (theme, box)
            assert -1 <= box['top'] < box['bottom'] <= geometry['height'] + 1, (theme, box)
        boxes = geometry['buttons'][:labels.count()]
        for i, a in enumerate(boxes):
            for b in boxes[i + 1:]:
                assert min(a['right'], b['right']) - max(a['left'], b['left']) <= 1 or \
                    min(a['bottom'], b['bottom']) - max(a['top'], b['top']) <= 1, (theme, a, b)
        page.locator('#new-session-dialog .modal-cancel').click()
    print('PASS 390px light/dark: all picker buttons visible without overflow', flush=True)


def check_unavailable(browser, base):
    context = browser.new_context(service_workers='block')
    context.add_init_script("localStorage.setItem('sessiondock.agentStartMode.agy', JSON.stringify('cli'))")
    intercepted, creates, errors = [], [], []
    def unavailable(route):
        response = route.fetch()
        body = response.json()
        body['sources']['agy'] = False
        body['resume_sources']['agy'] = False
        intercepted.append(True)
        route.fulfill(response=response, json=body)
    context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
    context.route('**/api/term/list*', unavailable)
    page = context.new_page()
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.on('request', lambda r: creates.append(r) if urlsplit(r.url).path == '/api/term/create' else None)
    try:
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded && T.sources.agy === false')
        open_picker(page)
        radio = page.locator('input[name="new-source"][value="agy"]')
        label = page.locator('#new-session-form label:has(input[value="agy"])')
        expect(radio).to_be_disabled()
        expect(radio).not_to_be_checked()
        assert '未安装' in (label.get_attribute('title') or '')
        box = label.bounding_box()
        assert box
        page.mouse.click(box['x'] + box['width'] / 2, box['y'] + box['height'] / 2)
        expect(radio).not_to_be_checked()
        assert intercepted and not creates and not errors, (intercepted, creates, errors)
        page.locator('#new-session-dialog .modal-cancel').click()
        print('PASS unavailable agy: capability-response override disables user selection', flush=True)
    finally:
        context.close()


def run(browser, base, root):
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    context.add_init_script("localStorage.setItem('sessiondock.agentStartMode.agy', JSON.stringify('cli'))")
    context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    on_popup(page, lambda popup: popup.accept())
    try:
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded && T.sources.agy === true')
        open_picker(page)
        page.locator('#new-session-form label:has(input[value="agy"])').click()
        # The picker may already select Agy and cache its catalog on opening.
        catalog = page.request.get(base + '/api/term/models?source=agy')
        assert catalog.status == 200, catalog.text()
        assert catalog.json()['models'] == [], catalog.json()
        expect(page.locator('#new-effort option')).to_have_text(['CLI 默认', 'low', 'medium', 'high', 'Extra', 'max'])
        page.locator('#new-effort').select_option('low')
        page.locator('#new-cwd').fill(str(root / 'work'))
        with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/create') as created:
            page.locator('#new-session-go').click()
        response = created.value
        assert response.status == 200, response.text()
        request, receipt = response.request.post_data_json, response.json()
        assert request['source'] == 'agy' and request['effort'] == 'low', request
        assert '--session-id' not in json.dumps(request), request
        assert receipt['running'] and receipt['launch_kind'] == 'new_pending', receipt
        assert not receipt.get('declared_sid'), receipt
        uid = 'tmux:' + receipt['name']
        page.wait_for_function('uid => S.sel === uid', arg=uid)
        item = page.locator(f'#side .item[data-uid="{uid}"]')
        expect(item).to_be_visible()
        expect(item.locator('use[href="#i-agy"]')).to_have_count(1)
        rows = pending(page, base, receipt['record_id'])
        assert len(rows) == 1 and rows[0]['source'] == 'agy' and rows[0]['running'], rows
        expect(page.locator('#composer')).to_be_visible()
        page.locator('#a-term').click()
        expect(page.locator('#termpane')).to_be_visible()
        wait_screen(page, 'AGY_PTY_READY')
        launches = [json.loads(line) for line in (root / 'argv.jsonl').read_text().splitlines()]
        launch = next(r for r in launches if '--effort' in r['argv'])
        assert '--session-id' not in launch['argv'] and launch['argv'] == ['--effort', 'low'], launch
        assert launch['cwd'] == str(root / 'work'), launch
        assert any(r['argv'] == ['--version'] for r in launches), launches
        for token in ('before-refresh', 'after-refresh'):
            if token == 'after-refresh':
                page.reload(wait_until='networkidle')
                expect(item).to_be_visible()
                item.click()
                if not page.locator('#termpane').is_visible():
                    page.locator('#a-term').click()
                expect(page.locator('#termpane')).to_be_visible()
                rows = pending(page, base, receipt['record_id'])
                assert len(rows) == 1 and rows[0]['name'] == receipt['name'] and rows[0]['running'], rows
            page.locator('#termpane .xterm-helper-textarea').focus()
            page.keyboard.type(token)
            page.keyboard.press('Enter')
            wait_screen(page, 'AGY_CONFIRMED:' + token)
        action = page.locator('#a-session-action')
        if not action.is_visible():
            page.locator('#a-more').click()
        expect(action).to_have_attribute('aria-label', '丢弃当前会话')
        with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/discard') as discarded:
            action.click()
        assert discarded.value.status == 200, discarded.value.text()
        expect(item).to_have_count(0)
        assert not pending(page, base, receipt['record_id'])
        deadline = time.monotonic() + 5
        while process_alive(launch['pid']) and time.monotonic() < deadline:
            page.wait_for_timeout(50)
        assert not process_alive(launch['pid']), 'fake agy process survived discard'
        page.reload(wait_until='networkidle')
        expect(item).to_have_count(0)
        assert not pending(page, base, receipt['record_id'])
        # Resume/takeover of a recorded Agy session preserves its native model
        # via session-scoped --settings even when global settings.json differs.
        seeded = page.locator('#side .item').filter(has_text=RESUME_TITLE)
        expect(seeded).to_have_count(1)
        seeded.click()
        expect(page.locator('.dhead')).to_contain_text(RESUME_MODEL)
        with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/takeover') as resumed:
            page.locator('#a-term').click()
        assert resumed.value.status == 200, resumed.value.text()
        expect(page.locator('#termpane')).to_be_visible()
        wait_screen(page, 'AGY_PTY_READY model=' + RESUME_MODEL)
        launches = [json.loads(line) for line in (root / 'argv.jsonl').read_text().splitlines()]
        resumed_launch = next(r for r in reversed(launches) if '--conversation' in r['argv'])
        assert resumed_launch['argv'][:3] == ['--conversation', RESUME_SID, '--settings'], resumed_launch
        assert json.loads(resumed_launch['argv'][3]) == {'model': RESUME_MODEL}, resumed_launch
        assert resumed_launch['active_model'] == RESUME_MODEL, resumed_launch
        settings_file = root / 'home/.gemini/antigravity-cli/settings.json'
        assert json.loads(settings_file.read_text()) == {'model': GLOBAL_MODEL}
        check_phone(page)
        assert not errors, errors
        print('PASS agy: new_pending, argv, icon, keyboard PTY, reload, discard and resume model preservation', flush=True)
    finally:
        context.close()
    check_unavailable(browser, base)


def seed_resume_session(root):
    native = root / 'home/.gemini/antigravity-cli'
    native.mkdir(parents=True, exist_ok=True)
    (native / 'settings.json').write_text(json.dumps({'model': GLOBAL_MODEL}))
    (native / 'conversations').mkdir(parents=True, exist_ok=True)
    (native / 'conversations' / f'{RESUME_SID}.db').write_bytes(b'')
    with closing(sqlite3.connect(native / 'conversation_summaries.db')) as db, db:
        db.execute(SCHEMA)
        db.execute('INSERT INTO conversation_summaries VALUES (?,?,?,?,?,?,?,?)',
                   (RESUME_SID, RESUME_TITLE, '2026-10-16T08:49:00Z',
                    json.dumps([(root / 'work').as_uri()]), '', 'DONE', 2, 'fixture'))
    first = record(0, 'USER_INPUT', '保持当前会话模型')
    first['content'] += (
        '\n<USER_SETTINGS_CHANGE>\n'
        f'The user changed setting `Model Selection` from None to {RESUME_MODEL}. '
        "No need to comment on this change if the user doesn't ask about it. "
        'If reporting what model you are, please use a human readable name instead of the exact string.\n'
        '</USER_SETTINGS_CHANGE>')
    write_transcript(native, RESUME_SID, [first, record(1, 'PLANNER_RESPONSE', '已完成首轮分析。')])
    return native


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--ptyhost', type=Path, default=Path(os.environ.get('PTYHOST', REPO / 'target/debug/ptyhost')))
    parser.add_argument('--frontend', help='frontend directory; otherwise SESSIONDOCK_TEST_WEB_DIR')
    args = parser.parse_args()
    # Resolve the installed browser cache before giving child CLIs a private HOME.
    os.environ.setdefault('PLAYWRIGHT_BROWSERS_PATH', str(
        Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache')) / 'ms-playwright'))
    if args.frontend:
        os.environ['SESSIONDOCK_TEST_WEB_DIR'] = str(Path(args.frontend).resolve())
    host_binary = args.ptyhost.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-') as tmp, private_hosts(Path(tmp)):
        root = Path(tmp).resolve()
        for name in ('home', 'host', 'ledger', 'state', 'work', 'bin', 'claude', 'codex', 'grok', 'opencode', 'mirror', 'proc'):
            (root / name).mkdir(mode=0o700)
        native = seed_resume_session(root)
        fake = root / 'bin/agy'
        script = root / 'bin/agy_impl.py'
        script.write_text(FAKE)
        py = str(Path(sys.executable).resolve())
        fake.write_text(f'#!/bin/bash\nexec -a agy "{py}" "{script}" "$@"\n')
        fake.chmod(0o700)
        env = {'HOME': str(root / 'home'), 'PATH': str(root / 'bin') + ':/usr/bin:/bin',
            'TERM': 'xterm-256color', 'LANG': 'C.UTF-8', 'AGY_TEST_ARGV': str(root / 'argv.jsonl'),
            'XDG_CONFIG_HOME': str(root / 'home/config'), 'XDG_DATA_HOME': str(root / 'home/data'),
            'XDG_CACHE_HOME': str(root / 'home/cache')}
        launcher = root / 'launcher.json'
        launcher.touch(mode=0o600)
        launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(host_binary),
            'host_dir': str(root / 'host'), 'adapters': [], 'profiles': [
                {'id': 'agy-cli-v1', 'source': 'agy', 'executable': str(fake), 'args': [], 'env': env}]}))
        with patch.dict(os.environ, env):
            initialize('--initialize-lifecycle', root / 'ledger', args.binary.resolve())
            try:
                with isolated_server(Corpus(root), args.binary, host_dir=root / 'host',
                        lifecycle_dir=root / 'ledger', launcher_config=launcher, state_dir=root / 'state',
                        file_roots=(root / 'work',), file_write_roots=(root / 'work',),
                        trash_dir=root / 'trash', audit_dir=root / 'audit',
                        extra_env={'SESSIONDOCK_OPENCODE_ROOT': str(root / 'opencode'),
                            'SESSIONDOCK_OPENCODE_DB': str(root / 'opencode/empty.db'),
                            'SESSIONDOCK_AGY_HOME': str(native),
                            'SESSIONDOCK_AGY_ROOT': str(root / 'mirror')}) as (base, _), sync_playwright() as pw:
                    options = {'headless': True}
                    if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                        options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                    browser = pw.chromium.launch(**options)
                    try:
                        print('Frontend: ' + ('legacy'), flush=True)
                        run(browser, base, root)
                    finally:
                        browser.close()
            finally:
                # Only this fixture's private host records; never use a default dir.
                for record in (root / 'host').glob('sessiondock-*.json'):
                    subprocess.run([str(host_binary), '--dir', str(root / 'host'), 'kill',
                        record.stem, '--force'], env=env, cwd=root / 'work', timeout=5,
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)


if __name__ == '__main__':
    main()
