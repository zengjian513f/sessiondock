#!/usr/bin/env python3
"""Real transfer job held at private node proxy; tasks UI and withdrawal."""
import argparse,json,tempfile,threading,http.client,time
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
from contextlib import ExitStack
from types import SimpleNamespace
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,isolated_server
from session_clone_browser import prepare
from hub_fixtures import Hub,free_port,scoped
from node_auth_fixtures import node_env,TOKEN
from browser_evidence_fixtures import run_with_evidence
from remaining_ui_sidebar_browser import header


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/tasks'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual):
  cases.append({'ids':ids.split(),'actual':actual});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-tasks-') as tmp,sync_playwright() as pw,ExitStack() as stack:
  root=Path(tmp);source=prepare(root/'source');dest=prepare(root/'dest');nodes=[];real=free_port();hold=threading.Event();release=threading.Event();mode={'hold':False}
  class Proxy(BaseHTTPRequestHandler):
   def log_message(self,*args):pass
   def do_GET(self):self.forward()
   def do_POST(self):self.forward()
   def forward(self):
    body=self.rfile.read(int(self.headers.get('Content-Length',0)))
    if mode['hold'] and self.path.endswith('/transfer/manifest'):hold.set();release.wait(40)
    conn=http.client.HTTPConnection('127.0.0.1',real,timeout=45)
    try:
     conn.request(self.command,self.path,body,headers={k:v for k,v in self.headers.items() if k.lower() not in ['host','connection']});r=conn.getresponse();data=r.read();self.send_response(r.status)
     for k,v in r.getheaders():
      if k.lower() not in ['content-length','connection','transfer-encoding']:self.send_header(k,v)
     self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    except (BrokenPipeError,ConnectionResetError):pass
    finally:conn.close()
  proxy=ThreadingHTTPServer(('127.0.0.1',0),Proxy);threading.Thread(target=proxy.serve_forever,daemon=True).start();stack.callback(proxy.server_close);stack.callback(proxy.shutdown);stack.callback(release.set)
  for i,c in enumerate([source,dest]):
   n=SimpleNamespace(name=['Source','Destination'][i],nid=['a','b'][i]*32,port=proxy.server_port if i==0 else free_port(),token=TOKEN);nodes.append(n)
   (c.root/'ids').mkdir();(c.root/'ids/node-id').write_text(n.nid);env=node_env(c.root,real if i==0 else n.port,'127.0.0.0/8');env['SESSIONDOCK_PROC_ROOT']=str(c.root/'proc');stack.enter_context(isolated_server(c,a.binary,state_dir=c.root/'state',trash_dir=c.root/'trash',extra_env=env))
  hubroot=root/'hub';hubroot.mkdir();hub=Hub(a.binary.resolve().with_name('sessiondock-hub'),hubroot,nodes);hub.start();stack.callback(hub.stop)
  browser=pw.chromium.launch(headless=True);stack.callback(browser.close);context=browser.new_context(viewport={'width':1280,'height':600});stack.callback(context.close);page=context.new_page();base=f'http://127.0.0.1:{hub.port}';page.goto(base,wait_until='networkidle')
  uid=scoped(nodes[0].nid,source.uid('a'));row=page.locator(f'#side .item[data-uid="{uid}"]');row.hover();page.wait_for_timeout(150);row.click(button='right');page.locator('#item-menu [data-act=clone]').click();dialog=page.locator('#clone-group-dialog')
  dialog.locator('#transfer-target').click();dialog.locator(f'#transfer-target-options [data-value="{nodes[1].nid}"]').click();dialog.locator('label:has(input[value=move])').click();dialog.locator('#transfer-new-ids').check();expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=20000)
  page.set_viewport_size({'width':1280,'height':460});table=dialog.locator('.transfer-table-scroll').first;table.hover();page.mouse.wheel(0,10000);expect(table.locator('tbody tr').last).to_be_in_viewport();assert table.evaluate('(e)=>e.scrollTop')>0;page.mouse.wheel(0,-10000);expect(table.locator('tbody tr').first).to_be_in_viewport()
  done('sd.transfer.plan-scroll','Actual wheel reaches first/last rows of real six-member transfer plan; table overflows at 460px height and actual scrollTop is positive')
  original={str(p):p.read_text() for c in [source,dest] for p in c.paths.values()};mode['hold']=True;dialog.locator('.clone-confirm').click()
  for _ in range(200):
   if hold.is_set():break
   page.wait_for_timeout(30)
  page.screenshot(path=str(a.output/'pending.png'))
  assert hold.is_set(),dialog.inner_text()
  journals=list((hubroot/'transfers').glob('*.json'));assert journals
  journal=next(p for p in journals if json.loads(p.read_text()).get('phase')=='preparing');operation=json.loads(journal.read_text())['request']['operation_id']
  other=context.new_page();other.goto(base,wait_until='networkidle')
  expect(other.locator('#transfer-tasks')).not_to_have_attribute('hidden','',timeout=15000)
  for action in ['close','escape','open']:
   header(other,'#transfer-tasks');panel=other.locator('#transfer-tasks-dialog');expect(panel).to_be_visible();task=panel.locator(f'tr[data-operation="{operation}"]');expect(task).to_be_visible()
   if action=='close':panel.locator('.transfer-close').click();expect(panel).to_have_count(0)
   elif action=='escape':other.keyboard.press('Escape');expect(panel).to_have_count(0)
   else:task.get_by_role('button',name='继续处理').click()
  done('sd.transfer.tasks sd.transfer.tasks-close sd.transfer.tasks-escape sd.transfer.task-open','Real durable pending Hub job listed; close/Escape preserve it; Continue restores exact operation')
  restored=other.locator('#clone-group-dialog');expect(restored).to_be_visible();expect(restored.locator('.transfer-abort')).to_be_visible();restored.locator('.transfer-abort').click();release.set()
  expect(restored.locator('.transfer-progress')).to_contain_text('撤回',timeout=30000)
  deadline=time.monotonic()+20
  while time.monotonic()<deadline:
   state=json.loads(journal.read_text())
   if state['phase'] in ['cancelled','aborted']:break
   other.wait_for_timeout(100)
  assert state['phase'] in ['cancelled','aborted'],state
  assert all(Path(p).read_text()==v for p,v in original.items())
  done('sd.transfer.withdraw','Actual withdraw button cancels real held job; durable journal terminal state and native sources/destination unchanged')
  other.screenshot(path=str(a.output/'tasks.png'))
 print('PASS remaining transfer task recovery',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/tasks')
