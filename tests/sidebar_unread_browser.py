#!/usr/bin/env python3
"""Inactive views use unread summaries without fetching cached message bodies, node and hub."""
from browser_runtime import js
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import unquote
from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, claude_row, encoded, isolated_server
from hub_http_suite import Hub, free_port
from node_auth_suite import node_env, TOKEN


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-unread-') as tmp, sync_playwright() as pw:
        root=Path(tmp);data=Corpus(root/'node');rows={}
        for src in ('claude','codex','grok'):(data.root/src).mkdir(parents=True)
        for i in range(20):
            sid=f'unread-{i}'
            rows[sid]=[claude_row(sid,'user','u0',None,f'Session {i}'),
                       claude_row(sid,'assistant','a0','u0','Initial reply')]
            data.put(sid,'claude',rows[sid],[])
        nid='a'*32;ids=data.root/'ids';ids.mkdir();(ids/'node-id').write_text(nid+'\n')
        node=SimpleNamespace(name='synthetic',nid=nid,port=free_port(),token=TOKEN)
        hubroot=root/'hub';hubroot.mkdir()
        with ExitStack() as stack:
            base,_=stack.enter_context(isolated_server(data,args.binary,extra_env=node_env(data.root,node.port,'127.0.0.0/8')))
            hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node])
            hub.start();stack.callback(hub.stop)
            browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
            for hub_mode in (False,True):
                for sid,path in data.paths.items():path.write_bytes(b''.join(encoded(row) for row in rows[sid]))
                context=browser.new_context();page=context.new_page();errors=[]
                page.on('pageerror',lambda e:errors.append(str(e)))
                url=f'http://127.0.0.1:{hub.port}' if hub_mode else base
                page.goto(url,wait_until='networkidle');page.wait_for_function(js('S.sessions.length===20 && T.listLoaded', 'runtime.core.state.catalog.sessions.length===20 && runtime.terminal.state.listLoaded'))
                # Warm native anchors without putting messages in the browser cache.
                checkpoints={}
                for sid in rows:
                    uid=data.uid(sid)
                    if hub_mode:uid=uid.replace(':',f':{nid}~',1)
                    result=context.request.get(url+'/api/messages/'+uid).json()
                    checkpoints[uid]={'end':result['end'],'head':result['version']['head'],'anchor':result['anchor']}
                page.evaluate(js('''points=>{S.unread.clear();cache.clear();S.cursors=new Map(Object.entries(points));
                  S.view='date';S.nest=true;S.closed.clear();renderView();renderSide()}''', """points=>{runtime.core.state.unread.unread.clear();runtime.core.cache.cache.clear();runtime.core.state.unread.cursors=new Map(Object.entries(points));
                  runtime.core.state.sidebar.view='date';runtime.core.state.sidebar.nest=true;runtime.core.state.sidebar.closed.clear();runtime.sidebarView.renderView();runtime.sidebarView.renderSide()}"""),checkpoints)
                page.locator('#side>.group>.ghead').first.click()
                requests=[]
                page.on('request',lambda r:requests.append((r.method,r.url)))
                ordinal=1
                for sid,path in data.paths.items():
                    with path.open('ab') as f:f.write(encoded(claude_row(sid,'assistant',f'a{ordinal}',f'a{ordinal-1}','New reply')))
                context.request.get(url+'/api/sessions?force=1')
                page.evaluate(js('pollSessions()', 'runtime.core.list.pollSessions()'))
                page.wait_for_function(js('S.unread.size===20 && [...S.unread.values()].every(r=>r.count===1)', 'runtime.core.state.unread.unread.size===20 && [...runtime.core.state.unread.unread.values()].every(r=>r.count===1)'))
                batches=[u for method,u in requests if '/api/sessions/unread' in u]
                assert len(batches)==1,requests
                assert not any('/api/messages/' in u for _,u in requests),requests
                page.locator('#side>.group>.ghead').first.click()
                assert page.locator('#side .item-status.counted').count()==20
                # Broken checkpoints reset individually; another unknown UID
                # must not suppress a valid entry in the same batch.
                endpoint=url+(f'/api/nodes/{nid}' if hub_mode else '')+'/api/sessions/unread'
                checkpoint=checkpoints[next(u for u in checkpoints if u.endswith(data.uid('unread-1').split(':',1)[1]))]
                result=context.request.post(endpoint,data={'views':[
                    {'uid':data.uid('unread-0'),'start':999999,'head':'bad'},
                    {'uid':'claude:missing'},
                    {'uid':data.uid('unread-1'),'start':checkpoint['end'],'head':checkpoint['head'],'anchor':checkpoint['anchor']}]}).json()['results']
                assert result[0]['reset'] and result[0]['incoming']==0,result
                assert result[1]['status']==404,result
                assert not result[2]['reset'] and result[2]['incoming']==1,result
                # Hold a background summary while a real click opens that view.
                held=[]
                def hold(route): held.append(route)
                page.route('**/api/sessions/unread',hold)
                race_sid='unread-3'
                with data.paths[race_sid].open('ab') as f:
                    f.write(encoded(claude_row(race_sid,'assistant',f'race-{ordinal}',f'a{ordinal}','Race reply')))
                context.request.get(url+'/api/sessions?force=1');page.evaluate(js('pollSessions()', 'runtime.core.list.pollSessions()'))
                for _ in range(100):
                    if held: break
                    page.wait_for_timeout(20)
                assert held
                race_uid=data.uid(race_sid)
                if hub_mode: race_uid=race_uid.replace(':',f':{nid}~',1)
                page.locator(f'#side .item[data-uid="{race_uid}"] .t').click()
                page.wait_for_function(js('uid=>S.sel===uid && cache.has(uid)', 'uid=>runtime.core.state.selection.sel===uid && runtime.core.cache.cache.has(uid)'),arg=race_uid)
                before=page.evaluate(js('uid=>S.cursors.get(uid)', 'uid=>runtime.core.state.unread.cursors.get(uid)'),race_uid)
                for route in held: route.fulfill(response=route.fetch())
                page.unroute('**/api/sessions/unread',hold)
                page.wait_for_function(js('sidebarSyncing.size===0', 'runtime.core.unread.sidebarSyncing.size===0'))
                assert page.evaluate(js('uid=>S.cursors.get(uid)', 'uid=>runtime.core.state.unread.cursors.get(uid)'),race_uid)==before
                assert page.evaluate(js('uid=>!S.unread.has(uid)', 'uid=>!runtime.core.state.unread.unread.has(uid)'),race_uid)
                # Previously opened history stays cached but receives no background
                # body reads. Repeated updates count from the unread checkpoint,
                # not the deliberately stale cache end.
                def uid_for(sid):
                    uid=data.uid(sid)
                    return uid.replace(':',f':{nid}~',1) if hub_mode else uid
                cached_uid=uid_for('unread-0');other_uid=uid_for('unread-1')
                page.locator(f'#side .item[data-uid="{cached_uid}"] .t').click()
                page.wait_for_function(js('uid=>S.sel===uid && cache.has(uid)', 'uid=>runtime.core.state.selection.sel===uid && runtime.core.cache.cache.has(uid)'),arg=cached_uid)
                page.locator(f'#side .item[data-uid="{other_uid}"] .t').click()
                page.wait_for_function(js('uid=>S.sel===uid && cache.has(uid)', 'uid=>runtime.core.state.selection.sel===uid && runtime.core.cache.cache.has(uid)'),arg=other_uid)
                old_end=page.evaluate(js('uid=>cache.get(uid).end', 'uid=>runtime.core.cache.cache.get(uid).end'),cached_uid)
                requests.clear()
                for n in (1,2):
                    with data.paths['unread-0'].open('ab') as f:
                        f.write(encoded(claude_row('unread-0','assistant',f'lazy-{n}',
                            'a1' if n==1 else 'lazy-1',f'Lazy cached reply {n}')))
                    context.request.get(url+'/api/sessions?force=1');page.evaluate(js('pollSessions()', 'runtime.core.list.pollSessions()'))
                    page.wait_for_function(js('arg=>S.unread.get(arg.uid)?.count===arg.n', 'arg=>runtime.core.state.unread.unread.get(arg.uid)?.count===arg.n'),arg={'uid':cached_uid,'n':n})
                    page.evaluate(js('syncSidebarUpdates(S.sessions)', 'runtime.core.unread.syncSidebarUpdates(runtime.core.state.catalog.sessions)'))
                    page.wait_for_function(js('sidebarSyncing.size===0', 'runtime.core.unread.sidebarSyncing.size===0'))
                    assert page.evaluate(js('uid=>S.unread.get(uid).count', 'uid=>runtime.core.state.unread.unread.get(uid).count'),cached_uid)==n
                    assert page.evaluate(js('uid=>cache.get(uid).end', 'uid=>runtime.core.cache.cache.get(uid).end'),cached_uid)==old_end
                assert not any('/api/messages/'+cached_uid in unquote(u) for _,u in requests),requests
                page.locator(f'#side .item[data-uid="{cached_uid}"] .t').click()
                page.wait_for_function("document.querySelector('#msgs').innerText.includes('Lazy cached reply 2')")
                assert any('/api/messages/'+cached_uid in unquote(u) for _,u in requests),requests
                assert page.evaluate(js('uid=>!S.unread.has(uid)', 'uid=>!runtime.core.state.unread.unread.has(uid)'),cached_uid)
                # A second event can arrive while the first summary is in flight.
                # Capture the first response before appending again; releasing it
                # must drain the pending cursor without needing a third event.
                pending_uid=uid_for('unread-2')
                page.evaluate(js('uid=>{closeUiEvents();clearUnread(uid)}', 'uid=>{runtime.core.events.closeUiEvents();runtime.core.status.clearUnread(uid)}'),pending_uid)
                first_summary=[]
                def hold_first_summary(route):
                    if not first_summary:
                        first_summary.append((route,route.fetch()))
                    else:
                        route.continue_()
                page.route('**/api/sessions/unread',hold_first_summary)
                with data.paths['unread-2'].open('ab') as f:
                    f.write(encoded(claude_row('unread-2','assistant','pending-1','a1','Pending first')))
                context.request.get(url+'/api/sessions?force=1');page.evaluate(js('pollSessions()', 'runtime.core.list.pollSessions()'))
                for _ in range(100):
                    if first_summary:break
                    page.wait_for_timeout(20)
                assert first_summary
                with data.paths['unread-2'].open('ab') as f:
                    f.write(encoded(claude_row('unread-2','assistant','pending-2','pending-1','Pending second')))
                update=context.request.get(url+'/api/sessions?force=1').json()
                row=next(row for row in update['sessions'] if row['uid']==pending_uid)
                page.evaluate(js('row=>syncSidebarUpdates([row])', 'row=>runtime.core.unread.syncSidebarUpdates([row])'),row)
                assert page.evaluate(js('uid=>sidebarPendingCursors.has(uid)', 'uid=>runtime.core.unread.sidebarPendingCursors.has(uid)'),pending_uid)
                route,response=first_summary[0];route.fulfill(response=response)
                page.wait_for_function(js('uid=>S.unread.get(uid)?.count===2 && !sidebarSyncing.has(uid)', 'uid=>runtime.core.state.unread.unread.get(uid)?.count===2 && !runtime.core.unread.sidebarSyncing.has(uid)'),arg=pending_uid)
                assert page.evaluate(js('uid=>!sidebarPendingCursors.has(uid)', 'uid=>!runtime.core.unread.sidebarPendingCursors.has(uid)'),pending_uid)
                page.unroute('**/api/sessions/unread',hold_first_summary)
                page.evaluate(js('startUiEvents()', 'runtime.core.events.startUiEvents()'))
                # Old-node fallback never downloads inactive message bodies.
                page.route('**/api/sessions/unread',lambda r:r.fulfill(status=404,json={'error':'old node'}))
                fallback_sid='unread-4'
                with data.paths[fallback_sid].open('ab') as f:
                    f.write(encoded(claude_row(fallback_sid,'assistant',f'fallback-{ordinal}',f'a{ordinal}','Fallback reply')))
                fallback_uid=data.uid(fallback_sid)
                if hub_mode: fallback_uid=fallback_uid.replace(':',f':{nid}~',1)
                context.request.get(url+'/api/sessions?force=1');page.evaluate(js('pollSessions()', 'runtime.core.list.pollSessions()'))
                page.wait_for_function(js('unreadBatchUnsupported.size===1 && sidebarSyncing.size===0', 'runtime.core.unread.unreadBatchUnsupported.size===1 && runtime.core.unread.sidebarSyncing.size===0'))
                assert page.evaluate(js('uid=>S.unread.get(uid)?.count', 'uid=>runtime.core.state.unread.unread.get(uid)?.count'),fallback_uid)==1
                assert not any('/api/messages/'+fallback_uid in unquote(u) for _,u in requests),requests
                # A normal user click still opens full history after summary sync.
                page.locator(f'#side .item[data-uid="{fallback_uid}"] .t').click()
                page.wait_for_function(js('uid=>S.sel===uid && cache.has(uid)', 'uid=>runtime.core.state.selection.sel===uid && runtime.core.cache.cache.has(uid)'),arg=fallback_uid)
                assert 'Fallback reply' in page.locator('#msgs').inner_text()
                assert not errors,errors
                context.close()
        print('PASS sidebar unread: node+hub; 20 folded views -> one summary; cached inactive history stays lazy; reopen catches up; race/reset/old-node fallback')


if __name__=='__main__':main()
