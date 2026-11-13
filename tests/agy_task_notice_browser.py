#!/usr/bin/env python3
"""Agy background command notices in node/Hub history and live SDK views."""
import argparse
import sqlite3
import tempfile
from pathlib import Path
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from agy_history_browser import record, write_transcript
from history_fixtures import BINARY, REPO
from hub_fixtures import Hub, free_port
from native_sdk_queue_browser import send, settled
from native_sdk_queue_fixtures import NativeQueueFixture
from node_auth_fixtures import TOKEN, node_env
from sdk_ui_fixtures import start_sdk


def notice(sender, body):
    return ('<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-13T10:08:47Z '
            f'sender={sender} priority=MESSAGE_PRIORITY_HIGH content={body}\n</SYSTEM_MESSAGE>')


def command(sender, code):
    return (f'Task id "{sender}" finished with result:\nCommand exited with code {code}.\n\n'
            'Command output:\n' + '\n'.join(f'synthetic task output {i}' for i in range(40)))


def expand_processes(page, surface):
    buttons = page.locator(surface + ' .turn-process.folded > .turn-toolbar .fold-toggle')
    while buttons.count():
        buttons.first.click()


def check_notice(page, surface, task, code, screenshots, label):
    item = page.locator(surface + ' .native-task-notice').filter(has_text=task).last
    expect(item).to_be_visible()
    expect(item).to_contain_text('后台任务已完成' if code == 0 else '后台任务失败')
    expect(item).to_contain_text(f'退出码 {code}')
    expect(item).to_have_attribute('data-role', 'system')
    expect(item).to_have_attribute('data-counted', 'false')
    assert item.evaluate('(e) => e.classList.contains("completed")') == (code == 0)
    expect(item.locator('details')).not_to_have_attribute('open', '')
    expect(item.locator('.event-detail-body')).to_have_count(0)
    assert item.evaluate('(e) => e.clientHeight') < 100
    item.scroll_into_view_if_needed()
    page.screenshot(path=str(screenshots / f'{label}-{task}-collapsed.png'), full_page=True)
    item.locator('summary').click()
    expect(item.locator('.event-detail-body')).to_contain_text('synthetic task output 39')
    expect(item.locator('.event-detail-body')).to_contain_text(f'Command exited with code {code}.')
    assert 'synthetic task output 0\nsynthetic task output 1' in item.locator('.event-detail-body').text_content()
    assert item.locator('.event-detail-body').evaluate('(e) => getComputedStyle(e).whiteSpace') == 'pre-wrap'
    item.locator('.event-detail-body').scroll_into_view_if_needed()
    page.screenshot(path=str(screenshots / f'{label}-{task}-expanded.png'), full_page=True)
    item.locator('summary').click()
    expect(item.locator('.event-detail-body')).to_be_hidden()
    assert page.evaluate('document.documentElement.scrollWidth <= innerWidth + 1')


def run(args):
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-agy-task-notice-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        fixture = NativeQueueFixture(root / 'agy', 'agy', args.binary.resolve(), REPO / 'target/debug/ptyhost')
        sid = fixture.sid
        success, failure = command(sid + '/task-40', 0), command(sid + '/task-41', 2)
        write_transcript(fixture.peer.native, sid, [
            record(0, 'USER_INPUT', '检查后台任务通知'),
            record(1, 'SYSTEM_MESSAGE', notice(sid + '/task-40', success)),
            record(2, 'SYSTEM_MESSAGE', notice(sid + '/task-41', failure)),
            record(3, 'SYSTEM_MESSAGE', notice(sid + '/task-42', 'Task id "'+sid+'/task-42" finished with result:\n未知结果，原文保留')),
            record(4, 'SYSTEM_MESSAGE', notice('another-sender', command(sid + '/task-44', 0))),
            record(5, 'USER_INPUT', command(sid + '/task-43', 0)),
            record(6, 'PLANNER_RESPONSE', '后台任务结束后仍可继续对话。'),
        ])
        with sqlite3.connect(fixture.peer.native / 'conversations' / (sid + '.db')) as db:
            db.execute('UPDATE fixture_steps SET next_index=7')
        port = free_port()
        fixture.extra.update(node_env(fixture.root, port, '127.0.0.0/8'))
        browser = pw.chromium.launch(headless=True)
        try:
            with fixture.server() as base:
                hub_root = root / 'hub'; hub_root.mkdir()
                hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                          [SimpleNamespace(name='AgyTaskNode', token=TOKEN, port=port)])
                hub.start()
                try:
                    for target, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                        for width, theme in [(1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark')]:
                            label = f'{target}-{width}-{theme}'
                            context = browser.new_context(viewport={'width': width, 'height': 900},
                                                          color_scheme=theme, service_workers='block')
                            context.route('**/*', lambda route: route.continue_()
                                          if route.request.url.startswith(url + '/') else route.abort())
                            page = context.new_page()
                            page.set_default_timeout(15000)
                            errors = []
                            page.on('pageerror', lambda error: errors.append(str(error)))
                            page.goto(url, wait_until='domcontentloaded')
                            page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=sid)
                            uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', sid)
                            page.locator(f'#side .item[data-uid="{uid}"]').click()
                            page.locator('#a-chat').click()
                            expect(page.locator('#msgs')).to_contain_text('后台任务结束后仍可继续对话。')
                            expand_processes(page, '#msgs')
                            page.locator('#msgs [data-role=system]').filter(has_text='task-40').first.scroll_into_view_if_needed()
                            page.screenshot(path=str(args.screenshots / f'{label}-history-before-check.png'), full_page=True)
                            for mode in ('chat', 'sdk'):
                                page.locator('#a-' + mode).click()
                                if mode == 'sdk': start_sdk(page)
                                surface = '#msgs' if mode == 'chat' else '.native-sdk-pane .sdk-msgs'
                                expand_processes(page, surface)
                                for task, code in [('task-40', 0), ('task-41', 2)]:
                                    check_notice(page, surface, task, code, args.screenshots, label + '-' + mode)
                                for task in ('task-42', 'task-44'):
                                    expect(page.locator(surface + ' .msg[data-role=system]').filter(has_text=task)).to_have_count(1)
                                expect(page.locator(surface + ' .msg[data-role=user]').filter(has_text='task-43')).to_have_count(1)
                            receipt = send(page, '__TASK_NOTICES__')
                            settled(page, fixture, receipt['request_id'])
                            page.wait_for_function('id => !NativeSdkView.running && NativeSdkView.raw.last_turn?.id === id',
                                                   arg=receipt['request_id'])
                            expand_processes(page, '.sdk-live')
                            for task, code in [('task-196', 0), ('task-197', 2)]:
                                check_notice(page, '.sdk-live', task, code, args.screenshots, label + '-live')
                            expect(page.locator('.native-sdk-pane .sdk-status')).to_have_text('就绪')
                            assert page.evaluate('NativeSdkView.connected')
                            # New-turn snapshots preserve the expanded result and native runtime.
                            item = page.locator('.sdk-live .native-task-notice').filter(has_text='task-196').last
                            item.locator('summary').click()
                            pid = fixture.native_pid()
                            following = send(page, 'continue after task notice')
                            settled(page, fixture, following['request_id'])
                            page.wait_for_function('id => !NativeSdkView.running && NativeSdkView.raw.last_turn?.id === id',
                                                   arg=following['request_id'])
                            assert fixture.native_pid() == pid
                            expand_processes(page, '.sdk-live')
                            expect(item.locator('.event-detail-body')).to_be_visible()
                            assert not errors, errors
                            context.close()
                            print('PASS', label, 'history/SDK/live success/failure disclosures and literal fallbacks', flush=True)
                finally:
                    hub.stop()
        finally:
            browser.close()
            fixture.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-agy-task-notice-review'))
    run(parser.parse_args())
