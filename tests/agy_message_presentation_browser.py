#!/usr/bin/env python3
"""Agy message presentation: synthetic native records, private server and Chromium.

BUG-20261016-084905-94b5af shapes only; no report text, native sessions or CLIs.
"""
from __future__ import annotations

import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile

sys.dont_write_bytecode = True
from playwright.sync_api import expect, sync_playwright
from agy_history_browser import SCHEMA, SEEDED, private_environment, record, write_transcript
from history_fixtures import BINARY, Corpus, encoded, isolated_server

TITLE = 'Agy 消息呈现 · 状态栏检查'
TARGET = '/synthetic/中文 工作目录/statusline.py'
COMMAND = 'python3 statusline.py --refresh-cache'
BEFORE = 'quota = 0\nprint(quota)\n'
AFTER = 'quota = current_usage\nprint(quota)\n'
FINAL = ('## 原因分析\n\n状态栏使用了旧的配额快照。\n\n'
         '| 项目 | 修复前 | 修复后 |\n| --- | --- | --- |\n'
         '| 周用量 | 0% | 当前真实用量 |\n\n已修复并完成验证。')


def planner(index, *calls):
    value = record(index, 'PLANNER_RESPONSE', '')
    value['tool_calls'] = list(calls)
    return value


def fixtures():
    edit = {'name': 'replace_file_content', 'args': {
        'TargetFile': TARGET, 'TargetContent': BEFORE, 'ReplacementContent': AFTER,
        'StartLine': 1, 'EndLine': 2, 'AllowMultiple': False,
        'toolSummary': 'Update statusline quota', 'Instruction': '保留完整原生参数'}}
    incomplete = {'name': 'replace_file_content', 'args': {
        'TargetFile': TARGET, 'ReplacementContent': '不可推断原文',
        'toolSummary': 'Incomplete native edit'}}
    calls = [
        {'name': 'run_command', 'args': {'CommandLine': COMMAND,
            'Cwd': '/synthetic/中文 工作目录', 'toolSummary': 'Refresh quota cache'}},
        {'name': 'view_file', 'args': {'AbsolutePath': TARGET, 'toolSummary': 'Read statusline.py'}},
        {'name': 'search_web', 'args': {'query': 'statusline quota 中文'}},
        {'name': 'read_url_content', 'args': {'Url': 'https://example.test/quota'}},
        {'name': 'manage_task', 'args': {'Action': 'kill', 'TaskId': 'fixture/task-62',
                                      'toolSummary': 'Cancel quota background task'}},
        {'name': 'unknown_native_tool', 'args': {'nested': {'preserve': '未知结构保留'}}},
        incomplete,
    ]
    error = record(6, 'GENERIC', '')
    error['status'], error['error'] = 'ERROR', '读取文档失败 agyerroronly'
    planner_error = record(7, 'ERROR_MESSAGE', '')
    planner_error['error'] = '模型输出失败 agyplannererroronly'
    values = [record(0, 'USER_INPUT', '检查状态栏用量，并解释修复内容。'),
              planner(1, *calls),
              record(2, 'GENERIC', 'Created At: 2026-10-16T16:48:00+08:00\n'
                     'Completed At: 2026-10-16T16:48:01+08:00\n\n'
                     'The command exited with code 0.\nOutput:\n当前配额 12.7%\n'
                     + '\n'.join(f'日志 {i}: 完整保留输出' for i in range(16))),
              planner(3, edit), record(4, 'GENERIC', 'Updated statusline.py'),
              error, planner_error, record(8, 'PLANNER_RESPONSE', FINAL)]
    return values, calls, edit


def run(binary, screenshots):
    screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-presentation-') as temporary:
        root = Path(temporary)
        for name in ('home', 'work', 'native', 'state', 'host', 'claude', 'codex', 'grok', 'proc'):
            (root / name).mkdir(mode=0o700)
        native = root / 'native'
        with closing(sqlite3.connect(native / 'conversation_summaries.db')) as db, db:
            db.execute(SCHEMA)
            db.execute('INSERT INTO conversation_summaries VALUES (?,?,?,?,?,?,?,?)',
                       (SEEDED, TITLE, '2026-10-16T08:49:00Z',
                        json.dumps([(root / 'work').as_uri()]), '', 'DONE', 9, 'fixture'))
        values, calls, edit = fixtures()
        path = write_transcript(native, SEEDED, values)
        with sync_playwright() as playwright:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = playwright.chromium.launch(**options)
            try:
                with private_environment(root), isolated_server(
                        Corpus(root), binary, host_dir=root / 'host', state_dir=root / 'state',
                        extra_env={'SESSIONDOCK_AGY_HOME': str(native),
                                   'SESSIONDOCK_AGY_ROOT': str(root / 'mirror'),
                                   'SESSIONDOCK_PROC_ROOT': str(root / 'proc')}) as (base, _):
                    for theme in ('light', 'dark'):
                        for width in (1280, 675, 390):
                            context = browser.new_context(viewport={'width': width, 'height': 900},
                                                          color_scheme=theme, service_workers='block')
                            context.route('**/*', lambda route: route.continue_()
                                          if route.request.url.startswith(base + '/') else route.abort())
                            page = context.new_page()
                            page.set_default_timeout(10000)
                            print(f'CHECK Agy presentation {theme} {width}', flush=True)
                            errors = []
                            page.on('pageerror', lambda error: errors.append(str(error)))
                            page.goto(base, wait_until='domcontentloaded')
                            page.locator('#side .item').filter(has_text=TITLE).click()
                            expect(page.locator('#msgs')).to_contain_text('已修复并完成验证')
                            expect(page.locator('#detail .dhead')).not_to_contain_text('跳过未知的Agy记录类型')
                            # Default process is folded; final Markdown stays visible.
                            expect(page.locator('#msgs > .turn-process.folded')).to_have_count(1)
                            expect(page.locator('#msgs .mb table')).to_have_count(1)
                            page.get_by_role('button', name='展开本轮过程', exact=True).click()
                            groups = page.get_by_role('button', name='展开工具调用组', exact=True)
                            while groups.count():
                                groups.first.click()
                            heads = page.locator('#msgs .tool-head')
                            expect(heads.filter(has_text='$ ' + COMMAND)).to_have_count(1)
                            expect(heads.filter(has_text='读 ' + TARGET)).to_have_count(1)
                            expect(heads.filter(has_text='搜索 statusline quota 中文')).to_have_count(1)
                            expect(heads.filter(has_text='抓 https://example.test/quota')).to_have_count(1)
                            expect(heads.filter(has_text='Cancel quota background task')).to_have_count(1)
                            # Known calls fold raw JSON; opening it exposes every native field.
                            command = page.locator('.tool-entry').filter(has=page.locator('.tool-head').filter(has_text='$ ' + COMMAND))
                            expect(command).to_have_count(1)
                            expect(command.locator('.tool-args')).to_be_hidden()
                            command.get_by_role('button', name='展开原始参数', exact=True).click()
                            expect(command.locator('.tool-args')).to_be_visible()
                            assert json.loads(command.locator('.tool-args').inner_text()) == calls[0]
                            command.get_by_role('button', name='收起参数', exact=True).click()
                            unknown = page.locator('.tool-entry').filter(has=page.locator('.tool-head').filter(has_text='unknown_native_tool'))
                            expect(unknown.locator('.tool-args')).to_be_visible()
                            assert json.loads(unknown.locator('.tool-args').inner_text()) == calls[5]
                            # Exact replacement uses the existing fragment diff and retains native params.
                            change = page.locator('#msgs .file-change-msg')
                            expect(change).to_have_count(1)
                            expect(change).to_contain_text('片段')
                            expect(change.locator('.diff-line.del')).to_contain_text('quota = 0')
                            expect(change.locator('.diff-line.add')).to_contain_text('quota = current_usage')
                            change.get_by_role('button', name='并排', exact=True).click()
                            expect(change.locator('.diff-split')).to_be_visible()
                            change.get_by_role('button', name='换行', exact=True).click()
                            change.get_by_role('button', name='展开原始参数', exact=True).click()
                            assert json.loads(change.locator('.tool-args').inner_text()) == edit
                            change.get_by_role('button', name='收起参数', exact=True).click()
                            change.get_by_role('button', name='统一', exact=True).click()
                            # Independent results and error-only records stay visible, without invented pairing.
                            expect(page.locator('#msgs .tool-out.err')).to_contain_text('agyerroronly')
                            expect(page.locator('#msgs')).to_contain_text('agyplannererroronly')
                            result = page.locator('#msgs .tool-entry[data-role=tool_result]').filter(has_text='当前配额')
                            expect(result).to_contain_text('工具结果')
                            result.get_by_role('button', name='展开全文', exact=False).click()
                            expect(result).to_contain_text('日志 15: 完整保留输出')
                            result.get_by_role('button', name='收起', exact=True).click()
                            command.scroll_into_view_if_needed()
                            page.screenshot(path=str(screenshots / f'{theme}-{width}-tools.png'))
                            change.scroll_into_view_if_needed()
                            page.screenshot(path=str(screenshots / f'{theme}-{width}-diff.png'))
                            assert page.evaluate('document.documentElement.scrollWidth <= innerWidth + 1')
                            assert not errors, errors
                            if theme == 'dark' and width == 390:
                                # Append with the page open: same summary/diff projection via live history.
                                more = planner(9, {'name': 'run_command', 'args': {
                                    'CommandLine': 'printf incremental-presentation'}})
                                with path.open('ab') as output:
                                    output.write(encoded(more))
                                page.wait_for_function('() => cache.get(viewKey(S.sel, S.agent))?.msgs.some(m => m.summary === "$ printf incremental-presentation")')
                                expect(page.locator('#msgs')).to_contain_text('$ printf incremental-presentation')
                            context.close()
                            print(f'PASS Agy presentation {theme} {width}: summaries/raw params/diff/errors/output/Markdown', flush=True)
            finally:
                browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-agy-presentation-review'))
    args = parser.parse_args()
    run(args.binary, args.screenshots)


if __name__ == '__main__':
    main()
