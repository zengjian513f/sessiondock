#!/usr/bin/env python3
"""Trusted terminal inputs checked against an independent private CLI byte receipt."""
import argparse
import json
from pathlib import Path
import shlex
import sys
import tempfile
import uuid
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,Corpus,codex_row,codex_message,isolated_server
import terminal_input_browser as fixture
from browser_evidence_fixtures import run_with_evidence

CLI=r'''
import os,tty
from pathlib import Path
tty.setraw(0)
os.write(1,b'RS_SHELL_READY\r\n\x1b[?2004h')
command=b''
while True:
 data=os.read(0,1)
 with open('input.bin','ab') as f:f.write(data)
 command+=data
 if data==b'\r':
  mode=command.rsplit(b'mode:',1)[-1].strip();command=b''
  if mode==b'plain':os.write(1,b'\x1b[?1049l\x1b[?1000l\x1b[?1006l\x1b[2J\x1b[HSELECT_FIRST second_word\r\n\x1b]8;;https://example.invalid/remaining-ui\x1b\\LINK_FIXTURE\x1b]8;;\x1b\\\r\n')
  if mode==b'alt':os.write(1,b'\x1b[?1049h\x1b[?1000lALT_READY')
  if mode==b'mouse':os.write(1,b'\x1b[?1000h\x1b[?1006hMOUSE_READY')
'''


def main():
    p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/terminal'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);results=[]
    def done(ids,actual,width):
        results.append({'ids':ids.split(),'actual':actual,'width':width});(a.output/'result.json').write_text(json.dumps({'scenarios':results},indent=2))
    with tempfile.TemporaryDirectory(prefix='sd-remaining-term-') as tmp,sync_playwright() as pw:
        root=Path(tmp)
        for name in ['host','work','claude','codex','grok']:(root/name).mkdir()
        script=root/'fake.py';script.write_text(CLI);fixture.SHELL=shlex.join([sys.executable,str(script)])
        corpus=Corpus(root);sid='synthetic-native-sid';corpus.put(sid,'codex',[codex_row('session_meta',{'id':sid,'cwd':str(root/'work')}),codex_message('user','Remaining input fixture')],[])
        with fixture.host(root,'remaining-'+uuid.uuid4().hex,corpus.uid(sid)),isolated_server(corpus,a.binary,host_dir=root/'host',file_write_roots=(root/'work',)) as (base,_):
            browser=pw.chromium.launch(headless=True)
            try:
                for width in [1280,390]:
                    context=browser.new_context(viewport={'width':width,'height':900},permissions=['clipboard-read','clipboard-write'])
                    context.route('https://example.invalid/**',lambda r:r.fulfill(body='local link destination'))
                    page=context.new_page();page.goto(base,wait_until='networkidle');fixture.open_console(page,corpus.uid(sid),history=False)
                    key=page.locator('#termpane .xterm-helper-textarea')
                    def sent(data,action):
                        before=(root/'work/input.bin').read_bytes() if (root/'work/input.bin').exists() else b''
                        action()
                        for _ in range(100):
                            now=(root/'work/input.bin').read_bytes()
                            if data in now[len(before):]:return
                            page.wait_for_timeout(30)
                        raise AssertionError((data,now[len(before):]))
                    def mode(value):
                        key.press_sequentially('mode:'+value);key.press('Enter')
                        fixture.xterm_contains(page,{'plain':'SELECT_FIRST','alt':'ALT_READY','mouse':'MOUSE_READY'}[value])
                        page.wait_for_timeout(200)
                    mode('plain')
                    sent(b'\x1b[200~clipboard receipt\x1b[201~',lambda:(page.evaluate("navigator.clipboard.writeText('clipboard receipt')"),key.press('Control+V')))
                    done('sd.terminal.paste','Trusted Ctrl+V produces bracketed paste bytes in fake CLI receipt',width)
                    page.locator('#termpane canvas').click();cdp=context.new_cdp_session(page)
                    sent('输入'.encode(),lambda:(cdp.send('Input.imeSetComposition',{'text':'输入','selectionStart':2,'selectionEnd':2}),cdp.send('Input.insertText',{'text':'输入'})))
                    done('sd.terminal.ime','CDP composition commit reaches fake CLI exactly as UTF-8; OS candidate UI not tested',width)
                    ctrl=page.locator('[data-term-modifier=ctrl]')
                    if ctrl.is_visible():
                        ctrl.click();sent(b'\x01',lambda:key.press('a'));expect(ctrl).to_have_attribute('aria-pressed','false')
                        sent(b'b',lambda:key.press('b'))
                        sent(b'\x1b',lambda:page.locator('[data-term-key=Escape]').click())
                        done('sd.terminal.ctrl-next sd.terminal.soft-key-escape','One-shot Ctrl+A then plain b and Escape verified in receipt',width)
                    else:
                        key.press('ControlRight');sent(b'\x01',lambda:key.press('a'));sent(b'b',lambda:key.press('b'))
                        done('sd.terminal.ctrl-next','Right Ctrl arms only next a; following b remains plain',width)
                    mode('plain')
                    metrics=page.evaluate('''()=>{const t=[...T.views.values()][0].term,b=t._canvas.getBoundingClientRect();return {x:b.x,y:b.y,cw:t.renderer.cellWidth,ch:t.renderer.cellHeight}}''')
                    x,y,cw,ch=[metrics[k] for k in ['x','y','cw','ch']]
                    page.screenshot(path=str(a.output/f'before-select-{width}.png'))
                    page.mouse.dblclick(x+3*cw,y+ch*.8)
                    page.wait_for_timeout(100)
                    selected=page.evaluate('navigator.clipboard.readText()')
                    assert selected=='SELECT_FIRST',repr(selected)
                    done('sd.terminal.select-double','Double click rendered word copies exact word to clipboard on mouseup',width)
                    with context.expect_page() as opened:
                        page.keyboard.down('Control');page.mouse.click(x+3*cw,y+1.5*ch);page.keyboard.up('Control')
                    popup=opened.value;popup.wait_for_load_state();assert popup.url=='https://example.invalid/remaining-ui';popup.close()
                    done('sd.terminal.link','Ctrl click OSC8 link opens exact URL; external destination replaced locally',width)
                    page.locator('#termpane canvas').click(button='right');expect(page.locator('.term-context-menu')).to_be_visible()
                    page.locator('#a-chat').click();expect(page.locator('.term-context-menu')).to_be_hidden();page.locator('#a-term').click()
                    done('sd.terminal.menu-outside','Pointer outside terminal menu closes it',width)
                    page.locator('#termpane canvas').click(button='right');page.locator('.term-context-menu').get_by_text('查找',exact=True).click()
                    expect(page.locator('.term-find')).to_be_visible();page.locator('.term-find input').fill('SELECT_FIRST')
                    page.locator('.term-find [aria-label=关闭查找]').click();expect(page.locator('.term-find')).to_be_hidden();expect(key).to_be_focused()
                    done('sd.terminal.find-close','Find menu, real text entry, close button restores terminal focus',width)
                    mode('alt');page.locator('#termpane canvas').hover();sent(b'\x1b[B',lambda:page.mouse.wheel(0,120))
                    done('sd.terminal.wheel-alt','Actual wheel on alternate screen reaches CLI as direction bytes',width)
                    mode('mouse');page.locator('#termpane canvas').hover();sent(b'\x1b[<65;',lambda:page.mouse.wheel(0,120))
                    done('sd.terminal.wheel-mouse','Actual wheel in mouse capture reaches CLI SGR wheel protocol',width)
                    page.screenshot(path=str(a.output/f'terminal-{width}.png'));context.close()
            finally:browser.close()
    print('PASS remaining terminal inputs',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/terminal')
