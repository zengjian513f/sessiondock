#!/usr/bin/env python3
"""Agy native parent ownership through Chromium; synthetic catalog, no CLI."""
from __future__ import annotations
import argparse
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
from types import SimpleNamespace
sys.dont_write_bytecode = True
from playwright.sync_api import expect, sync_playwright
from agy_history_browser import SCHEMA, record as history_record, system_record as history_system_record
from agy_history_browser import write_transcript, private_environment, execute
from history_fixtures import BINARY, REPO, Corpus, isolated_server, encoded, get_json
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from spawned_by_fixtures import proc_pid
from send_browser import initialize

PARENT = 'a6000000-0000-4000-8000-000000000011'
CHILD = 'a6000000-0000-4000-8000-000000000012'
GRANDCHILD = 'a6000000-0000-4000-8000-000000000013'
MISSING = 'a6000000-0000-4000-8000-000000000014'
ORPHAN = 'a6000000-0000-4000-8000-000000000015'
COMPLETED = 'a6000000-0000-4000-8000-000000000016'
TIMER_WAIT = '后台等待'
HISTORY_START = datetime.now(timezone.utc) - timedelta(minutes=1)


def stamp(index):
    return (HISTORY_START + timedelta(seconds=index)).isoformat()


def record(index, kind, text):
    return {**history_record(index, kind, text), 'created_at': stamp(index)}


def system_record(index, text, **kwargs):
    return {**history_system_record(index, text, **kwargs), 'created_at': stamp(index)}


def task(index, sid=CHILD, prompt='Check training progress.'):
    value = record(index, 'GENERIC', f'Created At: {stamp(index)}\n'
        f'Tool is running as a background task with task id: {sid}/task-{index}\n'
        f'Task Description: Timer: 10m, Prompt: {prompt}')
    value['status'] = 'RUNNING'
    return value


def pending_tasks():
    return [record(0, 'PLANNER_RESPONSE', 'History ' + CHILD), task(1), task(2),
            system_record(3, f'Task id "{CHILD}/task-1" finished with result:\nCommand exited with code 0.',
                          sender=CHILD + '/task-1'),
            # A task can outlive the bounded row-summary tail and a final answer.
            {**record(4, 'PLANNER_RESPONSE', '训练任务已提交，继续等待后台通知。'),
             'synthetic_padding': 'x' * (600 * 1024)},
            record(5, 'PLANNER_RESPONSE', '已启动后台定时器，等待检查训练进度。')]


def expect_child_header(page, running):
    badge = page.locator('#dlive')
    if running:
        expect(badge).to_be_visible()
        expect(badge).to_have_attribute('title', '子代理运行中')
    else:
        expect(badge).to_be_hidden()


def expect_read_only_child(page, status_row, state, screenshot=None, detail_text=None):
    row = page.locator(status_row)
    status = row.locator('.sdk-status')
    expect(status.locator('[role=status]')).to_have_text('前台待确认' if state == TIMER_WAIT else state)
    expect_child_header(page, state != '未运行')
    expect(row.locator('.composer-run-action')).to_be_hidden()
    expect(page.locator('#a-session-toggle')).to_have_count(0)
    if state == TIMER_WAIT:
        expect(status).to_have_attribute('data-state', 'neutral')
        background = row.locator('.sdk-background-status')
        expect(background).to_have_text(TIMER_WAIT)
        background.click()
    else:
        status.click()
    details = row.locator('.sdk-status-menu:popover-open')
    expect(details).to_contain_text('Agy 子代理由主会话管理')
    if state == TIMER_WAIT:
        expect(details).not_to_contain_text('Check training progress.')
        page.keyboard.press('Escape')
        row.get_by_role('button', name='后台任务 1').click()
        details = row.locator('.background-command-list:popover-open')
        expect(details).to_contain_text('Check training progress.')
        expect(details).to_contain_text('预计触发')
        expect(details).to_contain_text('设置于')
    if detail_text:
        expect(details).to_contain_text(detail_text)
        expect(details).to_contain_text('当前步骤已用时')
    if screenshot:
        page.screenshot(path=str(screenshot))
    page.keyboard.press('Escape')
    expect(details).to_have_count(0)


def run(binary, screenshots):
    screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-nesting-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        for name in ('home', 'native', 'work', 'proc', 'state', 'host', 'ledger', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        native = root / 'home/.gemini/antigravity-cli'
        native.mkdir(parents=True, mode=0o700)
        db = native / 'conversation_summaries.db'
        with sqlite3.connect(db) as connection:
            connection.execute(SCHEMA)
            for sid, title, parent, status in (
                (PARENT, '复核已关闭研报实验 / Agy native parent', '', 'CASCADE_RUN_STATUS_IDLE'),
                (CHILD, '', PARENT, 'CASCADE_RUN_STATUS_IDLE'),
                (GRANDCHILD, 'Nested reviewer / 核对实验结果与历史结论', CHILD, 'CASCADE_RUN_STATUS_RUNNING'),
                (COMPLETED, '已完成的审阅子代理', PARENT, 'CASCADE_RUN_STATUS_IDLE'),
                (ORPHAN, '暂时缺失的父会话', MISSING, 'CASCADE_RUN_STATUS_IDLE')):
                connection.execute('INSERT INTO conversation_summaries VALUES (?,?,?,?,?,?,?,?)',
                    (sid, title, stamp(0), json.dumps([(root / 'work').as_uri()]),
                     parent, status, 1, 'self'))
                write_transcript(native, sid, [record(0, 'PLANNER_RESPONSE', 'History ' + sid)])
        write_transcript(native, CHILD, pending_tasks())
        write_transcript(native, GRANDCHILD, [record(0, 'PLANNER_RESPONSE', 'History ' + GRANDCHILD),
                                              task(1, GRANDCHILD, 'Check the sibling task.')])
        (native / 'conversations').mkdir()
        parent_db = native / 'conversations' / (PARENT + '.db')
        parent_db.touch()
        proc_pid(root / 'proc', 100, 'agy', ['agy', '--conversation', PARENT], 1,
                 cwd=str(root / 'work'), fds={3: parent_db})
        executable = root / 'agy'
        executable.write_text('#!/bin/sh\nexit 1\n')
        executable.chmod(0o700)
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
            'host_dir': str(root / 'host'), 'adapters': [], 'profiles': [
                {'id': 'agy-read-only', 'source': 'agy', 'executable': str(executable), 'args': [], 'env': {}}]}))
        initialize('--initialize-lifecycle', root / 'ledger', binary.resolve())
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        try:
            with private_environment(root):
                for restart in (False, True):
                    node_port = free_port()
                    with isolated_server(Corpus(root), binary, state_dir=root / 'state', host_dir=root / 'host',
                        extra_env={**node_env(root, node_port, '127.0.0.0/8'),
                                   'SESSIONDOCK_AGY_HOME': str(native), 'SESSIONDOCK_AGY_ROOT': str(root / 'mirror'),
                                   'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_LAUNCHER_CONFIG': str(launcher),
                                   'SESSIONDOCK_LIFECYCLE_DIR': str(root / 'ledger'),
                                   'SESSIONDOCK_PROC_ROOT': str(root / 'proc')}) as (base, opener):
                        for width, theme in ([(1280, 'light')] if restart else
                                              [(1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark')]):
                            context = browser.new_context(viewport={'width': width, 'height': 900},
                                                          color_scheme=theme, service_workers='block')
                            context.route('**/*', lambda route: route.continue_()
                                          if route.request.url.startswith(base + '/') else route.abort())
                            page = context.new_page()
                            errors, opens = [], []
                            page.on('pageerror', lambda error: errors.append(str(error)))
                            page.on('request', lambda request: opens.append(request.url)
                                if request.method == 'POST' and request.url.split('?')[0].endswith('/open') else None)
                            page.set_default_timeout(15000)
                            page.goto(base)
                            page.wait_for_function('sid => S.sessions.some(r => r.sid === sid && r.agents === 3)', arg=PARENT)
                            rows = page.evaluate('S.sessions')
                            parent = next(row for row in rows if row['sid'] == PARENT)
                            assert not any(row['sid'] in (CHILD, GRANDCHILD, COMPLETED, ORPHAN) for row in rows)
                            child = page.locator(f'#side .item.agent[data-agent="{CHILD}"]')
                            caret = page.locator(f'#side .item[data-uid="{parent["uid"]}"] .nest-caret')
                            if not child.is_visible():
                                caret.click()
                            expect(child).to_be_visible()
                            agents = page.evaluate('uid => S.sessions.find(r => r.uid === uid).agent_items', parent['uid'])
                            agents = {row['id']: row for row in agents}
                            assert agents[CHILD]['active'] and agents[GRANDCHILD]['active']
                            caret.click()
                            expect(child).not_to_be_visible()
                            caret.click()
                            page.locator(f'#side .item[data-uid="{parent["uid"]}"]').click()
                            page.locator('#a-sdk').click()
                            expect(page.locator('.sdk-history')).to_contain_text('History ' + PARENT)
                            expect(page.locator('#sdk-send')).to_be_disabled()
                            page.locator('#a-session-title').click()
                            page.screenshot(path=str(screenshots / f'{theme}-{width}-sdk-parent-title.png'))
                            expect(page.locator('#session-view-menu')).to_be_hidden()
                            expect(page.get_by_role('dialog', name='重命名会话', exact=True)).to_be_hidden()
                            page.locator('#a-view-switch').click()
                            page.locator(f'#session-view-menu button[data-agent="{CHILD}"]').click()
                            page.locator('#a-chat').click()
                            expect(page.locator('#msgs')).to_contain_text('History ' + CHILD)
                            assert page.evaluate('S.agent') == CHILD
                            page.wait_for_function('uid => S.live.has(uid)', arg=parent['uid'])
                            page.locator('#a-sdk').click()
                            expect(page.locator('.sdk-history')).to_contain_text('History ' + CHILD)
                            page.screenshot(path=str(screenshots / f'{theme}-{width}-sdk-child.png'))
                            expect_read_only_child(page, '#sdk-composer', TIMER_WAIT)
                            expect(page.locator('#sdk-input')).to_be_disabled()
                            expect(page.locator('#sdk-send')).to_be_disabled()
                            page.locator('#a-view-switch').click()
                            expect(page.locator(f'#session-view-menu button[data-agent="{CHILD}"] .view-live')).to_be_visible()
                            page.screenshot(path=str(screenshots / f'{theme}-{width}-running-menu.png'))
                            page.locator(f'#session-view-menu button[data-agent="{COMPLETED}"]').click()
                            expect(page.locator('.sdk-history')).to_contain_text('History ' + COMPLETED)
                            expect_read_only_child(page, '#sdk-composer', '未运行')
                            page.screenshot(path=str(screenshots / f'{theme}-{width}-sdk-sibling-idle.png'))
                            page.locator('#a-view-switch').click()
                            page.locator(f'#session-view-menu button[data-agent="{CHILD}"]').click()
                            page.locator('#a-chat').click()
                            expect_read_only_child(page, '#conversation-status', TIMER_WAIT)
                            page.screenshot(path=str(screenshots / f'{theme}-{width}-child.png'))
                            page.reload()
                            expect(page.locator('#msgs')).to_contain_text('History ' + CHILD)
                            if width == 1280:
                                page.locator(f'#side .item.agent[data-agent="{GRANDCHILD}"]').click()
                                page.locator('#a-chat').click()
                                expect(page.locator('#msgs')).to_contain_text('History ' + GRANDCHILD)
                                child.click()
                            if not restart and width == 1280 and theme == 'light':
                                initial = get_json(opener, base, '/api/messages/' + parent['uid'] + '?agent=' + CHILD)
                                path = native / 'brain' / CHILD / '.system_generated/logs/transcript_full.jsonl'
                                with path.open('ab') as stream:
                                    stream.write(encoded(record(6, 'PLANNER_RESPONSE', 'agy child live tail')))
                                expect(page.locator('#msgs')).to_contain_text('agy child live tail')
                                delta = get_json(opener, base, '/api/messages/' + parent['uid'] + '?agent=' + CHILD
                                    + '&start=' + str(initial['end']) + '&anchor=' + initial['anchor']
                                    + '&head=' + initial['version']['head'])
                                assert not delta['reset'] and [m['text'] for m in delta['messages']] == ['agy child live tail']
                                old = page.locator('#stat').get_attribute('data-seq') or ''
                                page.locator('#q').fill('agy child live tail')
                                page.locator('#q').press('Enter')
                                page.wait_for_function('old => String(document.querySelector("#stat").dataset.seq || "") !== old', arg=old)
                                child.click()
                                expect(page.locator('#msgs')).to_contain_text('agy child live tail')
                                assert page.evaluate('S.agent') == CHILD
                                page.locator('#q').fill('')
                                page.locator('#q').press('Enter')
                                expect(page.locator('#side-search-state')).to_be_hidden()
                            if width == 390:
                                page.locator('.dhead .mobile-back').first.click()
                            page.screenshot(path=str(screenshots / f'{theme}-{width}-tree.png'))
                            assert not errors, errors
                            assert not opens, opens
                            context.close()
                            print(f'PASS native Agy ownership: {theme} {width}, restart={restart}', flush=True)
                        if not restart:
                            hub_root = root / 'hub'
                            hub_root.mkdir()
                            hub = Hub(binary.resolve().with_name('sessiondock-hub'), hub_root,
                                      [SimpleNamespace(name='AgyNode', token=TOKEN, port=node_port)])
                            hub.start()
                            try:
                                hub_base = f'http://127.0.0.1:{hub.port}'
                                context = browser.new_context(viewport={'width': 390, 'height': 844},
                                                              color_scheme='dark', service_workers='block')
                                context.route('**/*', lambda route: route.continue_()
                                              if route.request.url.startswith(hub_base + '/') else route.abort())
                                page = context.new_page()
                                opens = []
                                page.on('request', lambda request: opens.append(request.url)
                                    if request.method == 'POST' and request.url.endswith('/open') else None)
                                page.goto(f'{hub_base}/?sid=agy:{PARENT}/agent:{CHILD}')
                                expect(page.locator('.sdk-history')).to_contain_text('History ' + CHILD)
                                expect_read_only_child(page, '#sdk-composer', TIMER_WAIT,
                                                       screenshots / 'hub-dark-390-sdk-child-details.png')
                                expect(page.locator('#sdk-send')).to_be_disabled()
                                page.screenshot(path=str(screenshots / 'hub-dark-390-sdk-child.png'))
                                page.locator('#a-view-switch').click()
                                page.locator(f'#session-view-menu button[data-agent="{COMPLETED}"]').click()
                                expect(page.locator('.sdk-history')).to_contain_text('History ' + COMPLETED)
                                expect_read_only_child(page, '#sdk-composer', '未运行')
                                page.screenshot(path=str(screenshots / 'hub-dark-390-sdk-sibling-idle.png'))
                                assert not opens, opens
                                context.close()
                                print('PASS Hub Agy child history and runtime controls', flush=True)
                            finally:
                                hub.stop()
                        # Native parent changes must invalidate graph ownership without metadata writes.
                        if restart:
                            context = browser.new_context()
                            page = context.new_page()
                            page.goto(base)
                            uid = parent['uid']
                            page.locator(f'#side .item[data-uid="{uid}"] .nest-caret').click()
                            expect(page.locator(f'#side .item.agent[data-agent="{CHILD}"]')).to_be_visible()
                            expect(page.locator(f'#side .item[data-uid="{uid}"] .nest-caret')).to_have_attribute('aria-busy', 'false')
                            def activity(active):
                                page.wait_for_function('''({uid, child, active}) => {
                                    return S.sessions.find(r => r.uid === uid)?.agent_items
                                        ?.find(a => a.id === child)?.active === active;
                                }''', arg={'uid': uid, 'child': CHILD, 'active': active})
                            def append(value):
                                with (native / 'brain' / CHILD / '.system_generated/logs/transcript_full.jsonl').open('ab') as stream:
                                    stream.write(encoded(value))
                            activity(True)
                            # Quoted task notices and unrelated senders cannot close it.
                            append(record(7, 'USER_INPUT', system_record(7, 'wake', sender=CHILD + '/task-2')['content']))
                            append(record(8, 'GENERIC', system_record(8, 'wake', sender=CHILD + '/task-2')['content']))
                            append(system_record(9, 'wake', sender=GRANDCHILD + '/task-2'))
                            activity(True)
                            append(system_record(10, 'Check training progress.', sender=CHILD + '/task-2'))
                            activity(False)
                            child = page.locator(f'#side .item.agent[data-agent="{CHILD}"]')
                            if not child.is_visible():
                                page.locator(f'#side .item[data-uid="{uid}"] .nest-caret').click()
                            child.click()
                            page.locator('#a-chat').click()
                            expect(page.locator('#msgs')).to_contain_text('Check training progress.')
                            expect_read_only_child(page, '#conversation-status', '未运行')
                            page.locator('#a-sdk').click()
                            expect_read_only_child(page, '#sdk-composer', '未运行')
                            page.screenshot(path=str(screenshots / 'light-1280-sdk-child-idle.png'))
                            page.locator('#a-view-switch').click()
                            expect(page.locator('#session-view-menu')).to_be_visible()
                            expect(page.locator(f'#session-view-menu button[data-agent="{CHILD}"] .view-live')).to_have_count(0)
                            expect(page.locator(f'#session-view-menu button[data-agent="{GRANDCHILD}"] .view-live')).to_be_visible()
                            expect(page.locator(f'#side .item[data-uid="{uid}"] .item-status')).to_be_visible()
                            page.screenshot(path=str(screenshots / 'completed-menu.png'))
                            page.locator(f'#session-view-menu button[data-agent="{CHILD}"]').click()
                            append(task(11))
                            activity(True)
                            expect_read_only_child(page, '#sdk-composer', TIMER_WAIT)
                            expect_child_header(page, True)
                            proc_dir = root / 'proc/100'
                            proc_dir.rename(root / 'paused-proc')
                            page.wait_for_function('uid => !S.live.has(uid)', arg=uid)
                            expect(page.locator('#sdk-composer .sdk-status')).to_have_text('未运行')
                            expect_child_header(page, False)
                            (root / 'paused-proc').rename(proc_dir)
                            page.wait_for_function('uid => S.live.has(uid)', arg=uid)
                            expect_read_only_child(page, '#sdk-composer', TIMER_WAIT)
                            expect_child_header(page, True)
                            append(system_record(12, '[Notice] Native runtime restarted; background tasks have stopped.'))
                            activity(False)
                            expect(page.locator('#sdk-composer .sdk-status')).to_have_text('未运行')
                            expect_child_header(page, False)
                            # Rewrites/rewinds replace the task evidence; completed step updates close it.
                            write_transcript(native, CHILD, [record(0, 'PLANNER_RESPONSE', 'agy child live tail'), task(1)])
                            activity(True)
                            append(record(1, 'GENERIC', 'Command exited with code 0.'))
                            activity(False)
                            write_transcript(native, CHILD, [record(0, 'PLANNER_RESPONSE', 'agy child live tail')])
                            activity(False)
                            # The child has no USER_INPUT and receives no SDK
                            # stream. Only its own native step establishes a phase.
                            page.locator('#a-chat').click()
                            execute(db, 'UPDATE conversation_summaries SET status=? WHERE conversation_id=?',
                                    ('CASCADE_RUN_STATUS_RUNNING', CHILD))
                            activity(True)
                            def active_step(index, text='', **extra):
                                return {**record(index, 'PLANNER_RESPONSE', text),
                                        'status': 'ACTIVE', 'created_at': datetime.now(timezone.utc).isoformat(),
                                        **extra}
                            append(active_step(1, thinking='分析证据'))
                            expect_read_only_child(page, '#conversation-status', '思考中',
                                                   screenshots / 'child-thinking.png', '4 字符思考记录')
                            page.locator('#a-sdk').click()
                            append(active_step(2, tool_calls=[{'name': 'view_file', 'args': {}}]))
                            expect_read_only_child(page, '#sdk-composer', '执行工具',
                                                   screenshots / 'child-tool.png', 'view_file')
                            append(active_step(3, '逐项核对'))
                            expect_read_only_child(page, '#sdk-composer', '生成回复',
                                                   screenshots / 'child-reply.png', '4 字符回复')
                            append(record(4, 'PLANNER_RESPONSE', 'agy child live tail'))
                            execute(db, 'UPDATE conversation_summaries SET status=? WHERE conversation_id=?',
                                    ('CASCADE_RUN_STATUS_IDLE', CHILD))
                            activity(False)
                            expect_read_only_child(page, '#sdk-composer', '未运行')
                            page.locator('#a-chat').click()
                            print('PASS idle catalog with pending tasks, completion, wakeup, quotes, restart and rewrite', flush=True)
                            execute(db, 'UPDATE conversation_summaries SET parent_conversation_id=? WHERE conversation_id=?', ('', CHILD))
                            page.wait_for_function('sid => S.sessions.some(r => r.sid === sid && r.agents === 1)', arg=CHILD)
                            uid = page.evaluate('sid => S.sessions.find(r => r.sid === sid).uid', CHILD)
                            page.locator(f'#side .item[data-uid="{uid}"]').click()
                            page.locator('#a-chat').click()
                            expect(page.locator('#msgs')).to_contain_text('agy child live tail')
                            context.close()
                            print('PASS native parent change updates ownership', flush=True)
                    if not restart:
                        # Simulate a persisted pre-fix mirror with unchanged native files.
                        directory = root / 'mirror/cli' / CHILD
                        for filename, field, key in [('summary.json', 'session', 'background_tasks'),
                                                     ('summary.json', 'session', 'background_commands'),
                                                     ('mirror-version.json', 'catalog_session', 'activity_projection')]:
                            path = directory / filename
                            value = json.loads(path.read_text())
                            value[field].pop(key, None)
                            path.write_text(json.dumps(value))
        finally:
            browser.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-agy-nesting-review'))
    args = parser.parse_args()
    run(args.binary, args.screenshots)
