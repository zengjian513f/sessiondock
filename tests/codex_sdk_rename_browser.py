#!/usr/bin/env python3
"""SDK /rename through Chromium, native name RPC/index, and the real Hub.

Private fake daemon and synthetic sessions only; no real CLI or model.
"""
from __future__ import annotations

import argparse
import asyncio
import json
from pathlib import Path
import tempfile
import threading
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from new_session_model_browser import open_dialog
from sdk_ui_fixtures import start_sdk
from node_auth_fixtures import TOKEN, node_env


class RenameServer(FakeAppServer):
    def __init__(self, socket_path, rollout, index):
        super().__init__(socket_path, rollout, THREAD)
        self.index = index
        self.reject = False
        self.hold = False
        self.release = threading.Event()

    async def _handle(self, writer, message):
        method, mid, params = message.get('method'), message.get('id'), message.get('params') or {}
        if method != 'thread/name/set':
            return await super()._handle(writer, message)
        assert params['threadId'] == self.thread_id, params
        if self.hold:
            assert await asyncio.to_thread(self.release.wait, 10), 'rename was never released'
        if self.reject:
            await self.send(writer, {'id': mid, 'error': {'code': -32602, 'message': 'Synthetic rename rejected'}})
            return
        name = params['name'].strip()
        assert name
        with self.index.open('a') as file:
            file.write(json.dumps({'id': self.thread_id, 'thread_name': name,
                                   'updated_at': '2026-10-19T10:52:00Z'}, ensure_ascii=False) + '\n')
        await self.send(writer, {'id': mid, 'result': {}})
        # Codex emits threadName, not name; this must survive a bridge snapshot.
        await self.notify('thread/name/updated', {'threadName': name})


def exercise(browser, base, corpus, fake, width, theme, screenshots):
    label = f'{width}-{theme}'
    print(label + ': inline/dialog rename, busy turn, failures, draft race, reload, pending creation', flush=True)
    context = browser.new_context(viewport={'width': width, 'height': 844 if width < 500 else 900},
                                  color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(10000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='domcontentloaded')
    page.locator('#side .item').filter(has_text='earlier synthetic question').click()
    pane = page.locator('#session-content-view')
    box, send = pane.locator('#sdk-input'), pane.locator('#sdk-send')
    expect(box).to_be_visible()
    start_sdk(page)
    uid = page.evaluate('S.sel')
    row = page.locator(f'#side .item[data-uid="{uid}"]')
    starts = len(fake.methods('turn/start'))

    def command(text):
        box.fill(text)
        send.click()

    # The reported session was busy: rename must neither steer nor interrupt it.
    asyncio.run_coroutine_threadsafe(fake.notify('turn/started', {
        'turn': {'id': 'synthetic-active-turn', 'status': 'inProgress', 'items': []}}), fake.loop).result(5)
    expect(pane.locator('.sdk-status')).to_have_attribute('data-state', 'working')
    name = '判据核查 criteria inventory · 会话 01a153c5-107f-72f0-b336-deb05eef0b5f'
    command('/rename ' + name)
    expect(box).to_have_value('')
    expect(row).to_contain_text(name)
    expect(page.locator('.dhead h2')).to_have_text(name)
    assert fake.methods('thread/name/set')[-1]['params'] == {'threadId': THREAD, 'name': name}
    page.wait_for_function('name => CodexSdkView.summary?.resume?.thread?.name === name', arg=name)
    if screenshots:
        page.screenshot(path=str(screenshots / (label + '-renamed.png')))
    assert not fake.methods('turn/steer') and not fake.methods('turn/interrupt')

    # A bare command uses the shared form, cancellation/empty input retain it.
    command('/rename')
    dialog = page.get_by_role('dialog', name='重命名会话', exact=True)
    field = dialog.get_by_role('textbox', name='会话名称')
    expect(field).to_have_value(name)
    if screenshots:
        page.screenshot(path=str(screenshots / (label + '-dialog.png')))
    field.fill('')
    count = len(fake.methods('thread/name/set'))
    dialog.get_by_role('button', name='保存', exact=True).click()
    expect(dialog).to_be_visible()
    assert len(fake.methods('thread/name/set')) == count
    dialog.get_by_role('button', name='取消', exact=True).click()
    expect(box).to_have_value('/rename')
    command('/rename')
    dialog.get_by_role('textbox', name='会话名称').fill('  重命名后的会话 renamed conversation  ')
    dialog.get_by_role('button', name='保存', exact=True).click()
    expect(box).to_have_value('')
    expect(row).to_contain_text('重命名后的会话 renamed conversation')

    fake.reject = True
    command('/rename rejected name')
    expect(pane.locator('#sdk-status-menu')).to_contain_text('Synthetic rename rejected')
    expect(box).to_have_value('/rename rejected name')
    expect(row).to_contain_text('重命名后的会话 renamed conversation')
    if screenshots:
        pane.locator('.sdk-status').click()
        page.screenshot(path=str(screenshots / (label + '-rejected.png')))
        page.keyboard.press('Escape')
    fake.reject = False

    fake.hold = True
    count = len(fake.methods('thread/name/set'))
    with page.expect_request(lambda request: request.url.endswith('/api/codex-sdk/rename')):
        command('/rename saved while editing')
    page.expose_function('renameArrived', lambda: len(fake.methods('thread/name/set')) == count + 1)
    page.wait_for_function('async () => await window.renameArrived()')
    box.fill('发送期间输入的新草稿 stays unsent')
    fake.release.set()
    expect(send).to_be_enabled()
    expect(row).to_contain_text('saved while editing')
    expect(box).to_have_value('发送期间输入的新草稿 stays unsent')
    fake.hold = False
    box.fill('')
    page.reload(wait_until='domcontentloaded')
    expect(page.locator('.dhead h2')).to_have_text('saved while editing')
    expect(pane.locator('.sdk-history')).to_contain_text('earlier synthetic answer')
    # The retained live snapshot uses the actual notification's field.
    page.wait_for_function('CodexSdkView.summary?.resume?.thread?.name === "saved while editing"')
    command('/help')
    expect(pane.locator('#sdk-status-menu')).to_contain_text('/rename [名称]')
    command('/unknown-command')
    expect(pane.locator('#sdk-status-menu')).to_contain_text('不支持的命令')
    expect(box).to_have_value('/unknown-command')
    assert len(fake.methods('turn/start')) == starts

    # A newly created thread has no rollout yet; its receipt supplies the title.
    if width < 500:
        page.get_by_role('button', name='返回会话列表', exact=True).click()
    open_dialog(page)
    page.locator('input[name="new-source"][value="codex"]').check()
    page.locator('#new-cwd').fill(str(corpus.root))
    page.locator('#new-session-go').click()
    expect(page.locator('#new-session-dialog')).to_be_hidden()
    expect(box).to_be_visible()
    assert not fake.rollout.exists()
    command('/rename 首轮之前 pending thread')
    expect(box).to_have_value('')
    expect(page.locator('.dhead h2')).to_have_text('首轮之前 pending thread')
    page.reload(wait_until='domcontentloaded')
    expect(page.locator('.dhead h2')).to_have_text('首轮之前 pending thread')
    expect(box).to_be_visible()
    assert not fake.rollout.exists() and len(fake.methods('turn/start')) == starts
    assert not errors, errors
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-rename-') as temporary, sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        try:
            for mode, width, theme in [('node', 1280, 'light'), ('node', 390, 'dark'),
                                       ('node', 1280, 'dark'), ('node', 390, 'light'), ('hub', 1280, 'light')]:
                root = Path(temporary) / f'{mode}-{width}-{theme}'
                root.mkdir()
                corpus = build(root)
                port = free_port()
                env = environment(root, args.binary)
                if mode == 'hub':
                    env.update(node_env(root, port, '127.0.0.0/8'))
                with RenameServer(root / 'app.sock', corpus.paths['sdk'], root / 'session_index.jsonl') as fake:
                    with server(corpus, args.binary, root / 'app.sock', env) as base:
                        if mode == 'node':
                            exercise(browser, base, corpus, fake, width, theme, args.screenshots)
                        else:
                            hub_root = root / 'hub'
                            hub_root.mkdir()
                            hub = Hub((args.hub_binary or args.binary.with_name('sessiondock-hub')).resolve(),
                                      hub_root, [SimpleNamespace(name='SdkNode', token=TOKEN, port=port)])
                            hub.start()
                            try:
                                exercise(browser, f'http://127.0.0.1:{hub.port}', corpus, fake, width, theme, args.screenshots)
                            finally:
                                hub.stop()
        finally:
            browser.close()
    print('PASS SDK rename: inline/dialog, busy turn, errors/cancellation, draft edits, native titles/snapshots, reload, pending threads; node/Hub and desktop/mobile light/dark')


if __name__ == '__main__':
    main()
