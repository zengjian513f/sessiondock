#!/usr/bin/env python3
"""Native automatic compaction needs no manual RPC and preserves accepted input."""
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
from contextlib import ExitStack
from types import SimpleNamespace
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from playwright.sync_api import expect, sync_playwright
from codex_sdk_queue_browser import QueueServer
from codex_sdk_browser import THREAD, build, server
from codex_sdk_actions_browser import environment
from history_fixtures import BINARY


class CompactionServer(QueueServer):
    async def _handle(self, writer, message):
        if message.get('method') == 'thread/turns/list' and getattr(self, 'offline_consumed', None):
            await self.send(writer, {'id': message['id'], 'result': {'data': [self.offline_consumed], 'nextCursor': None}})
        elif message.get('method') == 'turn/start' and self.compact_next:
            self.compact_next = False
            self.turn = 'compacting-' + str(len(self.methods('turn/start')))
            self.current_text = message['params']['input'][0]['text']
            await self.notify('thread/status/changed', {'status': {'type': 'active'}})
            await self.notify('turn/started', {'turn': {'id': self.turn, 'status': 'inProgress', 'items': []}})
            await self.send(writer, {'id': message['id'], 'result': {'turn': {'id': self.turn}}})
            await self.notify('item/started', {'turnId': self.turn, 'item': {'id': 'compact', 'type': 'contextCompaction'}})
        else:
            await super()._handle(writer, message)

    def consume(self):
        async def finish():
            await self.notify('item/completed', {'turnId': self.turn, 'item': {'id': 'compact', 'type': 'contextCompaction'}})
            await self._user_item(self.turn, self.current_text)
        asyncio.run_coroutine_threadsafe(finish(), self.loop).result(timeout=5)


def open_page(browser, base, width, theme):
    context = browser.new_context(viewport={'width': width, 'height': 844},
                                  color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.goto(base, wait_until='networkidle')
    page.locator('#side .item').filter(has_text='earlier synthetic question').first.click()
    expect(page.locator('.sdk-history')).to_be_visible()
    # Opening an idle session is read-only; start it through the user control.
    if not page.locator('#sdk-input').is_visible():
        button = page.locator('#a-session-toggle')
        expect(button).to_have_attribute('aria-label', '启动会话')
        if not button.is_visible(): page.locator('#a-more').click()
        button.click()
    expect(page.locator('#sdk-input')).to_be_visible()
    expect(page.locator('#sdk-input')).to_be_enabled()
    page.locator('#sdk-input').click()
    page.wait_for_function('CodexSdkView.connected')
    return context, page


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-compact-') as tmp, sync_playwright() as p:
        root = Path(tmp); corpus = build(root)
        port = free_port()
        env = {**environment(root, args.binary), **node_env(root, port, '127.0.0.0/8')}
        browser = p.chromium.launch(headless=True)
        try:
            with CompactionServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake:
                with server(corpus, args.binary, root/'app.sock', env) as base, ExitStack() as cleanup:
                    hub_root = root/'hub'; hub_root.mkdir()
                    hub = Hub(args.binary.with_name('sessiondock-hub').resolve(), hub_root,
                              [SimpleNamespace(name='CompactionNode', token=TOKEN, port=port)])
                    hub.start(); cleanup.callback(hub.stop)
                    for mode, url, width, theme in [('node', base, 1280, 'light'), ('node', base, 390, 'dark'),
                            ('hub', f'http://127.0.0.1:{hub.port}', 1280, 'dark'),
                            ('hub', f'http://127.0.0.1:{hub.port}', 390, 'light')]:
                        context, page = open_page(browser, url, width, theme)
                        text = f'压缩之前输入的消息，接收确认后发送已完成 · {mode}-{width}'
                        fake.compact_next = True
                        page.locator('#sdk-input').fill(text); page.locator('#sdk-send').click()
                        row = page.locator('.sdk-queued').filter(has_text=text)
                        expect(row).to_have_count(0)
                        status = page.locator('.sdk-pane .sdk-status')
                        expect(status).to_have_text('压缩中')
                        expect(status).to_have_attribute('data-state', 'maintenance')
                        accepted = page.locator('.sdk-accepted').filter(has_text=text)
                        expect(accepted).to_be_visible()
                        expect(accepted.locator('button')).to_have_count(0)
                        expect(page.locator('[data-sdk-item="compact"]')).to_have_count(0)
                        starts = len(fake.methods('turn/start'))
                        page.reload(wait_until='networkidle')
                        expect(row).to_have_count(0)
                        expect(status).to_have_text('压缩中')
                        expect(accepted).to_be_visible()
                        assert len(fake.methods('turn/start')) == starts, 'reconnect must not resubmit during automatic compaction'
                        assert not fake.methods('thread/compact/start'), 'native automatic compaction needs no manual request'
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'compact-{mode}-{width}.png'))
                        page.locator('#sdk-stop').click()
                        expect(row).to_have_count(0)
                        starts = len(fake.methods('turn/start'))
                        page.reload(wait_until='networkidle')
                        expect(row).to_have_count(0)
                        expect(accepted).to_be_visible()
                        assert len(fake.methods('turn/start')) == starts
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'interrupted-{mode}-{width}.png'))
                        assert len(fake.methods('turn/start')) == starts
                        expect(page.locator('#sdk-send')).to_have_text('发送')
                        fake.compact_next = True
                        page.locator('#sdk-input').fill('压缩成功后才确认消息'); page.locator('#sdk-send').click()
                        expect(page.locator('.sdk-queued')).to_have_count(0)
                        expect(status).to_have_text('压缩中')
                        consumed = page.locator('.sdk-accepted').filter(has_text='压缩成功后才确认消息')
                        expect(consumed).to_be_visible()
                        fake.consume()
                        expect(consumed).to_have_count(0)
                        expect(page.locator('.sdk-queued')).to_have_count(0)
                        expect(page.locator('.sdk-live')).to_contain_text('压缩成功后才确认消息')
                        expect(status).to_have_text('处理中')
                        expect(status).to_have_attribute('data-state', 'working')
                        assert not fake.methods('thread/compact/start')
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'continued-{mode}-{width}.png'))
                        fake.complete(); context.close()
                        print(f'PASS {width} {theme}: automatic compact/reload/no manual RPC/interrupt without send regression/native consumption/continued turn', flush=True)
                    fake.compact_next = True
                    context, page = open_page(browser, base, 1280, 'dark')
                    page.locator('#sdk-input').fill('restart while compacting')
                    page.locator('#sdk-send').click()
                    expect(page.locator('.sdk-queued')).to_have_count(0)
                    expect(page.locator('.sdk-pane .sdk-status')).to_have_text('压缩中')
                    page.wait_for_timeout(200)
                    starts_before_restart = len(fake.methods('turn/start'))
                    context.close()
                entries = json.loads((root/'state/codex-sdk-queue.json').read_text())[THREAD]
                assert entries[-1]['state'] == 'sent'
                with server(corpus, args.binary, root/'app.sock', env) as base:
                    context, page = open_page(browser, base, 1280, 'dark')
                    expect(page.locator('.sdk-queued')).to_have_count(0)
                    expect(page.locator('.sdk-accepted').filter(has_text='restart while compacting')).to_be_visible()
                    page.locator('#sdk-stop').click()
                    expect(page.locator('.sdk-queued')).to_have_count(0)
                    assert len(fake.methods('turn/start')) == starts_before_restart
                    context.close()
                    print('PASS service restart preserves completed delivery without replay', flush=True)
                fake.offline_consumed = {'id': entries[-1]['turn_id'], 'status': 'completed', 'items': [{
                    'id': 'consumed-while-offline', 'type': 'userMessage',
                    'content': [{'type': 'text', 'text': 'restart while compacting'}]}]}
                with server(corpus, args.binary, root/'app.sock', env) as base:
                    context, page = open_page(browser, base, 1280, 'dark')
                    expect(page.locator('.sdk-accepted').filter(has_text='restart while compacting')).to_have_count(0)
                    assert len(fake.methods('turn/start')) == starts_before_restart
                    context.close()
                    print('PASS completed native consumption during service downtime reconciles receipt', flush=True)
        finally: browser.close()


if __name__ == '__main__': main()
