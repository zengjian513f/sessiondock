#!/usr/bin/env python3
"""Interrupted SDK outcomes and missing native endings through Chromium.

Private fake daemon, native records and HTTP CLI observations only.
"""
import argparse
import asyncio
from datetime import datetime, timezone
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_queue_browser import QueueServer
from history_fixtures import BINARY, codex_row, encoded
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


class InterruptedServer(QueueServer):
    def disconnect(self):
        async def close():
            for writer in list(self.clients):
                writer.close()
                await writer.wait_closed()
        asyncio.run_coroutine_threadsafe(close(), self.loop).result(timeout=5)


def exercise(browser, base, fake, corpus, screenshots, label, width, theme):
    print(f'{label} {width} {theme}: interruption / reload / native fallback', flush=True)
    context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme,
                                  service_workers='block')
    context.add_init_script("localStorage.setItem('sessiondock.theme', " + json.dumps(json.dumps(theme)) + ")")
    page = context.new_page()
    page.set_default_timeout(15000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    cli = {'observed_at': time.time(), 'instance': {'running': True, 'busy': False},
           'input': {'state': 'ready', 'code': ''}, 'queued': []}
    only_cli = False

    def messages(route):
        response = route.fetch()
        data = response.json()
        if only_cli:
            data = {'cli_only': True, 'cli': cli.copy()}
        else:
            data['cli'] = cli.copy()
        route.fulfill(response=response, json=data)

    page.route('**/api/messages/*', messages)
    page.goto(base, wait_until='networkidle')
    expect(page.locator('html')).to_have_attribute('data-theme', theme)
    page.locator('#side .item').filter(has_text='earlier synthetic question').first.click()
    page.wait_for_function('CodexSdkView.connected && !!CodexSdkView.historyData')
    box, send = page.locator('#sdk-input'), page.locator('#sdk-send')
    status = page.locator('.sdk-pane .sdk-status')

    def capture(name):
        status.click()
        expect(page.locator('#sdk-status-menu')).to_be_visible()
        page.screenshot(path=str(screenshots / f'{label}-{width}-{theme}-{name}.png'))
        page.keyboard.press('Escape')

    def reload():
        page.reload(wait_until='networkidle')
        # A node restart or changed owner can default to a terminal/chat view.
        if not page.locator('#sdk-input').is_visible():
            page.locator('#a-sdk').click()
        page.wait_for_function('CodexSdkView.connected && !!CodexSdkView.historyData')
        expect(page.locator('html')).to_have_attribute('data-theme', theme)

    def append(kind, **payload):
        row = codex_row('event_msg', {'type': kind, **payload})
        row['timestamp'] = datetime.now(timezone.utc).isoformat()
        with corpus.paths['sdk'].open('ab') as stream:
            stream.write(encoded(row))

    box.fill('停止本轮后应保留中断结果，不能只显示空闲 · ' + label)
    send.click()
    expect(send).to_have_text('排队')
    page.locator('#sdk-stop').click()
    expect(send).to_have_text('发送')
    capture('after-stop')
    # The fake intentionally writes no turn_aborted: only the live native RPC
    # carries the outcome, exactly what the bridge used to discard.
    expect(status).to_have_text('已中断')
    expect(status).to_have_attribute('data-state', 'warning')
    capture('live-interrupted')
    reload()
    expect(status).to_have_text('已中断')
    assert page.evaluate('CodexSdkView.summary.last_turn.status') == 'interrupted'
    box.fill('新回合和正常完成必须撤销之前的中断 · ' + label)
    send.click()
    expect(status).to_have_text('处理中')
    fake.complete()
    expect(status).to_have_text('就绪')
    reload()
    expect(status).to_have_text('就绪')

    # Releasing the subscription drops the bridge cache. Re-entry must use
    # persisted native evidence instead of requiring a prior browser event.
    page.locator('#a-chat').click()
    fake.disconnect()
    append('turn_aborted', turn_id='persisted-interruption')
    reload()
    expect(status).to_have_text('已中断')
    capture('native-interrupted')

    # The report's shape: task_started, no ending, newer ready/idle screen.
    append('task_started', turn_id='missing-native-ending')
    cli['observed_at'] = time.time() + 3
    reload()
    expect(status).to_have_text('已中断')
    expect(page.locator('#sdk-status-menu')).to_contain_text('原生回合未写入结束事件')
    capture('missing-ending')
    # Only CLI metadata changes; unchanged history must still repaint status.
    only_cli = True
    checkpoint = page.evaluate('CodexSdkView.historyData.end')
    cli['observed_at'] = time.time() + 3
    cli['instance'] = {'running': True, 'busy': True}
    page.wait_for_function('cache.get(viewKey(S.sel))?.cli?.instance?.busy === true')
    expect(status).not_to_have_text('已中断')
    cli['observed_at'] = time.time() + 3
    cli['instance'] = {'running': True, 'busy': False}
    page.wait_for_function('cache.get(viewKey(S.sel))?.cli?.instance?.busy === false')
    expect(status).to_have_text('已中断')
    assert page.evaluate('CodexSdkView.historyData.end') == checkpoint
    only_cli = False
    append('task_complete', turn_id='missing-native-ending')
    reload()
    expect(status).to_have_text('就绪')
    assert not errors, errors
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path, default=BINARY.with_name('sessiondock-hub'))
    parser.add_argument('--screenshots', type=Path, default=Path('target/sdk-interrupted'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-interrupted-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        corpus = build(root)
        env = environment(root, args.binary)
        port = free_port()
        env.update(node_env(root, port, '127.0.0.0/8'))
        with InterruptedServer(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake:
            with server(corpus, args.binary, root / 'app.sock', env) as base:
                hub_root = root / 'hub'; hub_root.mkdir()
                hub = Hub(args.hub_binary.resolve(), hub_root,
                          [SimpleNamespace(name='InterruptedNode', token=TOKEN, port=port)])
                hub.start()
                browser = pw.chromium.launch(headless=True)
                try:
                    for label, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                        for width, theme in [(1280, 'light'), (390, 'dark')]:
                            exercise(browser, url, fake, corpus, args.screenshots, label, width, theme)
                finally:
                    browser.close()
                    hub.stop()
    print('PASS SDK interrupted: native events, snapshots/reload, missing endings, node/Hub and themes')


if __name__ == '__main__':
    main()
