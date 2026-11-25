#!/usr/bin/env python3
"""Open and follow a running Agy task's native log on the node and Hub."""
import argparse
import json
import sqlite3
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright
from agy_history_browser import record, transcript, write_transcript
from agy_task_notice_browser import notice
from history_fixtures import BINARY, REPO, encoded
from hub_fixtures import Hub, free_port
from native_sdk_queue_fixtures import NativeQueueFixture
from node_auth_fixtures import TOKEN, node_env
from sdk_ui_fixtures import start_sdk

CHILD = 'a7600000-0000-4000-8000-000000000002'
SCHEDULER = 'a7600000-0000-4000-8000-000000000003'


def task(sid, index, uri, description='python report.py'):
    body = (f'Created At: 2026-11-25T12:04:56Z\n'
            f'Tool is running as a background task with task id: {sid}/task-{index}\n'
            f'Task Description: {description}\nTask logs are available at: {uri}\n'
            'Log output:\n旧输出快照')
    return {**record(index, 'GENERIC', body), 'status': 'RUNNING'}


def progress(sid, step, index, command):
    body = f'Background command is still running after 1800s: {command}'
    return record(step, 'SYSTEM_MESSAGE', notice(f'{sid}/task-{index}', body))


def open_footer(page, footer, tasks, screenshots, label, extra=0):
    count = len(tasks) + extra
    button = footer.get_by_role('button', name=f'后台任务 {count}', exact=True)
    expect(button).to_be_visible(timeout=20000)
    assert footer.evaluate('e => e.scrollWidth <= e.clientWidth + 1')
    button.focus()
    button.press('Enter')
    if len(tasks) == 1 and not extra:
        panel = page.get_by_role('dialog', name='后台输出')
        index, description = next(iter(tasks.items()))
        expect(panel.locator('.task-output-number')).to_have_text(f'任务 {index}')
        expect(panel.locator('.task-output-command')).to_have_text(description)
    else:
        panel = footer.get_by_role('dialog', name='后台任务')
        options = panel.locator('.task-commands').get_by_role('button')
        expect(options).to_have_count(len(tasks))
        for index, description in tasks.items():
            option = options.filter(has_text=f'任务 {index}')
            expect(option.locator('.command-name')).to_have_text('\n'.join(description.splitlines()[:3]))
            if len(description.splitlines()) > 3:
                expect(option.locator('small')).to_contain_text(f'共 {len(description.splitlines())} 行')
            assert option.evaluate('e => e.scrollWidth <= e.clientWidth + 1')
        assert panel.bounding_box()['y'] + panel.bounding_box()['height'] <= button.bounding_box()['y']
    expect(panel).to_be_visible()
    page.screenshot(path=str(screenshots / f'{label}-open.png'))
    return panel


def check_parent_menu(menu):
    expect(menu.get_by_text('未登记输出日志', exact=False)).to_be_visible()
    expect(menu.locator('.task-timers')).to_contain_text('检查主会话队列')
    expect(menu).not_to_contain_text('检查分片进度')
    expect(menu).not_to_contain_text('任务 7')
    expect(menu.locator('.task-overdue')).to_be_hidden()


def select_agent(page, agent):
    page.locator('#a-view-switch').click()
    page.locator(f'#session-view-menu [data-agent="{agent}"]').click()


def check_scheduler(page, footer, screenshots, label):
    menu = open_footer(page, footer, {}, screenshots, label, extra=2)
    expect(menu.locator('.task-commands')).to_be_hidden()
    expect(menu.locator('.task-timers')).to_contain_text('检查分片进度')
    expect(menu).not_to_contain_text('检查主会话队列')
    overdue = menu.locator('.task-overdue')
    expect(overdue.locator('summary')).to_have_text('已到期，待回执 · 1')
    expect(overdue.get_by_text('检查上一次导出')).to_be_hidden()
    overdue.locator('summary').click()
    expect(overdue.get_by_text('检查上一次导出')).to_be_visible()
    expect(menu.get_by_role('link')).to_have_count(0)
    page.screenshot(path=str(screenshots / f'{label}-receipts.png'))
    page.keyboard.press('Escape')
    return menu


def check_view(page, panel, log, index, screenshots, label):
    log.unlink(missing_ok=True)
    if index == 4:
        # Opening another task, then returning, must preserve keyboard access.
        panel.get_by_role('button').filter(has_text='任务 5').click()
        viewer = page.get_by_role('dialog', name='后台输出')
        expect(viewer.locator('.task-output-text')).to_contain_text('正在核对分区')
        expect(viewer.locator('.task-script pre')).not_to_be_visible()
        page.screenshot(path=str(screenshots / f'{label}-script.png'))
        viewer.get_by_text('完整命令（26 行）', exact=True).click()
        expect(viewer.locator('.task-script pre')).to_contain_text('echo done')
        page.screenshot(path=str(screenshots / f'{label}-long-command.png'))
        viewer.get_by_role('button', name='任务列表').click()
        expect(panel.get_by_role('button').filter(has_text='任务 5')).to_be_focused()
        if label == 'node-390-dark-sdk':
            held = []
            page.route('**/api/session/file?**', lambda route: held.append(route), times=1)
            panel.get_by_role('button').filter(has_text='任务 4').click()
            expect(viewer.get_by_role('status')).to_have_text('正在读取输出…')
            page.screenshot(path=str(screenshots / f'{label}-loading.png'))
            held[0].continue_()
        else:
            panel.get_by_role('button').filter(has_text='任务 4').click()
    viewer = page.get_by_role('dialog', name='后台输出')
    url = viewer.get_by_role('link', name='下载完整日志').get_attribute('href')
    query = parse_qs(urlsplit(url).query)
    assert query['ref'] == [log.as_uri()] and query.get('agent', []) == ([CHILD] if index == 7 else [])
    expect(viewer.get_by_role('heading')).to_have_text('后台输出')
    status = viewer.get_by_role('status')
    viewer.get_by_role('button', name='刷新').click()
    expect(status).to_contain_text('读取失败')
    page.screenshot(path=str(screenshots / f'{label}-missing.png'))
    log.touch()
    viewer.get_by_role('button', name='刷新').click()
    expect(status).to_have_text('暂时没有输出')
    page.screenshot(path=str(screenshots / f'{label}-empty.png'))
    log.write_text('任务开始\n正在读取第 1 批记录\n', encoding='utf-8')
    output = viewer.locator('.task-output-text')
    expect(output).to_contain_text('正在读取第 1 批记录', timeout=10000)
    with log.open('a', encoding='utf-8') as stream:
        stream.write(''.join(f'第 {i} 批记录已处理\n' for i in range(2, 5000)))
        stream.write('<b>最新进度</b>\n')
    expect(output).to_contain_text('第 4999 批记录已处理', timeout=10000)
    expect(output).not_to_contain_text('任务开始')
    expect(status).to_have_text('显示最近 64 KiB')
    assert '<b>最新进度</b>' in output.inner_text() and not output.locator('b').count()
    assert output.evaluate('(e) => e.scrollHeight - e.scrollTop - e.clientHeight < 2')
    assert viewer.evaluate('el => el.scrollWidth <= el.clientWidth + 1 && document.documentElement.scrollWidth <= innerWidth')
    page.screenshot(path=str(screenshots / f'{label}-output.png'))
    viewer.get_by_label('自动刷新').uncheck()
    if label == 'node-390-dark-sdk':
        output.focus()
        page.keyboard.press('Control+Home')
        page.wait_for_function("document.querySelector('.task-output-text').scrollTop < 2")
        with log.open('a', encoding='utf-8') as stream:
            stream.write('继续处理下一批\n')
        viewer.get_by_role('button', name='刷新').click()
        expect(output).to_contain_text('继续处理下一批')
        assert output.evaluate('e => e.scrollTop < 2')
    log.unlink()
    viewer.get_by_role('button', name='刷新').click()
    expect(status).to_contain_text('读取失败')
    expect(output).to_contain_text('最新进度')
    log.write_text('日志已重新创建\n')
    viewer.get_by_role('button', name='刷新').click()
    expect(output).to_have_text('日志已重新创建')
    with page.expect_download() as download:
        viewer.get_by_role('link', name='下载完整日志').click()
    assert download.value.path().read_text() == '日志已重新创建\n'
    viewer.get_by_role('button', name='关闭').click()


def check_completion(page, chat, native, log, screenshots, label):
    viewer = open_footer(page, chat, {7: 'python report.py'}, screenshots, label + '-before-finish')
    sender = f'{CHILD}/task-7'
    with transcript(native, CHILD).open('ab') as stream:
        stream.write(encoded(record(8, 'SYSTEM_MESSAGE', notice(sender,
            f'Task id "{sender}" finished with result:\nCommand exited with code 0.'))))
    expect(chat.locator('.background-commands')).to_have_count(0, timeout=20000)
    expect(viewer.locator('.task-output-text')).to_have_text('日志已重新创建')
    page.keyboard.press('Escape')
    expect(viewer).not_to_be_visible()
    for name in ['展开本轮过程', '展开工具调用组']:
        buttons = page.get_by_role('button', name=name, exact=True)
        while buttons.count():
            buttons.first.click()
    link = page.locator('#msgs').get_by_role('link', name='查看后台输出 · 任务 7')
    link.click()
    expect(viewer.locator('.task-output-text')).to_have_text('日志已重新创建')
    viewer.get_by_role('button', name='关闭').click()
    expect(link).to_be_focused()
    with page.context.expect_page() as opened:
        link.click(button='middle')
    standalone = opened.value
    expect(standalone.locator('.task-output-text')).to_have_text('日志已重新创建')
    standalone.screenshot(path=str(screenshots / f'{label}-standalone.png'))
    standalone.close()
    next_log = log.with_name('task-9.log')
    with transcript(native, CHILD).open('ab') as stream:
        stream.write(encoded(task(CHILD, 9, next_log.as_uri())))
    open_footer(page, chat, {9: 'python report.py'}, screenshots, label + '-new')
    page.keyboard.press('Escape')
    sender = f'{CHILD}/task-9'
    with transcript(native, CHILD).open('ab') as stream:
        stream.write(encoded(record(10, 'SYSTEM_MESSAGE', notice(sender,
            f'Task id "{sender}" was canceled with result:\nTool execution was canceled'))))
    expect(chat.locator('.background-commands')).to_have_count(0, timeout=20000)


def check_timer_receipts(page, footer, native, screenshots, label):
    menu = open_footer(page, footer, {}, screenshots, label + '-timers-only', extra=2)
    expect(menu.locator('.task-timers')).to_contain_text('检查分片进度')
    # The open list follows receipts for the selected grandchild.
    with transcript(native, SCHEDULER).open('ab') as stream:
        stream.write(encoded(record(6, 'SYSTEM_MESSAGE', notice(f'{SCHEDULER}/task-4', 'Timer fired'))))
    expect(footer.get_by_role('button', name='后台任务 1', exact=True)).to_be_visible(timeout=20000)
    expect(menu.locator('.task-timers')).to_be_hidden()
    expect(menu.locator('.task-overdue summary')).to_be_visible()
    with transcript(native, SCHEDULER).open('ab') as stream:
        stream.write(encoded(record(7, 'SYSTEM_MESSAGE', notice(f'{SCHEDULER}/task-5', 'Timer fired'))))
    expect(footer.locator('.background-commands')).to_have_count(0, timeout=20000)
    expect(menu).not_to_be_visible()


def check_foreground(page, sdk, fixture, log, screenshots, label):
    native, parent = fixture.peer.native, fixture.sid
    page.locator('#a-view-switch').click()
    page.locator('#session-view-menu [data-agent=""]').click()
    page.locator('#a-sdk').click()
    start_sdk(page)
    (fixture.peer.root / 'native-echo-store').touch()
    page.locator('#sdk-input').fill('hold:foreground')
    page.locator('#sdk-send').click()
    expect(page.locator('.sdk-msgs')).to_contain_text('Agy running foreground', timeout=20000)
    expect(sdk.locator('.background-commands')).to_have_count(0, timeout=20000)
    active_log = log.with_name('task-20.log')
    active_log.write_text('正在生成合并报告\n')
    with transcript(native, parent).open('ab') as stream:
        stream.write(encoded(task(parent, 20, active_log.as_uri(), 'python merge.py')))
        stream.write(encoded(progress(parent, 21, 20, 'python merge.py')))
    with sqlite3.connect(native / 'conversations' / f'{parent}.db') as db:
        db.executemany('INSERT INTO steps VALUES (?,1,3,0,?)', [(20, b''), (21, b'')])
    (fixture.peer.root / 'foreground-status').write_text('IDLE')
    expect(sdk.locator('.sdk-status').get_by_role('status')).to_have_text('等待回执', timeout=20000)
    expect(sdk.get_by_role('button', name='后台任务 1', exact=True)).to_be_visible(timeout=20000)
    page.screenshot(path=str(screenshots / f'{label}-waiting-with-task.png'))
    viewer = open_footer(page, sdk, {20: 'python merge.py'}, screenshots, label + '-active')
    assert page.evaluate('NativeSdkView.running')
    expect(viewer.locator('.task-output-text')).to_have_text('正在生成合并报告')
    page.keyboard.press('Escape')
    fixture.peer.release('foreground')
    page.wait_for_function('!NativeSdkView.running', timeout=20000)


def seed_old_mirrors(browser, fixture):
    with fixture.server() as base:
        page = browser.new_page()
        page.goto(base)
        page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=fixture.sid)
        page.close()
    for sid in (fixture.sid, CHILD):
        directory = fixture.peer.mirror / 'cli' / sid
        summary = directory / 'summary.json'
        data = json.loads(summary.read_text())
        data['session'].update(background_commands=[], background_logs=[], background_tasks=len(data['session']['background_timers']))
        summary.write_text(json.dumps(data))
        version = directory / 'mirror-version.json'
        data = json.loads(version.read_text())
        data['catalog_session']['activity_projection'] = 4
        version.write_text(json.dumps(data))


def run(args):
    args.screenshots.mkdir(exist_ok=True, parents=True)
    with tempfile.TemporaryDirectory(prefix='sd-task-output-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        fixture = NativeQueueFixture(root / 'node', 'agy', args.binary.resolve(), REPO / 'target/debug/ptyhost')
        parent, native = fixture.sid, fixture.peer.native
        fixture.peer.seed(CHILD)
        fixture.peer.seed(SCHEDULER)
        with sqlite3.connect(native / 'conversation_summaries.db') as db:
            db.executemany('UPDATE conversation_summaries SET parent_conversation_id=?, title=? WHERE conversation_id=?',
                           [(parent, '报告生成', CHILD), (CHILD, '分片调度', SCHEDULER)])
        now = datetime.now(timezone.utc)
        reminders = []
        for index, prompt, created in [(4, '检查分片进度', now), (5, '检查上一次导出', now - timedelta(hours=2))]:
            native_log = native / 'brain' / SCHEDULER / '.system_generated/tasks' / f'task-{index}.log'
            native_log.parent.mkdir(exist_ok=True)
            native_log.touch()
            reminders.append({**task(SCHEDULER, index, native_log.as_uri(), 'Timer: 12m, Prompt: ' + prompt),
                              'created_at': created.isoformat()})
        write_transcript(native, SCHEDULER, reminders)
        parent_tasks = {4: 'python report.py', 5: 'python reconcile.py --input pending-records.jsonl --output reports/summary.json\n'
                        + '\n'.join(f'python verify.py --partition {i}' for i in range(24)) + '\necho done'}
        child_tasks = {7: 'python report.py'}
        logs = {}
        for sid, index in [(parent, 4), (CHILD, 7)]:
            log = logs[sid] = native / 'brain' / sid / '.system_generated/tasks' / f'task-{index}.log'
            log.parent.mkdir()
            wrong_log = task(sid, 2, log.as_uri())
            quoted = task(sid, 3, log.with_name('task-3.log').as_uri())
            log.write_text('正在检查进度\n')
            records = [record(0, 'USER_INPUT', '查看后台任务'),
                       {**quoted, 'type': 'PLANNER_RESPONSE'}, task(sid, index, log.as_uri())]
            if sid == parent:
                records.insert(1, wrong_log)
                log.with_name('task-5.log').write_text('已读取待处理记录\n正在核对分区 1 / 24\n')
                records.append(task(parent, 5, log.with_name('task-5.log').as_uri(), parent_tasks[5]))
                own_timer = task(parent, 6, log.with_name('task-6.log').as_uri(), 'Timer: 1h, Prompt: 检查主会话队列')
                records.append({**own_timer, 'created_at': now.isoformat()})
                records.extend([progress(parent, 7, 4, parent_tasks[4]), progress(parent, 8, 5, parent_tasks[5]),
                                record(9, 'SYSTEM_MESSAGE', notice(f'{parent}/task-4',
                                    f'Task id "{parent}/task-5" finished with result:\nCommand exited with code 0.'))])
            else:
                records.append(progress(CHILD, 11, index, child_tasks[index]))
            write_transcript(native, sid, records)
        with sqlite3.connect(native / 'conversations' / f'{parent}.db') as db:
            db.execute('UPDATE fixture_steps SET next_index=10')
        port = free_port()
        fixture.extra.update(node_env(fixture.root, port, '127.0.0.0/8'))
        browser = pw.chromium.launch(headless=True)
        try:
            seed_old_mirrors(browser, fixture)
            # Upgrade the parent from its unchanged native transcript and the child from its cached copy.
            child_transcript = transcript(native, CHILD)
            saved = child_transcript.rename(child_transcript.with_suffix('.saved'))
            with fixture.server() as base:
                hub_root = root / 'hub'
                hub_root.mkdir()
                hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                          [SimpleNamespace(name='TaskNode', token=TOKEN, port=port)])
                hub.start()
                try:
                    for target, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                        for width, theme in [(1280, 'light'), (390, 'dark')]:
                            label = f'{target}-{width}-{theme}'
                            context = browser.new_context(viewport={'width': width, 'height': 850}, color_scheme=theme)
                            context.route('**/*', lambda route: route.continue_()
                                          if route.request.url.startswith(url + '/') else route.abort())
                            page = context.new_page()
                            page.goto(url)
                            page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=parent)
                            uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', parent)
                            page.locator(f'#side .item[data-uid="{uid}"]').click()
                            sdk = page.locator('#sdk-composer .sdk-status-row')
                            chat = page.locator('#conversation-status')
                            page.locator('#a-chat').click()
                            menu = open_footer(page, chat, parent_tasks, args.screenshots, label + '-chat', extra=2)
                            check_parent_menu(menu)
                            page.keyboard.press('Escape')
                            expect(menu).not_to_be_visible()
                            page.locator('#a-sdk').click()
                            start_sdk(page)
                            menu = open_footer(page, sdk, parent_tasks, args.screenshots, label + '-sdk', extra=2)
                            check_parent_menu(menu)
                            check_view(page, menu, logs[parent], 4, args.screenshots, label + '-sdk')
                            select_agent(page, SCHEDULER)
                            page.locator('#a-sdk').click()
                            check_scheduler(page, sdk, args.screenshots, label + '-scheduler-sdk')
                            page.locator('#a-chat').click()
                            check_scheduler(page, chat, args.screenshots, label + '-scheduler-chat')
                            select_agent(page, CHILD)
                            page.locator('#a-sdk').click()
                            menu = open_footer(page, sdk, child_tasks, args.screenshots, label + '-child-sdk')
                            page.keyboard.press('Escape')
                            page.locator('#a-chat').click()
                            menu = open_footer(page, chat, child_tasks, args.screenshots, label + '-child-chat')
                            check_view(page, menu, logs[CHILD], 7, args.screenshots, label + '-child-chat')
                            if saved.exists():
                                saved.rename(child_transcript)
                            if target == 'hub' and width == 390:
                                check_completion(page, chat, native, logs[CHILD], args.screenshots, label)
                                select_agent(page, SCHEDULER)
                                page.locator('#a-sdk').click()
                                check_timer_receipts(page, sdk, native, args.screenshots, label)
                                check_foreground(page, sdk, fixture, logs[parent], args.screenshots, label)
                            context.close()
                            print('PASS', label, 'parent/child footers in SDK and chat, scoped log and follow/retry', flush=True)
                finally:
                    hub.stop()
        finally:
            browser.close()
            fixture.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-agy-task-output'))
    run(parser.parse_args())
