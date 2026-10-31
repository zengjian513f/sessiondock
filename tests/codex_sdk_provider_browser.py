#!/usr/bin/env python3
"""Explicit provider confirmation: No never resumes; Yes migrates then starts."""
from __future__ import annotations
import argparse
import json
import fcntl
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_queue_browser import QueueServer
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from sdk_ui_fixtures import start_sdk


class ProviderServer(QueueServer):
    async def _handle(self, writer, message):
        method, params = message.get('method'), message.get('params') or {}
        provider = json.loads(self.rollout.read_text().splitlines()[0])['payload'].get('model_provider') or 'openai'
        if method == 'thread/read':
            await self.send(writer, {'id':message['id'], 'result':{'thread':{'id':THREAD,'modelProvider':provider}}})
            return
        if method == 'thread/loaded/list':
            await self.send(writer, {'id':message['id'], 'result':{'data':[THREAD] if self.methods('thread/resume') else []}})
            return
        if method == 'thread/resume' and provider != 'openai':
            await self.send(writer, {'id':message['id'], 'error':{'code':-32602,'message':'process-level provider mismatch'}})
            return
        await super()._handle(writer,message)


def seed_db(root, path):
    rows=path.read_text().splitlines()
    meta=json.loads(rows[0]);meta['payload']['model_provider']='openai_catalog_cache'
    rows[0]=json.dumps(meta);path.write_text('\n'.join(rows)+'\n')
    (root/'thread-writer-locks').mkdir()
    db=sqlite3.connect(root/'state_5.sqlite')
    db.execute('CREATE TABLE threads (id TEXT PRIMARY KEY, model_provider TEXT, rollout_path TEXT)')
    db.execute('INSERT INTO threads VALUES (?,?,?)',(THREAD,'openai_catalog_cache',str(path)))
    db.commit();db.close()


def exercise(browser,base,fake,root,screenshots,label,width):
    context=browser.new_context(viewport={'width':width,'height':844},color_scheme='dark' if width<500 else 'light')
    page=context.new_page()
    requests=[];page.on('request',lambda r:requests.append(r.url))
    page.goto(base,wait_until='networkidle')
    page.locator('#side .item').filter(has_text='earlier synthetic question').first.click()
    uid=page.evaluate('S.sel')
    body={'uid':uid}
    denied=page.request.post(base+'/api/codex-sdk/migrate-provider',data={**body,'source_provider':'openai_catalog_cache'})
    assert denied.status==409,denied.text()
    refused=page.request.post(base+'/api/codex-sdk/open',data=body)
    assert refused.status==409,refused.text()
    assert not fake.methods('thread/resume')
    page.locator('.sdk-pane .composer-run-action').click()
    popup=page.locator('dialog.app-popup[open]')
    expect(popup).to_contain_text('openai_catalog_cache')
    expect(popup).to_contain_text('改为 openai 后启动')
    if screenshots:page.screenshot(path=str(screenshots/f'{label}-prompt.png'),full_page=True)
    popup.get_by_role('button',name='否，不启动',exact=True).click()
    expect(page.locator('.sdk-pane .composer-run-action')).to_be_visible()
    assert not page.evaluate('CodexSdkView.connected')
    assert not fake.methods('thread/resume') and not fake.methods('turn/start')
    db=sqlite3.connect(root/'state_5.sqlite')
    assert db.execute('select model_provider from threads').fetchone()[0]=='openai_catalog_cache'
    assert not list((root/'state').glob('provider-backups/*'))
    assert not any('/codex-sdk/migrate-provider' in r for r in requests),requests
    # The terminal entry uses the same confirmation and cancels before takeover.
    page.locator('#a-term').click()
    expect(popup).to_contain_text('openai_catalog_cache')
    popup.get_by_role('button',name='否，不启动',exact=True).click()
    assert not any('/api/term/takeover' in r for r in requests),requests
    page.locator('#a-sdk').click()
    # Native writer locks block confirmed migration without partial edits.
    with (root/'thread-writer-locks'/f'{THREAD}.lock').open('a+b') as writer:
        fcntl.flock(writer,fcntl.LOCK_EX|fcntl.LOCK_NB)
        page.locator('.sdk-pane .composer-run-action').click()
        popup.get_by_role('button',name='是，修改并启动',exact=True).click()
        expect(popup).to_contain_text('仍被原生 Codex 占用')
        assert db.execute('select model_provider from threads').fetchone()[0]=='openai_catalog_cache'
        assert not list((root/'state').glob('provider-backups/*'))
        assert not fake.methods('thread/resume')
        popup.get_by_role('button',name='知道了',exact=True).click()
    page.locator('.sdk-pane .composer-run-action').click()
    popup.get_by_role('button',name='是，修改并启动',exact=True).click()
    page.wait_for_function('CodexSdkView.connected')
    assert db.execute('select model_provider from threads').fetchone()[0]=='openai'
    assert json.loads(fake.rollout.read_text().splitlines()[0])['payload']['model_provider']=='openai'
    backups=list((root/'state/provider-backups').glob('*/rollout.jsonl'))
    assert len(backups)==1
    assert json.loads(backups[0].read_text().splitlines()[0])['payload']['model_provider']=='openai_catalog_cache'
    expect(page.locator('.sdk-history')).to_contain_text('earlier synthetic answer')
    assert fake.methods('thread/resume')[-1]['params']['modelProvider']=='openai'
    page.locator('#sdk-input').fill('Continue after confirmed provider migration')
    page.locator('#sdk-send').click()
    expect(page.locator('.sdk-live')).to_contain_text('Continue after confirmed provider migration')
    fake.complete();page.reload(wait_until='networkidle');start_sdk(page)
    expect(popup).to_have_count(0)
    assert len(fake.methods('turn/start'))==1
    assert len(backups)==1
    if screenshots:page.screenshot(path=str(screenshots/f'{label}-ready.png'),full_page=True)
    db.close();context.close()
    print(f'PASS {label}: no/CLI-cancel leaves provider and runtime untouched; yes backs up, migrates, resumes and sends; reload no replay')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--screenshots',type=Path)
    args=parser.parse_args()
    if args.screenshots:args.screenshots.mkdir(parents=True,exist_ok=True)
    with sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            for mode,width in [('node',1280),('hub',390)]:
                with tempfile.TemporaryDirectory(prefix='sd-confirm-') as tmp:
                    root=Path(tmp);corpus=build(root);seed_db(root,corpus.paths['sdk']);port=free_port()
                    env={**environment(root,args.binary),**node_env(root,port,'127.0.0.0/8')}
                    with ProviderServer(root/'app.sock',corpus.paths['sdk'],THREAD) as fake, server(corpus,args.binary,root/'app.sock',env) as base:
                        if mode=='hub':
                            hr=root/'hub';hr.mkdir();hub=Hub(args.binary.with_name('sessiondock-hub').resolve(),hr,[SimpleNamespace(name='ProviderNode',token=TOKEN,port=port)])
                            hub.start()
                            try:exercise(browser,f'http://127.0.0.1:{hub.port}',fake,root,args.screenshots,mode,width)
                            finally:hub.stop()
                        else:exercise(browser,base,fake,root,args.screenshots,mode,width)
        finally:browser.close()

if __name__=='__main__':main()
