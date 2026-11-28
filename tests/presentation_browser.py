#!/usr/bin/env python3
"""Backend presentation plans exercised by history clicks and a private native CLI."""
import argparse
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
import tempfile
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, build_corpus, isolated_server, codex_row
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from tool_group_fold_browser import message, tool, result, activity
from claude_sdk_fixtures import ClaudeFixture
from sdk_mode_fixtures import provider
from claude_sdk_browser import open_sdk, send, wait_snapshot

CHECK = r"""({messages, presentation, native = false}) => {
  rememberPresentation(messages, presentation);
  const clean = plan => plan.map(p => p.m ? {m:p.m, sealed:!!p.sealedTurnHead}
    : p.g ? {g:p.g, open:!!p.open}
    : {turn:{items:p.turn.items, key:p.turn.key, hasConclusion:p.turn.hasConclusion,
      inferred:p.turn.inferred, interrupted:p.turn.interrupted, plan:clean(p.turn.plan)},open:p.open});
  const failures = [];
  for (const span of presentation.spans) {
    const part = messages.slice(span.start,span.end);
    for (const [mode, opts] of Object.entries({active:{},fold:{foldTail:true},complete:{tailComplete:true}})) {
      if (!span[mode]) continue;
      for (const openTail of [false,true]) {
        const options = {...opts,openTail};
        const server = backendPlan(part,options);
        if (!server) throw Error('backend plan not consumed: '+mode);
        const local = planTurnsLocal(native ? part.map(m=>({...m,turn_id:m.turn_id||String(m.id)})) : part,options);
        const a=clean(server), b=clean(local);
        // Native fallback fills turn_id on a copy; compare the same exposed identities.
        if (native) {
          const strip = value => { if (Array.isArray(value)) return value.map(strip);
            if(value && typeof value==='object') return Object.fromEntries(Object.entries(value)
              .filter(([k])=>k!=='turn_id').map(([k,v])=>[k,strip(v)])); return value; };
          if(JSON.stringify(strip(a))!==JSON.stringify(strip(b))) failures.push({mode,a:strip(a),b:strip(b)});
        } else if (JSON.stringify(a)!==JSON.stringify(b)) failures.push({mode,a,b});
        for(const p of server) if(p.turn && JSON.stringify(p.turn.summary)!==JSON.stringify(turnProcessSummary(p.turn.items)))
          failures.push({mode,summary:p.turn.summary,expected:turnProcessSummary(p.turn.items)});
      }
    }
  }
  return failures;
}"""


def exercise_resume(browser, fixture, base, screenshots, label):
    page = browser.new_page(viewport={'width': 1280, 'height': 900}, color_scheme='light')
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    uid = page.locator('#side .item[data-uid^="claude:"]').first.get_attribute('data-uid')
    open_sdk(page, fixture, expect, uid=uid)
    send(page, 'presentation-resume')
    expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final presentation-resume')
    page.wait_for_function('NativeSdkView.raw && !NativeSdkView.running')
    page.reload(wait_until='domcontentloaded')
    expect(page.locator('.native-sdk-pane')).to_be_visible()
    live = page.locator('.sdk-live')
    folded = live.locator(':scope > .turn-process.folded').last
    expect(folded).to_be_visible()
    page.wait_for_function("""NativeSdkView.wireSnapshot.version !== undefined
        && NativeSdkView.raw.presentation?.version === 1
        && [...NativeSdkView.nodes.values()].some(node => node._sdkMaterialize)""")
    page.evaluate("""() => {
      const view = NativeSdkView;
      const pending = [...view.nodes.values()].find(node => node._sdkMaterialize
        && node._sdkMessage.role === 'thinking');
      window.replayThinking = {id: pending.dataset.sdkItem, item: pending._sdkRawItem};
    }""")
    gate = fixture.received.with_name('presentation-state')
    gate.write_text('running')
    page.wait_for_function('NativeSdkView.running')
    assert page.evaluate("""NativeSdkView.raw.items.find(i => i.id === replayThinking.id) === replayThinking.item""")
    thinking = live.locator('[data-role="thinking"]')
    expect(thinking).to_be_visible()
    expect(thinking).to_contain_text('Claude resumed process detail')
    if screenshots:
        page.screenshot(path=str(screenshots / f'{label}-claude-resumed-wide.png'), full_page=True)
        page.set_viewport_size({'width': 390, 'height': 900})
        page.emulate_media(color_scheme='dark')
        page.evaluate('showMobileDetail()')
        expect(thinking).to_be_visible()
        page.screenshot(path=str(screenshots / f'{label}-claude-resumed-narrow.png'), full_page=True)
    gate.write_text('idle')
    page.wait_for_function('!NativeSdkView.running')
    expect(folded).to_be_visible()
    folded.locator('.turn-preview .fold-toggle').click()
    expect(live.locator(':scope > .turn-process:not(.folded)')).to_contain_text('Claude resumed process detail')
    assert not errors, errors
    page.close()
    print(f'PASS {label} Claude folded replay resumes from a native delta and folds on idle')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as pw, tempfile.TemporaryDirectory(prefix='sessiondock-presentation-') as temp:
        browser = pw.chromium.launch(headless=True, args=['--no-sandbox'])
        corpus = build_corpus(Path(temp)/'history')
        rows=[codex_row('session_meta', {'id':'planned','cwd':'/synthetic/presentation'})]
        for turn in range(8):
            tid=f'planned-{turn}'
            rows += [message(tid,'user',f'规划测试 {turn}'),activity(tid,'started')]
            for index in range(30):
                rows += [message(tid,'assistant',f'检查进展 {index}','commentary'),tool(tid,index),result(tid,index,f'结果 {index}')]
            rows += [message(tid,'assistant',f'最终结论 {turn}','final_answer'),activity(tid,'complete')]
        for index,row in enumerate(rows):
            row['timestamp']=(datetime(2026,11,16,tzinfo=timezone.utc)+timedelta(milliseconds=25*index)).isoformat()
        corpus.put('planned','codex',rows,[])
        with isolated_server(corpus,args.binary) as (base,_):
            for width,theme in [(1280,'light'),(390,'dark')]:
                page=browser.new_page(viewport={'width':width,'height':900},color_scheme=theme)
                errors=[]
                page.on('pageerror',lambda e:errors.append(str(e)))
                page.goto(base,wait_until='domcontentloaded')
                page.wait_for_function('T.listLoaded')
                page.locator(f'#side .item[data-uid="{corpus.uid("planned")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('最终结论 7')
                entry=page.evaluate('cache.get(viewKey(S.sel,S.agent))')
                assert entry.get('presentation'), 'history response omitted presentation'
                failures=page.evaluate(CHECK,{'messages':entry['msgs'],'presentation':entry['presentation']})
                assert not failures,json.dumps(failures,ensure_ascii=False)[:6000]
                folded=page.locator('#msgs > .turn-process').last
                expect(folded.locator('.turn-process-body .msg')).to_have_count(0)
                folded.locator('.turn-preview .fold-toggle').click()
                expect(folded.locator('.turn-process-body')).to_contain_text('检查进展')
                if args.screenshots: page.screenshot(path=str(args.screenshots/f'history-{width}-{theme}.png'),full_page=True)
                assert not errors,errors
                page.close()
        fixture=ClaudeFixture(Path(temp)/'native')
        with fixture.server(args.binary) as base:
            page=browser.new_page(viewport={'width':1280,'height':900})
            page.goto(base,wait_until='domcontentloaded')
            open_sdk(page,fixture,expect)
            for prompt in ['presentation-heavy','presentation second']:
                send(page,prompt)
                expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final '+prompt,timeout=30000)
                wait_snapshot(page,fixture.uid,lambda raw:not raw['running'])
            page.close()
            for server_plan in [False,True]:
                page=browser.new_page(viewport={'width':1280,'height':900})
                page.goto(base,wait_until='domcontentloaded')
                page.evaluate('''serverPlan => {
                  window.builtThinking=0; window.thinkingRenderMs=0; window.replayWork=[];
                  const original=msgNode;
                  msgNode=m=>{const start=performance.now(); if(m.role==='thinking') builtThinking++;
                    try {return original(m);} finally {if(m.role==='thinking') thinkingRenderMs+=performance.now()-start;} };
                  const apply=NativeSdkView.applySnapshot;
                  NativeSdkView.applySnapshot=function(raw) {
                    return apply.call(this,serverPlan?raw:{...raw,presentation:null});
                  };
                  const commit=NativeSdkView.applySnapshotNow;
                  NativeSdkView.applySnapshotNow=function(...args) {
                    const start=performance.now();
                    try {return commit.apply(this,args);} finally { replayWork.push(performance.now()-start); }
                  };
                }''',server_plan)
                page.wait_for_function('T.listLoaded')
                page.locator(f'#side .item[data-uid="{fixture.uid}"]').click()
                page.locator('#a-sdk').click()
                page.wait_for_function('NativeSdkView.connected')
                page.wait_for_function('NativeSdkView.items.some(m=>m.role==="thinking") && !NativeSdkView.snapshotBuild')
                built=page.evaluate('window.builtThinking')
                assert built == (0 if server_plan else 160), ('unexpected replay materialization',server_plan,built)
                print('Native replay profile:',{'server_plan':server_plan,'hidden_markdown_built':built,
                      'thinking_render_ms':page.evaluate('window.thinkingRenderMs'),
                      'commit_ms':page.evaluate('window.replayWork')})
                folded=page.locator('.sdk-live > .turn-process').first
                folded.locator('.turn-preview .fold-toggle').click()
                page.wait_for_function('window.builtThinking === 160')
                expect(folded.locator('.turn-process-body')).to_contain_text('Presentation thinking 159')
                if server_plan:
                    payload=page.evaluate('({messages:NativeSdkView.items,presentation:NativeSdkView.raw.presentation,native:true})')
                    assert payload.get('presentation'), 'native publication omitted presentation'
                    failures=page.evaluate(CHECK,payload)
                    assert not failures,json.dumps(failures,ensure_ascii=False)[:6000]
                    if args.screenshots: page.screenshot(path=str(args.screenshots/'native-wide.png'),full_page=True)
                page.close()
        fixture = ClaudeFixture(Path(temp) / 'resume')
        with fixture.server(args.binary) as base:
            exercise_resume(browser, fixture, base, args.screenshots, 'node')
        root = Path(temp) / 'resume-via-hub'
        root.mkdir()
        with provider('claude', root, args.binary) as (_, fixture, extra):
            hub_root = root / 'hub'
            hub_root.mkdir()
            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root, [
                SimpleNamespace(name='ClaudeReplayNode', token=TOKEN,
                                port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
            hub.start()
            try:
                exercise_resume(browser, fixture, f'http://127.0.0.1:{hub.port}', args.screenshots, 'hub')
            finally:
                hub.stop()
        for source in ['agy','grok','kiro','opencode','codebuddy','droid']:
            root=Path(temp)/source
            root.mkdir()
            with provider(source,root,args.binary) as (base,peer,_):
                page=browser.new_page(viewport={'width':1280,'height':900})
                page.goto(base,wait_until='domcontentloaded')
                page.wait_for_function('T.listLoaded')
                uid=page.locator(f'#side .item[data-uid^="{source}:"]').first.get_attribute('data-uid')
                open_sdk(page,peer,expect,uid=uid)
                send(page,'presentation check')
                wait_snapshot(page,uid,lambda raw:not raw['running'] and any(
                    'presentation check' in str(m.get('text',m.get('content','')))
                    for m in raw.get('items',[])))
                payload=page.evaluate('''source=>{
                  const raw=NativeSdkView.raw, presentation=raw.presentation;
                  return {presentation,native:true,messages:presentation.ids.map(id=>
                    nativeSdkMessage(raw.items.find(m=>String(m.id)===id),source))};
                }''',source)
                failures=page.evaluate(CHECK,payload)
                assert not failures,(source,json.dumps(failures,ensure_ascii=False)[:6000])
                for width,theme in [(1280,'light'),(390,'dark')]:
                    page.set_viewport_size({'width':width,'height':900})
                    page.emulate_media(color_scheme=theme)
                    if not page.locator('.native-sdk-pane').is_visible():
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator('.native-sdk-pane')).to_be_visible()
                    page.evaluate('() => new Promise(resolve => requestAnimationFrame(resolve))')
                    if args.screenshots: page.screenshot(path=str(args.screenshots/f'{source}-{width}-{theme}.png'),full_page=True)
                print('PASS native publication and semantic plan:',source)
                page.close()
        browser.close()
    print('PASS backend presentation: real history spans, folded expansion, native CLI publication, semantic plans and summaries')

if __name__=='__main__': main()
