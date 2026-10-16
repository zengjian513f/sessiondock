#!/usr/bin/env python3
"""Real Chromium node/Hub deletion with private synthetic Agy picker and native files."""
import argparse
import json
import os
from pathlib import Path
import sqlite3
import shutil
import sys
import tempfile
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from agy_delete_fixtures import FAKE, IDS, seed
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from hub_fixtures import Hub, free_port
from send_browser import initialize
from agy_browser import process_alive


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path, default=REPO/'target/debug/sessiondock-hub')
    args=parser.parse_args()
    root=Path(tempfile.mkdtemp(prefix='sessiondock-agy-delete-'))
    print('ROOT',root,flush=True)
    for n in ('home','host','ledger','state','work','bin','claude','codex','grok','opencode','mirror','proc','hub'):
        (root/n).mkdir(mode=0o700)
    native=root/'native';seed(native,root/'work')
    with sqlite3.connect(native/'conversation_summaries.db') as c:
        deleted_row=c.execute('SELECT * FROM conversation_summaries WHERE conversation_id=?',(IDS[0],)).fetchone()
    fake=root/'bin/agy';fake.write_text('#!'+str(Path(sys.executable).resolve())+'\n'+FAKE);fake.chmod(0o700)
    env={'HOME':str(root/'home'),'TERM':'xterm-256color','PATH':'/usr/bin:/bin','LANG':'C.UTF-8',
         'DELETE_NATIVE':str(native),'DELETE_LOG':str(root/'calls.jsonl')}
    config=root/'launcher.json';config.touch(mode=0o600)
    config.write_text(json.dumps({'schema':2,'host_binary':str(REPO/'target/debug/ptyhost'),'host_dir':str(root/'host'),
        'profiles':[{'id':'agy-cli-v1','source':'agy','executable':str(fake),'args':[],'env':env}]}))
    initialize('--initialize-lifecycle',root/'ledger',args.binary.resolve())
    token_file=root/'node.token';token_file.touch(mode=0o600);token_file.write_text('t'*40+'\n');port=free_port()
    extra={'SESSIONDOCK_AGY_HOME':str(native),'SESSIONDOCK_AGY_ROOT':str(root/'mirror'),
        'SESSIONDOCK_PROC_ROOT':str(root/'proc'),'SESSIONDOCK_NODE_BIND':f'127.0.0.1:{port}',
        'SESSIONDOCK_NODE_TOKEN_FILE':str(token_file),'SESSIONDOCK_NODE_ID_FILE':str(root/'node.id'),
        'SESSIONDOCK_NODE_PEERS':'127.0.0.0/8'}
    def exists(sid):
        with sqlite3.connect(f'file:{native}/conversation_summaries.db?mode=ro',uri=True) as c:
            return bool(c.execute('SELECT 1 FROM conversation_summaries WHERE conversation_id=?',(sid,)).fetchone())
    with isolated_server(Corpus(root),args.binary,host_dir=root/'host',lifecycle_dir=root/'ledger',
            launcher_config=config,state_dir=root/'state',trash_dir=root/'trash',extra_env=extra) as (base,_), sync_playwright() as pw:
        options={'headless':True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):options['executable_path']=os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser=pw.chromium.launch(**options)
        hub=Hub(args.hub_binary,root/'hub',[SimpleNamespace(port=port,token='t'*40,name='Fixture')]);hub.start()
        try:
            page=browser.new_page(viewport={'width':1280,'height':900});page.set_default_timeout(35000)
            def open_sid(url,sid):
                page.goto(url);page.wait_for_function('sid => S.sessions.some(r=>r.sid===sid)',arg=sid)
                uid=page.evaluate('sid => S.sessions.find(r=>r.sid===sid).uid',sid)
                item=page.locator(f'#side .item[data-uid="{uid}"]')
                if not item.is_visible() and page.locator('#detail .mobile-back:visible').count():
                    page.locator('#detail .mobile-back:visible').click()
                item.click()
                expect(page.locator('#msgs')).to_contain_text('fixture '+sid)
                return uid
            def menu():
                if not page.locator('#a-session-action').is_visible():page.locator('#a-more').click()
                page.locator('#a-session-action').click()
                popup=page.locator('dialog.app-popup[open]');expect(popup).to_contain_text('Agy 会话会永久删除');return popup
            uid=open_sid(base,IDS[0])
            for width in (1280,390):
                page.set_viewport_size({'width':width,'height':900})
                for theme in ('light','dark'):
                    page.emulate_media(color_scheme=theme)
                    open_sid(base,IDS[0]);popup=menu()
                    page.screenshot(path=str(root/f'confirm-{width}-{theme}.png'))
                    popup.locator('[data-popup-action="cancel"]').click()
            page.set_viewport_size({'width':1280,'height':900})
            open_sid(base,IDS[0])
            assert exists(IDS[0]) and not (root/'calls.jsonl').exists()
            popup=menu()
            with page.expect_response(lambda r:r.request.method=='DELETE') as response:popup.locator('[data-popup-action="ok"]').click()
            assert response.value.status==200,response.value.text()
            assert not exists(IDS[0]) and exists(IDS[1])
            repeat=page.request.delete(base+'/api/session/'+uid)
            assert repeat.status==404,repeat.text()
            print('PASS cancel, same-title SID deletion, native files, repeated delete',flush=True)
            # Replay the first inconsistent production event: a stale catalog row
            # reappears after confirmed deletion, without either native payload.
            with sqlite3.connect(native/'conversation_summaries.db') as c:
                c.execute('INSERT INTO conversation_summaries VALUES (?,?,?,?,?,?,?,?)',deleted_row)
            page.wait_for_timeout(1600)
            page.reload();page.wait_for_function('T.listLoaded')
            assert not page.evaluate('sid => S.sessions.some(r=>r.sid===sid)',IDS[0])
            assert not (root/'mirror/cli'/IDS[0]).exists()
            assert exists(IDS[0]), 'the mirror must not rewrite native catalog data'
            print('PASS stale native index does not resurrect a confirmed deletion',flush=True)
            for mode,sid in [('wrong',IDS[1]),('refuse',IDS[2])]:
                (native/'mode').write_text(mode);open_sid(base,sid);popup=menu()
                with page.expect_response(lambda r:r.request.method=='DELETE',timeout=40000) as response:popup.locator('[data-popup-action="ok"]').click()
                assert response.value.status==500 and response.value.json()['code']=='agy_remove_failed',response.value.text()
                expect(page.locator('dialog.app-popup[open]')).to_contain_text('删除失败')
                page.locator('dialog.app-popup[open] [data-popup-action="ok"]').click()
                assert exists(sid) and (native/'conversations'/(sid+'.db')).exists()
                calls=[json.loads(l) for l in (root/'calls.jsonl').read_text().splitlines()]
                if mode=='wrong':assert not any(c.get('sid')==sid and c.get('input') in ('\x1bOS','y') for c in calls)
                assert not list((root/'host').glob('.agy-delete-*'))
                print('PASS',mode,'preserves native session and cleans worker',flush=True)
            (native/'mode').unlink()
            hub_url=f'http://127.0.0.1:{hub.port}'
            (native/'conversations'/(IDS[3]+'.db')).unlink()
            shutil.rmtree(native/'brain'/IDS[3])
            uid=open_sid(hub_url,IDS[3]);popup=menu()
            with page.expect_response(lambda r:r.request.method=='DELETE',timeout=40000) as response:popup.locator('[data-popup-action="ok"]').click()
            assert response.value.status==200,response.value.text()
            assert exists(IDS[3]) and exists(IDS[4])
            assert (root/'mirror/deleted'/IDS[3]).is_file()
            assert not (native/'conversations'/(IDS[3]+'.db')).exists()
            print('PASS actual Hub forwards scoped Agy deletion to owning node',flush=True)
            open_sid(hub_url,IDS[4])
            page.locator(f'#side .item[data-uid="{page.evaluate("S.sel")}"]').click(button='right')
            page.locator('#item-menu [data-act="pick"]').click()
            uid2=page.evaluate('sid => S.sessions.find(r=>r.sid===sid).uid',IDS[5])
            page.locator(f'#side .item[data-uid="{uid2}"] .t').click()
            page.locator('#side-pick-delete').click();popup=page.locator('dialog.app-popup[open]')
            expect(popup).to_contain_text('Agy 会话会永久删除')
            with page.expect_response(lambda r:'/api/sessions/delete' in r.url,timeout=40000) as response:popup.locator('[data-popup-action="ok"]').click()
            assert response.value.status==200,response.value.text()
            expect(page.locator('#detail')).to_contain_text('会话已从 Agy 删除')
            expect(page.locator('#detail-open-trash')).to_have_count(0)
            assert not exists(IDS[4]) and not exists(IDS[5])
            assert exists(IDS[1]) and exists(IDS[2])
            page.reload();page.wait_for_function('T.listLoaded')
            assert not list((root/'host').glob('.agy-delete-*'))
            calls=[json.loads(l) for l in (root/'calls.jsonl').read_text().splitlines()]
            assert all(not process_alive(c['pid']) for c in calls if 'start' in c)
            print('PASS Hub batch deletes both selected SIDs, keeps failures, refresh and worker cleanup',flush=True)
        finally:
            browser.close();hub.stop()
    with isolated_server(Corpus(root),args.binary,host_dir=root/'host',lifecycle_dir=root/'ledger',
            launcher_config=config,state_dir=root/'state',trash_dir=root/'trash',extra_env=extra) as (base,_), sync_playwright() as pw:
        browser=pw.chromium.launch(**options)
        try:
            page=browser.new_page();page.goto(base);page.wait_for_function('T.listLoaded')
            assert not page.evaluate('sid => S.sessions.some(r=>r.sid===sid)',IDS[0])
            # A real native payload appearing again overrides the old receipt.
            (native/'conversations'/(IDS[0]+'.db')).write_bytes(b'restored native fixture')
            page.wait_for_function('sid => S.sessions.some(r=>r.sid===sid)',arg=IDS[0],timeout=20000)
            uid=page.evaluate('sid => S.sessions.find(r=>r.sid===sid).uid',IDS[0])
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            expect(page.locator('#detail')).to_contain_text('仅找到 Agy 的会话索引')
            assert not (root/'mirror/deleted'/IDS[0]).exists()
            print('PASS deletion survives service restart; genuine native restoration becomes visible',flush=True)
        finally:
            browser.close()


if __name__=='__main__':main()
