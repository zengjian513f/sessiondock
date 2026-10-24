#!/usr/bin/env python3
"""Retained Codex subscriptions survive quiet pages, transport loss and node restart."""
from __future__ import annotations
import argparse
import asyncio
import json
from pathlib import Path
import subprocess
import tempfile

from codex_sdk_browser import THREAD, build, server, open_sdk
from codex_sdk_fixtures import FakeAppServer
from grok_sdk_lifecycle_browser import advance
from history_fixtures import BINARY


class RetainedPeer(FakeAppServer):
    loaded = True

    async def _handle(self, writer, message):
        if message.get('method') == 'thread/loaded/list':
            # Discovery must not adopt unrelated daemon sessions.
            await self.send(writer, {'id': message['id'], 'result': {
                'data': ([self.thread_id] if self.loaded else []) + ['external-thread']}})
        else:
            await super()._handle(writer, message)

    def disconnect(self):
        async def close():
            for writer in list(self.clients):
                writer.close()
                await writer.wait_closed()
        asyncio.run_coroutine_threadsafe(close(), self.loop).result(5)


def main():
    from playwright.sync_api import sync_playwright, expect
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-codex-retention-') as temporary:
        root = Path(temporary)
        corpus = build(root)
        (root / 'state').mkdir()
        library = root / 'clock.so'
        subprocess.run(['cc', '-shared', '-fPIC', '-O2', str(Path(__file__).with_name('grok_sdk_idle_clock.c')),
                        '-o', str(library), '-ldl'], check=True, timeout=30)
        marker = root / 'clock'; marker.write_text('0')
        env = {'SESSIONDOCK_STATE_DIR': str(root / 'state'), 'LD_PRELOAD': str(library),
               'SESSIONDOCK_TEST_CLOCK_FILE': str(marker)}
        socket = root / 'app.sock'
        with RetainedPeer(socket, corpus.paths['sdk'], THREAD) as fake, sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            context = browser.new_context(service_workers='block')
            def open_page(base):
                page = context.new_page()
                page.goto(base, wait_until='networkidle')
                open_sdk(page, corpus, expect)
                expect(page.locator('#sdk-input')).to_be_visible()
                return page
            def retained():
                return json.loads((root / 'state/codex-sdk-subscriptions.json').read_text())['threads']
            with server(corpus, args.binary, socket, env) as base:
                page = open_page(base)
                fake.wait_for(lambda: fake.methods('thread/resume'))
                page.locator('#sdk-input').fill('approve retention browser message')
                page.locator('#sdk-send').click()
                fake.wait_for(lambda: fake.methods('turn/start'))
                page.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
                expect(page.locator('.sdk-live')).to_contain_text('All done after approval', timeout=15000)
                page.close()
                observer = context.new_page(); observer.goto(base + '/api/health')
                advance(observer, marker, 70); advance(observer, marker, 3700)
                assert not fake.methods('thread/unsubscribe'), 'quiet page triggered native unsubscribe'
                assert THREAD in retained()
                n = len(fake.methods('thread/resume'))
                fake.disconnect()
                fake.wait_for(lambda: len(fake.methods('thread/resume')) > n)
                assert len(fake.methods('turn/start')) == 1, 'reconnect replayed input'
                observer.close()
            n = len(fake.methods('thread/resume'))
            # No browser opens this service: the persisted intent drives recovery.
            with server(corpus, args.binary, socket, env) as base:
                fake.wait_for(lambda: len(fake.methods('thread/resume')) > n)
                assert len(fake.methods('turn/start')) == 1
                assert all(m['params']['threadId'] == THREAD for m in fake.methods('thread/resume'))
                page = open_page(base)
                # Exercise explicit stop through the SDK HTTP boundary from Chromium.
                result = page.evaluate('''async uid => {const r=await fetch('/api/codex-sdk/stop', {
                    method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({uid})});
                    return {status:r.status,body:await r.json()};}''', corpus.uid('sdk'))
                assert result['status'] == 200, result
                assert THREAD not in retained()
                page.close()
            n = len(fake.methods('thread/resume'))
            with server(corpus, args.binary, socket, env) as base:
                observer = context.new_page(); observer.goto(base + '/api/health')
                advance(observer, marker, 70)
                assert len(fake.methods('thread/resume')) == n, 'restart reopened explicitly stopped thread'
                observer.close()
                page = open_page(base)
                fake.wait_for(lambda: len(fake.methods('thread/resume')) > n)
                page.close()
            # A previously retained thread no longer loaded by the daemon must
            # not be resurrected from its rollout on service restart.
            fake.loaded = False
            n = len(fake.methods('thread/resume'))
            with server(corpus, args.binary, socket, env) as base:
                observer = context.new_page(); observer.goto(base + '/api/health')
                advance(observer, marker, 70)
                assert len(fake.methods('thread/resume')) == n, 'recovery loaded a historical thread'
                observer.close()
            fake.loaded = True
            with server(corpus, args.binary, socket, env) as base:
                fake.wait_for(lambda: len(fake.methods('thread/resume')) > n)
                asyncio.run_coroutine_threadsafe(fake.notify('thread/closed', {}), fake.loop).result(5)
                fake.wait_for(lambda: THREAD not in retained())
                n = len(fake.methods('thread/resume'))
                fake.disconnect()
                observer = context.new_page(); observer.goto(base + '/api/health')
                advance(observer, marker, 70)
                assert len(fake.methods('thread/resume')) == n, 'native close was automatically undone'
                observer.close()
            context.close(); browser.close()
    print('PASS Codex quiet retention, detached reconnect/restart without replay, explicit stop and loaded-only recovery')


if __name__ == '__main__':
    main()
