#!/usr/bin/env python3
"""Continue a native session through the new-session dialog and real fake-CLI SEND.

Private runtime and synthetic history; desktop/phone, light/dark, cancellation,
create failure/retry, retained draft after refused SEND, and ordinary-create reset.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import sys
import tempfile
import uuid

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY, REPO, Corpus, claude_row, isolated_server
from private_hosts import private_hosts
from send_browser import SETTINGS, initialize, wait_history
from session_global_actions_browser import click_action
from hub_fixtures import FakeNode, Hub

SID = '019a29ee-d9b0-741c-a645-776a523e6b6d'
TITLE = '接续长标题会话 / Continue the existing investigation without losing its reference'


def open_continue(page):
    click_action(page, '[data-continue-session]')
    expect(page.locator('#new-session-dialog')).to_be_visible()
    expect(page.locator('#new-session-title')).to_have_text('接续会话')
    expect(page.locator('#continue-session-reference')).to_contain_text(SID)
    expect(page.locator('#continue-session-description')).to_be_focused()
    expect(page.locator('#new-session-form label:has(input[value="shell"])')).to_be_hidden()


def check_hub(browser, root):
    print('continue hub: preserve origin on another machine/source', flush=True)
    nodes = [FakeNode('a' * 32, 'NodeA'), FakeNode('b' * 32, 'NodeB')]
    hub = Hub(BINARY.parent / 'sessiondock-hub', root, nodes)
    try:
        nodes[1].set(create_info={'record_id': str(uuid.uuid4()), 'launch_id': str(uuid.uuid4()),
                                 'instance_id': str(uuid.uuid4()), 'running': True,
                                 'state': 'running', 'launch_kind': 'new_pending'})
        hub.start()
        context = browser.new_context(viewport={'width': 390, 'height': 844}, service_workers='block')
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.goto(f'http://127.0.0.1:{hub.port}/', wait_until='networkidle')
        page.wait_for_function('S.sessions.length === 2 && T.listLoaded')
        page.locator('#side .item[data-uid^="claude:' + 'a' * 32 + '~"]').click()
        expect(page.locator('#msgs')).to_be_visible()
        origin = page.evaluate('S.sel')
        click_action(page, '[data-continue-session]')
        expect(page.locator('#new-node')).to_have_value('a' * 32)
        page.select_option('#new-node', 'b' * 32)
        page.locator('#new-session-form label:has(input[value="codex"])').click()
        page.locator('#new-model').click()
        page.locator('#new-model-options [role=option]').filter(has_text='NodeB-codex').click()
        expect(page.locator('#new-model-label')).to_have_text('NodeB-codex')
        page.fill('#new-cwd', '/home/NodeB/work')
        page.fill('#continue-session-description', '换到另一台机器继续分析')
        page.locator('#continue-session-go').click()
        expect(page.locator('#new-session-dialog')).to_be_hidden()
        expect(page.locator('#cinput')).to_have_value('')
        page.wait_for_function('!composerSending && !newSessionCreating')
        writes = nodes[1].state()['writes']
        created = [body for path, body in writes if path == '/api/term/create']
        sent = [body for path, body in writes if path == '/api/session/conversation/send']
        assert len(created) == len(sent) == 1, writes
        assert created[0]['source'] == 'codex' and created[0]['model'] == 'NodeB-codex', created
        assert origin in sent[0]['text'] and '机器：NodeA' in sent[0]['text'], sent
        assert sent[0]['uid'] == 'tmux:same-terminal', sent
        assert not any(path == '/api/term/create' for path, _ in nodes[0].state()['writes'])
        assert not errors, errors
        context.close()
    finally:
        hub.stop()
        for node in nodes:
            node.stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-continue-') as directory, private_hosts(Path(directory)):
        root = Path(directory)
        for name in ('host', 'work', 'ledger', 'state', 'bin', 'home', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        corpus.put(SID, 'claude', [
            claude_row(SID, 'user', 'u', None, TITLE, cwd=str(root / 'work')),
            claude_row(SID, 'assistant', 'a', 'u', '已有调查结果', cwd=str(root / 'work'))],
            [TITLE, '已有调查结果'])
        wrapper = root / 'bin/fake-claude'
        wrapper.write_text('#!/bin/sh\nexec ' + shlex.quote(sys.executable) + ' '
                           + shlex.quote(str(REPO / 'tests/fake_claude_cli.py')) + ' "$@"\n')
        wrapper.chmod(0o700)
        configuration = root / 'launcher.json'
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'claude-cli-v1', 'source': 'claude',
                'executable': str(wrapper), 'args': ['--settings', SETTINGS, '--reply'],
                'new_args': ['--session-id', '{session_id}'], 'resume_args': ['--resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TERM': 'xterm-256color',
                        'LANG': 'C.UTF-8', 'SESSIONDOCK_TEST_CLAUDE_ROOT': str(root / 'claude')}}]}))
        initialize('--initialize-lifecycle', root / 'ledger')
        with isolated_server(corpus, BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                             launcher_config=configuration, state_dir=root / 'state',
                             file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), \
                sync_playwright() as playwright:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = playwright.chromium.launch(**launch)
            for width, height, theme in ((1280, 844, 'light'), (390, 844, 'dark'),
                                         (1280, 844, 'dark'), (320, 844, 'light'),
                                         (472, 287, 'light'), (472, 287, 'dark')):
                print(f'continue {width}x{height} {theme}', flush=True)
                context = browser.new_context(viewport={'width': width, 'height': height},
                                              color_scheme=theme, service_workers='block')
                context.add_init_script("Object.defineProperty(navigator, 'standalone', {value: true})")
                context.route('**/api/term/models?*', lambda route: route.fulfill(json={
                    'models': [{'id': 'synthetic-model', 'name': 'Synthetic model',
                                'efforts': ['low', 'high']}], 'default_model': 'synthetic-model'}))
                page = context.new_page()
                errors, sends, creates = [], [], []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.on('request', lambda request: sends.append(request.post_data_json)
                        if '/api/session/conversation/send' in request.url else None)
                page.on('request', lambda request: creates.append(request.post_data_json)
                        if request.url.endswith('/api/term/create') else None)
                page.goto(base, wait_until='networkidle')
                page.wait_for_function('T.listLoaded')
                page.locator(f'#side .item[data-uid="{corpus.uid(SID)}"]').click()
                expect(page.locator('#msgs')).to_contain_text('已有调查结果')
                open_continue(page)
                page.fill('#continue-session-description', '暂存后取消')
                page.locator('#new-session-dialog .modal-close').click()
                assert page.evaluate('S.sel') == corpus.uid(SID) and not creates and not sends
                open_continue(page)
                text = f'继续调查第 {width} 项，保留原会话的上下文引用。'
                page.fill('#continue-session-description', text)
                page.locator('#continue-session-description').press('Enter' if width < 720 else 'Shift+Enter')
                expect(page.locator('#continue-session-description')).to_have_value(text + '\n')
                assert not creates
                page.fill('#continue-session-description', text)
                page.select_option('#new-effort', 'high')
                page.locator('#continue-session-go').scroll_into_view_if_needed()
                expect(page.locator('#continue-session-go')).to_be_in_viewport()
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f'{width}-{theme}-dialog.png'))
                assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                # A creation error keeps the original session and editable prompt.
                page.route('**/api/term/create', lambda route: route.fulfill(status=503,
                           json={'error': '合成的启动失败'}), times=1)
                page.locator('#continue-session-go').click()
                expect(page.locator('#new-session-error')).to_have_text('合成的启动失败')
                expect(page.locator('#continue-session-description')).to_have_value(text)
                assert not sends and page.evaluate('S.sel') == corpus.uid(SID)
                if width == 1280 and theme == 'light':
                    page.route('**/api/session/conversation?uid=tmux*', lambda route: route.fulfill(
                        status=503, json={'error': '合成的草稿读取失败'}), times=1)
                    page.locator('#continue-session-go').click()
                    expect(page.locator('#new-session-error')).to_contain_text('合成的草稿读取失败')
                    assert not sends
                # The initial SEND still respects the backend refusal. The new
                # session retains its prefixed draft and ordinary Send can retry.
                refused = width == 320
                if refused:
                    page.route('**/api/session/conversation/send', lambda route: route.fulfill(status=409,
                               json={'error': '合成的输入未就绪', 'code': 'cli_not_ready'}), times=1)
                if width >= 720:
                    page.locator('#continue-session-description').press('Enter')
                else:
                    page.locator('#continue-session-go').click()
                expect(page.locator('#new-session-dialog')).to_be_hidden()
                if refused:
                    expect(page.locator('dialog.app-popup[open]')).to_contain_text('合成的输入未就绪')
                    page.locator('dialog.app-popup[open] button[data-popup-action="ok"]').click()
                    assert corpus.uid(SID) in page.locator('#cinput').input_value()
                    page.reload(wait_until='networkidle')
                    expect(page.locator('#cinput')).to_have_value(sends[0]['text'])
                    expect(page.locator('#csend')).to_be_enabled(timeout=15000)
                    page.locator('#csend').click()
                wait_history(page, 'OK: 接续以下会话：')
                expect(page.locator('#cinput')).to_have_value('')
                assert len(sends) == (2 if refused else 1), sends
                prompt = sends[-1]['text']
                assert prompt.startswith('接续以下会话：\n会话标识：' + corpus.uid(SID)), prompt
                assert f'会话 ID：{SID}' in prompt and prompt.endswith(text), prompt
                assert sends[-1]['uid'] != corpus.uid(SID), sends
                assert creates[-1]['source'] == 'claude' and creates[-1]['effort'] == 'high', creates
                assert creates[-1]['request_id'] == creates[-2]['request_id'], creates
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f'{width}-{theme}-sent.png'))
                # Ordinary new-session creation has no stale continuation UI.
                if width < 720:
                    page.locator('.mobile-back').click()
                if not page.locator('#new-session').is_visible():
                    page.locator('#header-more-btn').click()
                page.locator('#new-session').click()
                expect(page.locator('#new-session-title')).to_have_text('新建会话')
                expect(page.locator('#continue-session-composer')).to_be_hidden()
                expect(page.locator('#new-session-form label:has(input[value="shell"])')).to_be_visible()
                page.locator('#new-session-dialog .modal-cancel').click()
                assert not errors, errors
                context.close()
            (root / 'hub').mkdir(mode=0o700)
            check_hub(browser, root / 'hub')
            browser.close()
    print('PASS session_continue_browser: prefixed first SEND, native reply, create retry, '
          'refused SEND retains draft across reload, cancel/reset; desktop/phone light/dark')


if __name__ == '__main__':
    main()
