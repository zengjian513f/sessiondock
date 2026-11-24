#!/usr/bin/env python3
"""Inactive views use unread summaries without fetching cached message bodies, node and hub."""

import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace
from urllib.parse import unquote
from playwright.sync_api import expect, sync_playwright
from agy_history_browser import SCHEMA, record, write_transcript
from history_fixtures import BINARY, Corpus, claude_row, encoded, isolated_server
from hub_fixtures import Hub, free_port
from node_auth_fixtures import node_env, TOKEN


def agy_unread(page, url, native, db, case, *, mobile, screenshot):
    ids = [f'a6000000-0000-4000-8000-{case * 3 + i:012d}' for i in (1, 2, 3)]
    paths = []
    with db:
        db.execute('DELETE FROM conversation_summaries')
        for sid, title in zip(ids, ('Agy 主会话', '检查实验结果', '复核历史记录')):
            paths.append(write_transcript(native, sid, [
                record(0, 'USER_INPUT', f'请继续：{title}'),
                record(1, 'PLANNER_RESPONSE', f'初始回复：{title}'),
            ]))
            db.execute('INSERT INTO conversation_summaries VALUES (?,?,?,?,?,?,?,?)',
                       (sid, title, '2026-10-04T00:00:01Z', json.dumps([native.parent.as_uri()]),
                        '' if sid == ids[0] else ids[0], 'CASCADE_RUN_STATUS_IDLE', 2, 'reviewer'))
    page.goto(url)
    uid = page.wait_for_function('sid => S.sessions.find(row => row.sid === sid)?.uid', arg=ids[0]).json_value()
    main = page.locator(f'#side .item[data-uid="{uid}"]')
    caret = main.locator('.nest-caret')
    child, sibling = [page.locator(f'#side .item[data-agent="{sid}"]') for sid in ids[1:]]
    messages = page.locator('#msgs')

    def show_list():
        if mobile:
            page.locator('.dhead .mobile-back').first.click()

    def open_row(row, message):
        row.locator('.t').click()
        expect(messages).to_contain_text(message)
        show_list()

    open_row(main, '初始回复：Agy 主会话')
    if not child.is_visible():
        caret.click()
    page.wait_for_function('''({uid, ids, ends}) => {
        const row = S.sessions.find(row => row.uid === uid);
        return row?.cursor?.end === ends[0] && ids.slice(1).every((id, i) =>
            row.agent_items?.find(item => item.id === id)?.cursor?.end === ends[i + 1]);
    }''', arg={'uid': uid, 'ids': ids, 'ends': [path.stat().st_size for path in paths]})
    open_row(child, '初始回复：检查实验结果')
    open_row(sibling, '初始回复：复核历史记录')
    open_row(main, '初始回复：Agy 主会话')

    # Fold the children before the real native append. The notification must
    # survive unfolding, and the main history must keep its original cursor.
    caret.click()
    expect(child).to_be_hidden()
    with paths[1].open('ab') as stream:
        stream.write(encoded(record(2, 'PLANNER_RESPONSE', '子代理的新回复')))
    caret.click()
    expect(child).to_be_visible()
    page.wait_for_function('''({uid, child, end}) =>
        S.cursors.get(viewKey(uid, child))?.end === end''',
        arg={'uid': uid, 'child': ids[1], 'end': paths[1].stat().st_size})
    if screenshot:
        page.screenshot(path=screenshot.with_name(screenshot.name + '-unread.png'))
    child_badge = child.locator('.item-status.counted')
    expect(child_badge).to_have_text('1')
    expect(main.locator('.item-status.counted')).to_have_count(0)
    expect(sibling.locator('.item-status.counted')).to_have_count(0)
    assert page.evaluate('uid => cache.get(uid).end', uid) == paths[0].stat().st_size

    # Main and sibling badges are independent, survive reload and are cleared
    # by opening their own view, including via the view switcher.
    open_row(sibling, '初始回复：复核历史记录')
    with paths[0].open('ab') as stream:
        stream.write(encoded(record(2, 'PLANNER_RESPONSE', '主会话的新回复')))
    with paths[2].open('ab') as stream:
        stream.write(encoded(record(2, 'PLANNER_RESPONSE', '另一子代理的新回复')))
    main_badge = main.locator('.item-status.counted')
    sibling_badge = sibling.locator('.item-status.counted')
    expect(main_badge).to_have_text('1')
    if not mobile:
        # The desktop is still showing the sibling, so only its view updates.
        expect(messages).to_contain_text('另一子代理的新回复')
        expect(sibling_badge).to_have_count(0)
    else:
        expect(sibling_badge).to_have_text('1')
    open_row(main, '主会话的新回复')
    expect(messages).not_to_contain_text('子代理的新回复')
    expect(main_badge).to_have_count(0)
    expect(child_badge).to_have_text('1')
    page.reload()
    if mobile:
        expect(main).to_be_visible()
    if not child.is_visible():
        caret.click()
    expect(child_badge).to_have_text('1')
    if mobile:
        main.locator('.t').click()
    page.locator('#a-view-switch').click()
    page.locator(f'#session-view-menu button[data-agent="{ids[1]}"]').click()
    expect(messages).to_contain_text('子代理的新回复')
    if screenshot:
        page.screenshot(path=screenshot.with_name(screenshot.name + '-opened.png'))
    show_list()
    expect(child_badge).to_have_count(0)
    if mobile:
        expect(sibling_badge).to_have_text('1')
        open_row(sibling, '另一子代理的新回复')
        expect(sibling_badge).to_have_count(0)

    # An older node's summary fallback also belongs to the exact child.
    page.route('**/api/sessions/unread', lambda route: route.fulfill(status=404, json={'error': 'old node'}))
    with paths[2].open('ab') as stream:
        stream.write(encoded(record(3, 'PLANNER_RESPONSE', '兼容旧节点的子代理回复')))
    expect(sibling_badge).to_have_text('1')
    expect(main_badge).to_have_count(0)
    open_row(sibling, '兼容旧节点的子代理回复')
    expect(sibling_badge).to_have_count(0)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--screenshots', type=Path)
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
        native = data.root / 'agy'
        native.mkdir()
        db = sqlite3.connect(native / 'conversation_summaries.db')
        with db:
            db.execute(SCHEMA)
        extra_env = node_env(data.root, node.port, '127.0.0.0/8')
        extra_env.update(SESSIONDOCK_AGY_HOME=str(native), SESSIONDOCK_AGY_ROOT=str(data.root / 'agy-mirror'))
        with ExitStack() as stack:
            stack.callback(db.close)
            base,_=stack.enter_context(isolated_server(data,args.binary,extra_env=extra_env))
            hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node])
            hub.start();stack.callback(hub.stop)
            browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
            for hub_mode in (False,True):
                for sid,path in data.paths.items():path.write_bytes(b''.join(encoded(row) for row in rows[sid]))
                context=browser.new_context();page=context.new_page();errors=[]
                page.on('pageerror',lambda e:errors.append(str(e)))
                url=f'http://127.0.0.1:{hub.port}' if hub_mode else base
                page.goto(url,wait_until='networkidle');page.wait_for_function('S.sessions.length===20 && T.listLoaded')
                # Warm native anchors without putting messages in the browser cache.
                checkpoints={}
                for sid in rows:
                    uid=data.uid(sid)
                    if hub_mode:uid=uid.replace(':',f':{nid}~',1)
                    result=context.request.get(url+'/api/messages/'+uid).json()
                    checkpoints[uid]={'end':result['end'],'head':result['version']['head'],'anchor':result['anchor']}
                page.evaluate('''points=>{S.unread.clear();cache.clear();S.cursors=new Map(Object.entries(points));
                  S.view='date';S.nest=true;S.closed.clear();renderView();renderSide()}''',checkpoints)
                page.locator('#side>.group>.ghead').first.click()
                requests=[]
                page.on('request',lambda r:requests.append((r.method,r.url)))
                ordinal=1
                for sid,path in data.paths.items():
                    with path.open('ab') as f:f.write(encoded(claude_row(sid,'assistant',f'a{ordinal}',f'a{ordinal-1}','New reply')))
                context.request.get(url+'/api/sessions?force=1')
                page.evaluate('pollSessions()')
                page.wait_for_function('S.unread.size===20 && [...S.unread.values()].every(r=>r.count===1)')
                batches=[u for method,u in requests if '/api/sessions/unread' in u]
                assert len(batches)==1,requests
                assert not any('/api/messages/' in u for _,u in requests),requests
                page.locator('#side>.group>.ghead').first.click()
                page.wait_for_function("document.querySelectorAll('#side .item-status.counted').length === 20")
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
                context.request.get(url+'/api/sessions?force=1');page.evaluate('pollSessions()')
                for _ in range(100):
                    if held: break
                    page.wait_for_timeout(20)
                assert held
                race_uid=data.uid(race_sid)
                if hub_mode: race_uid=race_uid.replace(':',f':{nid}~',1)
                page.locator(f'#side .item[data-uid="{race_uid}"] .t').click()
                page.wait_for_function('uid=>S.sel===uid && cache.has(uid)',arg=race_uid)
                before=page.evaluate('uid=>S.cursors.get(uid)',race_uid)
                for route in held: route.fulfill(response=route.fetch())
                page.unroute('**/api/sessions/unread',hold)
                page.wait_for_function('sidebarSyncing.size===0')
                assert page.evaluate('uid=>S.cursors.get(uid)',race_uid)==before
                assert page.evaluate('uid=>!S.unread.has(uid)',race_uid)
                # Previously opened history stays cached but receives no background
                # body reads. Repeated updates count from the unread checkpoint,
                # not the deliberately stale cache end.
                def uid_for(sid):
                    uid=data.uid(sid)
                    return uid.replace(':',f':{nid}~',1) if hub_mode else uid
                cached_uid=uid_for('unread-0');other_uid=uid_for('unread-1')
                page.locator(f'#side .item[data-uid="{cached_uid}"] .t').click()
                page.wait_for_function('uid=>S.sel===uid && cache.has(uid)',arg=cached_uid)
                page.locator(f'#side .item[data-uid="{other_uid}"] .t').click()
                page.wait_for_function('uid=>S.sel===uid && cache.has(uid)',arg=other_uid)
                old_end=page.evaluate('uid=>cache.get(uid).end',cached_uid)
                requests.clear()
                for n in (1,2):
                    with data.paths['unread-0'].open('ab') as f:
                        f.write(encoded(claude_row('unread-0','assistant',f'lazy-{n}',
                            'a1' if n==1 else 'lazy-1',f'Lazy cached reply {n}')))
                    context.request.get(url+'/api/sessions?force=1');page.evaluate('pollSessions()')
                    page.wait_for_function('arg=>S.unread.get(arg.uid)?.count===arg.n',arg={'uid':cached_uid,'n':n})
                    page.evaluate('syncSidebarUpdates(S.sessions)')
                    page.wait_for_function('sidebarSyncing.size===0')
                    assert page.evaluate('uid=>S.unread.get(uid).count',cached_uid)==n
                    assert page.evaluate('uid=>cache.get(uid).end',cached_uid)==old_end
                assert not any('/api/messages/'+cached_uid in unquote(u) for _,u in requests),requests
                page.locator(f'#side .item[data-uid="{cached_uid}"] .t').click()
                page.wait_for_function("document.querySelector('#msgs').innerText.includes('Lazy cached reply 2')")
                assert any('/api/messages/'+cached_uid in unquote(u) for _,u in requests),requests
                assert page.evaluate('uid=>!S.unread.has(uid)',cached_uid)
                # A second event can arrive while the first summary is in flight.
                # Capture the first response before appending again; releasing it
                # must drain the pending cursor without needing a third event.
                pending_uid=uid_for('unread-2')
                page.evaluate('uid=>{closeUiEvents();clearUnread(uid)}',pending_uid)
                first_summary=[]
                def hold_first_summary(route):
                    if not first_summary:
                        first_summary.append((route,route.fetch()))
                    else:
                        route.continue_()
                page.route('**/api/sessions/unread',hold_first_summary)
                with data.paths['unread-2'].open('ab') as f:
                    f.write(encoded(claude_row('unread-2','assistant','pending-1','a1','Pending first')))
                context.request.get(url+'/api/sessions?force=1');page.evaluate('pollSessions()')
                for _ in range(100):
                    if first_summary:break
                    page.wait_for_timeout(20)
                assert first_summary
                with data.paths['unread-2'].open('ab') as f:
                    f.write(encoded(claude_row('unread-2','assistant','pending-2','pending-1','Pending second')))
                update=context.request.get(url+'/api/sessions?force=1').json()
                row=next(row for row in update['sessions'] if row['uid']==pending_uid)
                page.evaluate('row=>syncSidebarUpdates([row])',row)
                assert page.evaluate('uid=>sidebarPendingCursors.has(uid)',pending_uid)
                route,response=first_summary[0];route.fulfill(response=response)
                page.wait_for_function('uid=>S.unread.get(uid)?.count===2 && !sidebarSyncing.has(uid)',arg=pending_uid)
                assert page.evaluate('uid=>!sidebarPendingCursors.has(uid)',pending_uid)
                page.unroute('**/api/sessions/unread',hold_first_summary)
                page.evaluate('startUiEvents()')
                # Old-node fallback never downloads inactive message bodies.
                page.route('**/api/sessions/unread',lambda r:r.fulfill(status=404,json={'error':'old node'}))
                fallback_sid='unread-4'
                fallback_uid=uid_for(fallback_sid)
                page.evaluate('uid=>clearUnread(uid)',fallback_uid)
                with data.paths[fallback_sid].open('ab') as f:
                    f.write(encoded(claude_row(fallback_sid,'assistant',f'fallback-{ordinal}',f'a{ordinal}','Fallback reply')))
                context.request.get(url+'/api/sessions?force=1');page.evaluate('pollSessions()')
                page.wait_for_function('uid=>unreadBatchUnsupported.size===1 && !sidebarSyncing.has(uid) && S.unread.get(uid)?.count===1',
                                       arg=fallback_uid)
                assert not any('/api/messages/'+fallback_uid in unquote(u) for _,u in requests),requests
                # A normal user click still opens full history after summary sync.
                page.locator(f'#side .item[data-uid="{fallback_uid}"] .t').click()
                page.wait_for_function('uid=>S.sel===uid && cache.has(uid)',arg=fallback_uid)
                assert 'Fallback reply' in page.locator('#msgs').inner_text()
                assert not errors,errors
                context.close()
            case = 0
            for url, label in ((base, 'node'), (f'http://127.0.0.1:{hub.port}', 'hub')):
                for width, theme in ((1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark')):
                    case += 1
                    screenshot = None
                    if args.screenshots:
                        args.screenshots.mkdir(parents=True, exist_ok=True)
                        screenshot = args.screenshots / f'{label}-{width}-{theme}'
                    context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
                    try:
                        agy_unread(context.new_page(), url, native, db, case, mobile=width == 390, screenshot=screenshot)
                        print(f'PASS Agy unread: {label}, {width}px, {theme}', flush=True)
                    finally:
                        context.close()
        print('PASS sidebar unread: node+hub; 20 folded views -> one summary; cached inactive history stays lazy; reopen catches up; race/reset/old-node fallback')
        print('PASS Agy unread: main and two subagents stay independent on node and Hub, wide/narrow, light/dark')


if __name__=='__main__':main()
