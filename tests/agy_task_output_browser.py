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


def task(sid, index, uri):
    body = (f'Created At: 2026-11-25T12:04:56Z\n'
            f'Tool is running as a background task with task id: {sid}/task-{index}\n'
            f'Task Description: python report.py\nTask logs are available at: {uri}\n'
            'Log output:\n旧输出快照')
    return {**record(index, 'GENERIC', body), 'status': 'RUNNING'}


def check_view(page, log, index, screenshots, label):
    log.unlink(missing_ok=True)
    page.get_by_role('button', name='更多会话操作').click()
    menu = page.locator('#session-actions-menu')
    expect(menu.locator('[id^="a-output-"]')).to_have_count(1)
    button = menu.get_by_role('menuitem', name=f'查看后台输出 · task-{index}')
    expect(button).to_be_visible()
    page.screenshot(path=str(screenshots / f'{label}-menu.png'))
    with page.expect_popup() as opened:
        button.click()
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


def run(args):
    args.screenshots.mkdir(exist_ok=True, parents=True)
    with tempfile.TemporaryDirectory(prefix='sd-task-output-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        fixture = NativeQueueFixture(root / 'node', 'agy', args.binary.resolve(), REPO / 'target/debug/ptyhost')
        parent, native = fixture.sid, fixture.peer.native
        fixture.peer.seed(CHILD)
        with sqlite3.connect(native / 'conversation_summaries.db') as db:
            db.execute('UPDATE conversation_summaries SET parent_conversation_id=? WHERE conversation_id=?', (parent, CHILD))
        logs = {}
        for sid, index in [(parent, 4), (CHILD, 7)]:
            log = logs[sid] = native / 'brain' / sid / '.system_generated/tasks' / f'task-{index}.log'
            log.parent.mkdir()
            wrong_log = task(sid, 2, log.as_uri())
            quoted = task(sid, 3, log.with_name('task-3.log').as_uri())
            write_transcript(native, sid, [record(0, 'USER_INPUT', '查看后台任务'),
                wrong_log, {**quoted, 'type': 'PLANNER_RESPONSE'}, task(sid, index, log.as_uri())])
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
                            page.locator('#a-sdk').click()
                            start_sdk(page)
                            check_view(page, logs[parent], 4, args.screenshots, label + '-sdk')
                            page.locator('#a-view-switch').click()
                            page.locator(f'#session-view-menu [data-agent="{CHILD}"]').click()
                            page.locator('#a-chat').click()
                            check_view(page, logs[CHILD], 7, args.screenshots, label + '-child-chat')
                            if target == 'hub' and width == 390:
                                sender = f'{CHILD}/task-7'
                                with transcript(native, CHILD).open('ab') as stream:
                                    stream.write(encoded(record(8, 'SYSTEM_MESSAGE', notice(sender,
                                        f'Task id "{sender}" finished with result:\nCommand exited with code 0.'))))
                                expect(page.locator('#a-output-task-7')).to_have_count(0, timeout=20000)
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
                                next_log = logs[CHILD].with_name('task-9.log')
                                with transcript(native, CHILD).open('ab') as stream:
                                    stream.write(encoded(task(CHILD, 9, next_log.as_uri())))
                                page.locator('#a-more').click()
                                expect(page.get_by_role('menuitem', name='查看后台输出 · task-9')).to_be_visible(timeout=20000)
                            context.close()
                            print('PASS', label, 'SDK/child chat, scoped log, missing/empty/append/tail/retry', flush=True)
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
