#!/usr/bin/env python3
"""Kiro official catalog scopes, legacy UID migration and deletion across restart.

Private fake CLI peers share native SIDs but use different account homes. Browser
navigation must keep their cached histories and durable creation receipts apart.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
import shutil
from pathlib import Path
from types import SimpleNamespace
import tempfile
import threading
from urllib.parse import quote

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import server
from history_fixtures import BINARY
from hub_fixtures import FakeNode, Hub
from kiro_sdk_fixtures import KiroFixture
from kiro_sdk_browser import select_sdk
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize


def api(page, method, payload):
    result = page.evaluate("""async ({method,payload}) => {
      const r=await fetch('/api/native-sdk/'+method,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)});
      return {status:r.status,body:await r.json()};
    }""", {"method": method, "payload": payload})
    assert result["status"] == 200 and result["body"].get("ok") is not False, result
    return result["body"]


def remove(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').click(button="right")
    page.locator('#item-menu [data-act="delete"]').click()
    popup = page.locator('dialog.app-popup')
    expect(popup).to_contain_text('Kiro 会话会通过原生接口永久删除')
    popup.get_by_role('button', name='确定', exact=True).click()
    page.wait_for_function('uid => !S.sessions.some(row=>row.uid===uid)', arg=uid)



def hub_aliases(browser, binary, root, alias):
    hub_root = root / 'hub'
    hub_root.mkdir()
    nodes = [FakeNode('a' * 32, 'alias-a'), FakeNode('b' * 32, 'alias-b')]
    hub = None
    try:
        for i, node in enumerate(nodes):
            row = node.state()['row']
            node.set(row={**row, 'source':'kiro', 'uid':f'kiro:canonical-{i}', 'aliases':[alias]})
        hub = Hub(binary.with_name('sessiondock-hub'), hub_root, nodes)
        hub.start()
        base = f'http://127.0.0.1:{hub.port}'
        cases = [(alias, '', None), (alias, nodes[0].nid, 0),
                 (alias.replace('kiro:', f'kiro:{nodes[1].nid}~'), '', 1),
                 (alias.replace('kiro:', f'kiro:{nodes[1].nid}~'), nodes[0].nid, None)]
        for spec, node, selected in cases:
            context = browser.new_context(service_workers='block')
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base+'/') else route.abort())
            page = context.new_page()
            page.goto(base + '/?sid=' + quote(spec) + ('&node=' + node if node else ''), wait_until='domcontentloaded')
            page.wait_for_function('T.listLoaded && S.sessions.length===2')
            expected = f'kiro:{nodes[selected].nid}~canonical-{selected}' if selected is not None else None
            assert page.evaluate('spec=>uidOfDeepLink(spec)', spec) == expected
            if expected:
                expect(page.locator(f'.dhead[data-uid="{expected}"]')).to_be_visible()
            else:
                assert page.evaluate('S.sel') is None, 'Ambiguous/conflicting alias selected a node'
            context.close()
    finally:
        if hub:
            hub.stop()
        for node in nodes:
            node.stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-kiro-catalog-') as temporary:
        root = Path(temporary)
        for source in ('claude', 'codex', 'grok', 'state', 'trash', 'work'):
            (root / source).mkdir()
        binary = args.binary.resolve(strict=True)
        peers = [KiroFixture(root / name, binary.with_name('ptyhost')) for name in ('account-a', 'account-b')]
        for peer in peers:
            initialize('--initialize-lifecycle', peer.root / 'ledger', binary)
        a, b = peers
        legacy_uid, alias = 'kiro:legacycatalog001', 'kiro:legacycatalog002'
        legacy = [{'source':'kiro', 'uid':uid, 'sid':a.sid, 'cwd':str(a.work)} for uid in (legacy_uid, alias)]
        (root / 'state/kiro-legacy-identities.json').write_text(json.dumps(legacy))

        with sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            @contextmanager
            def account(peer):
                extra = {**peer.env, 'SESSIONDOCK_NATIVE_SDKS':'auto',
                    'SESSIONDOCK_LIFECYCLE_DIR':str(peer.root / 'ledger'),
                    'SESSIONDOCK_PTYHOST_DIR':str(peer.host_dir),
                    'SESSIONDOCK_STATE_DIR':str(root / 'state'),
                    'SESSIONDOCK_TRASH_DIR':str(root / 'trash')}
                with server(SimpleNamespace(root=root), binary, 'off', extra) as base:
                    context = browser.new_context(viewport={'width':1280, 'height':900}, service_workers='block')
                    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base+'/') else route.abort())
                    page = context.new_page()
                    page.set_default_timeout(15000)
                    page.goto(base, wait_until='domcontentloaded')
                    page.wait_for_function('T.listLoaded')
                    page.wait_for_function("async sid=>{await loadSessions(true);return S.sessions.some(r=>r.sid===sid)}", arg=peer.empty_sid, polling=300)
                    try:
                        yield page
                    finally:
                        context.close()

            create = {'source':'kiro','cwd':str(root/'work'),'request_id':'catalog-stable-create'}
            try:
                with account(a) as page:
                    row = page.evaluate('sid=>S.sessions.find(r=>r.sid===sid)', a.sid)
                    assert row['uid'] == legacy_uid, row
                    assert sum(r['sid'] == a.sid for r in page.evaluate('S.sessions')) == 1
                    select_sdk(page, legacy_uid)
                    with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/open') and r.request.method == 'POST') as opened:
                        page.locator('#sdk-input').click()
                    assert opened.value.ok, opened.value.text()
                    page.locator('#sdk-input').fill('ACCOUNT_A_CACHED_MARKER')
                    page.locator('#sdk-send').click()
                    expect(page.locator('#msgs')).to_contain_text('Kiro ACP V3 final answer for: ACCOUNT_A_CACHED_MARKER')
                    loads = len(a.received('session/load'))
                    assert loads == 1, 'The first send did not load exactly one native session'
                    alias_open = api(page, 'open', {'uid':alias})
                    assert alias_open['uid'] == legacy_uid, alias_open
                    assert len(a.received('session/load')) == loads, 'Alias created a second SDK actor'
                    api(page, 'stop', {'uid':legacy_uid})
                    page.reload(wait_until='domcontentloaded')
                    page.wait_for_function('T.listLoaded')
                    # A busy native stream replaces the app-owned projection
                    # while the browser opens SDK and reads/saves its draft.
                    # Identity must come from the official catalog, not this cache.
                    messages = Path(row['path']) / 'messages.jsonl'
                    original = messages.read_bytes() if messages.exists() else b''
                    record = b''.join(json.dumps({'format':'sessiondock-kiro-projection-v1',
                        'item':{'id':f'busy-history-{i}','role':'assistant','text':'x' * 1000}}).encode() + b'\n'
                        for i in range(5000))
                    done = threading.Event()
                    def stream_projection():
                        while not done.is_set():
                            temporary = messages.with_suffix('.streaming')
                            temporary.write_bytes(record)
                            temporary.replace(messages)
                            done.wait(.002)
                    writer = threading.Thread(target=stream_projection)
                    writer.start()
                    try:
                        select_sdk(page, legacy_uid)
                        page.locator('#sdk-input').fill('DRAFT_DURING_HISTORY_STREAM')
                        statuses = page.evaluate("""async uid => {
                          const statuses=[];
                          for(let i=0;i<12;i++) {
                            const r=await fetch('/api/session/conversation?uid='+encodeURIComponent(uid));
                            statuses.push(r.status); await r.text();
                          }
                          return statuses;
                        }""", legacy_uid)
                        assert statuses == [200] * 12, statuses
                        page.wait_for_function("""async uid => {
                          const r=await fetch('/api/session/conversation?uid='+encodeURIComponent(uid));
                          return r.ok && JSON.stringify(await r.json()).includes('DRAFT_DURING_HISTORY_STREAM');
                        }""", arg=alias)
                    finally:
                        done.set()
                        writer.join()
                        messages.write_bytes(original)
                    page.goto(page.url.split('?')[0] + '?sid=' + quote(alias), wait_until='domcontentloaded')
                    expect(page.locator(f'.dhead[data-uid="{legacy_uid}"]')).to_be_visible()
                    expect(page.locator('#msgs')).to_contain_text('ACCOUNT_A_CACHED_MARKER')
                    first = api(page, 'create', create)
                    api(page, 'stop', {'uid':first['uid']})
                    for changed in ({'mode':'autopilot'}, {'autopilot':True}):
                        conflict = page.evaluate("""async payload => {
                          const r=await fetch('/api/native-sdk/create',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)});
                          return {status:r.status,body:await r.json()};
                        }""", {**create, **changed})
                        assert conflict['status'] != 200 and '运行模式' in conflict['body'].get('error',''), conflict
                        assert len(a.received('session/new')) == 1
                    page.evaluate('loadSessions(true)')
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots/'account-a-history.png'), full_page=True)

                with account(b) as page:
                    row = page.evaluate('sid=>S.sessions.find(r=>r.sid===sid)', b.sid)
                    b_uid = row['uid']
                    assert b_uid != legacy_uid, row
                    assert not any(r['uid'] == first['uid'] for r in page.evaluate('S.sessions'))
                    page.locator(f'#side .item[data-uid="{b_uid}"]').click()
                    expect(page.locator('#msgs')).not_to_contain_text('ACCOUNT_A_CACHED_MARKER')
                    assert not b.received('session/load'), 'Listing another scope loaded a native session'
                    second = api(page, 'create', create)
                    assert second['uid'] != first['uid'], (first, second)
                    assert len(b.received('session/new')) == 1
                    api(page, 'stop', {'uid':second['uid']})
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots/'account-b-isolated.png'), full_page=True)

                # Returning to the first account preserves UID, history and the original creation receipt.
                with account(a) as page:
                    row = page.evaluate('sid=>S.sessions.find(r=>r.sid===sid)', a.sid)
                    assert row['uid'] == legacy_uid
                    page.locator(f'#side .item[data-uid="{legacy_uid}"]').click()
                    expect(page.locator('#msgs')).to_contain_text('ACCOUNT_A_CACHED_MARKER')
                    repeated = api(page, 'create', create)
                    assert repeated['uid'] == first['uid'], (first, repeated)
                    assert len(a.received('session/new')) == 1
                    projection = Path(row['path'])
                    backup = root / 'deleted-projection-backup'
                    shutil.copytree(projection, backup)
                    remove(page, legacy_uid)
                    assert a.received('session/delete')[-1]['params']['sessionId'] == a.sid

                # Simulate a crash after durable native-delete/tombstone commit but
                # before removal of the app-owned projection directory.
                shutil.copytree(backup, projection)
                # A tombstone remains authoritative even though the legacy identity receipt remains on disk.
                with account(a) as page:
                    assert not projection.exists(), 'Restart left a tombstoned projection visible'
                    rows = page.evaluate('S.sessions')
                    assert all(r['sid'] != a.sid and r['uid'] not in (legacy_uid, alias) for r in rows), rows
                    assert json.loads((root/'state/kiro-legacy-identities.json').read_text()) == legacy
                    assert len(a.received('session/new')) == 1
                hub_aliases(browser, binary, root, alias)
                print('PASS Kiro catalog browser: same SID isolated by account, legacy UID retained, cached history scoped, creation receipt replay scoped, delete tombstone survives restart', flush=True)
            finally:
                browser.close()
                stop_private_sdkhosts(root)


if __name__ == '__main__':
    main()
