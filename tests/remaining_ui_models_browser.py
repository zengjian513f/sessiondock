#!/usr/bin/env python3
"""New/report source/model/directory controls with isolated fake CLI catalogs."""
import argparse,json,sys,tempfile
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,REPO,Corpus,isolated_server
from composer_editor_browser import fixture
from private_hosts import private_hosts
from new_session_model_browser import CODEX_CACHE,GROK_CACHE,OPENCODE_MODELS,open_dialog
from bug_report_node_browser import open_report
from browser_evidence_fixtures import run_with_evidence


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/models'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual,width):
  cases.append({'ids':ids.split(),'actual':actual,'width':width});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-models-') as tmp,private_hosts(Path(tmp)),sync_playwright() as pw:
  root=Path(tmp);launcher=fixture(root,a.binary)
  for n in ['codex-home','grok-home','home/.claude']:(root/n).mkdir()
  (root/'codex-home/models_cache.json').write_text(json.dumps(CODEX_CACHE));(root/'grok-home/models_cache.json').write_text(json.dumps(GROK_CACHE))
  (root/'home/.claude/settings.json').write_text('{}')
  env={'PATH':'/usr/bin:/bin','HOME':str(root/'home'),'TERM':'xterm-256color','LANG':'C.UTF-8','CODEX_HOME':str(root/'codex-home'),'GROK_HOME':str(root/'grok-home'),'SESSIONDOCK_TEST_ARGV_LOG':str(root/'argv.jsonl'),'SESSIONDOCK_TEST_MODELS':'\n'.join(OPENCODE_MODELS),'SESSIONDOCK_TEST_CODEX_BUNDLED':str(root/'bundled.json')}
  data=json.loads(launcher.read_text());data['profiles']=[{'id':s+'-cli-v1','source':s,'executable':sys.executable,'args':[str(REPO/'tests/fake_model_cli.py')],'env':env} for s in ['claude','codex','grok','opencode','agy']];launcher.write_text(json.dumps(data))
  (root/'work/alpha').mkdir();(root/'work/alpine').mkdir()
  with isolated_server(Corpus(root),a.binary,host_dir=root/'host',lifecycle_dir=root/'ledger',launcher_config=launcher,state_dir=root/'state',audit_dir=root/'audit',extra_env={'SESSIONDOCK_BUG_REPORT_DIR':str(root/'reports'),'SESSIONDOCK_BUG_REPORT_REPO':str(root/'work'),'SESSIONDOCK_BUG_REPORT_WORKER':'true'}) as (base,_):
   browser=pw.chromium.launch(headless=True)
   try:
    for width in [1280,390]:
     context=browser.new_context(viewport={'width':width,'height':900});page=context.new_page();page.goto(base,wait_until='networkidle');open_dialog(page)
     for source in ['claude','codex','grok','opencode','agy','shell']:
      radio=page.locator(f'input[name=new-source][value={source}]');radio.locator("..").click();expect(radio).to_be_checked()
      if source=='shell':expect(page.locator('#new-model')).to_be_disabled()
      else:expect(page.locator('#new-model')).to_be_visible()
     done('sd.new.source sd.new.source-agy','Click every source including Agy and shell; model visibility follows source',width)
     page.locator('input[name=new-source][value=opencode]').locator('..').click();expect(page.locator('#new-model')).to_be_enabled()
     page.locator('#new-model').click();expect(page.locator('#new-model-menu')).to_be_visible();page.locator('#new-session-dialog h2').click();expect(page.locator('#new-model-menu')).to_be_hidden()
     done('sd.new.model-outside','Pointer outside model popover closes it',width)
     cwd=page.locator('#new-cwd');cwd.fill(str(root/'work/al'))
     expect(page.locator('#new-cwd-options [data-cwd-option]')).to_have_count(2)
     cwd.press('Tab');expect(cwd).to_have_value(str(root/'work/alp'))
     cwd.press('ArrowDown');cwd.press('Enter');assert cwd.input_value() in [str(root/'work/alpha')+'/',str(root/'work/alpine')+'/'],cwd.input_value()
     done('sd.new.cwd-complete sd.new.cwd-keyboard','Tab common-prefix completion then ArrowDown/Enter selects real temporary directory',width)
     page.locator('#new-cwd-clear').click();expect(cwd).to_have_value('');expect(cwd).to_be_focused()
     done('sd.new.cwd-clear','Clear button empties directory and retains keyboard focus',width)
     cwd.fill(str(root/'work/al'));page.locator('#new-cwd-options [data-cwd-option]').first.click();assert 'alp' in cwd.input_value()
     done('sd.new.cwd-click','Click matching directory fills launch directory',width)
     page.locator('#new-session-dialog .modal-cancel').click();open_report(page)
     for source in ['claude','grok','agy','opencode']:
      radio=page.locator(f'#bug-report-source input[value={source}]');radio.locator("..").click();expect(radio).to_be_checked();expect(page.locator('#bug-report-model')).to_be_visible()
      done('sd.report.source-'+source,'Actual source selection refreshes visible model picker using fake CLI catalog',width)
     expect(page.locator('#bug-report-model')).to_be_enabled();page.locator('#bug-report-model').click()
     search=page.locator('#bug-report-model-search');expect(search).to_be_visible();search.fill('zeta')
     expect(page.locator('#bug-report-model-options [role=option]')).to_have_count(1)
     search.press('ArrowDown');search.press('Enter');expect(page.locator('#bug-report-model-label')).to_have_text('prov/zeta-1')
     page.locator('#bug-report-model').press('ArrowDown');page.keyboard.press('Escape');expect(page.locator('#bug-report-model-menu')).to_be_hidden();expect(page.locator('#bug-report-dialog')).to_be_visible()
     done('sd.report.model-search sd.report.model-keyboard','Search filters actual catalog; ArrowDown/Enter selects; Escape closes only picker',width)
     page.screenshot(path=str(a.output/f'models-{width}.png'));context.close()
   finally:browser.close()
 print('PASS remaining model/directory controls',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/models')
