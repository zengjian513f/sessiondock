#!/usr/bin/env python3
"""Terminal layout, native image paste and ownership through real browser inputs."""
import argparse,json,shlex,sys,tempfile,uuid
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,Corpus,codex_row,codex_message,isolated_server
import terminal_input_browser as fixture
from remaining_ui_terminal_browser import CLI
from remaining_ui_composer_browser import paste_image
from remaining_ui_sidebar_browser import header
from browser_evidence_fixtures import run_with_evidence


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/terminal-state'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual):
  cases.append({'ids':ids.split(),'actual':actual});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-term-state-') as tmp,sync_playwright() as pw:
  root=Path(tmp)
  for n in ['host','work','claude','codex','grok']:(root/n).mkdir()
  script=root/'fake.py';script.write_text(CLI);fixture.SHELL=shlex.join([sys.executable,str(script)])
  c=Corpus(root);sid='synthetic-native-sid';c.put(sid,'codex',[codex_row('session_meta',{'id':sid,'cwd':str(root/'work')}),codex_message('user','Terminal state fixture')],[])
  with fixture.host(root,'state-'+uuid.uuid4().hex,c.uid(sid)),isolated_server(c,a.binary,host_dir=root/'host',file_write_roots=(root/'work',)) as (base,_):
   browser=pw.chromium.launch(headless=True)
   try:
    context=browser.new_context(viewport={'width':1280,'height':900},permissions=['clipboard-read','clipboard-write']);page=context.new_page();page.goto(base,wait_until='networkidle');fixture.open_console(page,c.uid(sid))
    grip=page.locator('#tgrip');b=grip.bounding_box();page.mouse.move(b['x']+b['width']/2,b['y']+b['height']/2);page.mouse.down();page.mouse.move(b['x']+b['width']/2,b['y']+230,steps=15);page.mouse.up()
    saved=page.evaluate("({h:localStorage.getItem('sessiondock.termh'),mode:localStorage.getItem('sessiondock.termmode')})");assert json.loads(saved['mode'])=='normal' and float(saved['h'])>100,saved
    page.reload(wait_until='networkidle');fixture.open_console(page,c.uid(sid),history=False);assert page.evaluate("localStorage.getItem('sessiondock.termh')")==saved['h']
    done('sd.terminal.resize','Real grip drag creates split layout; persisted height survives full reload')
    header(page,'#settings');page.locator('.settings-tab[data-tab=features]').click();page.locator('#setting-console-paste-files').check();page.locator('#settings-dialog .modal-close').click()
    paste_image(page,page.locator('#termpane canvas'))
    savedfile=root/'work/sessiondock_attachments/1/image.png'
    for _ in range(200):
     if savedfile.exists() and b'./sessiondock_attachments/1/image.png' in (root/'work/input.bin').read_bytes():break
     page.wait_for_timeout(30)
    assert savedfile.read_bytes()[:8]==b'\x89PNG\r\n\x1a\n';assert b'./sessiondock_attachments/1/image.png' in (root/'work/input.bin').read_bytes()
    done('sd.terminal.paste-files','Settings opt-in then native clipboard image Ctrl+V saves PNG in private cwd and sends quoted path to independent PTY byte receipt')
    other=browser.new_context(viewport={'width':390,'height':900});second=other.new_page();second.goto(base,wait_until='networkidle');second.locator(f'#side .item[data-uid="{c.uid(sid)}"]').click();second.locator('#a-term').click()
    popup=second.locator('.app-popup');expect(popup).to_be_visible();(a.output/'takeover-prompt.txt').write_text(popup.inner_text());popup.get_by_role('button',name='取消',exact=True).click();expect(second.locator('#termpane')).to_be_hidden()
    before=(root/'work/input.bin').read_bytes();page.locator('#termpane .xterm-helper-textarea').press_sequentially('OWNER_ONE');page.wait_for_timeout(300);assert b'OWNER_ONE' in (root/'work/input.bin').read_bytes()[len(before):]
    second.locator('#a-term').click();popup.get_by_role('button',name='确定',exact=True).click();second.wait_for_function('T.ws?.readyState===WebSocket.OPEN');before=(root/'work/input.bin').read_bytes();second.locator('#termpane .xterm-helper-textarea').press_sequentially('OWNER_TWO');second.wait_for_timeout(300);assert b'OWNER_TWO' in (root/'work/input.bin').read_bytes()[len(before):]
    done('sd.terminal.takeover','Second browser cancels owner conflict leaving first receipt writable, then explicitly accepts and writes through new lease')
    second.screenshot(path=str(a.output/'terminal-state.png'));other.close();context.close()
   finally:browser.close()
 print('PASS remaining terminal state',flush=True)
if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/terminal-state')
