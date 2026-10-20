#!/usr/bin/env python3
"""Native automatic compaction needs no manual RPC and preserves accepted input."""
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
from playwright.sync_api import expect, sync_playwright
from codex_sdk_queue_browser import QueueServer
from codex_sdk_browser import THREAD, build, server
from codex_sdk_actions_browser import environment
from history_fixtures import BINARY


class CompactionServer(QueueServer):
    async def _handle(self, writer, message):
        if message.get('method') == 'turn/start' and self.compact_next:
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
    return context, page


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-compact-') as tmp, sync_playwright() as p:
        root = Path(tmp); corpus = build(root)
        env = environment(root, args.binary)
        browser = p.chromium.launch(headless=True)
        try:
            with CompactionServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake:
                with server(corpus, args.binary, root/'app.sock', env) as base:
                    for width, theme in [(1280, 'light'), (390, 'dark')]:
                        context, page = open_page(browser, base, width, theme)
                        text = f'压缩之前输入的消息，必须保留并允许重试 · {width}'
                        fake.compact_next = True
                        page.locator('#sdk-input').fill(text); page.locator('#sdk-send').click()
                        row = page.locator('.sdk-queued').filter(has_text=text)
                        expect(row).to_contain_text('正在发送')
                        status = page.locator('.sdk-pane .sdk-status')
                        expect(status).to_have_text('压缩中')
                        expect(status).to_have_attribute('data-state', 'maintenance')
                        expect(page.locator('[data-sdk-item="compact"]')).to_have_count(0)
                        starts = len(fake.methods('turn/start'))
                        page.reload(wait_until='networkidle')
                        expect(row).to_contain_text(text)
                        expect(status).to_have_text('压缩中')
                        assert len(fake.methods('turn/start')) == starts, 'reconnect must not resubmit during automatic compaction'
                        assert not fake.methods('thread/compact/start'), 'native automatic compaction needs no manual request'
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'compact-{width}.png'))
                        page.locator('#sdk-stop').click()
                        expect(row).to_contain_text('内容已保留')
                        starts = len(fake.methods('turn/start'))
                        page.reload(wait_until='networkidle')
                        expect(row).to_contain_text('内容已保留')
                        assert len(fake.methods('turn/start')) == starts
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'interrupted-{width}.png'))
                        row.locator('[data-queue-action="retry"]').click()
                        expect(page.locator('.sdk-live')).to_contain_text(text)
                        expect(row).to_have_count(0)
                        fake.complete()
                        expect(page.locator('#sdk-send')).to_have_text('发送')
                        fake.compact_next = True
                        page.locator('#sdk-input').fill('压缩成功后才确认消息'); page.locator('#sdk-send').click()
                        expect(page.locator('.sdk-queued')).to_contain_text('正在发送')
                        expect(status).to_have_text('压缩中')
                        fake.consume()
                        expect(page.locator('.sdk-queued')).to_have_count(0)
                        expect(page.locator('.sdk-live')).to_contain_text('压缩成功后才确认消息')
                        expect(status).to_have_text('处理中')
                        expect(status).to_have_attribute('data-state', 'working')
                        assert not fake.methods('thread/compact/start')
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'continued-{width}.png'))
                        fake.complete(); context.close()
                        print(f'PASS {width} {theme}: automatic compact/reload/no manual RPC/interrupt/retry/native confirmation/continued turn', flush=True)
                    fake.compact_next = True
                    context, page = open_page(browser, base, 1280, 'dark')
                    page.locator('#sdk-input').fill('restart while compacting')
                    page.locator('#sdk-send').click()
                    expect(page.locator('.sdk-queued')).to_contain_text('正在发送')
                    context.close()
                entries = json.loads((root/'state/codex-sdk-queue.json').read_text())[THREAD]
                assert entries[-1]['state'] in ('sending', 'failed') and entries[-1]['input']
                with server(corpus, args.binary, root/'app.sock', env) as base:
                    context, page = open_page(browser, base, 1280, 'dark')
                    expect(page.locator('.sdk-queued')).to_contain_text('restart while compacting')
                    expect(page.locator('.sdk-queued')).to_contain_text('核对')
                    page.locator('#sdk-stop').click()
                    page.locator('.sdk-queued [data-queue-action="cancel"]').click()
                    expect(page.locator('.sdk-queued')).to_have_count(0)
                    context.close()
                    print('PASS service restart retains unconfirmed input without replay', flush=True)
        finally: browser.close()


if __name__ == '__main__': main()
