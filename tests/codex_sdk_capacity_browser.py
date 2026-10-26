#!/usr/bin/env python3
"""Capacity backoff through Chromium and a private fake daemon, with real timers."""
from __future__ import annotations
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from codex_sdk_errors_browser import ErrorsServer
from codex_sdk_compose_browser import saved
from codex_sdk_queue_browser import open_page
from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from history_fixtures import BINARY, codex_row, encoded
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env

REASONING = "This model's reasoning capability is disabled. Please choose a different model."
CAPACITY = 'Selected model is at capacity. Please try a different model.'

class CapacityServer(ErrorsServer):
    def __init__(self, *args):
        super().__init__(*args)
        self.retries = []
        self.reject_retry = False

    def finish_reply(self, text, *, native=False, phase='final_answer'):
        async def complete():
            turn = self.turn
            if not native:
                await self._agent(turn, turn + '-reply', text, phase)
            with self.rollout.open('ab') as rollout:
                rollout.write(encoded(codex_row('event_msg', {'type': 'task_complete',
                    'turn_id': turn, 'last_agent_message': text if phase == 'final_answer' else 'Done'})))
            self.turn = None
            await self.notify('turn/completed', {'turn': {
                'id': turn, 'status': 'completed', 'items': [], 'error': None}})
            await self.notify('thread/status/changed', {'status': {'type': 'idle'}})
        asyncio.run_coroutine_threadsafe(complete(), self.loop).result(timeout=5)

    async def _handle(self, writer, message):
        params = message.get('params', {})
        if message.get('method') == 'turn/start' and params.get('input') == []:
            assert not self.turn, 'retry overlapped active work'
            assert set(params) == {'threadId', 'input'}, 'retry changed settings or replayed input'
            self.retries.append(time.monotonic())
            if self.reject_retry:
                await self.send(writer, {'id': message['id'], 'error': {'code': -32000, 'message': 'Synthetic continuation rejected'}})
                return
            self.turn = f'capacity-retry-{len(self.retries)}'
            await self.notify('thread/status/changed', {'status': {'type': 'active', 'activeFlags': []}})
            await self.notify('turn/started', {'turn': {'id': self.turn, 'status': 'inProgress', 'items': []}})
            await self._agent(self.turn, self.turn + '-agent', '容量恢复后继续处理', 'commentary')
            await self.send(writer, {'id': message['id'], 'result': {'turn': {'id': self.turn}}})
        else:
            await super()._handle(writer, message)


def exercise(browser, base, fake, width, theme, screenshots, long):
    context, page = open_page(browser, base, width, theme)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    box, send = page.locator('#sdk-input'), page.locator('#sdk-send')
    def submit(text):
        box.fill(text); send.click(); expect(box).to_have_value('')
        expect(page.locator('.sdk-queued')).to_have_count(0)
        expect(send).to_have_text('排队')
    def fail(delay, native=False):
        at = time.monotonic()
        fake.finish('failed', None if native else CAPACITY,
                    native_error=CAPACITY if native else None, status_type='systemError')
        page.wait_for_function('delay => CodexSdkView.summary?.capacity_retry?.delay_seconds === delay', arg=delay)
        expect(page.locator('#sdk-stop')).to_be_visible()
        expect(page.locator('.sdk-pane .sdk-status')).not_to_have_class('sdk-status blocked')
        expect(page.locator('#sdk-status-menu')).to_contain_text(f'{delay} 秒后自动重试')
        return at
    def shot(label):
        if screenshots:
            status = page.locator('.sdk-pane .sdk-status')
            if status.is_enabled(): status.click()
            page.screenshot(path=str(screenshots / f'{width}-{theme}-{label}.png'))
            page.keyboard.press('Escape')

    submit('容量失败前已消费的原始请求，不应重放附件或原消息')
    fake.emit_error(CAPACITY, True)
    page.wait_for_function('!!CodexSdkView.summary?.retry_error')
    assert page.evaluate('CodexSdkView.summary.capacity_retry == null'), 'native retry was duplicated'
    start = fail(15, native=True)
    shot('waiting')
    box.fill('未发送的草稿')
    saved(page, page.evaluate('CodexSdkView.uid'), '未发送的草稿')
    page.reload(wait_until='networkidle')
    expect(box).to_have_value('未发送的草稿')
    page.wait_for_function('CodexSdkView.summary?.capacity_retry?.delay_seconds === 15')
    before = len(fake.retries)
    page.wait_for_function('!!CodexSdkView.turn', timeout=22000)
    assert len(fake.retries) == before + 1
    assert fake.retries[-1] - start >= 14.8, 'retry fired early'
    expect(box).to_have_value('未发送的草稿')
    expect(page.locator('.sdk-live')).to_contain_text('容量恢复后继续处理')
    shot('resumed')
    if long:
        for delay in (30, 60):
            start = fail(delay)
            print(f'waiting for actual {delay}s backoff', flush=True)
            page.wait_for_function('!!CodexSdkView.turn', timeout=(delay + 8) * 1000)
            assert fake.retries[-1] - start >= delay - .2
        fail(120)
    else:
        fail(30)
    # Cancelling the timer uses the ordinary stop control, without interrupting
    # a completed native turn. Reload cannot re-arm it from old history.
    before = len(fake.retries)
    page.locator('#sdk-stop').click()
    page.wait_for_function('CodexSdkView.summary?.capacity_retry == null')
    page.reload(wait_until='networkidle')
    expect(box).to_be_enabled()
    assert page.evaluate('CodexSdkView.summary.capacity_retry == null')
    assert len(fake.retries) == before
    submit('用户的新输入重置退避')
    fail(15)
    submit('等待中直接发新输入，取消旧重试')
    page.wait_for_function('CodexSdkView.summary?.capacity_retry == null')
    fake.finish('completed')
    expect(send).to_have_text('发送')
    if long:
        submit('关闭全部页面以后依然重试')
        fail(15)
        context.close()
        deadline = time.monotonic() + 23
        while len(fake.retries) == before and time.monotonic() < deadline:
            # Wait for the fake daemon's observable request, not production data.
            threading_event.wait(.1)
        assert len(fake.retries) == before + 1
        context, page = open_page(browser, base, width, theme)
        box, send = page.locator('#sdk-input'), page.locator('#sdk-send')
        page.on('pageerror', lambda error: errors.append(str(error)))
        expect(page.locator('.sdk-live')).to_contain_text('容量恢复后继续处理')
        fake.finish('completed')
        expect(page.locator('#sdk-send')).to_have_text('发送')
    # BUG-20261026-030436-0b8fbd: this upstream refusal arrives as
    # assistant output and a successful completion, without any error code.
    submit('思考能力停用后继续原上下文')
    before = len(fake.retries)
    started = time.monotonic()
    fake.finish_reply(REASONING)
    page.wait_for_function('CodexSdkView.summary?.capacity_retry?.delay_seconds === 15')
    expect(page.locator('#sdk-status-menu')).to_contain_text(REASONING)
    expect(page.locator('.sdk-pane .sdk-status')).to_have_text('正在重试')
    shot('reasoning-disabled')
    page.reload(wait_until='networkidle')
    page.wait_for_function('CodexSdkView.summary?.capacity_retry?.delay_seconds === 15')
    page.wait_for_function('!!CodexSdkView.turn', timeout=22000)
    assert len(fake.retries) == before + 1
    assert fake.retries[-1] - started >= 14.8
    # Missing live message: the same completed turn's native last reply is
    # authoritative. This must retain the previous delay rather than reset it.
    fake.finish_reply(REASONING, native=True)
    page.wait_for_function('CodexSdkView.summary?.capacity_retry?.delay_seconds === 30')
    expect(page.locator('#sdk-status-menu')).to_contain_text(REASONING)
    page.locator('#sdk-stop').click()
    page.wait_for_function('CodexSdkView.summary?.capacity_retry == null')
    page.reload(wait_until='networkidle')
    assert page.evaluate('CodexSdkView.summary.capacity_retry == null')
    # A structured error also participates, but quotes and commentary do not.
    submit('结构化错误使用相同退避')
    fake.finish('failed', REASONING)
    page.wait_for_function('CodexSdkView.summary?.capacity_retry?.delay_seconds === 15')
    page.locator('#sdk-stop').click()
    for text, phase in ((f'Quoted error: {REASONING}', 'final_answer'), (REASONING, 'commentary')):
        submit('正常说明不应触发退避')
        fake.finish_reply(text, phase=phase)
        expect(send).to_have_text('发送')
        page.wait_for_timeout(1000)  # cover the node's native-file fallback tick
        assert page.evaluate('CodexSdkView.summary.capacity_retry == null')
    assert not errors, errors
    context.close()
    print(f'PASS {width}/{theme}: capacity/native fallback, real backoff, reload, stop, new input, no replay', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path, default=BINARY.with_name('sessiondock-hub'))
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-capacity-') as tmp, sync_playwright() as p:
        root = Path(tmp); corpus = build(root); env = environment(root, args.binary)
        port = free_port()
        env.update(node_env(root, port, '127.0.0.0/8'))
        browser = p.chromium.launch(headless=True)
        try:
            with CapacityServer(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake:
                with server(corpus, args.binary, root / 'app.sock', env) as base:
                    exercise(browser, base, fake, 1280, 'light', args.screenshots, True)
                    exercise(browser, base, fake, 390, 'dark', args.screenshots, False)
                    hub_root = root / 'hub'; hub_root.mkdir()
                    hub = Hub(args.hub_binary.resolve(), hub_root, [SimpleNamespace(name='CapacityNode', token=TOKEN, port=port)])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', fake, 1280, 'dark', args.screenshots, False)
                        exercise(browser, f'http://127.0.0.1:{hub.port}', fake, 390, 'light', args.screenshots, False)
                    finally: hub.stop()
        finally: browser.close()

if __name__ == '__main__':
    import threading
    threading_event = threading.Event()
    main()
