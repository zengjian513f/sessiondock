"""Accepted moves survive closed browsers/restarts and freeze both nodes."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, isolated_server
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN, GOOD
from session_clone_browser import prepare
from session_bundle_browser import Peer, reopen_transfer
from session_background_fixtures import BlockedNode
from transfer_ui import select_target


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--peer', required=True)
    parser.add_argument('--screenshots', type=Path, default=Path('target/background-move-review'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-background-') as temporary, sync_playwright() as pw, ExitStack() as stack:
        root = Path(temporary)
        source = prepare(root/'source')
        for name in ('state', 'proc', 'ids', 'trash'):
            (source.root/name).mkdir(parents=True, exist_ok=True)
        source_port = free_port()
        proxy = BlockedNode(source_port)
        stack.callback(proxy.close)
        a = SimpleNamespace(name='源机器', nid='a'*32, port=proxy.port, token=TOKEN)
        b = SimpleNamespace(name='目标机器', nid='b'*32, port=free_port(), token=TOKEN)
        (source.root/'ids/node-id').write_text(a.nid+'\n')
        env = {**node_env(source.root, source_port, '127.0.0.0/8'), 'SESSIONDOCK_PROC_ROOT': str(source.root/'proc')}
        base, _ = stack.enter_context(isolated_server(source, args.binary, state_dir=source.root/'state', trash_dir=source.root/'trash', extra_env=env))
        peer = Peer(args.peer, root, source, {kind:str(source.root/kind) for kind in ('claude', 'codex', 'grok')}, b, args.binary)
        stack.callback(peer.close)
        peer.call('start')
        hubroot = root/'hub'
        hubroot.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [a, b])
        hub.start()
        stack.callback(hub.stop)
        browser = pw.chromium.launch(headless=True)
        stack.callback(browser.close)
        context = browser.new_context(viewport={'width':1280, 'height':900}, service_workers='block')
        stack.callback(context.close)
        page = context.new_page()
        page.set_default_timeout(20000)
        huburl = f'http://127.0.0.1:{hub.port}'

        def begin(uid, destination):
            page.goto(huburl, wait_until='networkidle')
            row = page.locator(f'#side .item[data-uid="{uid}"]')
            if not row.is_visible(): page.locator('#nest-toggle').click()
            row.click(button='right')
            page.locator('#item-menu [data-act="clone"]').click()
            dialog = page.locator('#clone-group-dialog')
            expect(dialog.locator('.transfer-tree input[data-uid]').first).to_be_attached()
            select_target(dialog, destination.nid)
            expect(dialog.locator('.clone-confirm')).to_be_enabled()
            with page.expect_response('**/api/session/transfer/start') as accepted:
                dialog.locator('.clone-confirm').click()
            assert accepted.value.status == 202, accepted.value.text()
            task = accepted.value.json()
            expect(dialog.locator('.clone-confirm')).to_have_text('后台迁移中')
            expect(dialog.locator('.clone-cancel')).to_have_text('关闭')
            return task, dialog

        task, dialog = begin(scoped(a.nid, source.uid('a')), b)
        request = task['request']
        operation = request['operation_id']
        journal = hubroot/'transfers'/f'{operation}.json'
        assert json.loads(journal.read_text())['execution_requested']
        repeated = page.request.post(huburl+'/api/session/transfer/start', data=request)
        assert repeated.status == 202 and repeated.json()['request'] == request
        conflict = page.request.post(huburl+'/api/session/transfer/start', data={**request, 'target_node':a.nid})
        assert conflict.status == 409
        cancel_calls = []
        page.on('request', lambda r: cancel_calls.append(r.url) if r.url.endswith('/cancel') else None)
        for width, theme in ((1280, 'light'), (390, 'dark')):
            page.set_viewport_size({'width':width, 'height':900})
            page.emulate_media(color_scheme=theme)
            page.screenshot(path=str(args.screenshots/f'background-{width}-{theme}.png'), full_page=True)
        dialog.locator('.clone-cancel').click()
        expect(dialog).not_to_be_visible()
        assert not cancel_calls, cancel_calls
        new_starts = []
        page.on('request', lambda r: new_starts.append(r.url) if r.url.endswith('/transfer/start') else None)
        for escape in (False, True):
            dialog = reopen_transfer(page, hub, operation)
            expect(dialog.locator('.clone-confirm')).to_be_disabled()
            if escape: dialog.press('Escape')
            else: dialog.locator('.transfer-close').click()
            expect(dialog).not_to_be_visible()
        assert not new_starts and not cancel_calls, (new_starts, cancel_calls)

        # Both direct node and Hub commands must reject mutations immediately,
        # including operations absent from the visible menu.
        local = source.uid('a')
        commands = [('/api/term/create', {'resume_uid':local}),
                    ('/api/codex-sdk/open', {'uid':local}),
                    ('/api/codex-sdk/turn', {'uid':local, 'text':'must not send'}),
                    ('/api/session/conversation', {'uid':local, 'draft':{}}),
                    ('/api/session/star', {'uid':local, 'starred':True}),
                    ('/api/session/nest', {'uid':local, 'parent_uid':source.uid('unrelated')}),
                    ('/api/sessions/delete', {'uids':[local]}),
                    ('/api/session/clone/plan', {'uid':local, 'mode':'move', 'target_node':b.nid})]
        for node in (a, b):
            for path, body in commands:
                denied = page.request.post(f'http://127.0.0.1:{node.port}'+path, headers=GOOD, data=body, timeout=5000)
                assert denied.status == 409 and denied.json()['code'] == 'move_session_locked', (node.name,path,denied.text())
        denied = page.request.post(base+'/api/session/star', data={'uid':local, 'starred':True})
        assert denied.status == 409
        allowed = page.request.post(base+'/api/session/star', data={'uid':source.uid('unrelated'), 'starred':True})
        assert allowed.ok, allowed.text()
        for path, relation in (('/api/session/nest', 'parent_uid'),
                               ('/api/session/predecessor', 'predecessor_uid'),
                               ('/api/codex-sdk/create', 'continuation_uid')):
            denied = page.request.post(huburl+path, data={
                'uid':scoped(b.nid,source.uid('unrelated')),
                relation:scoped(a.nid,local), '_node':b.nid})
            assert denied.status == 409 and denied.json()['code'] == 'move_session_locked', (path,denied.text())
        page.set_viewport_size({'width':1280, 'height':900})
        page.locator(f'#side .item[data-uid="{scoped(a.nid,local)}"]').click(button='right')
        expect(page.locator('#item-menu [data-act="clone"]')).to_have_attribute('aria-disabled','true')
        page.screenshot(path=str(args.screenshots/'frozen-menu.png'), full_page=True)
        page.locator('#item-menu').press('Escape')
        page.locator(f'#side .item[data-uid="{scoped(a.nid,local)}"]').click()
        page.get_by_role('button',name='对话',exact=True).click()
        expect(page.locator('#msgs')).to_contain_text('Branch A')
        page.locator('#a-more').click()
        expect(page.locator('#a-clone-group')).to_have_attribute('aria-disabled','true')
        expect(page.locator('#a-session-action')).to_have_attribute('aria-disabled','true')
        page.screenshot(path=str(args.screenshots/'frozen-history.png'), full_page=True)
        page.keyboard.press('Escape')
        peer.call('stop')
        peer.call('start')
        denied = page.request.post(f'http://127.0.0.1:{b.port}/api/session/star', headers=GOOD, data={'uid':local,'starred':True})
        assert denied.status == 409, denied.text()
        print('PASS durable acceptance, idempotency, close without cancel, both-node mutation fences and target restart', flush=True)

        # No page, no progress polling, and a Hub restart. Only backend recovery
        # can advance this accepted operation to completion.
        context.close()
        hub.stop()
        hub.start()
        proxy.blocked = False
        deadline = time.monotonic()+90
        while time.monotonic() < deadline:
            saved = json.loads(journal.read_text())
            if saved['phase'] == 'complete': break
            time.sleep(.2)
        assert saved['phase'] == 'complete', saved.get('error')
        assert not source.paths['a'].exists()
        context = browser.new_context(viewport={'width':1280,'height':900}, service_workers='block')
        stack.callback(context.close)
        page = context.new_page()
        page.goto(huburl,wait_until='networkidle')
        assert page.request.get(huburl+'/api/session/frozen').json()['uids'] == []
        print('PASS backend completes accepted move with all pages closed across Hub restart', flush=True)

        # Explicit cancellation is durable too; a restart must not turn it into
        # another execution when the source reconnects.
        proxy.blocked = True
        task, dialog = begin(saved['result']['target_uid'], a)
        request = task['request']
        journal = hubroot/'transfers'/f'{request["operation_id"]}.json'
        proxy.block_cancel = True
        dialog.locator('.transfer-abort').click()
        deadline = time.monotonic()+10
        while time.monotonic() < deadline and not json.loads(journal.read_text()).get('cancel_requested'):
            page.wait_for_timeout(50)
        assert json.loads(journal.read_text())['cancel_requested']
        context.close()
        hub.stop()
        hub.start()
        proxy.blocked = False
        proxy.block_cancel = False
        deadline = time.monotonic()+45
        while time.monotonic() < deadline:
            saved = json.loads(journal.read_text())
            if saved['phase'] == 'aborted': break
            time.sleep(.2)
        assert saved['phase'] == 'aborted', saved.get('error')
        assert not source.paths['a'].exists(), 'cancelled move-back must retain target ownership'
        print('PASS explicit cancellation survives Hub restart and releases fences without moving history', flush=True)


if __name__ == '__main__':
    main()
