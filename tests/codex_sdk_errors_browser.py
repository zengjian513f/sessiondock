#!/usr/bin/env python3
"""SDK recoverable retries and final failures through real Chromium/SSE snapshots.

Synthetic rollouts, held fake app-server turns and private loopback services only.
"""
from __future__ import annotations

import argparse
import asyncio
import json
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_queue_browser import QueueServer
from history_fixtures import BINARY, codex_row, encoded


class ErrorsServer(QueueServer):
    def emit_error(self, message, retry):
        asyncio.run_coroutine_threadsafe(self.notify('error', {
            'turnId': self.turn, 'willRetry': retry,
            'error': {'message': message, 'codexErrorInfo': 'other', 'additionalDetails': None},
        }), self.loop).result(timeout=5)

    def progress(self, label):
        asyncio.run_coroutine_threadsafe(self._agent(self.turn, self.turn + '-progress',
            '重试后恢复输出 · ' + label, 'commentary'), self.loop).result(timeout=5)

    def reasoning(self, started):
        method = 'item/started' if started else 'item/reasoning/textDelta'
        params = ({'item': {'id': self.turn + '-reasoning', 'type': 'reasoning', 'summary': [], 'content': []}}
                  if started else {'itemId': self.turn + '-reasoning', 'contentIndex': 0, 'delta': 'Resumed reasoning'})
        asyncio.run_coroutine_threadsafe(self.notify(method, {'turnId': self.turn, **params}),
                                        self.loop).result(timeout=5)

    def finish(self, status, message=None, *, native_error=None, status_type='idle'):
        async def complete():
            turn, self.turn = self.turn, None
            if native_error:
                with self.rollout.open('ab') as native:
                    native.write(encoded(codex_row('event_msg', {'type': 'task_complete',
                        'turn_id': turn, 'error': {'message': native_error,
                            'codex_error_info': 'server_overloaded'}})))
            error = ({'message': message, 'codexErrorInfo': 'other', 'additionalDetails': None}
                     if message else None)
            await self.notify('turn/completed', {'turn': {
                'id': turn, 'status': status, 'items': [], 'error': error,
            }})
            await self.notify('thread/status/changed', {'status': {'type': status_type}})
        asyncio.run_coroutine_threadsafe(complete(), self.loop).result(timeout=5)


def exercise(browser, base, fake, width, theme, screenshots):
    label = f'{width}-{theme}'
    context = browser.new_context(viewport={'width': width, 'height': 900}, service_workers='block')
    context.add_init_script("localStorage.setItem('sessiondock.theme', JSON.stringify(" + json.dumps(theme) + "));")
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    expect(page.locator('html')).to_have_attribute('data-theme', theme)
    page.locator('#side .item').filter(has_text='earlier synthetic question').first.click()
    box, send, status = page.locator('#sdk-input'), page.locator('#sdk-send'), page.locator('.sdk-status')
    expect(box).to_be_enabled()

    def submit(text):
        box.fill(text)
        send.click()
        expect(box).to_have_value('')
        expect(send).to_have_text('排队')

    def reload():
        page.reload(wait_until='networkidle')
        expect(box).to_be_enabled()
        page.wait_for_function('CodexSdkView.connected === true')

    def screenshot(name):
        if screenshots:
            # The existing compact status control exposes the full explanation
            # on click; include that user-visible detail in the visual check.
            expanded = status.is_enabled()
            if expanded:
                status.click()
                expect(page.locator('#sdk-status-menu')).to_be_visible()
            page.screenshot(path=str(screenshots / f'{label}-{name}.png'))
            if expanded:
                page.keyboard.press('Escape')

    submit('请检查重试后继续执行并保留草稿 · ' + label)
    starts = len(fake.methods('turn/start'))
    fake.emit_error('Temporary upstream connection error · 正在恢复网络连接', True)
    expect(status).to_contain_text('正在重试')
    expect(status).to_contain_text('Temporary upstream connection error')
    expect(status).not_to_have_class('sdk-status blocked')
    page.wait_for_function('CodexSdkView.summary.last_error === null && !!CodexSdkView.summary.retry_error')
    screenshot('retrying')
    # The reload must recover the retry classification from the service snapshot.
    reload()
    expect(status).to_contain_text('正在重试')
    expect(status).not_to_have_class('sdk-status blocked')
    fake.progress(label)
    expect(page.locator('.sdk-live')).to_contain_text('重试后恢复输出')
    expect(status).not_to_contain_text('Temporary upstream connection error')
    page.wait_for_function('CodexSdkView.summary.retry_error === null')
    screenshot('resumed')
    # Progress clears the transient state in the service too, not just the page.
    reload()
    expect(status).not_to_contain_text('Temporary upstream connection error')
    page.wait_for_function('CodexSdkView.summary.retry_error === null')
    # Unsummarized native reasoning is not painted as conversation, but its
    # resumed stream must still clear transient errors in the page and snapshot.
    fake.reasoning(True)
    fake.emit_error('Temporary error before native reasoning resumes', True)
    expect(status).to_contain_text('Temporary error before native reasoning')
    fake.reasoning(False)
    expect(status).not_to_contain_text('Temporary error before native reasoning')
    page.wait_for_function('CodexSdkView.summary.retry_error === null')
    reload()
    page.wait_for_function('CodexSdkView.summary.retry_error === null')
    fake.emit_error('Second temporary error before successful completion', True)
    expect(status).to_contain_text('Second temporary error')
    fake.finish('completed')
    expect(send).to_have_text('发送')
    expect(status).not_to_contain_text('Second temporary error')
    page.wait_for_function('CodexSdkView.summary.retry_error === null && CodexSdkView.summary.last_error === null')
    reload()
    expect(status).not_to_contain_text('error')
    # Inject only a stream transport failure; recovery reads the real SSE snapshot.
    box.fill('断线后仍应保留的未发送草稿 · ' + label)
    page.evaluate('''() => {
        window.closedSdkStream = CodexSdkView.es;
        closedSdkStream.close();
        closedSdkStream.dispatchEvent(new Event('error'));
    }''')
    page.wait_for_function('CodexSdkView.es !== closedSdkStream && CodexSdkView.es?.readyState === 1')
    expect(box).to_have_value('断线后仍应保留的未发送草稿 · ' + label)
    expect(status).not_to_contain_text('error')
    assert len(fake.methods('turn/start')) == starts, 'reload/reconnect submitted another turn'
    screenshot('completed')

    submit('最终失败应持续可见 · ' + label)
    fake.emit_error('Final upstream failure · 请检查当前服务', False)
    expect(status).to_contain_text('错误：Final upstream failure')
    expect(status).to_have_class('sdk-status blocked')
    # Later item notifications must not hide a final failure.
    fake.progress('final failure detail')
    expect(status).to_contain_text('Final upstream failure')
    fake.finish('failed')
    expect(send).to_have_text('发送')
    reload()
    expect(status).to_contain_text('错误：Final upstream failure')
    expect(status).to_have_class('sdk-status blocked')
    screenshot('final-failure')

    submit('仅 completed failed 带错误，没有单独 error 通知 · ' + label)
    expect(status).not_to_contain_text('Final upstream failure')
    fake.finish('failed', 'Failure supplied only by turn.error · 原生最终错误')
    expect(status).to_contain_text('Failure supplied only by turn.error')
    expect(status).to_have_class('sdk-status blocked')
    reload()
    expect(status).to_contain_text('Failure supplied only by turn.error')
    expect(status).to_have_class('sdk-status blocked')
    screenshot('turn-failure')

    submit('容量错误前的用户请求 · ' + label)
    capacity = 'Selected model is at capacity. Please try a different model.'
    fake.finish('failed', native_error=capacity, status_type='systemError')
    expect(send).to_have_text('发送')
    expect(status).to_contain_text(capacity)
    screenshot('capacity-failure')
    reload()
    expect(status).to_contain_text(capacity)
    starts = len(fake.methods('turn/start'))
    submit('容量错误后再次发送 · ' + label)
    expect(page.locator('.sdk-live')).to_contain_text('容量错误后再次发送')
    expect(page.locator('.sdk-queued')).to_have_count(0)
    assert len(fake.methods('turn/start')) == starts + 1, 'systemError left the next input queued'
    submit('失败回合结束后自动续发 · ' + label)
    expect(page.locator('.sdk-queued')).to_have_count(1)
    fake.finish('failed', status_type='systemError')
    expect(page.locator('.sdk-live')).to_contain_text('失败回合结束后自动续发')
    expect(page.locator('.sdk-queued')).to_have_count(0)
    assert len(fake.methods('turn/start')) == starts + 2, 'queued input did not resume after failure'
    fake.finish('completed')
    expect(send).to_have_text('发送')
    expect(status).not_to_contain_text(capacity)

    # A native successful completion always supersedes earlier error notices.
    submit('最终成功应清除本轮较早的错误通知 · ' + label)
    fake.emit_error('Earlier notice superseded by completed turn', False)
    expect(status).to_contain_text('Earlier notice superseded')
    fake.finish('completed')
    expect(send).to_have_text('发送')
    expect(status).not_to_contain_text('Earlier notice superseded')
    reload()
    page.wait_for_function('CodexSdkView.summary.last_error === null && CodexSdkView.summary.retry_error === null')
    assert not errors, errors
    assert not fake.methods('turn/steer') and not fake.methods('turn/interrupt')
    context.close()
    print(f'PASS {label}: retry/progress/success, reload/reconnect and final error snapshots', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-errors-') as tmp, sync_playwright() as p:
        root = Path(tmp)
        corpus = build(root)
        env = environment(root, args.binary)
        browser = p.chromium.launch(headless=True)
        try:
            with ErrorsServer(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake:
                with server(corpus, args.binary, root / 'app.sock', env) as base:
                    for width, theme in ((1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark')):
                        exercise(browser, base, fake, width, theme, args.screenshots)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
