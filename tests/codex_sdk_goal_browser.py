#!/usr/bin/env python3
"""BUG-20261021-050110-ff6a79: SDK goal resume through Chromium and native RPC.

Private synthetic sessions and fake app-server only; no real model or CLI.
"""
from __future__ import annotations

import argparse
import asyncio
from pathlib import Path
import tempfile
import threading
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from sdk_ui_fixtures import start_sdk
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


class GoalServer(FakeAppServer):
    def __init__(self, socket_path, rollout):
        super().__init__(socket_path, rollout, THREAD)
        self.reject = None
        self.hold = False
        self.release = threading.Event()
        self.goal = {'threadId': THREAD, 'objective': 'Finish the existing synthetic research',
                     'status': 'blocked', 'tokenBudget': 100000, 'tokensUsed': 32000,
                     'timeUsedSeconds': 120, 'createdAt': 1790000000, 'updatedAt': 1790000120}

    async def _handle(self, writer, message):
        method, mid, params = message.get('method'), message.get('id'), message.get('params') or {}
        if not method.startswith('thread/goal/'):
            return await super()._handle(writer, message)
        assert params['threadId'] == THREAD, params
        if method == 'thread/goal/get':
            assert params == {'threadId': THREAD}
            await self.send(writer, {'id': mid, 'result': {'goal': self.goal}})
            return
        assert params['origin'] == 'user', params
        if self.hold:
            assert await asyncio.to_thread(self.release.wait, 10), 'goal mutation was never released'
        if self.reject:
            await self.send(writer, {'id': mid, 'error': {'code': -32602, 'message': self.reject}})
            return
        if method == 'thread/goal/clear':
            self.goal = None
            await self.send(writer, {'id': mid, 'result': {'cleared': True}})
            await self.notify('thread/goal/cleared', {})
            return
        assert method == 'thread/goal/set', method
        if self.goal is None:
            self.goal = {'threadId': THREAD, 'status': 'active', 'tokensUsed': 0, 'timeUsedSeconds': 0}
        self.goal.update({k: v for k, v in params.items() if k not in ['threadId', 'origin']})
        await self.send(writer, {'id': mid, 'result': {'goal': self.goal.copy()}})
        await self.notify('thread/goal/updated', {'goal': self.goal.copy()})


def exercise(browser, base, corpus, fake, width, theme, screenshots, mode="node"):
    label = f'{mode}-{width}-{theme}'
    print(label + ': goal resume, busy/idle, native errors, draft race, reload', flush=True)
    context = browser.new_context(viewport={'width': width, 'height': 844 if width < 500 else 900},
                                  color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(10000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='domcontentloaded')
    page.locator('#side .item').filter(has_text='earlier synthetic question').click()
    start_sdk(page)
    pane = page.locator('#session-content-view')
    box, send = pane.locator('#sdk-input'), pane.locator('#sdk-send')
    expect(box).to_be_visible()
    expect(send).to_be_enabled()
    before = fake.goal.copy()

    def command(text):
        box.fill(text)
        send.click()

    # The reported session was busy, with a stalled goal.
    asyncio.run_coroutine_threadsafe(fake.notify('turn/started', {
        'turn': {'id': 'synthetic-active-turn', 'status': 'inProgress', 'items': []}}), fake.loop).result(5)
    expect(pane.locator('.sdk-status')).to_have_attribute('data-state', 'working')
    command('/goal resume')
    expect(box).to_have_value('')
    expect(pane.locator('#sdk-status-menu')).to_contain_text('目标已恢复')
    assert fake.goal == {**before, 'status': 'active'}
    assert len(fake.methods('thread/goal/set')) == 1
    if screenshots:
        pane.locator('.sdk-status').click()
        page.screenshot(path=str(screenshots / (label + '-resumed.png')))
        page.keyboard.press('Escape')

    for rejection in ['No goal is currently set.', 'goals feature is disabled', 'Method not found']:
        fake.reject = rejection
        command('/goal resume')
        expect(pane.locator('#sdk-status-menu')).to_contain_text(rejection)
        expect(box).to_have_value('/goal resume')
    if screenshots:
        pane.locator('.sdk-status').click()
        page.screenshot(path=str(screenshots / (label + '-rejected.png')))
        page.keyboard.press('Escape')
    fake.reject = None

    fake.hold = True
    count = len(fake.methods('thread/goal/set'))
    with page.expect_request(lambda request: request.url.endswith('/api/codex-sdk/goal/set')):
        command('/goal resume')
    page.expose_function('goalArrived', lambda: len(fake.methods('thread/goal/set')) == count + 1)
    page.wait_for_function('async () => await window.goalArrived()')
    box.fill('发送期间的新草稿 stays unsent')
    fake.release.set()
    expect(send).to_be_enabled()
    expect(box).to_have_value('发送期间的新草稿 stays unsent')
    fake.hold = False
    box.fill('')
    page.reload(wait_until='domcontentloaded')
    start_sdk(page)
    expect(box).to_be_visible()
    assert len(fake.methods('thread/goal/set')) == count + 1

    # Native continuation owns scheduling; the bridge sends no turn itself.
    asyncio.run_coroutine_threadsafe(fake.notify('turn/completed', {
        'turn': {'id': 'synthetic-active-turn', 'status': 'completed', 'items': []}}), fake.loop).result(5)
    fake.goal['status'] = 'paused'
    command('/goal resume')
    expect(box).to_have_value('')
    assert fake.goal == {**before, 'status': 'active'}
    count = len(fake.methods('thread/goal/set'))
    for unsupported in ['/goal resume extra', '/goal new objective']:
        command(unsupported)
        expect(pane.locator('#sdk-status-menu')).to_contain_text('不支持的命令')
        expect(box).to_have_value(unsupported)
    assert len(fake.methods('thread/goal/set')) == count
    command('/goal resume')
    expect(box).to_have_value('')

    # The Goal icon stays immediately to the right of compact, even on mobile.
    goal_button = pane.locator('#sdk-goal')
    expect(goal_button).to_be_visible()
    assert goal_button.evaluate('e => e.previousElementSibling.dataset.sdkAction === "compact"')
    goal_button.click()
    panel = pane.locator('#sdk-goal-menu')
    expect(panel).to_be_visible()
    expect(panel.locator('#sdk-goal-state')).to_have_text('进行中')
    objective, budget = panel.locator('#sdk-goal-objective'), panel.locator('#sdk-goal-budget')
    expect(objective).to_have_value(before['objective'])
    expect(budget).to_have_value('100000')
    expect(objective).to_be_enabled()
    assert panel.bounding_box()['y'] + panel.bounding_box()['height'] <= goal_button.bounding_box()['y']
    assert page.locator('#right').evaluate('e => e.scrollLeft') == 0
    bounds = panel.bounding_box()
    assert bounds['x'] >= 0 and bounds['x'] + bounds['width'] <= width
    if screenshots:
        page.screenshot(path=str(screenshots / (label + '-panel.png')))
    panel.get_by_role('button', name='暂停', exact=True).click()
    expect(panel.locator('#sdk-goal-state')).to_have_text('已暂停')
    assert fake.methods('thread/goal/set')[-1]['params'] == {'threadId': THREAD, 'status': 'paused', 'origin': 'user'}
    panel.get_by_role('button', name='恢复', exact=True).click()
    expect(panel.locator('#sdk-goal-state')).to_have_text('进行中')
    assert fake.methods('thread/goal/set')[-1]['params'] == {'threadId': THREAD, 'status': 'active', 'origin': 'user'}

    edited = '核对完整研究结果、保留现有会话与实验状态，并记录证据。' * 4
    objective.fill(edited)
    budget.fill('250000')
    # External updates must refresh lifecycle/usage without overwriting edits.
    fake.goal['status'] = 'blocked'
    asyncio.run_coroutine_threadsafe(fake.notify('thread/goal/updated', {'goal': fake.goal.copy()}), fake.loop).result(5)
    expect(panel.locator('#sdk-goal-state')).to_have_text('受阻')
    expect(objective).to_have_value(edited)
    fake.reject = 'Synthetic goal update rejected'
    panel.get_by_role('button', name='保存修改', exact=True).click()
    expect(panel.locator('.sdk-goal-result')).to_contain_text(fake.reject)
    expect(objective).to_have_value(edited)
    if screenshots:
        page.screenshot(path=str(screenshots / (label + '-panel-error.png')))
    fake.reject = None
    panel.get_by_role('button', name='保存修改', exact=True).click()
    expect(panel.locator('.sdk-goal-result')).to_have_text('目标已保存。')
    assert fake.goal['objective'] == edited and fake.goal['tokenBudget'] == 250000
    assert fake.goal['status'] == 'blocked' and fake.goal['tokensUsed'] == before['tokensUsed']
    panel.get_by_role('button', name='清除', exact=True).click()
    expect(panel.locator('#sdk-goal-state')).to_have_text('未设置')
    expect(objective).to_have_value('')
    expect(budget).to_have_value('')
    if screenshots:
        page.screenshot(path=str(screenshots / (label + '-empty.png')))
    objective.fill('新目标：验证浏览器完整路径')
    panel.get_by_role('button', name='设置目标', exact=True).click()
    expect(panel.locator('#sdk-goal-state')).to_have_text('进行中')
    assert fake.methods('thread/goal/set')[-1]['params'] == {
        'threadId': THREAD, 'origin': 'user', 'objective': '新目标：验证浏览器完整路径',
        'status': 'active', 'tokenBudget': None}
    page.keyboard.press('Escape')
    expect(panel).to_be_hidden()
    expect(goal_button).to_have_attribute('aria-expanded', 'false')
    command('/goal pause')
    expect(box).to_have_value('')
    assert fake.goal['status'] == 'paused'
    command('/goal')
    expect(panel).to_be_visible()
    expect(panel.locator('#sdk-goal-state')).to_have_text('已暂停')
    page.keyboard.press('Escape')
    command('/goal clear')
    expect(box).to_have_value('')
    assert fake.goal is None
    page.reload(wait_until='domcontentloaded')
    start_sdk(page)
    expect(goal_button).to_be_visible()
    goal_button.click()
    expect(panel.locator('#sdk-goal-state')).to_have_text('未设置')
    page.keyboard.press('Escape')
    command('/help')
    expect(pane.locator('#sdk-status-menu')).to_contain_text('/goal resume')
    assert not fake.methods('turn/start') and not fake.methods('turn/steer') and not fake.methods('turn/interrupt')
    expect(pane.locator('.sdk-queued')).to_have_count(0)
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
    with tempfile.TemporaryDirectory(prefix='sd-sdk-goal-') as temporary, sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        try:
            for mode, width, theme in [('node', 1280, 'light'), ('node', 390, 'dark'),
                                       ('node', 1280, 'dark'), ('node', 390, 'light'), ('node', 320, 'dark'), ('hub', 1280, 'light')]:
                root = Path(temporary) / f'{mode}-{width}-{theme}'
                root.mkdir()
                corpus = build(root)
                port = free_port()
                env = environment(root, args.binary)
                if mode == 'hub':
                    env.update(node_env(root, port, '127.0.0.0/8'))
                with GoalServer(root / 'app.sock', corpus.paths['sdk']) as fake:
                    with server(corpus, args.binary, root / 'app.sock', env) as base:
                        if mode == 'node':
                            exercise(browser, base, corpus, fake, width, theme, args.screenshots, mode)
                        else:
                            hub_root = root / 'hub'
                            hub_root.mkdir()
                            hub = Hub((args.hub_binary or args.binary.with_name('sessiondock-hub')).resolve(),
                                      hub_root, [SimpleNamespace(name='SdkNode', token=TOKEN, port=port)])
                            hub.start()
                            try:
                                exercise(browser, f'http://127.0.0.1:{hub.port}', corpus, fake, width, theme, args.screenshots, mode)
                            finally:
                                hub.stop()
        finally:
            browser.close()
    print('PASS SDK Goal: create/edit/resume/pause/clear, busy/idle, preserved goal/budget, native rejection, draft race, external updates, reload; node/Hub and desktop/mobile light/dark')


if __name__ == '__main__':
    main()
