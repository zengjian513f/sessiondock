#!/usr/bin/env python3
"""Resume native providers through Chromium, with a different daemon default."""
from __future__ import annotations

import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_queue_browser import QueueServer
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from sdk_ui_fixtures import start_sdk


class ProviderServer(QueueServer):
    provider = 'openai'
    default_provider = 'openai_catalog_cache'

    async def _handle(self, writer, message):
        method, params = message.get('method'), message.get('params') or {}
        if method == 'thread/read':
            assert params == {'threadId': THREAD, 'includeTurns': False}, params
            await self.send(writer, {'id': message['id'], 'result': {
                'thread': {'id': THREAD, 'modelProvider': self.provider}}})
            return
        if method == 'thread/resume':
            provider = params.get('modelProvider', self.default_provider)
            if self.provider and provider != self.provider:
                await self.send(writer, {'id': message['id'], 'error': {'code': -32602,
                    'message': f'This chat uses {self.provider}, but your current provider is {provider}.'}})
                return
        await super()._handle(writer, message)


def exercise(browser, base, fake, screenshots, label):
    context = browser.new_context(viewport={'width': 1280, 'height': 844}, color_scheme='dark')
    page = context.new_page()
    page.goto(base, wait_until='networkidle')
    page.locator('#side .item').filter(has_text='earlier synthetic question').first.click()
    start_sdk(page)
    expect(page.locator('.sdk-history')).to_contain_text('earlier synthetic answer')
    resumes = len(fake.methods('thread/resume'))
    assert resumes and fake.methods('thread/resume')[-1]['params'].get('modelProvider') == (fake.provider or None)
    assert not fake.methods('turn/start'), 'opening submitted input'
    page.locator('#sdk-input').fill('Continue with the original provider')
    page.locator('#sdk-send').click()
    expect(page.locator('.sdk-live')).to_contain_text('Continue with the original provider')
    fake.complete()
    expect(page.locator('#sdk-send')).to_have_text('发送')
    page.reload(wait_until='networkidle')
    page.wait_for_function('CodexSdkView.connected')
    assert len(fake.methods('thread/resume')) == resumes, 'reload resumed an already attached thread'
    assert len(fake.methods('turn/start')) == 1, 'reload replayed input'
    fake.disconnect()
    fake.wait_for(lambda: len(fake.methods('thread/resume')) > resumes)
    page.wait_for_function('CodexSdkView.connected')
    assert fake.methods('thread/resume')[-1]['params'].get('modelProvider') == (fake.provider or None)
    assert len(fake.methods('turn/start')) == 1, 'reconnect replayed input'
    if screenshots:
        page.screenshot(path=str(screenshots / f'{label}.png'), full_page=True)
    context.close()
    print(f'PASS {label}: native provider, send, reload without replay', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        try:
            for mode, provider in [('node', 'openai'), ('hub', 'custom-provider'), ('node', '')]:
                with tempfile.TemporaryDirectory(prefix='sd-provider-') as tmp:
                    root = Path(tmp)
                    corpus = build(root)
                    port = free_port()
                    env = {**environment(root, args.binary), **node_env(root, port, '127.0.0.0/8')}
                    with ProviderServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake:
                        fake.provider = provider
                        with server(corpus, args.binary, root/'app.sock', env) as base:
                            if mode == 'hub':
                                hub_root = root/'hub'
                                hub_root.mkdir()
                                hub = Hub(args.binary.with_name('sessiondock-hub').resolve(), hub_root,
                                          [SimpleNamespace(name='ProviderNode', token=TOKEN, port=port)])
                                hub.start()
                                try:
                                    exercise(browser, f'http://127.0.0.1:{hub.port}', fake, args.screenshots, 'hub-custom')
                                finally:
                                    hub.stop()
                            else:
                                exercise(browser, base, fake, args.screenshots, 'node-' + (provider or 'unspecified'))
        finally:
            browser.close()


if __name__ == '__main__':
    main()
