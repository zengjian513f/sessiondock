#!/usr/bin/env python3
"""Managed SDK markers across node/Hub, reload and daemon disconnect.

Real HTTP/SSE and Chromium; synthetic proc, native records and app-server only.
"""
import argparse
import asyncio
from pathlib import Path
import re
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD, build, server
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, codex_row, codex_message
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from spawned_by_fixtures import proc_pid


class StatusDaemon(FakeAppServer):
    async def _handle(self, writer, message):
        if message.get('method') == 'thread/resume' and message['params']['threadId'] != THREAD:
            await self.send(writer, {'id': message['id'], 'error': {
                'code': -32000, 'message': 'thread already has an active writer'}})
            return
        await super()._handle(writer, message)

    def disconnect(self):
        async def close():
            for writer in list(self.clients):
                writer.close()
                await writer.wait_closed()
        asyncio.run_coroutine_threadsafe(close(), self.loop).result(timeout=5)


def exercise(browser, base, fake, width, theme, screenshots, label):
    print(label, 'open SDK, reload, external writer, disconnect', flush=True)
    context = browser.new_context(viewport={'width': width, 'height': 900},
                                  color_scheme=theme, service_workers='block')
    page = context.new_page()
    page.set_default_timeout(15000)
    errors = []
    page.on('pageerror', lambda error: errors.append(error.stack))
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded && S.sessions.length === 2')
    uid = page.evaluate('S.sessions.find(r => r.sid === "' + THREAD + '").uid')
    independent = page.evaluate('S.sessions.find(r => r.sid === "external-cli-thread").uid')
    card = page.locator(f'#side .item[data-uid="{uid}"]')
    external = page.locator(f'#side .item[data-uid="{independent}"]')
    marker = card.locator(':scope > .ico > .item-status')
    card.click()
    expect(page.locator('#sdk-model')).to_have_value('fake-sol')
    expect(page.locator('#dlive')).to_have_class(re.compile(r'\bsdk\b'))
    expect(marker).to_have_class(re.compile(r'\bsdk\b'))
    expect(marker).to_have_class(re.compile(r'\bvisible\b'))
    expect(page.locator('#dlive')).to_have_attribute('title', re.compile('受管 SDK'))
    data = page.request.get(base + '/api/live').json()
    assert uid in data['sdk_uids'] and uid in data['uids'], data
    assert uid not in data['tmux_uids'], 'SDK must not claim a terminal'
    # Use the existing blue token, including dark mode; no DOM mutation.
    assert marker.evaluate("e => getComputedStyle(e).backgroundColor") == page.locator('#dlive').evaluate(
        "e => { const c = getComputedStyle(e); return c.backgroundColor; }")
    if screenshots:
        page.screenshot(path=str(screenshots / (label + '-sdk.png')))
    page.reload(wait_until='domcontentloaded')
    expect(page.locator('#sdk-input')).to_be_visible()
    expect(page.locator('#dlive')).to_have_class(re.compile(r'\bsdk\b'))
    # Show the sidebar through its actual mobile navigation.
    if width < 500:
        page.locator('#detail .mobile-back').click()
    external.click()
    expect(page.locator('.sdk-status')).to_contain_text('独立运行')
    expect(page.locator('#dlive')).not_to_have_class(re.compile(r'\bsdk\b'))
    expect(page.locator('#dlive')).not_to_have_class(re.compile(r'\btmux\b'))
    expect(page.locator('#dlive')).to_have_class(re.compile(r'\bvisible\b'))
    if width < 500:
        page.locator('#detail .mobile-back').click()
    card.click()
    expect(page.locator('#sdk-model')).to_have_value('fake-sol')
    page.locator('#a-chat').click()
    expect(page.locator('#msgs')).to_contain_text('SDK 状态回归')
    # Let the global 2-second observer sample ownership before disconnecting
    # without an SDK subscriber; that is the background invalidation path.
    page.wait_for_timeout(2500)
    fake.disconnect()
    expect(marker).not_to_have_class(re.compile(r'\bsdk\b'), timeout=20000)
    expect(page.locator('#dlive')).not_to_have_class(re.compile(r'\bsdk\b'))
    expect(page.locator('#dlive')).to_have_class(re.compile(r'\bvisible\b'))
    data = page.request.get(base + '/api/live').json()
    assert uid not in data['sdk_uids'] and uid in data['uids'], data
    page.locator('#a-sdk').click()
    expect(page.locator('#sdk-model')).to_have_value('fake-sol')
    expect(page.locator('#dlive')).to_have_class(re.compile(r'\bsdk\b'))
    assert not errors, errors
    context.close()
    fake.disconnect()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    expect.set_options(timeout=20000)
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-status-') as tmp:
        root = Path(tmp)
        corpus = build(root)
        corpus.put('sdk', 'codex', [codex_row('session_meta', {'id': THREAD, 'cwd': str(root)}),
                   codex_message('user', 'SDK 状态回归：已中断的历史仍属于受管会话 Managed SDK'),
                   codex_row('event_msg', {'type': 'turn_aborted'})], [])
        corpus.put('external', 'codex', [codex_row('session_meta', {
                   'id': 'external-cli-thread', 'cwd': str(root)}),
                   codex_message('user', 'Independent CLI 独立进程')], [])
        proc = root / 'proc'
        proc.mkdir()
        (proc / 'stat').write_text('btime 1700000000\n')
        proc_pid(proc, 400, 'codex', ['codex', 'app-server'], 1, fds={3: corpus.paths['sdk']})
        proc_pid(proc, 500, 'codex', ['codex', 'resume', 'external-cli-thread'], 1,
                 fds={3: corpus.paths['external']})
        port = free_port()
        env = node_env(root, port, '127.0.0.0/8')
        env['SESSIONDOCK_PROC_ROOT'] = str(proc)
        with StatusDaemon(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake:
            with server(corpus, args.binary, root / 'app.sock', env) as base:
                hub_root = root / 'hub'
                hub_root.mkdir()
                hub = Hub((args.hub_binary or args.binary.with_name('sessiondock-hub')).resolve(),
                          hub_root, [SimpleNamespace(name='SdkNode', token=TOKEN, port=port)])
                hub.start()
                try:
                    with sync_playwright() as pw:
                        browser = pw.chromium.launch(headless=True)
                        try:
                            for mode, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                                for width, theme in [(1280, 'light'), (390, 'dark'), (1280, 'dark'), (390, 'light')]:
                                    exercise(browser, url, fake, width, theme, args.screenshots,
                                             f'{mode}-{width}-{theme}')
                        finally:
                            browser.close()
                finally:
                    hub.stop()
    print('PASS SDK blue markers: node/Hub, reload, failed resume stays green, disconnect/reconnect, widths/themes')


if __name__ == '__main__':
    main()
