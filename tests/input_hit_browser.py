#!/usr/bin/env python3
"""Input hit/long-error regression with real private metadata and launch results."""
import argparse
import json
from pathlib import Path
import tempfile
from urllib.parse import urljoin
from browser_evidence_fixtures import run_with_evidence


def main():
    from playwright.sync_api import expect, sync_playwright
    from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
    from composer_editor_browser import fixture
    from private_hosts import private_hosts
    from new_session_model_browser import open_dialog, pick_source
    from remaining_ui_sidebar_browser import header
    from input_hit_fixtures import hit, hit_fill, hit_select, peers, wheel_to, error_tail, single_line_button
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,default=BINARY)
    p.add_argument('--output',type=Path,default=Path('target/input-hit'))
    p.add_argument('--case', choices=['1280-light-100','390-light-100','390-dark-125','1280-dark-150'], help='Run a single case')
    args=p.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    cases=[]
    with tempfile.TemporaryDirectory(prefix='sd-input-hit-') as tmp, private_hosts(Path(tmp)), sync_playwright() as pw:
        root=Path(tmp);launcher=fixture(root,args.binary);c=Corpus(root)
        for sid in ['first','origin']:
            c.put(sid,'codex',[codex_row('session_meta',{'id':sid,'cwd':'/synthetic/input-hit','timestamp':'2026-10-07T10:00:00Z'}),codex_message('user',sid+' 输入命中'),codex_message('assistant','private answer',2)],[])
        with isolated_server(c,args.binary,host_dir=root/'host',lifecycle_dir=root/'ledger',launcher_config=launcher,state_dir=root/'state') as (base,_):
            browser=pw.chromium.launch(headless=True)
            try:
                for width,theme,scale in [(1280,'light',100),(390,'light',100),(390,'dark',125),(1280,'dark',150)]:
                    if args.case and args.case != f'{width}-{theme}-{scale}':continue
                    context=browser.new_context(viewport={'width':width,'height':850})
                    context.add_init_script(f"localStorage.setItem('sessiondock.theme', JSON.stringify('{theme}'));localStorage.setItem('sessiondock.interfaceScale', JSON.stringify({scale}));")
                    page=context.new_page();tag=f'{width}-{theme}-{scale}'
                    page.goto(base,wait_until='networkidle')
                    # Existing toolbar entry and preferences, then reload persistence.
                    header(page,'#settings');page.locator('.settings-tab[data-tab=features]').click()
                    hit_fill(page.locator('#setting-cleanup-days'),'7');page.keyboard.press('Tab')
                    page.keyboard.press('Escape');page.reload(wait_until='networkidle')
                    header(page,'#settings');expect(page.locator('#setting-cleanup-days')).to_have_value('7');page.keyboard.press('Escape')
                    hit_fill(page.locator('#q'),'first');expect(page.locator('#side .item[data-uid]')).to_have_count(1)
                    hit_fill(page.locator('#q'),'');page.locator(f'#side .item[data-uid="{c.uid("first")}"]').click()
                    page.locator('#a-info').click()
                    frame=page.frame_locator('#session-content-view iframe')
                    expect(frame.locator('#info-summary')).to_contain_text('first')
                    # Follow the actual information entry into its standalone equivalent.
                    url=page.locator('#session-content-view iframe').get_attribute('src')
                    page.goto(urljoin(base, url).replace('embedded=1','embedded=0'),wait_until='networkidle')
                    page.locator('#add-predecessor').click()
                    fields=['#relation-search','#relation-target','#relation-kind','#relation-state','#relation-artifact','#relation-evidence']
                    hit_fill(page.locator(fields[0]),'origin');hit_select(page.locator(fields[1]),c.uid('origin'))
                    hit_select(page.locator(fields[2]),'references');hit_select(page.locator(fields[3]),'candidate')
                    artifact='/synthetic/'+('long-path/'*8)+'HANDOFF.md'
                    evidence='实际输入 '+tag+'\n'+'长依据文字 '*25
                    hit_fill(page.locator(fields[4]),artifact);hit_fill(page.locator(fields[5]),evidence)
                    peers(page,fields+['#relation-save','#relation-cancel'])
                    error='写入失败 '+('临时节点不可用，请保留草稿并重试。 '*40)+' END-OF-ERROR'
                    page.route('**/api/session/predecessor',lambda r:r.fulfill(status=503,json={'error':error}),times=1)
                    page.locator('#relation-save').click();expect(page.locator('#relation-error')).to_contain_text('END-OF-ERROR')
                    # The error bottom is reached with wheel, not scrollTop or locator scrolling.
                    error_tail(page,page.locator('#relation-error'),page.locator('body'))
                    wheel_to(page,page.locator('#relation-save'),page.locator('body'),direction=-1)
                    peers(page,fields+['#relation-save','#relation-cancel'])
                    page.screenshot(path=str(args.output/(tag+'-relation-error.png')))
                    single_line_button(page.locator('#relation-save'))
                    page.locator('#relation-save').click();expect(page.locator('#info-editor')).to_be_hidden()
                    saved=json.loads((root/'state/session-metadata.json').read_text())
                    edge=saved['sessions'][c.uid('first')]['predecessors'][0]
                    assert edge['session_uid']==c.uid('origin') and edge['artifact']==artifact and edge['evidence']==evidence
                    page.reload(wait_until='networkidle');expect(page.locator('#info-predecessors')).to_contain_text('origin')
                    page.locator('#add-successor').click();hit_fill(page.locator('#relation-evidence'),'cancelled edit')
                    page.locator('#relation-cancel').click();assert json.loads((root/'state/session-metadata.json').read_text())==saved
                    page.goto(base,wait_until='networkidle')
                    if page.locator('.mobile-back').is_visible():page.locator('.mobile-back').click()
                    before_hosts=set((root/'host').glob('*.json'))
                    open_dialog(page);pick_source(page,'claude')
                    hit_fill(page.locator('#new-cwd'),str(root/'work/claude-area'))
                    # Same failure mode as the review finding: long error shares the action row.
                    page.route('**/api/term/create',lambda r:r.fulfill(status=503,json={'error':error}),times=1)
                    page.locator('#new-session-go').click();expect(page.locator('#new-session-error')).to_contain_text('END-OF-ERROR')
                    page.screenshot(path=str(args.output/(tag+'-new-error-before-wheel.png')))
                    wheel_to(page,page.locator('#new-session-go'),page.locator('#new-session-dialog'))
                    peers(page,['#new-session-error','#new-session-go','#new-session-dialog .modal-cancel'])
                    page.screenshot(path=str(args.output/(tag+'-new-error.png')))
                    single_line_button(page.locator('#new-session-go'))
                    try:
                        error_tail(page,page.locator('#new-session-error'),page.locator('#new-session-dialog'))
                    except AssertionError:
                        page.screenshot(path=str(args.output/(tag+'-error-unreachable.png')))
                        (args.output/'failure-layout.json').write_text(json.dumps(page.locator('#new-session-form').evaluate('''el=>({form:el.getBoundingClientRect().toJSON(),dialog:el.parentElement.getBoundingClientRect().toJSON(),scrollTop:el.scrollTop,scrollHeight:el.scrollHeight,clientHeight:el.clientHeight,viewport:[innerWidth,innerHeight]})'''),indent=2))
                        raise
                    page.screenshot(path=str(args.output/(tag+'-new-error-end.png')))
                    wheel_to(page,page.locator('#new-session-go'),page.locator('#new-session-dialog'),direction=-1)
                    hit(page.locator('#new-session-go'));page.locator('#new-session-go').click()
                    expect(page.locator('#new-session-dialog')).to_be_hidden(timeout=20000)
                    expect(page.locator('#cinput')).to_be_visible(timeout=20000)
                    assert set((root/'host').glob('*.json'))-before_hosts, 'retry must create a new private host'
                    cases.append({'viewport':width,'theme':theme,'scale':scale,'relation':'persisted exact artifact/evidence; reload; cancellation unchanged','launch':'503 then actual fake CLI host; dialog closed','settings':'edited days survive reload','search':'real filtered native row','screenshots':[tag+'-relation-error.png',tag+'-new-error.png']})
                    (args.output/'result.json').write_text(json.dumps({'scenarios':cases},ensure_ascii=False,indent=2))
                    context.close()
            finally:browser.close()
    print('PASS input hit, peer layout, long errors and independent results',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/input-hit')
