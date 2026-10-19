#!/usr/bin/env python3
"""Deferred parent deletion and confirmed descendant purge through Chromium."""
from contextlib import ExitStack
import argparse
import json
from pathlib import Path
import shutil
import tempfile
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, batch35_meta, codex_message, encoded, isolated_server
from trash_browser import Dialogs, click_delete, open_trash as open_trash_dialog
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import TOKEN, node_env
from spawned_by_fixtures import proc_pid

PARENT='01a10000-0000-7000-8000-000000000001'
GEN='01a10000-0000-7000-8000-000000000002'
A='01a10000-0000-7000-8000-000000000003'
B='01a10000-0000-7000-8000-000000000004'
GRAND='01a10000-0000-7000-8000-000000000005'
OTHER='01a10000-0000-7000-8000-000000000006'


def fixture(root):
    root.mkdir(parents=True,exist_ok=True)
    corpus=Corpus(root)
    old=[batch35_meta(PARENT),codex_message('user','Parent inherited prefix'),codex_message('assistant','Parent inherited answer',2)]
    path=corpus.put('old','codex',old,[])
    final=path.with_name('rollout-2026-10-19T00-00-00-'+PARENT+'.jsonl');path.rename(final);corpus.paths['old']=final
    current=[batch35_meta(PARENT,history_base={'thread_id':PARENT,'end_byte_offset':final.stat().st_size}),codex_message('assistant','Parent new generation',3)]
    path=corpus.put('parent','codex',current,[])
    final=path.with_name('rollout-2026-10-19T00-00-00-'+PARENT+'_'+GEN+'.jsonl');path.rename(final);corpus.paths['parent']=final
    for name,sid in [('a',A),('b',B)]:
        corpus.put(name,'codex',[batch35_meta(sid,forked_from_id=PARENT,history_base={'thread_id':GEN,'end_byte_offset':final.stat().st_size}),codex_message('user','Branch '+name),codex_message('assistant','Answer '+name,4)],[])
    corpus.put('grand','codex',[batch35_meta(GRAND,forked_from_id=A,history_base={'thread_id':A,'end_byte_offset':corpus.paths['a'].stat().st_size}),codex_message('assistant','Grandchild answer',5)],[])
    corpus.put('other','codex',[batch35_meta(OTHER),codex_message('user','Unrelated retained conversation')],[])
    state=root/'state';state.mkdir(exist_ok=True)
    (state/'session-metadata.json').write_text(json.dumps({'schema_version':1,'revision':1,'sessions':{corpus.uid(n):{'fork_parent_visible':True} for n in corpus.paths}}))
    proc=root/'proc';proc.mkdir(exist_ok=True);(proc/'stat').write_text('btime 1700000000\n')
    return corpus


def run(browser, binary, root, screenshots, mode):
    corpus=fixture(root);trash=root/'trash';trash.mkdir();node=SimpleNamespace(name='PendingNode',token=TOKEN,port=free_port())
    hubroot=root/'hub';hubroot.mkdir();hub=None
    uid=lambda name: scoped(node.nid,corpus.uid(name)) if mode=='hub' else corpus.uid(name)
    # The second server reconstructs the hidden marker from the manifest.
    for stage in (0,1):
        print(f"START {mode} stage {stage}",flush=True)
        with ExitStack() as stack:
            env=node_env(root,node.port,'127.0.0.0/8') if mode=='hub' else {}
            env['SESSIONDOCK_PROC_ROOT']=str(root/'proc')
            base,_=stack.enter_context(isolated_server(corpus,binary,state_dir=root/'state' if mode=='hub' else None,trash_dir=trash,extra_env=env))
            if mode=='hub':
                if hub is None: hub=Hub(binary.resolve().with_name('sessiondock-hub'),hubroot,[node])
                hub.start();stack.callback(hub.stop);node.nid=(root/'ids/node-id').read_text().strip()
                base=f'http://127.0.0.1:{hub.port}'
            context=browser.new_context(viewport={'width':1280 if mode=='node' else 390,'height':900},service_workers='block',color_scheme='light' if mode=='node' else 'dark');stack.callback(context.close)
            context.route('**/*',lambda route:route.continue_() if route.request.url.startswith(base+'/') else route.abort())
            page=context.new_page();page.set_default_timeout(10000);dialogs=Dialogs(page);errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
            page.goto(base,wait_until='networkidle')
            def open_trash(page, narrow):
                if narrow and page.locator('.mobile-back').first.is_visible(): page.locator('.mobile-back').first.click()
                open_trash_dialog(page,narrow)
            def card(name):return page.locator(f'#side .item[data-uid="{uid(name)}"]')
            def select(name):
                if mode=='hub' and page.locator('.mobile-back').first.is_visible():page.locator('.mobile-back').first.click()
                card(name).click()
                expect(page.locator('#msgs')).to_contain_text('Parent inherited answer' if name!='other' else 'Unrelated retained conversation')
            def delete(name):
                select(name)
                dialogs.expect(('父会话将标记待删除' if name in ('parent','a') else '服务端回收站',True))
                click_delete(page,mode=='hub')
                expect(card(name)).to_have_count(0);dialogs.drained()
            def entry(name):
                data=page.request.get(base+'/api/trash').json()
                return next(row for row in data['items'] if row['uid']==uid(name))
            def row(name):return page.locator(f'.trash-item[data-id="{entry(name)["id"]}"]')
            def purge(name):
                dialogs.expect(('彻底删除',True));row(name).get_by_role('button',name='彻底删除',exact=True).click()
                expect(page.locator('#trash-note')).to_contain_text('已彻底删除');dialogs.drained()
            if stage==0:
                delete('parent')
                assert all(p.exists() for p in corpus.paths.values())
                page.locator('#q').fill('Parent inherited prefix');page.locator('#q').press('Enter')
                page.wait_for_function('S.results !== null')
                expect(card('parent')).to_have_count(0)
                assert page.evaluate('(uid) => !S.results.some(row => row.uid === uid)',uid('parent'))
                page.reload(wait_until='networkidle')
                select('grand');expect(page.locator('#msgs')).to_contain_text('Grandchild answer')
                open_trash(page,mode=='hub');expect(row('parent')).to_contain_text('待删除')
                row('parent').get_by_role('button',name='恢复',exact=True).click()
                expect(page.locator('#trash-note')).to_contain_text('已恢复');page.locator('#trash-done').click()
                expect(card('parent')).to_have_count(1)
                delete('parent')
                assert entry('parent')['deferred'] is True
            else:
                expect(card('parent')).to_have_count(0)
                select('b');expect(page.locator('#msgs')).to_contain_text('Parent inherited answer')
                delete('a');delete('grand');delete('b')
                open_trash(page,mode=='hub')
                assert corpus.paths['old'].exists() and corpus.paths['parent'].exists() and corpus.paths['a'].exists()
                purge('grand')
                assert not corpus.paths['a'].exists(),'last descendant purge must collect pending intermediate parent'
                assert corpus.paths['parent'].exists(),'recoverable sibling still pins parent'
                purge('b')
                expect(page.locator('#trash-list')).to_contain_text('没有已删除的会话')
                assert all(not corpus.paths[n].exists() for n in ('old','parent','a','b','grand'))
                assert corpus.paths['other'].exists()
                page.locator('#trash-done').click()
                # A second tree exercises explicit cascade, cancellation,
                # running-child refusal and Hub clear-all confirmation.
                fixture(root);page.reload(wait_until='networkidle');delete('b')
                # A different native session now occupies a trashed child's
                # old path. Purging the old tree must preserve that session.
                corpus.paths['b'].write_bytes(encoded(batch35_meta('replacement-unrelated'))+encoded(codex_message('user','Replacement unrelated conversation')))
                page.reload(wait_until='networkidle');delete('parent');open_trash(page,mode=='hub')
                dialogs.expect(('连同 3 个子会话一起彻底删除',False))
                row('parent').get_by_role('button',name='彻底删除',exact=True).click();dialogs.drained()
                assert all(p.exists() for p in corpus.paths.values())
                if screenshots:
                    # A second real page has no automatic popup responder, so
                    # its confirmation remains visible for visual review.
                    visual=browser.new_context(viewport={'width':1280 if mode=='node' else 390,'height':900},color_scheme='light' if mode=='node' else 'dark',service_workers='block')
                    visual.route('**/*',lambda r:r.continue_() if r.request.url.startswith(base+'/') else r.abort())
                    try:
                        shot=visual.new_page();shot.goto(base,wait_until='networkidle');open_trash_dialog(shot,mode=='hub')
                        shot.locator('.trash-item').filter(has_text='待删除').get_by_role('button',name='彻底删除',exact=True).click()
                        popup=shot.locator('dialog.app-popup[open]');expect(popup).to_contain_text('连同 3 个子会话一起彻底删除')
                        shot.screenshot(path=str(screenshots/f'{mode}-cascade-confirm.png'))
                        popup.locator('[data-popup-action="cancel"]').click()
                    finally:visual.close()
                m=entry('parent');rejected=page.request.post(base+'/api/trash/purge',data={'id':m['id']})
                assert rejected.status==409,rejected.text()
                extra=corpus.put('late','codex',[batch35_meta('late-child',forked_from_id=B),codex_message('user','New child after preview')],[])
                confirmed=[r['uid'] for r in m['sessions']+m['cascade_sessions']]
                rejected=page.request.post(base+'/api/trash/purge',data={'id':m['id'],'uids':confirmed})
                assert rejected.status==409,rejected.text()
                extra.unlink();corpus.paths.pop('late')
                proc_pid(root/'proc',400,'codex',['codex'],1,fds={3:corpus.paths['a']})
                dialogs.expect(('连同 3 个子会话一起彻底删除',True))
                row('parent').get_by_role('button',name='彻底删除',exact=True).click()
                expect(page.locator('#trash-note')).to_contain_text('仍在运行');dialogs.drained()
                assert all(p.exists() for p in corpus.paths.values())
                shutil.rmtree(root/'proc/400')
                if screenshots:page.screenshot(path=str(screenshots/f'{mode}-pending-parent.png'))
                dialogs.expect(('连同 3 个子会话一起彻底删除',True))
                if mode=='hub':page.locator('#trash-purge-all').click()
                else:row('parent').get_by_role('button',name='彻底删除',exact=True).click()
                expect(page.locator('#trash-list')).to_contain_text('没有已删除的会话');dialogs.drained()
                assert all(not corpus.paths[n].exists() for n in ('old','parent','a','grand'))
                assert corpus.paths['other'].exists() and corpus.paths['b'].exists()
                page.locator('#trash-done').click();page.reload(wait_until='networkidle')
                expect(page.locator('#side .item')).to_have_count(2)
                expect(card('b')).to_contain_text('Replacement unrelated conversation')
                assert not errors,errors
                print(f'PASS {mode}: deferred/restore/restart, inherited history, last-dependent collection, cascade warning/cancel/liveness/scope and unrelated preservation',flush=True)


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY);parser.add_argument('--screenshots',type=Path);args=parser.parse_args()
    if args.screenshots:args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-parent-trash-') as tmp,sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            for mode in ('node','hub'):run(browser,args.binary,Path(tmp)/mode,args.screenshots,mode)
        finally:browser.close()

if __name__=='__main__':main()
