#!/usr/bin/env python3
"""SDK FIFO queue, explicit steer/cancel/retry and reload through Chromium.

Private fake app-server only. All turns are held until the test completes them.
"""
from __future__ import annotations
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD, build, server
from codex_sdk_controls_browser import ControlsServer
from codex_sdk_actions_browser import environment
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


class QueueServer(ControlsServer):
    async def _handle(self, writer, message):
        method, params = message.get('method'), message.get('params') or {}
        if method == 'turn/start':
            assert not self.turn, 'a queued input started before the preceding turn finished'
            text = params['input'][0]['text']
            if text.startswith('reject once') and not getattr(self, 'rejected', False):
                self.rejected = True
                await self.send(writer, {'id': message['id'], 'error': {'code': -32000, 'message': 'Synthetic queued rejection'}})
                return
            self.turn = f"queue-turn-{len(self.methods('turn/start'))}"
            self.current_text = text
            await self.notify('thread/status/changed', {'status': {'type': 'active', 'activeFlags': []}})
            await self.notify('turn/started', {'turn': {'id': self.turn, 'status': 'inProgress', 'items': []}})
            await self._user_item(self.turn, text)
            await self.send(writer, {'id': message['id'], 'result': {'turn': {'id': self.turn}}})
        elif method == 'thread/resume':
            await self.send(writer, {'id': message['id'], 'result': {'thread': {'id': THREAD,
                'status': {'type': 'active' if self.turn else 'idle'}, 'turns': []},
                'model': self.settings['model'], 'reasoningEffort': self.settings['effort']}})
        else:
            await super()._handle(writer, message)

    def complete(self):
        async def finish():
            turn, self.turn = self.turn, None
            await self.notify('thread/status/changed', {'status': {'type': 'idle'}})
            await self.notify('turn/completed', {'turn': {'id': turn, 'status': 'completed', 'items': []}})
        asyncio.run_coroutine_threadsafe(finish(), self.loop).result(timeout=5)


def open_page(browser, base, width, theme):
    context = browser.new_context(viewport={'width': width, 'height': 844}, color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.goto(base, wait_until='networkidle')
    page.locator('#side .item').filter(has_text='earlier synthetic question').first.click()
    expect(page.locator('#sdk-input')).to_be_enabled()
    return context, page


def exercise(browser, base, fake, width, theme, screenshots, label):
    context, page = open_page(browser, base, width, theme)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    box, send = page.locator('#sdk-input'), page.locator('#sdk-send')
    def submit(text):
        box.fill(text); send.click(); expect(box).to_have_value('')
    def row(text):
        return page.locator('.sdk-queued').filter(has_text=text)
    starts, steers = len(fake.methods('turn/start')), len(fake.methods('turn/steer'))
    submit('working ' + label)
    expect(send).to_have_text('排队')
    # A lost acknowledgment followed by reload/retry must keep one submission.
    cancel_text = '撤销这条尚未发送的消息 ' + label
    def lose_reply(route):
        route.fetch()
        route.abort('failed')
    page.route('**/api/codex-sdk/turn', lose_reply)
    box.fill(cancel_text); send.click()
    expect(row('撤销这条')).to_be_visible()
    expect(page.locator('.sdk-status')).to_contain_text('发送失败')
    expect(box).to_have_value(cancel_text)
    page.unroute('**/api/codex-sdk/turn', lose_reply)
    page.reload(wait_until='networkidle')
    expect(box).to_have_value(cancel_text)
    send.click(); expect(box).to_have_value('')
    expect(page.locator('.sdk-queued')).to_have_count(1)
    submit('立即引导当前工作 ' + label)
    submit('FIFO first · 中文与 realistic-long-project-path ' + label)
    submit('FIFO second ' + label)
    expect(page.locator('.sdk-queued')).to_have_count(4)
    assert len(fake.methods('turn/start')) == starts + 1
    assert len(fake.methods('turn/steer')) == steers
    row('撤销这条').locator('[data-queue-action="cancel"]').click()
    expect(page.locator('.sdk-queued')).to_have_count(3)
    row('立即引导当前').locator('[data-queue-action="steer"]').click()
    expect(page.locator('.sdk-queued')).to_have_count(2)
    assert len(fake.methods('turn/steer')) == steers + 1
    assert fake.methods('turn/steer')[-1]['params']['expectedTurnId'] == fake.turn
    page.reload(wait_until='networkidle')
    expect(page.locator('.sdk-queued')).to_have_count(2)
    if screenshots:
        page.screenshot(path=str(screenshots / f'{label}-queue.png'))
    fake.complete()
    expect(page.locator('.sdk-live')).to_contain_text('FIFO first')
    expect(page.locator('.sdk-queued')).to_have_count(1)
    assert 'FIFO first' in fake.methods('turn/start')[-1]['params']['input'][0]['text']
    fake.complete()
    expect(page.locator('.sdk-live')).to_contain_text('FIFO second')
    expect(page.locator('.sdk-queued')).to_have_count(0)
    fake.rejected = False
    submit('reject once ' + label)
    fake.complete()
    expect(row('reject once')).to_contain_text('Synthetic queued rejection')
    if screenshots:
        page.screenshot(path=str(screenshots / f'{label}-failed.png'))
    row('reject once').locator('[data-queue-action="retry"]').click()
    expect(page.locator('.sdk-live')).to_contain_text('reject once')
    expect(page.locator('.sdk-queued')).to_have_count(0)
    fake.complete()
    expect(send).to_have_text('发送')
    assert not errors, errors
    context.close()
    print('PASS ' + label + ': queue/cancel/explicit steer/FIFO/reload/retry', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-queue-') as tmp, sync_playwright() as p:
        root = Path(tmp); corpus = build(root)
        state = root/'state'
        port = free_port()
        env = {**environment(root, args.binary), **node_env(root, port, '127.0.0.0/8')}
        browser = p.chromium.launch(headless=True)
        try:
            with QueueServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake:
                with server(corpus, args.binary, root/'app.sock', env) as base:
                    hub_root = root/'hub'; hub_root.mkdir()
                    hub = Hub(args.binary.with_name('sessiondock-hub').resolve(), hub_root,
                              [SimpleNamespace(name='QueueNode', token=TOKEN, port=port)])
                    hub.start()
                    try:
                        for mode, url, width, theme in [('node', base, 1280, 'light'), ('node', base, 390, 'dark'),
                                ('hub', f'http://127.0.0.1:{hub.port}', 1280, 'dark'), ('hub', f'http://127.0.0.1:{hub.port}', 390, 'light')]:
                            exercise(browser, url, fake, width, theme, args.screenshots, f'{mode}-{width}-{theme}')
                    finally: hub.stop()
                    context, page = open_page(browser, base, 390, 'dark')
                    page.locator('#sdk-input').fill('working across service restart')
                    page.locator('#sdk-send').click()
                    expect(page.locator('#sdk-send')).to_have_text('排队')
                    page.locator('#sdk-input').fill('durable queued message across service restart')
                    page.locator('#sdk-send').click()
                    expect(page.locator('.sdk-queued')).to_have_count(1)
                    context.close()
                assert json.loads((state/'codex-sdk-queue.json').read_text())
                with server(corpus, args.binary, root/'app.sock', env) as base:
                    context, page = open_page(browser, base, 390, 'dark')
                    expect(page.locator('.sdk-queued')).to_contain_text('durable queued message')
                    fake.complete()
                    expect(page.locator('.sdk-live')).to_contain_text('durable queued message')
                    expect(page.locator('.sdk-queued')).to_have_count(0)
                    fake.complete(); context.close()
                print('PASS persisted queue across node restart without premature send', flush=True)
        finally: browser.close()


if __name__ == '__main__': main()
