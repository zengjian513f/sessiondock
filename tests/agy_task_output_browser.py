#!/usr/bin/env python3
"""Open and follow a running Agy task's native log on the node and Hub."""
import argparse
import sqlite3
import tempfile
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


def task(sid, index, uri, description='python report.py'):
    body = (f'Created At: 2026-11-25T12:04:56Z\n'
            f'Tool is running as a background task with task id: {sid}/task-{index}\n'
            f'Task Description: {description}\nTask logs are available at: {uri}\n'
            'Log output:\n旧输出快照')
    return {**record(index, 'GENERIC', body), 'status': 'RUNNING'}


def open_list(page, footer, tasks, screenshots, label):
    button = footer.get_by_role('button', name=f'后台命令 {len(tasks)}', exact=True)
    expect(button).to_be_visible(timeout=20000)
    button.focus()
    button.press('Enter')
    menu = footer.get_by_role('dialog', name='后台命令')
    expect(menu).to_be_visible()
    expect(menu.get_by_role('link')).to_have_count(len(tasks))
    for index, description in tasks.items():
        expect(menu.get_by_role('link', name=f'task-{index} · {description} 查看输出')).to_be_visible()
    assert menu.bounding_box()['y'] + menu.bounding_box()['height'] <= button.bounding_box()['y']
    assert footer.evaluate('e => e.scrollWidth <= e.clientWidth + 1')
    page.screenshot(path=str(screenshots / f'{label}-list.png'))
    return menu


def check_view(page, menu, log, index, screenshots, label):
    log.unlink(missing_ok=True)
    with page.expect_popup() as opened:
        menu.get_by_role('link', name=f'task-{index} · python report.py 查看输出').click()
    viewer = opened.value
    query = parse_qs(urlsplit(viewer.url).query)
    assert query['ref'] == [log.as_uri()] and query.get('agent', []) == ([CHILD] if index == 7 else [])
    expect(viewer.get_by_role('heading')).to_have_text(f'后台输出 · task-{index}')
    status = viewer.get_by_role('status')
    expect(status).to_contain_text('读取失败')
    log.touch()
    viewer.get_by_role('button', name='刷新').click()
    expect(status).to_have_text('暂时没有输出')
    log.write_text('任务开始\n正在读取第 1 批记录\n', encoding='utf-8')
    output = viewer.locator('pre')
    expect(output).to_contain_text('正在读取第 1 批记录', timeout=10000)
    with log.open('a', encoding='utf-8') as stream:
        stream.write(''.join(f'第 {i} 批记录已处理\n' for i in range(2, 5000)))
        stream.write('<b>最新进度</b>\n')
    expect(output).to_contain_text('第 4999 批记录已处理', timeout=10000)
    expect(output).not_to_contain_text('任务开始')
    expect(status).to_have_text('显示最近 64 KiB')
    assert '<b>最新进度</b>' in output.inner_text() and not output.locator('b').count()
    assert output.evaluate('(e) => e.scrollHeight - e.scrollTop - e.clientHeight < 2')
    assert viewer.evaluate('document.documentElement.scrollWidth <= innerWidth')
    viewer.screenshot(path=str(screenshots / f'{label}-output.png'))
    viewer.get_by_label('自动刷新').uncheck()
    log.unlink()
    viewer.get_by_role('button', name='刷新').click()
    expect(status).to_contain_text('读取失败')
    expect(output).to_contain_text('最新进度')
    log.write_text('日志已重新创建\n')
    viewer.get_by_role('button', name='刷新').click()
    expect(output).to_have_text('日志已重新创建')
    with viewer.expect_download() as download:
        viewer.get_by_role('link', name='下载完整日志').click()
    assert download.value.path().read_text() == '日志已重新创建\n'
    viewer.close()


def check_completion(page, chat, native, log, screenshots, label):
    open_list(page, chat, {7: 'python report.py'}, screenshots, label + '-before-finish')
    sender = f'{CHILD}/task-7'
    with transcript(native, CHILD).open('ab') as stream:
        stream.write(encoded(record(8, 'SYSTEM_MESSAGE', notice(sender,
            f'Task id "{sender}" finished with result:\nCommand exited with code 0.'))))
    expect(chat.locator('.background-commands')).to_have_count(0, timeout=20000)
    for name in ['展开本轮过程', '展开工具调用组']:
        buttons = page.get_by_role('button', name=name, exact=True)
        while buttons.count():
            buttons.first.click()
    link = page.locator('#msgs').get_by_role('link', name='查看后台输出 · task-7')
    with page.expect_popup() as opened:
        link.click()
    viewer = opened.value
    expect(viewer.locator('pre')).to_have_text('日志已重新创建')
    viewer.close()
    next_log = log.with_name('task-9.log')
    with transcript(native, CHILD).open('ab') as stream:
        stream.write(encoded(task(CHILD, 9, next_log.as_uri())))
    open_list(page, chat, {9: 'python report.py'}, screenshots, label + '-new')
    page.keyboard.press('Escape')


def check_foreground(page, sdk, fixture, log, screenshots, label):
    native, parent = fixture.peer.native, fixture.sid
    page.locator('#a-view-switch').click()
    page.locator('#session-view-menu [data-agent=""]').click()
    page.locator('#a-sdk').click()
    start_sdk(page)
    page.locator('#sdk-input').fill('hold:foreground')
    page.locator('#sdk-send').click()
    expect(page.locator('.sdk-msgs')).to_contain_text('Agy running foreground', timeout=20000)
    active_log = log.with_name('task-20.log')
    active_log.write_text('正在生成合并报告\n')
    with transcript(native, parent).open('ab') as stream:
        stream.write(encoded(task(parent, 20, active_log.as_uri(), 'python merge.py')))
    menu = open_list(page, sdk, {20: 'python merge.py'}, screenshots, label + '-active')
    assert page.evaluate('NativeSdkView.running')
    with page.expect_popup() as opened:
        menu.get_by_role('link').click()
    viewer = opened.value
    expect(viewer.locator('pre')).to_have_text('正在生成合并报告')
    viewer.close()
    fixture.peer.release('foreground')
    page.wait_for_function('!NativeSdkView.running', timeout=20000)


def run(args):
    args.screenshots.mkdir(exist_ok=True, parents=True)
    with tempfile.TemporaryDirectory(prefix='sd-task-output-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        fixture = NativeQueueFixture(root / 'node', 'agy', args.binary.resolve(), REPO / 'target/debug/ptyhost')
        parent, native = fixture.sid, fixture.peer.native
        fixture.peer.seed(CHILD)
        with sqlite3.connect(native / 'conversation_summaries.db') as db:
            db.execute('UPDATE conversation_summaries SET parent_conversation_id=? WHERE conversation_id=?', (parent, CHILD))
        parent_tasks = {4: 'python report.py', 5: 'python reconcile.py --input pending-records.jsonl --output reports/summary.json'}
        child_tasks = {7: 'python report.py'}
        logs = {}
        for sid, index in [(parent, 4), (CHILD, 7)]:
            log = logs[sid] = native / 'brain' / sid / '.system_generated/tasks' / f'task-{index}.log'
            log.parent.mkdir()
            wrong_log = task(sid, 2, log.as_uri())
            quoted = task(sid, 3, log.with_name('task-3.log').as_uri())
            records = [record(0, 'USER_INPUT', '查看后台任务'), wrong_log,
                       {**quoted, 'type': 'PLANNER_RESPONSE'}, task(sid, index, log.as_uri())]
            if sid == parent:
                records.append(task(parent, 5, log.with_name('task-5.log').as_uri(), parent_tasks[5] + '\necho done'))
            write_transcript(native, sid, records)
        with sqlite3.connect(native / 'conversations' / f'{parent}.db') as db:
            db.execute('UPDATE fixture_steps SET next_index=10')
        port = free_port()
        fixture.extra.update(node_env(fixture.root, port, '127.0.0.0/8'))
        browser = pw.chromium.launch(headless=True)
        try:
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
                            menu = open_list(page, chat, parent_tasks, args.screenshots, label + '-chat')
                            page.keyboard.press('Escape')
                            expect(menu).not_to_be_visible()
                            page.locator('#a-sdk').click()
                            start_sdk(page)
                            menu = open_list(page, sdk, parent_tasks, args.screenshots, label + '-sdk')
                            check_view(page, menu, logs[parent], 4, args.screenshots, label + '-sdk')
                            page.locator('#a-view-switch').click()
                            page.locator(f'#session-view-menu [data-agent="{CHILD}"]').click()
                            page.locator('#a-sdk').click()
                            menu = open_list(page, sdk, child_tasks, args.screenshots, label + '-child-sdk')
                            page.keyboard.press('Escape')
                            page.locator('#a-chat').click()
                            menu = open_list(page, chat, child_tasks, args.screenshots, label + '-child-chat')
                            check_view(page, menu, logs[CHILD], 7, args.screenshots, label + '-child-chat')
                            if target == 'hub' and width == 390:
                                check_completion(page, chat, native, logs[CHILD], args.screenshots, label)
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
