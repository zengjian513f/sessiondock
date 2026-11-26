#!/usr/bin/env python3
"""OpenCode background work in ordinary and SDK views with a fake native CLI."""
import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import sqlite3
import tempfile

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY, Corpus, isolated_server
from opencode_sdk_fixtures import FakeOpenCode, SID
from sdk_ui_fixtures import start_sdk
from send_browser import initialize

CHILD = 'ses_opencode_background_child'
PARENT_JOB = 'job_opencode_parent'
PARENT_SHELL = 'sh_opencode_parent'
CHILD_JOB = 'job_opencode_child'
CHILD_SHELL = 'sh_opencode_child'
OLD_SHELL = 'sh_opencode_already_finished'


def fixture(native):
    native.create(CHILD, str(native.cwd), parent=SID)
    native.append(CHILD, {'id': 'msg_opencode_child', 'type': 'user',
                          'text': 'OpenCode child history', 'time': {'created': 1}})
    native.append(SID, {'id': 'msg_opencode_shell_complete', 'type': 'shell',
                        'shellID': OLD_SHELL, 'status': 'exited', 'command': 'finished native shell',
                        'time': {'created': 2, 'completed': 3}, 'metadata': {'background': True},
                        'output': {'size': 13, 'cursor': 13, 'truncated': False, 'output': 'finished tail'}})
    native.background_jobs = [
        {'kind': 'shell', 'id': PARENT_JOB, 'sessionID': SID, 'shellID': PARENT_SHELL,
         'command': 'parent shell command', 'status': 'running'},
        {'kind': 'shell', 'id': CHILD_JOB, 'sessionID': CHILD, 'shellID': CHILD_SHELL,
         'command': 'child shell command', 'status': 'running'},
        {'kind': 'shell', 'id': 'job_finished', 'sessionID': SID, 'shellID': OLD_SHELL,
         'command': 'finished native shell', 'status': 'completed'},
        {'kind': 'subagent', 'id': CHILD, 'sessionID': SID, 'childSessionID': CHILD,
         'description': 'reports through the native child view', 'status': 'running'},
        {'kind': 'shell', 'id': 'job_project_only', 'shellID': 'sh_project_only',
         'command': 'project-wide shell with no session evidence', 'status': 'running'},
    ]
    native.shell_logs.update({PARENT_SHELL: b'early forbidden prefix\n' + b'x' * 70_000 + b'\nlatest parent tail',
                              CHILD_SHELL: b'', 'sh_project_only': b'project-only sensitive output'})
    native.background_timers = [
        {'id': 'aut_opencode_parent', 'sessionID': SID, 'status': 'active', 'tz': 'UTC',
         'trigger': {'type': 'cron', 'cron': '0 0 1 * *'},
         'action': {'type': 'prompt', 'prompt': 'parent native timer'},
         'time': {'created': 1795695751967}, 'nextDueAt': 1893456000000},
        {'id': 'aut_opencode_child', 'sessionID': CHILD, 'status': 'active', 'tz': 'UTC',
         'trigger': {'type': 'at', 'at': '2030-01-01T00:00:00Z'},
         'action': {'type': 'prompt', 'prompt': 'child native timer'},
         'time': {'created': 1795695751967}, 'nextDueAt': 1893456000000},
        {'id': 'aut_opencode_paused', 'sessionID': SID, 'status': 'paused', 'tz': 'UTC',
         'trigger': {'type': 'at', 'at': '2030-01-01T00:00:00Z'},
         'action': {'type': 'prompt', 'prompt': 'paused native timer'},
         'time': {'created': 1795695751967}, 'nextDueAt': 1893456000000},
    ]
    with closing(sqlite3.connect(native.db)) as db, db:
        db.executescript("""
            CREATE TABLE job (id TEXT, session_id TEXT, kind TEXT, status TEXT,
                              recovery TEXT, time_created INTEGER);
            CREATE TABLE automation (id TEXT, session_id TEXT, trigger TEXT, action TEXT,
                                     tz TEXT, status TEXT, next_due_at INTEGER, time_created INTEGER);
        """)
        for job in native.background_jobs:
            db.execute('INSERT INTO job VALUES (?,?,?,?,?,?)',
                       (job['id'], job.get('sessionID', SID), job['kind'], job['status'], json.dumps(job), 1))
        for timer in native.background_timers:
            db.execute('INSERT INTO automation VALUES (?,?,?,?,?,?,?,?)',
                       (timer['id'], timer['sessionID'], json.dumps(timer['trigger']),
                        json.dumps(timer['action']), timer['tz'], timer['status'],
                        timer['nextDueAt'], timer['time']['created']))


def open_tasks(page, footer, command, prompt, screenshot=None):
    footer.get_by_role('button', name='后台任务 2', exact=True).click()
    menu = footer.get_by_role('dialog', name='后台任务')
    expect(menu).to_contain_text(prompt)
    for absent in ('project-wide shell', 'finished native shell', 'paused native timer',
                   'reports through the native child view'):
        expect(menu).not_to_contain_text(absent)
    if screenshot:
        page.screenshot(path=str(screenshot))
    menu.get_by_role('button').filter(has_text=command).click()
    return page.get_by_role('dialog', name='后台输出')


def exercise(page, native, base, screenshots=None):
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=SID)
    uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', SID)
    child_uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', CHILD)

    def open_conversation(session_uid):
        card = page.locator(f'#side .item[data-uid="{session_uid}"]')
        if not card.is_visible():
            page.locator('#detail .mobile-back').click()
        card.click()
        page.locator('#a-chat').click()

    open_conversation(uid)
    expect(page.locator('#msgs')).to_contain_text('SDK native history')
    chat = page.locator('#conversation-status')
    expect(chat.get_by_role('button', name='后台任务 2', exact=True)).to_be_visible()
    assert native.received == [], 'opening ordinary history should only read the mirror'

    dialog = open_tasks(page, chat, 'parent shell command', 'parent native timer',
                        screenshots / 'conversation-list.png' if screenshots else None)
    expect(dialog.locator('.task-output-text')).to_contain_text('latest parent tail')
    expect(dialog.locator('.task-output-text')).not_to_contain_text('early forbidden prefix')
    expect(dialog.get_by_role('link', name='下载完整日志')).to_have_count(0)
    assert len(dialog.locator('.task-output-text').text_content().encode()) <= 65536
    if screenshots:
        page.screenshot(path=str(screenshots / 'conversation-output.png'))
    page.keyboard.press('Escape')

    def selected_output(task, session_uid=uid):
        response = page.request.post(base + '/api/native-sdk/background_output',
                                     data={'uid': session_uid, 'task': task})
        assert response.ok, response.text()
        return response.json()['raw']

    assert selected_output(CHILD_JOB) is None
    assert selected_output('job_project_only') is None
    assert not any(f'/api/shell/{CHILD_SHELL}/output' in path or '/api/shell/sh_project_only/output' in path
                   for _, path, _ in native.received)
    before = len(native.received)
    assert selected_output(OLD_SHELL) == {'output': 'finished tail'}
    assert len(native.received) == before, 'settled native message already retains its own output'

    open_conversation(child_uid)
    child = open_tasks(page, chat, 'child shell command', 'child native timer')
    expect(child.get_by_role('status')).to_have_text('暂时没有输出')
    if screenshots:
        page.screenshot(path=str(screenshots / 'child-empty.png'))
    with native.lock:
        native.shell_logs[CHILD_SHELL] = b'child-only live result'
    child.get_by_role('button', name='刷新').click()
    expect(child.locator('.task-output-text')).to_contain_text('child-only live result')
    assert selected_output(PARENT_JOB, child_uid) is None
    page.keyboard.press('Escape')
    open_conversation(uid)

    page.locator('#a-sdk').click()
    start_sdk(page)
    sdk = page.locator('#sdk-composer .sdk-status-row')
    dialog = open_tasks(page, sdk, 'parent shell command', 'parent native timer')
    expect(dialog.locator('.task-output-text')).to_contain_text('latest parent tail')
    if screenshots:
        page.screenshot(path=str(screenshots / 'sdk-output.png'))
    page.keyboard.press('Escape')
    page.wait_for_function('() => NativeSdkView.raw?.background_commands?.some(c => c.output_available)')
    assert page.evaluate('() => NativeSdkView.raw.background_commands.every(c => !Object.hasOwn(c, "output"))')
    assert page.evaluate('() => NativeSdkView.raw.background_timers[0].due_at') == '2030-01-01T00:00:00.000Z'

    with native.lock:
        native.background_unavailable = True
        native.background_jobs[0]['status'] = 'completed'
    before = sum('/api/experimental/job?' in path for _, path, _ in native.received)
    for _ in range(80):
        if sum('/api/experimental/job?' in path for _, path, _ in native.received) > before:
            break
        page.wait_for_timeout(100)
    else:
        raise AssertionError('OpenCode SDK did not retry the background observation')
    expect(sdk.get_by_role('button', name='后台任务 2', exact=True)).to_be_visible()
    with native.lock:
        native.background_unavailable = False
    with closing(sqlite3.connect(native.db)) as db, db:
        db.execute("UPDATE job SET status='completed' WHERE id=?", (PARENT_JOB,))
    expect(sdk.get_by_role('button', name='后台任务 1', exact=True)).to_be_visible(timeout=20000)
    page.locator('#a-chat').click()
    expect(chat.get_by_role('button', name='后台任务 1', exact=True)).to_be_visible(timeout=15000)
    assert all(method == 'GET' for method, _, _ in native.received), native.received


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--host-binary', type=Path, default=BINARY.parent / 'ptyhost')
    parser.add_argument('--screenshots', type=Path)
    parser.add_argument('--mobile', action='store_true')
    args = parser.parse_args()
    run_background(args.binary, args.host_binary, args.screenshots, args.mobile)


def run_background(binary, host_binary, screenshots=None, mobile=False):
    if screenshots:
        screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-opencode-background-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        for name in ('claude', 'codex', 'grok', 'host', 'state', 'proc'):
            (root / name).mkdir()
        (root / 'proc' / 'stat').write_text('btime 1700000000\n')
        with FakeOpenCode(root) as native:
            fixture(native)
            launcher = root / 'launcher.json'
            launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(host_binary.resolve(strict=True)),
                                            'host_dir': str(root / 'host'), 'profiles': [native.profile()]}))
            initialize('--initialize-lifecycle', root / 'ledger', binary.resolve(strict=True))
            env = {'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_OPENCODE_DB': str(native.db),
                   'SESSIONDOCK_OPENCODE_ROOT': str(root / 'mirror'), 'SESSIONDOCK_PROC_ROOT': str(root / 'proc')}
            with isolated_server(Corpus(root), binary, host_dir=root / 'host', state_dir=root / 'state',
                                 lifecycle_dir=root / 'ledger', launcher_config=launcher, extra_env=env) as (base, _):
                options = {'headless': True}
                if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                    options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                browser = pw.chromium.launch(**options)
                context = browser.new_context(
                    service_workers='block', viewport={'width': 390 if mobile else 1280, 'height': 850},
                    color_scheme='dark' if mobile else 'light',
                )
                context.route('**/*', lambda route: route.continue_()
                              if route.request.url.startswith(base + '/') else route.abort())
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                exercise(page, native, base, screenshots)
                assert not errors, errors
                browser.close()
    print('OpenCode background Chromium: ordinary and SDK views, scoped child output, tail and native expiry passed')


if __name__ == '__main__':
    main()
