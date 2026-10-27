#!/usr/bin/env python3
"""Recover a silent, still-open Codex socket after a cancelled browser request."""
from __future__ import annotations

import argparse
import subprocess
import tempfile
from pathlib import Path

from codex_sdk_browser import build, server, open_sdk
from codex_sdk_retention_browser import RetainedPeer, THREAD
from grok_sdk_lifecycle_browser import advance
from history_fixtures import BINARY


class SilentPeer(RetainedPeer):
    silent_writer = None
    stall_start = False

    async def _handle(self, writer, message):
        if writer is self.silent_writer:
            return
        if message.get('method') == 'turn/start' and self.stall_start:
            self.stall_start = False
            self.silent_writer = writer
            return
        if message.get('method') == 'turn/interrupt' and self.silent_writer is None:
            # Native interruption succeeds but its reply/completion never arrives.
            # Keep the transport open; only a fresh connection can observe idle.
            self.turn = None
            self.silent_writer = writer
            return
        await super()._handle(writer, message)


def main():
    from playwright.sync_api import sync_playwright, expect
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-codex-stalled-') as temporary:
        root = Path(temporary)
        corpus = build(root)
        old = corpus.paths['sdk']
        native = old.with_name('rollout-2026-09-11T10-00-00-' + THREAD + '.jsonl')
        native.write_bytes(old.read_bytes().replace(b'01a1sdk0-0000-7000-8000-000000000001', THREAD.encode()))
        old.unlink(); corpus.paths['sdk'] = native
        (root / 'state').mkdir()
        library = root / 'clock.so'
        subprocess.run(['cc', '-shared', '-fPIC', '-O2', str(Path(__file__).with_name('grok_sdk_idle_clock.c')),
                        '-o', str(library), '-ldl'], check=True, timeout=30)
        marker = root / 'clock'; marker.write_text('0')
        env = {'SESSIONDOCK_STATE_DIR': str(root / 'state'), 'LD_PRELOAD': str(library),
               'SESSIONDOCK_TEST_CLOCK_FILE': str(marker)}
        socket = root / 'app.sock'
        with SilentPeer(socket, native, THREAD) as fake, sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            page = browser.new_page()
            with server(corpus, args.binary, socket, env) as base:
                page.goto(base, wait_until='networkidle')
                open_sdk(page, corpus, expect)
                page.locator('#sdk-input').fill('hold synthetic turn')
                page.locator('#sdk-send').click()
                page.wait_for_function('CodexSdkView.turn?.status === "inProgress"')
                page.locator('#sdk-stop').click()
                fake.wait_for(lambda: fake.silent_writer is not None)
                resumes = len(fake.methods('thread/resume'))
                # Navigation abandons the HTTP interrupt; its native deadline
                # must still retire the connection instead of leaving it usable.
                page.reload(wait_until='domcontentloaded')
                page.wait_for_function('CodexSdkView.connected')
                page.locator('#sdk-input').fill('unsent draft survives recovery')
                advance(page, marker, 125)
                page.wait_for_function('CodexSdkView.connected && !CodexSdkView.turn', timeout=20000)
                assert len(fake.methods('thread/resume')) > resumes
                assert len(fake.methods('turn/start')) == 1, 'uncertain input was replayed'
                assert len(fake.methods('turn/interrupt')) == 1, 'interrupt was replayed'
                expect(page.locator('#sdk-input')).to_have_value('unsent draft survives recovery')
                page.locator('#sdk-input').fill('approve explicit continuation')
                page.locator('#sdk-send').click()
                page.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
                expect(page.locator('.sdk-live')).to_contain_text('All done after approval', timeout=15000)
                assert len(fake.methods('turn/start')) == 2
                page.wait_for_function('!CodexSdkView.turn')
                fake.stall_start = True
                page.locator('#sdk-input').fill('uncertain delivery must not replay')
                page.locator('#sdk-send').click()
                fake.wait_for(lambda: len(fake.methods('turn/start')) == 3)
                resumes = len(fake.methods('thread/resume'))
                advance(page, marker, 125)
                fake.wait_for(lambda: len(fake.methods('thread/resume')) > resumes)
                expect(page.locator('.sdk-queued')).to_contain_text('连接中断', timeout=15000)
                assert len(fake.methods('turn/start')) == 3, 'timed-out input replayed on reconnect'
                page.locator('.sdk-queued [data-queue-action="cancel"]').click()
                expect(page.locator('.sdk-queued')).to_have_count(0)
            browser.close()
    print('PASS Chromium: cancelled interrupt, silent socket timeout, fresh idle snapshot, preserved draft, no replay, explicit continuation')


if __name__ == '__main__':
    main()
