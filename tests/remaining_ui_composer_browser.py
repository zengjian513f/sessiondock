#!/usr/bin/env python3
"""Remaining composer/report inputs: real private server, fake CLI, durable drafts.

Clipboard image uses Chromium's clipboard and Ctrl+V. File drop uses CDP drag
input with private local files. Neither path calls an application handler.
IME composition is CDP text composition, not an OS candidate-window claim.
"""
import argparse
import base64
import json
from pathlib import Path
import tempfile
import time
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, isolated_server
from composer_editor_browser import fixture, type_text
from private_hosts import private_hosts
from send_browser import wait_history
from bug_report_node_browser import open_report
from browser_evidence_fixtures import run_with_evidence

PNG = base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAE0lEQVR4nGP8z8DAwMDAxIBMAQAUQAEF3SN5DgAAAABJRU5ErkJggg==')


def durable(page, root, text, present=True):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        files = list((root/'state').rglob('conversation-ledger.json'))
        if files and (text in files[0].read_text()) == present:
            return
        page.wait_for_timeout(50)
    raise AssertionError(f'durable draft expectation not reached: {text!r}, present={present}')


def drop(page, context, selector, path, cancel=False):
    box = page.locator(selector).bounding_box()
    cdp = context.new_cdp_session(page)
    data = {'items': [], 'files': [str(path)], 'dragOperationsMask': 1}
    point = {'x': box['x']+box['width']/2, 'y': box['y']+box['height']/2, 'data': data}
    cdp.send('Input.dispatchDragEvent', {'type':'dragEnter', **point})
    cdp.send('Input.dispatchDragEvent', {'type':'dragOver', **point})
    expect(page.locator(selector)).to_have_class(__import__('re').compile('dragover'))
    if cancel:
        cdp.send('Input.dispatchDragEvent', {'type':'dragCancel', **point})
    else:
        cdp.send('Input.dispatchDragEvent', {'type':'drop', **point})
    cdp.detach()


def paste_image(page, editor):
    # Native clipboard paste: browser generates the trusted paste event.
    page.evaluate('''async data => {
      const b=Uint8Array.from(atob(data), c=>c.charCodeAt(0));
      const canvas=document.createElement('canvas'); canvas.width=2;canvas.height=2;
      canvas.getContext('2d').fillRect(0,0,2,2);
      const blob=await new Promise(r=>canvas.toBlob(r));
      await navigator.clipboard.write([new ClipboardItem({'image/png':blob})]);
    }''', base64.b64encode(PNG).decode())
    editor.click();page.keyboard.press('Control+V')


def choose(page, add, menu, kind, name, data):
    page.locator(add).click()
    with page.expect_file_chooser() as fc:
        page.locator(f'{menu} [data-attach={kind}]').click()
    fc.value.set_files({'name':name,'mimeType':{'image':'image/png','video':'video/mp4','audio':'audio/wav','file':'text/plain'}[kind], 'buffer':data})


def main():
    p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY)
    p.add_argument('--output',type=Path,default=Path('target/remaining-ui/composer'))
    args=p.parse_args();args.output.mkdir(parents=True,exist_ok=True);results=[]
    def passed(ids, detail, width):
        results.append({'ids':ids.split(), 'actual':detail,'width':width})
        (args.output/'result.json').write_text(json.dumps({'scenarios':results},ensure_ascii=False,indent=2))
    with tempfile.TemporaryDirectory(prefix='sd-remaining-composer-') as tmp, private_hosts(Path(tmp)), sync_playwright() as pw:
        root=Path(tmp);launcher=fixture(root,args.binary)
        with isolated_server(Corpus(root),args.binary,host_dir=root/'host', lifecycle_dir=root/'ledger',launcher_config=launcher,state_dir=root/'state',audit_dir=root/'audit',file_roots=(root/'work',),file_write_roots=(root/'work',),extra_env={'SESSIONDOCK_BUG_REPORT_DIR':str(root/'reports'),'SESSIONDOCK_BUG_REPORT_REPO':str(root/'work')}) as (base,_):
            browser=pw.chromium.launch(headless=True)
            try:
                for width,theme in [(1280,'light'),(390,'dark')]:
                    context=browser.new_context(viewport={'width':width,'height':900},permissions=['clipboard-read','clipboard-write'],color_scheme=theme)
                    page=context.new_page();page.goto(base,wait_until='networkidle')
                    page.locator('#new-session:visible, #header-more-btn:visible').first.click()
                    if not page.locator('#new-session-dialog').is_visible(): page.locator('#new-session').click()
                    page.locator('input[name=new-source][value=claude]').check()
                    page.locator('#new-cwd').fill(str(root/'work/claude-area'))
                    page.locator('#new-session-go').click()
                    editor=page.locator('#cinput');expect(editor).to_be_enabled()
                    seed=f'quote seed {width}'
                    type_text(editor,seed);page.locator('#csend').click()
                    expect(editor).to_have_value('');expect(page.locator('#msgs')).to_contain_text(seed)
                    wait_history(page,seed)
                    expect(page.locator('#csend')).to_be_enabled()
                    editor.click();editor.press('ArrowUp')
                    page.locator('#input-history [role=option]').filter(has_text=seed).click()
                    expect(editor).to_have_value(seed)
                    passed('sd.composer.history-click','ArrowUp then actual candidate click restores submitted text',width)
                    type_text(editor,'')
                    cdp=context.new_cdp_session(page);editor.click()
                    cdp.send('Input.imeSetComposition',{'text':'测试','selectionStart':2,'selectionEnd':2})
                    expect(editor).to_have_value('测试')
                    cdp.send('Input.insertText',{'text':'测试'})
                    expect(editor).to_have_value('测试');expect(page.locator('#csend')).to_have_attribute('aria-busy','false')
                    passed('sd.composer.ime','CDP composition and commit retain draft without SEND; OS candidate window not exercised',width)
                    type_text(editor,'')
                    page.locator('#cadd').click();page.locator('#attach-menu [data-attach=quote]').click()
                    quote=page.locator('#compose-items .draft-quote textarea');expect(quote).to_be_visible()
                    quote.fill(f'persisted quotation {width}');durable(page,root,f'persisted quotation {width}')
                    page.reload(wait_until='networkidle');expect(quote).to_have_value(f'persisted quotation {width}')
                    passed('sd.composer.quote-add sd.composer.quote-edit','Menu quote, typed text, independent ledger and reload recovery',width)
                    page.locator('#compose-items .draft-quote .draft-remove').click();expect(quote).to_have_count(0)
                    durable(page,root,f'persisted quotation {width}',False)
                    passed('sd.composer.quote-remove','Remove button persists absence in ledger',width)
                    # Select actual rendered message with pointer input; no Range/selection mutation.
                    para=page.locator('#msgs .bubble').filter(has_text=seed).first
                    if not para.count(): para=page.locator('#msgs').get_by_text(seed,exact=True).first
                    para.dblclick();selection=page.evaluate('getSelection().toString()');assert selection
                    page.locator('#cadd').click();page.locator('#attach-menu [data-attach=quote]').click()
                    expect(quote).to_have_value(selection.strip())
                    page.locator('#compose-items .draft-quote .draft-remove').click()
                    passed('sd.composer.select-quote','Pointer selection captured into quote card',width)
                    for kind,ext,data in [('image','png',PNG),('video','mp4',b'fixture video'),('audio','wav',b'fixture audio')]:
                        name=f'chooser-{width}.{ext}';choose(page,'#cadd','#attach-menu',kind,name,data)
                        expect(page.locator('#compose-items')).to_contain_text(name);durable(page,root,name)
                        passed('sd.composer.choose-'+kind,'Native chooser adds typed attachment, durable draft metadata',width)
                    paste_image(page,editor);expect(page.locator('#compose-items .draft-card')).to_have_count(4)
                    expect(editor).to_have_value('')
                    passed('sd.composer.paste-files','Trusted Ctrl+V image paste stages attachment without duplicate text',width)
                    path=root/'work'/f'drop-{width}.txt';path.write_text('private drop receipt')
                    drop(page,context,'#composer',path);expect(page.locator('#compose-items')).to_contain_text(path.name);durable(page,root,path.name)
                    passed('sd.composer.drop','CDP native file drag/drop stages actual private file',width)
                    page.screenshot(path=str(args.output/f'composer-{width}.png'))
                    # Report staging and capture are real backend persistence; no worker launch.
                    if width<720:page.locator('.mobile-back').click()
                    open_report(page);desc=page.locator('#bug-report-description')
                    page.locator('#bug-report-add').click();expect(page.locator('#bug-report-attach-menu')).to_be_visible()
                    desc.click();expect(page.locator('#bug-report-attach-menu')).to_be_hidden()
                    passed('sd.report.attach-outside','Open attachment picker then click description closes picker',width)
                    for kind,ext in [('video','mp4'),('audio','wav'),('file','txt')]:
                        name=f'report-{width}.{ext}';choose(page,'#bug-report-add','#bug-report-attach-menu',kind,name,b'private report attachment')
                        expect(page.locator('#bug-report-items')).to_contain_text(name);durable(page,root,name)
                        passed('sd.report.attach-'+kind,'Native chooser and independent durable report draft',width)
                    paste_image(page,desc);expect(page.locator('#bug-report-items .draft-card')).to_have_count(4)
                    passed('sd.report.paste','Trusted clipboard image paste in report form',width)
                    drop(page,context,'#bug-report-form',path);expect(page.locator('#bug-report-items')).to_contain_text(path.name)
                    passed('sd.report.drop','Native CDP file drop into report form',width)
                    page.locator('#bug-report-type label:has(input[value=feature])').click()
                    page.locator('#bug-report-type label:has(input[value=bug])').click()
                    desc.fill(f'private capture {width}');durable(page,root,f'private capture {width}')
                    page.screenshot(path=str(args.output/f'report-{width}.png'))
                    with page.expect_response(lambda r:r.url.endswith('/api/bug-report')) as response:
                        page.locator('#bug-report-go').click()
                    data=response.value.json();assert response.value.status==201,data
                    manifest=json.loads((Path(data['path'])/'manifest.json').read_text())
                    assert manifest['report_type']=='bug' and manifest['status']=='captured',manifest
                    assert len(manifest['attachments'])==5,manifest
                    assert any(p.read_bytes()==b'private report attachment' for p in (Path(data['path'])/'attachments').iterdir())
                    passed('sd.report.type-bug','Click feature then bug; capture manifest bug, five actual attachments, no worker',width)
                    context.close()
            finally: browser.close()
    print('PASS remaining composer/report browser paths',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/composer')
