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
import time
from urllib.parse import quote

from playwright.sync_api import expect, sync_playwright

from browser_runtime import wait_for_async
from codex_sdk_browser import server
from history_fixtures import BINARY
from hub_fixtures import FakeNode, Hub
from kiro_sdk_fixtures import KiroFixture
from kiro_sdk_browser import select_sdk
from runtime_layers_browser import rows as runtime_rows
from sdk_ui_fixtures import start_sdk
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


def wait_for(condition, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(.05)
    raise AssertionError(description)


def exercise_refresh(account, peer, root, uid, deleted_uid, projection):
    catalog = projection.parents[1] / 'catalog.json'
    hold = peer.sessions / 'list-hold-second'
    waiting = peer.sessions / 'list-second-waiting'
    fail = peer.sessions / 'list-fail-second'

    def change_title(path, title):
        file = path / 'session.json'
        data = json.loads(file.read_text())
        file.write_text(json.dumps({**data, 'title':title}))

    def file_stamp(path):
        stat = path.stat()
        return stat.st_ino, stat.st_mtime_ns

    def entries():
        return json.loads(catalog.read_text())['entries']

    def refresh_errors():
        return sum(row.get('event') == 'kiro.catalog_unavailable'
                   for row in runtime_rows(root / 'state/runtime-logs'))

    def summary_errors():
        return sum(row.get('event') == 'kiro.summary_unavailable'
                   and row.get('sid') == peer.empty_sid
                   for row in runtime_rows(root / 'state/runtime-logs'))

    def wait_for_title(page, sid, title, timeout=15000):
        wait_for_async(page, """async ({sid,title})=>{
          await loadSessions(true);
          return S.sessions.some(row=>row.sid===sid && row.title===title);
        }""", arg={'sid':sid, 'title':title}, timeout=timeout, polling=300)

    # Older catalog.json files can omit the defaulted fields.
    legacy = json.loads(catalog.read_text())
    legacy.pop('capabilities')
    for key in ('metadata', 'aliases', 'deleted', 'host_key'):
        legacy['entries'][peer.empty_sid].pop(key)
    catalog.write_text(json.dumps(legacy))
    primary = projection / 'summary.json'
    old_title = entries()[peer.sid]['metadata']['title']
    new_title = 'Synthetic Kiro refreshed first page'
    change_title(peer.v3_dir, new_title)
    hold.touch()
    fail.touch()
    try:
        with account(peer) as page:
            wait_for(waiting.exists, 'Kiro did not request the second page')
            assert entries()[peer.sid]['metadata']['title'] == old_title
            expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_contain_text(old_title)
            errors = refresh_errors()
            hold.unlink()
            wait_for(lambda: refresh_errors() > errors, 'Failed list request did not finish')
            assert entries()[peer.sid]['metadata']['title'] == old_title
            expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_contain_text(old_title)
    finally:
        hold.unlink(missing_ok=True)
        fail.unlink()
        waiting.unlink(missing_ok=True)

    # The peer captures both pages before the browser binds, renames and deletes.
    # Force one derived summary publication to fail after the catalog commit.
    confirmed_title = 'Kiro title confirmed during pagination'
    later_title = 'Kiro title from a later native listing'
    empty_title = 'Synthetic Kiro recovered second page'
    change_title(peer.empty_dir, empty_title)
    empty_summary = Path(entries()[peer.empty_sid]['path']) / 'summary.json'
    backup = empty_summary.with_suffix('.backup')
    deleted_sid = next(sid for sid, entry in entries().items() if entry['uid'] == deleted_uid)
    hold.touch()
    try:
        with account(peer) as page:
            wait_for(waiting.exists, 'Kiro did not pause the stale second page')
            select_sdk(page, uid)
            start_sdk(page)
            bound_key = entries()[peer.sid]['host_key']
            assert bound_key
            with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/rename')) as renamed:
                page.locator('#sdk-input').fill('/rename ' + confirmed_title)
                page.locator('#sdk-send').click()
            assert renamed.value.ok and renamed.value.json()['title'] == confirmed_title
            expect(page.locator('.dhead h2')).to_have_text(confirmed_title)
            assert entries()[peer.sid]['metadata']['title'] == confirmed_title
            remove(page, deleted_uid)
            assert entries()[deleted_sid]['deleted']
            empty_summary.rename(backup)
            empty_summary.mkdir()
            errors = summary_errors()
            hold.unlink()
            wait_for(lambda: summary_errors() > errors, 'Summary publication did not report its failure')
            wait_for_title(page, peer.sid, confirmed_title)
            expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_contain_text(confirmed_title)
            latest = entries()
            assert latest[peer.sid]['metadata']['title'] == confirmed_title
            assert json.loads(primary.read_text())['session']['title'] == confirmed_title
            assert latest[peer.empty_sid]['metadata']['title'] == empty_title
            assert latest[peer.sid]['host_key'] == bound_key
            assert latest[deleted_sid]['deleted'] and latest[deleted_sid]['host_key'] is None
            assert not Path(latest[deleted_sid]['path']).exists()
            assert not page.locator(f'#side .item[data-uid="{deleted_uid}"]').count()
            assert empty_summary.is_dir()
            api(page, 'stop', {'uid':uid})
            change_title(peer.v3_dir, later_title)
            wait_for_title(page, peer.sid, later_title, timeout=45000)
            assert entries()[peer.sid]['metadata']['title'] == later_title
            assert empty_summary.is_dir()
    finally:
        hold.unlink(missing_ok=True)
        waiting.unlink(missing_ok=True)

    # Restart with one summary still blocked and another stale.
    stale = json.loads(primary.read_text())
    stale['session']['title'] = stale['native']['title'] = old_title
    primary.write_text(json.dumps(stale))
    catalog_errors, projection_errors = refresh_errors(), summary_errors()
    fail.touch()
    try:
        with account(peer, wait_sid=peer.sid) as page:
            assert page.evaluate("async () => (await fetch('/api/health')).ok")
            expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_contain_text(later_title)
            assert entries()[deleted_sid]['deleted']
            assert empty_summary.is_dir()
            wait_for(lambda: summary_errors() > projection_errors, 'Startup did not report the blocked summary')
            wait_for(lambda: refresh_errors() > catalog_errors, 'Failed list request after restart did not finish')
            manifest_stamp, primary_stamp = file_stamp(catalog), file_stamp(primary)
            empty_summary.rmdir()
            backup.rename(empty_summary)
            wait_for_title(page, peer.empty_sid, empty_title, timeout=45000)
            assert file_stamp(catalog) == manifest_stamp
            assert file_stamp(primary) == primary_stamp
    finally:
        fail.unlink()


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
            def account(peer, *, wait_sid=None):
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
                    wait_for_async(page, "async sid=>{await loadSessions(true);return S.sessions.some(r=>r.sid===sid)}",
                                   arg=wait_sid or peer.empty_sid, polling=300)
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
                        start_sdk(page)
                    assert opened.value.ok, opened.value.text()
                    page.locator('#sdk-input').fill('ACCOUNT_A_CACHED_MARKER')
                    page.locator('#sdk-send').click()
                    expect(page.locator('.sdk-msgs')).to_contain_text('Kiro ACP V3 final answer for: ACCOUNT_A_CACHED_MARKER')
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
                    expect(page.locator('.sdk-msgs')).to_contain_text('ACCOUNT_A_CACHED_MARKER')
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
                    expect(page.locator('.sdk-msgs')).not_to_contain_text('ACCOUNT_A_CACHED_MARKER')
                    assert not b.received('session/load'), 'Listing another scope loaded a native session'
                    second = api(page, 'create', create)
                    assert second['uid'] != first['uid'], (first, second)
                    assert len(b.received('session/new')) == 1
                    api(page, 'stop', {'uid':second['uid']})
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots/'account-b-isolated.png'), full_page=True)

                exercise_refresh(account, b, root, b_uid, second['uid'], Path(row['path']))

                # Returning to the first account preserves UID, history and the original creation receipt.
                with account(a) as page:
                    row = page.evaluate('sid=>S.sessions.find(r=>r.sid===sid)', a.sid)
                    assert row['uid'] == legacy_uid
                    page.locator(f'#side .item[data-uid="{legacy_uid}"]').click()
                    expect(page.locator('.sdk-msgs')).to_contain_text('ACCOUNT_A_CACHED_MARKER')
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
                print('PASS Kiro catalog browser: account isolation, legacy identity and receipts, concurrent rename/binding/tombstone, later native title, startup and retry with a blocked summary', flush=True)
            finally:
                browser.close()
                stop_private_sdkhosts(root)


if __name__ == '__main__':
    main()
