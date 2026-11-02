#!/usr/bin/env python3
"""Opt-in Chromium move of a complex mixed native family across private hosts."""
# run_validation: skip
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, isolated_server
from session_clone_browser import prepare
from session_files_browser import fixture, uid
from session_bundle_browser import Peer, rejected_bundles
from session_transfer_browser import ident
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN
from transfer_ui import select_target, seed_names


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--peer', required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-mixed-move-') as temporary, sync_playwright() as pw, ExitStack() as stack:
        root = Path(temporary)
        source = prepare(root/'source')
        roots, claude, side, agent = fixture(root/'files')
        roots['codex'] = str(source.root/'codex')
        title, _, _ = seed_names(source.root/'codex', ident(2))
        metadata = source.root/'state/session-metadata.json'
        saved = json.loads(metadata.read_text())
        selected = uid('claude', claude[2])
        grok_uid = uid('grok', root/'files/grok/project'/ident(10))
        saved['sessions'][selected] = {'nest_parent': {'source':'codex','sid':ident(2)}, 'nest_initialized':True}
        saved['sessions'][grok_uid] = {'nest_parent': {'source':'claude','sid':ident(2)}, 'nest_initialized':True}
        metadata.write_text(json.dumps(saved))
        a = SimpleNamespace(name='source', nid='a'*32, port=free_port(), token=TOKEN)
        b = SimpleNamespace(name='destination', nid='b'*32, port=free_port(), token=TOKEN)
        (source.root/'ids').mkdir(); (source.root/'ids/node-id').write_text(a.nid+'\n')
        (source.root/'trash').mkdir()
        peer = Peer(args.peer, root, source, roots, b, args.binary)
        stack.callback(peer.close)
        peer.call('seed_cwd', path=str(root/'files/cwd'), entries=[{'relative':'','kind':'directory','mode':0o700}])
        peer.call('start')
        env = node_env(source.root, a.port, '127.0.0.0/8')
        env.update({f'SESSIONDOCK_{kind.upper()}_ROOT':path for kind,path in roots.items()})
        env['SESSIONDOCK_PROC_ROOT'] = str(source.root/'proc')
        env['SESSIONDOCK_CODEX_INDEX'] = str(source.root/'codex/session_index.jsonl')
        stack.enter_context(isolated_server(source,args.binary,state_dir=source.root/'state',
            trash_dir=source.root/'trash',extra_env=env))
        hubroot = root/'hub'; hubroot.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [a,b])
        hub.start(); stack.callback(hub.stop)
        browser = pw.chromium.launch(headless=True); stack.callback(browser.close)
        page = browser.new_page(); page.set_default_timeout(30000)
        page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
        anchor = scoped(a.nid, selected)
        page.locator(f'#side .item[data-uid="{anchor}"]').click(button='right')
        page.locator('#item-menu [data-act="clone"]').click()
        dialog = page.locator('#clone-group-dialog')
        expect(dialog.locator('.transfer-tree tbody tr')).to_have_count(14)
        with page.expect_response('**/api/session/clone/plan') as planned:
            select_target(dialog, b.nid)
        assert planned.value.ok, planned.value.text()
        plan = planned.value.json()
        assert plan['new_ids'] is False and plan['mode'] == 'move'
        assert {row['source'] for row in plan['sessions']} == {'codex','claude','grok'}
        shots = Path('target/mixed-move-review'); shots.mkdir(parents=True, exist_ok=True)
        for width, theme in ((1280, 'light'), (390, 'dark')):
            page.set_viewport_size({'width':width,'height':900}); page.emulate_media(color_scheme=theme)
            page.screenshot(path=str(shots/f'tree-{width}-{theme}.png'),full_page=True)
            dialog.locator('.transfer-table-scroll').hover(); page.mouse.wheel(0,1000)
            page.wait_for_function("document.querySelector('#clone-group-dialog .transfer-table-scroll').scrollTop > 0")
            page.screenshot(path=str(shots/f'tree-bottom-{width}-{theme}.png'),full_page=True)
        page.set_viewport_size({'width':1280,'height':900}); page.emulate_media(color_scheme='light')
        # Root mismatch and malformed archives must fail before publishing native
        # files, then the very same confirmed operation can still complete.
        rejected_bundles(a,b,plan['operation_id'])
        expect(dialog.locator('.clone-confirm')).to_be_enabled()
        with page.expect_response('**/api/session/transfer/clone',timeout=90000) as completed:
            dialog.locator('.clone-confirm').click()
        assert completed.value.ok, completed.value.text()
        result = completed.value.json()
        assert result['phase']=='complete' and result['new_ids'] is False
        expect(dialog).not_to_be_visible(timeout=30000)
        assert source.paths['unrelated'].exists()
        assert not source.paths['a'].exists() and not claude[2].exists()
        assert json.loads(peer.read(source.paths['a']).splitlines()[0])['payload']['id'] == ident(2)
        assert json.loads(peer.read(root/'files/grok/project'/ident(10)/'summary.json'))
        assert peer.read(side)
        target_metadata = json.loads(peer.read(root/'destination/state/session-metadata.json'))['sessions']
        assert target_metadata[source.uid('a')]['group'] == '待办'
        assert target_metadata[source.uid('a')]['starred'] is True
        assert target_metadata[selected]['nest_parent'] == {'source':'codex','sid':ident(2)}
        assert target_metadata[grok_uid]['nest_parent'] == {'source':'claude','sid':ident(2)}
        audit = root/'audit'; audit.mkdir()
        for name in ('state_5.sqlite','thread_history_1.sqlite','goals_1.sqlite'):
            (audit/name).write_bytes(peer.read(source.root/'codex'/name))
        with sqlite3.connect(audit/'state_5.sqlite') as database:
            assert database.execute('SELECT name FROM threads WHERE id=?',(ident(2),)).fetchone()[0] == 'Native a'
            for table in ('thread_attachments','thread_artifacts'):
                assert database.execute(f'SELECT count(*) FROM {table} WHERE thread_id=?',(ident(2),)).fetchone()[0] == 1
        with sqlite3.connect(audit/'goals_1.sqlite') as database:
            row = database.execute('SELECT status,tokens_used,time_used_seconds FROM thread_goals WHERE thread_id=?',(ident(2),)).fetchone()
            assert row == ('blocked',123,45),row
        with sqlite3.connect(audit/'thread_history_1.sqlite') as database:
            assert database.execute('SELECT count(*) FROM thread_realtime_items WHERE thread_id=?',(ident(2),)).fetchone()[0] == 5
        with sqlite3.connect(source.root/'codex/state_5.sqlite') as database:
            assert database.execute('SELECT count(*) FROM threads WHERE id=?',(ident(8),)).fetchone()[0] == 1
            assert database.execute('SELECT count(*) FROM threads WHERE id=?',(ident(2),)).fetchone()[0] == 0
        peer.call('stop'); peer.call('start'); hub.stop(); hub.start()
        page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
        row = page.locator(f'#side .item[data-uid="{scoped(b.nid,source.uid("a"))}"]')
        expect(row).to_contain_text(title)
        print('PASS mixed fixed-ID move: native Codex goals/realtime/attachments/names, Claude subagents/results, Grok dependencies; source cleanup, malformed bundle rejection, nesting and restart',flush=True)


if __name__ == '__main__':
    main()
