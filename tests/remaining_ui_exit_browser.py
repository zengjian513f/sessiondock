#!/usr/bin/env python3
"""Browser-created private shell final-screen scroll and real cleanup stop."""
import argparse,json,os,tempfile,time
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,REPO,Corpus,codex_row,codex_message,isolated_server
from composer_editor_browser import fixture
from private_hosts import private_hosts
from new_session_model_browser import open_dialog,pick_source
from remaining_ui_sidebar_browser import header
from terminal_final_screen_browser import SHELL,shows_final_screen
from session_stop_browser import FAKE_CODEX,session_action,wait_xterm
from browser_evidence_fixtures import run_with_evidence


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/exit'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual):
  cases.append({'ids':ids.split(),'actual':actual});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-exit-') as tmp,private_hosts(Path(tmp)),sync_playwright() as pw:
  root=Path(tmp);launcher=fixture(root,a.binary);cfg=json.loads(launcher.read_text());cfg['adapters']=[{'id':'synthetic-shell-v1','source':'shell','executable':'/bin/sh','args':['-c',SHELL],'env':{'PATH':'/usr/bin:/bin','TERM':'xterm-256color'}}];launcher.write_text(json.dumps(cfg));c=Corpus(root)
  with isolated_server(c,a.binary,host_dir=root/'host',lifecycle_dir=root/'ledger',state_dir=root/'state',launcher_config=launcher) as (base,_):
   browser=pw.chromium.launch(headless=True)
   try:
    page=browser.new_page(viewport={'width':1280,'height':900});page.goto(base,wait_until='networkidle');open_dialog(page);pick_source(page,'shell');page.locator('#new-cwd').fill(str(root/'work'));page.locator('#new-session-go').click();page.wait_for_function('T.ws?.readyState===WebSocket.OPEN');kb=page.locator('#termpane .xterm-helper-textarea');kb.press_sequentially('lines');kb.press('Enter');page.wait_for_timeout(300);kb.press_sequentially('quit');kb.press('Enter');shows_final_screen(page)
    box=page.locator('#xterm');expect(box).to_be_visible();box.hover();[(page.mouse.wheel(0,-600),page.wait_for_timeout(20)) for _ in range(60)];assert page.evaluate('currentTermViewObject().term.buffer.active.viewportY')==0;[(page.mouse.wheel(0,600),page.wait_for_timeout(20)) for _ in range(60)];m=page.evaluate('()=>{const t=currentTermViewObject().term;return {top:t.buffer.active.viewportY,base:t.buffer.active.baseY}}');assert m['top']>0 and m['top']==m['base'],m
    assert list((root/'host/screens').rglob('snapshot.json'))
    page.reload(wait_until='networkidle');page.locator('#side .item').first.click();shows_final_screen(page);page.set_viewport_size({'width':390,'height':844});page.locator('#side .item').first.click();box.hover();[(page.mouse.wheel(0,-600),page.wait_for_timeout(20)) for _ in range(60)];assert page.evaluate('currentTermViewObject().term.buffer.active.viewportY')==0
    done('sd.terminal.final-scroll','UI-created fake shell prints 120 lines and exits; real wheel reaches overflow first/last; snapshot exists and fresh reload remains read-only, narrow view scrolls to beginning')
    page.screenshot(path=str(a.output/'final-screen.png'));page.close()
   finally:browser.close()
  sid='11111111-2222-4333-8444-555555555555';c.put(sid,'codex',[codex_row('session_meta',{'id':sid,'cwd':str(root/'work')}),codex_message('user','Old private cleanup fixture')],[]);old=time.time()-3*86400;os.utime(c.paths[sid],(old,old));native=c.paths[sid].read_bytes()
  fake=root/'fake-codex';fake.write_text(FAKE_CODEX);fake.chmod(0o700);cfg['profiles'].append({'id':'codex-cli-v1','source':'codex','executable':str(fake),'args':[],'resume_args':['resume','{sid}'],'env':{'PATH':'/usr/bin:/bin','TERM':'xterm-256color'}});launcher.write_text(json.dumps(cfg))
  with isolated_server(c,a.binary,host_dir=root/'host',state_dir=root/'state',lifecycle_dir=root/'ledger',launcher_config=launcher) as (base,_):
   browser=pw.chromium.launch(headless=True)
   try:
    page=browser.new_page(viewport={'width':1280,'height':900});page.goto(base,wait_until='networkidle');row=page.locator(f'#side .item[data-uid="{c.uid(sid)}"]');row.click();expect(page.locator('#msgs')).to_contain_text('Old private cleanup fixture');expect(session_action(page)).to_have_attribute('aria-label','启动会话');session_action(page).click();page.wait_for_timeout(600);page.screenshot(path=str(a.output/'cleanup-starting.png'));(a.output/'cleanup-starting.txt').write_text(page.locator('body').inner_text());wait_xterm(page,'RS_SHELL_READY');record=next(json.loads(p.read_text()) for p in (root/'host').glob('*.json') if json.loads(p.read_text()).get('meta',{}).get('source')=='codex');pid=record['host_pid'];row.hover();page.wait_for_timeout(150);row.click(button='right');page.locator('#item-menu [data-act=pick]').click();expect(page.locator('#side-pick-stop')).to_be_enabled()
    page.route('**/api/session/stop',lambda r:r.fulfill(status=409,json={'error':'private stop transport failure'}));page.locator('#side-pick-stop').click();page.locator('.app-popup').get_by_role('button',name='确定',exact=True).click();expect(page.locator('#side-stop-summary')).to_contain_text('失败 1');page.locator('#side-stop-summary').click();expect(page.locator('#side-stop-errors')).to_contain_text('private stop transport failure');assert Path(f'/proc/{pid}').exists()
    done('sd.lifecycle.stop-details','Real bulk-stop selection and confirm, injected per-item failure, summary click expands exact error; original private process remains alive (frontend failure branch)')
    page.unroute('**/api/session/stop');page.locator('#side-pick-cancel').click();header(page,'#session-cleanup');expect(page.locator('#session-cleanup-status')).to_contain_text('找到 1 个');page.locator('#session-cleanup-start').click();expect(page.locator('#session-cleanup-status')).to_contain_text('已停止 1/1，失败 0，未确认 0',timeout=20000);page.locator('#session-cleanup-close').click()
    for _ in range(100):
     if not Path(f'/proc/{pid}').exists():break
     page.wait_for_timeout(30)
    assert not Path(f'/proc/{pid}').exists();assert c.paths[sid].read_bytes()==native
    done('sd.lifecycle.cleanup-start','Real old native history + live private PTY; cleanup Start stops exact process, native history bytes unchanged; no production access')
    page.screenshot(path=str(a.output/'cleanup.png'))
   finally:browser.close()
 print('PASS remaining exit and cleanup',flush=True)
if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/exit')
