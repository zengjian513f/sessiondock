"""Chromium move publication, large source retirement and restart recovery."""
# run_validation: serial
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import sqlite3
import tempfile
import time
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, isolated_server
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN, GOOD
from session_clone_browser import prepare
from session_transfer_browser import ident
from transfer_ui import select_target, submit_transfer, wait_transfer
from session_bundle_browser import Peer


def main():
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument('--binary',type=Path,default=BINARY);ap.add_argument('--peer',required=True);args=ap.parse_args()
    with tempfile.TemporaryDirectory(prefix='native-receipt-browser-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root=Path(tmp);source=prepare(root/'source');target=Corpus(root/'target')
        for corpus in (source,target):
            for folder in ('state','proc','ids','trash','codex','claude','grok'):(corpus.root/folder).mkdir(parents=True,exist_ok=True)
        # Several small native rows become one receipt larger than its bounded chunks.
        with sqlite3.connect(source.root/'codex/state_5.sqlite') as db:
            for i in range(48):
                db.execute('INSERT INTO thread_dynamic_tools VALUES (?,?,?,?,?)',(ident(2),i,'tool-'+str(i),'payload-'+('语句'*2048),'{}'))
        nodes=[SimpleNamespace(name=name,nid=letter*32,port=free_port(),token=TOKEN) for name,letter in [('source','a'),('destination','b')]]
        peer=Peer(args.peer,root,source,{kind:str(source.root/kind) for kind in ('claude','codex','grok')},nodes[1],args.binary)
        stack.callback(peer.close)
        def start_target():peer.call('start')
        def sql(path,query=None,script=None,params=(),legacy=None):
            return peer.call('receipt_fixture_sql',path=str(path),query=query,script=script,params=params,legacy=legacy).get('rows')
        for corpus,node in zip((source,target),nodes):(corpus.root/'ids/node-id').write_text(node.nid+'\n')
        stack.enter_context(isolated_server(source,args.binary,state_dir=source.root/'state',trash_dir=source.root/'trash',extra_env={**node_env(source.root,nodes[0].port,'127.0.0.0/8'),'SESSIONDOCK_PROC_ROOT':str(source.root/'proc'),'SESSIONDOCK_LOG_REQUESTS':'all'}))
        start_target()
        hubroot=root/'hub';hubroot.mkdir();hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,nodes);hub.start();stack.callback(hub.stop)
        browser=pw.chromium.launch(headless=True);stack.callback(browser.close);page=browser.new_page();page.set_default_timeout(30000)
        state_db=source.root/'codex/state_5.sqlite';history_db=source.root/'codex/thread_history_1.sqlite'
        sql(state_db,script="CREATE TABLE _sessiondock_clone_journal(operation_id TEXT PRIMARY KEY,receipt TEXT NOT NULL); CREATE TRIGGER fixture_receipt_limit BEFORE INSERT ON _sessiondock_clone_journal WHEN length(new.receipt)>65536 BEGIN SELECT RAISE(ABORT,'fixture SQLite single-value limit'); END;")
        def execute():
            page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
            uid=scoped(nodes[0].nid,source.uid('a'))
            page.locator(f'#side .item[data-uid="{uid}"]').click(button='right');page.locator('#item-menu [data-act="clone"]').click()
            dialog=page.locator('#clone-group-dialog');expect(dialog.locator('.transfer-tree input[data-uid]').first).to_be_attached()
            select_target(dialog,nodes[1].nid)
            try:expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=30000)
            except AssertionError:
                print('PLAN FAILURE',dialog.inner_text(),flush=True);raise
            return submit_transfer(page,dialog,f'http://127.0.0.1:{hub.port}',timeout=120)
        def empty_receipts():
            for path in (source.root/'codex').glob('*.sqlite'):
                for table in ('_sessiondock_clone_journal','_sessiondock_clone_journal_chunks'):
                    if sql(path,'SELECT 1 FROM sqlite_master WHERE name=?',params=[table]):assert sql(path,'SELECT count(*) FROM '+table)[0][0]==0,path
        def wait_recovered():
            deadline=time.monotonic()+30
            while True:
                try:
                    assert sql(state_db,'SELECT count(*) FROM threads')[0][0]==0
                    empty_receipts();break
                except AssertionError:
                    assert time.monotonic()<deadline,'native restart compensation did not finish';page.wait_for_timeout(100)
        # Fail after the state DB commits; hold compensation so its actual receipt
        # can be inspected and then recovered by a new target process.
        for legacy in (False,True):
            sql(history_db,script="CREATE TRIGGER fixture_fail BEFORE INSERT ON thread_items BEGIN SELECT RAISE(ABORT,'fixture publish failure'); END")
            sql(state_db,script="CREATE TRIGGER fixture_hold BEFORE DELETE ON threads BEGIN SELECT RAISE(ABORT,'fixture hold recovery'); END")
            response=execute();assert response['phase'] != 'complete',response;print('EXPECTED FAILURE',response.get('error'),flush=True)
            operation,raw=sql(state_db,'SELECT operation_id,receipt FROM _sessiondock_clone_journal')[0];header=json.loads(raw)
            assert header['chunked_receipt_version']==1 and header['chunks']>4,header
            parts=sql(state_db,'SELECT chunk,length(data) FROM _sessiondock_clone_journal_chunks WHERE operation_id=? ORDER BY chunk',params=[operation])
            assert [i for i,_ in parts]==list(range(header['chunks']))
            assert all(0<size<=65536 for _,size in parts)
            assert sql(state_db,'SELECT count(*) FROM thread_dynamic_tools')[0][0]==48
            if legacy:sql(state_db,legacy=operation)
            sql(state_db,script='DROP TRIGGER fixture_hold')
            assert source.paths['a'].is_file()
            sql(history_db,script='DROP TRIGGER fixture_fail')
            peer.call('stop');start_target();wait_recovered()
            wait_transfer(page,f'http://127.0.0.1:{hub.port}',response['request'],phases=('aborted',))
            dialog=page.locator('#clone-group-dialog')
            if dialog.is_visible():
                expect(dialog.locator('.clone-confirm')).to_have_text('迁移所选会话',timeout=30000)
                with page.expect_response(lambda r:r.url.endswith('/cancel'),timeout=30000) as cancelled:
                    dialog.locator('.clone-cancel').click()
                assert cancelled.value.ok,cancelled.value.text()
                expect(dialog).not_to_be_visible(timeout=30000)
            page.reload(wait_until='networkidle');page.wait_for_timeout(1500)
            print('PASS restart native rollback, '+('legacy receipt' if legacy else 'multi-chunk receipt'),flush=True)
        # The reported family had roughly 70k projection rows. A per-row scan
        # of the entire plan makes source retirement quadratic at this size.
        row_count=70000
        payload=json.dumps({'type':'message','role':'assistant','content':[]})
        with sqlite3.connect(history_db) as db:
            db.executemany('INSERT INTO thread_items VALUES (?,?,?,?)',
                ((ident(2),ident(20),f'bulk-{i:06}',payload) for i in range(row_count)))
            db.execute('INSERT INTO thread_items VALUES (?,?,?,?)',
                (ident(99),ident(20),'unrelated-keep',payload))
            # Fail after files reach trash, then exercise the same cleanup
            # transaction against missing, changed and newly added source rows.
            db.execute("CREATE TRIGGER fixture_retire_hold BEFORE DELETE ON thread_items BEGIN SELECT RAISE(ABORT,'fixture hold source cleanup'); END")
        response=execute();assert response['phase']=='cleanup_pending',response
        journal=next(p for p in (source.root/'state/transfers').glob('*/operation.json')
                     if json.loads(p.read_text())['phase']=='retiring')
        operation=journal.parent.name
        retire_url=f'http://127.0.0.1:{nodes[0].port}/api/session/transfer/retire'
        with sqlite3.connect(history_db) as db:
            db.execute("UPDATE thread_items SET item_json=? WHERE item_id='bulk-000000'",('concurrent change',))
        rejected=page.request.post(retire_url,headers=GOOD,data={'operation_id':operation})
        assert rejected.status==409 and rejected.json()['code']=='move_plan_stale',rejected.text()
        with sqlite3.connect(history_db) as db:
            assert db.execute("SELECT count(*) FROM thread_items WHERE thread_id=?",(ident(2),)).fetchone()[0]==row_count
            db.execute("UPDATE thread_items SET item_json=? WHERE item_id='bulk-000000'",(payload,))
            db.execute('INSERT INTO thread_items VALUES (?,?,?,?)',(ident(2),ident(20),'concurrent-new',payload))
        rejected=page.request.post(retire_url,headers=GOOD,data={'operation_id':operation})
        assert rejected.status==409 and rejected.json()['code']=='move_plan_stale',rejected.text()
        with sqlite3.connect(history_db) as db:
            db.execute('DROP TRIGGER fixture_retire_hold')
            db.execute("DELETE FROM thread_items WHERE item_id IN ('concurrent-new','bulk-000001')")
        # A previously removed row is allowed on retry; unknown/changed rows
        # above are not. Browser progress must observe autonomous completion.
        expect(page.locator('#clone-group-dialog')).not_to_be_visible(timeout=30000)
        page.get_by_role('button',name='对话',exact=True).click()
        expect(page.locator('#msgs')).to_contain_text('Branch A',timeout=30000)
        empty_receipts()
        assert sql(state_db,'SELECT count(*) FROM thread_dynamic_tools')[0][0]==48
        assert not source.paths['a'].exists()
        with sqlite3.connect(history_db) as db:
            assert db.execute('SELECT item_id FROM thread_items').fetchall()==[('unrelated-keep',)]
        assert sql(history_db,'SELECT count(*) FROM thread_items WHERE thread_id=?',params=[ident(2)])[0][0]==row_count
        assert sql(history_db,"SELECT item_json FROM thread_items WHERE item_id='bulk-000001'")[0][0]==payload
        assert json.loads(journal.read_text())['phase']=='retired'
        timings=[]
        for _ in range(100):
            timings=[json.loads(line)['ms'] for log in (source.root/'state/runtime-logs').glob('*.jsonl')
                     for line in log.read_text().splitlines()
                     if '"/api/session/transfer/retire"' in line]
            if timings:break
            page.wait_for_timeout(100)
        assert timings and max(timings)<20000,('source retirement exceeded 20s',timings)
        print(f'PASS 70k projection retirement, concurrent row protection, partial cleanup retry; retire requests {timings} ms',flush=True)
        print('PASS Chromium move publishes native rows and terminal cleanup removes all receipt chunks',flush=True)

if __name__=='__main__':main()
