#!/usr/bin/env python3
"""Question fallback, unsaved navigation and report receipt via private fake CLI."""
import argparse,json,tempfile,time
from pathlib import Path
from input_hit_fixtures import hit_fill

from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,REPO,Corpus,isolated_server
from composer_editor_browser import fixture,type_text
from private_hosts import private_hosts
from remaining_ui_composer_browser import durable
from new_session_model_browser import open_dialog,pick_source
from bug_report_node_browser import open_report
from browser_evidence_fixtures import run_with_evidence


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/composer-state'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual):
  cases.append({'ids':ids.split(),'actual':actual});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-composer-state-') as tmp,private_hosts(Path(tmp)),sync_playwright() as pw:
  root=Path(tmp);launcher=fixture(root,a.binary);cfg=json.loads(launcher.read_text());screen=root/'screen';frame=json.loads((REPO/'tests/fixtures/cli_menus_claude.json').read_text())[0]
  screen.write_text(json.dumps({'sequence':1,'screen':frame['screen'],'expected_bytes':'','cursor':frame.get('cursor')}));cfg['profiles'][0]['args']=[str(REPO/'tests/cli_menus_browser.py'),'--fake'];cfg['profiles'][0]['env']['SESSIONDOCK_TEST_SCREEN']=str(screen);launcher.write_text(json.dumps(cfg))
  with isolated_server(Corpus(root),a.binary,host_dir=root/'host',lifecycle_dir=root/'ledger',launcher_config=launcher,state_dir=root/'state',audit_dir=root/'audit',extra_env={'SESSIONDOCK_BUG_REPORT_DIR':str(root/'reports'),'SESSIONDOCK_BUG_REPORT_WORKER':'true','SESSIONDOCK_BUG_REPORT_REPO':str(root/'work')}) as (base,_):
   browser=pw.chromium.launch(headless=True)
   try:
    context=browser.new_context(viewport={'width':1280,'height':900});page=context.new_page();page.goto(base,wait_until='networkidle');open_dialog(page);pick_source(page,'claude');hit_fill(page.locator('#new-cwd'), str(root/'work/claude-area'));page.locator('#new-session-go').click()
    card=page.locator('#conversation-question');expect(card).to_be_visible(timeout=20000);card.get_by_role('button',name='打开终端',exact=True).click();expect(page.locator('#termpane')).to_be_visible();page.wait_for_function('T.ws?.readyState===WebSocket.OPEN')
    done('sd.conversation.question-terminal','Real fake CLI trust screen is recognized as a question; card button opens same native PTY and live lease')
    page.locator('#a-chat').click();screen.write_text(json.dumps({'sequence':2,'screen':'Unrecognized private screen, no ready prompt','expected_bytes':''}));expect(page.locator('#composer-input-status')).to_contain_text('终端',timeout=15000)
    assert page.locator('#composer-input-status button').count()==0
    page.locator('#a-term').click();expect(page.locator('#termpane')).to_be_visible();page.locator('#a-chat').click()
    done('sd.conversation.inspect-input','Current unknown-input notice has no dedicated terminal button; current title terminal control opens same PTY. Original inventory anchor is obsolete; no dedicated-button coverage claimed')
    held=[]
    def draft(route):
     if route.request.method=='POST':held.append(route)
     else:route.continue_()
    page.route('**/api/session/conversation',draft);type_text(page.locator('#cinput'),'unsaved navigation fixture');page.wait_for_timeout(600)
    dialogs=[]
    def dismiss(d):dialogs.append(d.type);d.dismiss()
    page.on('dialog',dismiss)
    try:page.reload(timeout=4000)
    except Exception as e:
     assert 'net::ERR_ABORTED' in str(e) or 'Timeout' in type(e).__name__,str(e)
    assert dialogs==['beforeunload'],dialogs;expect(page.locator('#cinput')).to_have_value('unsaved navigation fixture')
    for r in held:r.continue_()
    page.unroute('**/api/session/conversation',draft);durable(page,root,'unsaved navigation fixture');page.remove_listener('dialog',dismiss);page.reload(wait_until='networkidle');expect(page.locator('#cinput')).to_have_value('unsaved navigation fixture')
    done('sd.composer.navigate-unsaved','Real typing with held draft POST triggers browser beforeunload on refresh; dismiss preserves text; real save then refresh restores durable draft')
    open_report(page);hit_fill(page.locator('#bug-report-description'), 'Isolated receipt test, no external provider');page.locator('#bug-report-source label:has(input[value=claude])').click();page.locator('#bug-report-go').click();toast=page.locator('#bug-report-toast');expect(toast).to_be_visible(timeout=20000)
    receipt=toast.inner_text();(a.output/'receipt.txt').write_text(receipt);toast.get_by_role('button',name='打开',exact=True).click();expect(page.locator('#bug-report-toast')).to_be_hidden();expect(page.locator('#cinput')).to_be_visible()
    manifests=list((root/'reports').rglob('manifest.json'));assert manifests;manifest=json.loads(manifests[-1].read_text());(a.output/'receipt-manifest-keys.json').write_text(json.dumps(sorted(manifest)))
    assert len(list((root/'host').glob('*.json')))>=2
    done('sd.report.receipt-open','Local report launches configured free fake CLI; receipt Open navigates to worker; private report manifest and two host records independently verified')
    page.screenshot(path=str(a.output/'composer-state.png'));context.close()
   finally:browser.close()
 print('PASS remaining composer state',flush=True)
if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/composer-state')
