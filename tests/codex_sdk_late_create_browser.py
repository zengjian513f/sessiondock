#!/usr/bin/env python3
"""Late native create replies retain their original parent and migration tree."""
import argparse
import asyncio
from concurrent.futures import ThreadPoolExecutor
import http.client
import json
from pathlib import Path
import subprocess
import tempfile

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD, build
from codex_sdk_fixtures import FakeAppServer
from grok_sdk_lifecycle_browser import advance
from history_fixtures import BINARY, codex_message, codex_row
from hub_fixtures import free_port
from new_session_model_browser import open_dialog
from sdk_creation_retention_fixtures import node
from sdkhost_fixtures import stop_private_sdkhosts
from spawned_by_fixtures import proc_pid


class LateCreate(FakeAppServer):
    held = None
    hold_id = None

    async def _handle(self, writer, message):
        if message.get('method') == 'thread/start':
            self.hold_id = message['id']
        if message.get('method') == 'thread/loaded/list':
            await self.send(writer, {'id': message['id'], 'result': {'data': []}})
            return
        await super()._handle(writer, message)

    async def send(self, writer, message):
        if message.get('id') == self.hold_id and 'result' in message:
            self.held = (writer, message)
            self.hold_id = None
            return
        await super().send(writer, message)

    def release(self):
        writer, message = self.held
        asyncio.run_coroutine_threadsafe(super().send(writer, message), self.loop).result(5)


def exercise(root, binary, pw, restart):
    corpus = build(root)
    state = root/'state'; state.mkdir()
    proc = root/'request-proc'; proc.mkdir()
    (proc/'stat').write_text('btime 1700000000\n')
    (proc/'net').mkdir(); (proc/'net/tcp').write_text('header\n')
    proc_pid(proc, 200, 'python', ['python', 'dispatch.py'], 1,
             env=[('CODEX_THREAD_ID', THREAD)], fds={4: 'socket:[900]'})
    library = root/'clock.so'
    subprocess.run(['cc', '-shared', '-fPIC', '-O2', str(Path(__file__).with_name('grok_sdk_idle_clock.c')),
                    '-o', str(library), '-ldl'], check=True)
    marker = root/'clock'; marker.write_text('0')
    env = {'SESSIONDOCK_PROC_ROOT': str(proc), 'LD_PRELOAD': str(library),
           'SESSIONDOCK_TEST_CLOCK_FILE': str(marker)}
    port = free_port(); socket = root/'app.sock'
    with LateCreate(socket, corpus.paths['sdk'], THREAD) as fake, ThreadPoolExecutor() as pool:
        browser = pw.chromium.launch(headless=True)
        page = browser.new_page()
        page.set_default_timeout(20000)
        held = []

        def forward(body):
            connection = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
            connection.connect()
            local = connection.sock.getsockname()[1]
            (proc/'net/tcp').write_text('header\n' +
                f' 0: 0100007F:{local:04X} 0100007F:{port:04X} 01 0 0 0 0 0 900\n')
            connection.request('POST', '/api/codex-sdk/create', json.dumps(body), {'Content-Type': 'application/json'})
            response = connection.getresponse()
            result = response.status, response.read()
            connection.close()
            return result

        def intercept(route):
            body = route.request.post_data_json
            held.append((route, pool.submit(forward, body), body))

        def recover(base):
            fake.release()
            # No caller exists now. Only the persisted original request can
            # establish the parent; do not infer it from title or recovery peer.
            page.wait_for_function('''async () => {
                const r = await fetch('/api/term/list');
                return (await r.json()).sdk_sessions?.some(s => s.sid === %s);
            }''' % json.dumps(fake.thread_id))
            page.unroute('**/api/codex-sdk/create', intercept)
            with page.expect_response('**/api/codex-sdk/create') as completed:
                page.locator('#new-session-go').click()
            assert completed.value.ok, completed.value.text()
            result = completed.value.json(); uid = result['uid']
            assert result['thread_id'] == fake.thread_id
            expect(page.locator('#new-session-dialog')).to_be_hidden()
            expect(page.locator('#sdk-input')).to_be_visible()
            assert len(fake.methods('thread/start')) == 1, 'original create was replayed'
            saved = json.loads((state/'session-metadata.json').read_text())['sessions']
            assert saved[uid]['nest_parent']['sid'] == THREAD, saved[uid]
            page.locator('#sdk-input').fill('approve first message after late creation')
            page.locator('#sdk-send').click()
            page.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
            expect(page.locator('.sdk-live')).to_contain_text('All done after approval')
            # A native subagent is part of the recovered parent's move unit.
            child = fake.rollout.with_name('late-subagent.jsonl')
            child.write_text('\n'.join(json.dumps(row) for row in [
                codex_row('session_meta', {'id': '11111111-2222-7333-8444-555555555555',
                    'cwd': str(root), 'timestamp': '2026-10-17T10:01:00Z',
                    'source': {'subagent': {'thread_spawn': {'parent_thread_id': fake.thread_id}}}}),
                codex_message('user', 'synthetic native child'),
            ])+'\n')
            page.reload(wait_until='domcontentloaded')
            page.wait_for_function('T.listLoaded')
            if page.locator('#nest-toggle').get_attribute('aria-pressed') != 'true':
                page.locator('#nest-toggle').click()
            parent = page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]')
            row = page.locator(f'#side .item[data-uid="{uid}"]')
            if not row.is_visible(): parent.locator('.nest-caret').click()
            expect(row).to_have_attribute('data-depth', '1')
            row.click()
            tree = page.request.post(base+'/api/session/transfer/tree', data={'uid': corpus.uid('sdk')})
            assert tree.ok, tree.text()
            sids = {s['sid'] for s in tree.json()['sessions']}
            assert {THREAD, fake.thread_id, '11111111-2222-7333-8444-555555555555'} <= sids, tree.text()
            row.click(button='right')
            page.locator('#item-menu [data-act="detach"]').click()
            retry = page.request.post(base+'/api/codex-sdk/create', data=held[0][2])
            assert retry.ok and retry.json()['uid'] == uid
            saved = json.loads((state/'session-metadata.json').read_text())['sessions']
            assert not saved[uid].get('nest_parent'), 'retry overrode explicit detach'
            assert len(fake.methods('thread/start')) == 1

        try:
            with node(corpus, binary, socket, port, state, env) as base:
                page.goto(base, wait_until='domcontentloaded')
                page.wait_for_function('T.listLoaded')
                page.route('**/api/codex-sdk/create', intercept)
                open_dialog(page)
                page.locator('input[name=new-source][value=codex]').check()
                page.locator('#new-cwd').fill(str(root))
                page.locator('#new-session-go').click()
                page.wait_for_timeout(100)
                fake.wait_for(lambda: fake.held is not None)
                advance(page, marker, 125)
                status, raw = held[0][1].result(20)
                assert status == 502, (status, raw)
                held[0][0].fulfill(status=status, content_type='application/json', body=raw)
                expect(page.locator('#new-session-error')).not_to_be_empty()
                (proc/'net/tcp').write_text('header\n')
                if not restart: recover(base)
            if restart:
                with node(corpus, binary, socket, port, state, env) as base:
                    recover(base)
            print(f'PASS late creation: timeout, restart={restart}, same SID, original parent, first SEND, move tree, detach', flush=True)
        finally:
            browser.close()
            stop_private_sdkhosts(root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-late-create-') as temporary, sync_playwright() as pw:
        for restart in (False, True):
            root = Path(temporary)/str(restart); root.mkdir()
            exercise(root, args.binary, pw, restart)


if __name__ == '__main__':
    main()
