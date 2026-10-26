#!/usr/bin/env python3
"""External Kiro ACP: native history, live owner, no implicit load/control, node and Hub."""
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
import argparse
import json
import shutil
import tempfile

from playwright.sync_api import sync_playwright, expect
from codex_sdk_browser import server
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from kiro_sdk_fixtures import KiroFixture
from node_auth_fixtures import TOKEN, node_env
from send_browser import initialize
from sdkhost_fixtures import stop_private_sdkhosts
from spawned_by_fixtures import START, proc_pid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-kext-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root = Path(tmp)
        for name in ('claude', 'codex', 'grok', 'state'):
            (root / name).mkdir()
        fixture = KiroFixture(root, binary.with_name('ptyhost'))
        stack.callback(stop_private_sdkhosts, root)
        initialize('--initialize-lifecycle', root / 'ledger', binary)
        native = fixture.home / '.kiro/sessions/bucket' / fixture.sid
        native.mkdir(parents=True)
        summary = {'dataModelVersion':1, 'id':fixture.sid, 'title':'外部 ACP 独立审查',
                   'workspacePaths':[str(fixture.work)], 'modelId':'native-observed-model',
                   'createdAt':'2026-10-23T15:18:58Z','lastModifiedAt':'2026-10-23T15:25:41Z','status':'in_progress'}
        (native / 'session.json').write_text(json.dumps(summary))
        def record(mid, kind, **fields):
            return json.dumps({'id':mid,'timestamp':'2026-10-23T15:25:41Z','payload':{'type':kind,**fields}}, ensure_ascii=False)+'\n'
        history = record('u1','user',content='外部审查任务：核对原生历史、工具结果及运行状态。')
        history += record('a1','assistant',content='正在检查审查材料和相关证据。',operationType='Reasoning')
        history += record('tc1','tool_call',toolCallId='call-1',toolName='read_file',args={'path':'review.txt'},status='in_progress')
        history += record('tr1','tool_result',toolCallId='call-1',content='原生工具输出：证据完整。',success=True)
        for i in range(24):
            history += record(f'thought-{i}','assistant',content=f'核对第 {i+1} 项材料：保留原始证据、工具输出和审查结论。',operationType='Reasoning')
            history += record(f'call-{i}','tool_call',toolCallId=f'audit-{i}',toolName='execute_bash',args={'command':f'cat evidence-{i}.txt'},status='completed')
            history += record(f'result-{i}','tool_result',toolCallId=f'audit-{i}',content='已核对材料内容与记录。\n' * 12,success=True)
        history += record('a2','assistant',content='已读取外部会话内容。')
        messages = native / 'messages.jsonl'
        messages.write_text(history)
        START[700] = 10000
        proc_pid(fixture.proc, 700, 'node', ['node','/private/kiro/acp-server.js'], 1)
        log = root / '.kiro/logs/current/kiro.log'
        log.parent.mkdir(parents=True)
        log.write_text(json.dumps({'timestamp':'2026-10-23T15:21:53Z','message':f'[KiroAgent] ACP session/load complete sessionId={fixture.sid} messages=5'})+'\n')
        fd = fixture.proc / '700/fd'
        fd.mkdir(exist_ok=True)
        (fd / '21').symlink_to(log)
        # A process merely mentioning a SID without owner evidence must not light it.
        proc_pid(fixture.proc, 701, 'node', ['node','/private/kiro/acp-server.js',fixture.empty_sid], 1)
        port = free_port()
        env = {**fixture.env, **node_env(root,port,'127.0.0.0/8'), 'SESSIONDOCK_NATIVE_SDKS':'auto',
               'SESSIONDOCK_STATE_DIR':str(root/'state'),'SESSIONDOCK_LIFECYCLE_DIR':str(root/'ledger'),
               'SESSIONDOCK_PTYHOST_DIR':str(fixture.host_dir)}
        base = stack.enter_context(server(SimpleNamespace(root=root),binary,'off',env))
        hub_root = root / 'hub'; hub_root.mkdir()
        hub = Hub(binary.with_name('sessiondock-hub'),hub_root,[SimpleNamespace(name='External',token=TOKEN,port=port)])
        hub.start(); stack.callback(hub.stop)
        browser = pw.chromium.launch(headless=True); stack.callback(browser.close)
        for kind, url in [('node',base),('hub',f'http://127.0.0.1:{hub.port}')]:
            page = browser.new_page(viewport={'width':1280,'height':900})
            page.set_default_timeout(20000)
            page.goto(url,wait_until='domcontentloaded')
            page.wait_for_function('T.listLoaded')
            page.wait_for_function('''async sid=>{await loadSessions(true);return S.sessions.some(r=>r.sid===sid)}''',arg=fixture.sid)
            uid = page.evaluate('sid=>S.sessions.find(r=>r.sid===sid).uid',fixture.sid)
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            expect(page.locator('#msgs')).to_contain_text('已读取外部会话内容。')
            expect(page.locator('#msgs')).to_contain_text('正在检查审查材料和相关证据。')
            page.wait_for_function('uid=>S.live?.has(uid)',arg=uid)
            expect(page.locator(f'#side .item[data-uid="{uid}"] .item-status')).to_have_class(__import__('re').compile(r'.*visible.*'))
            assert not fixture.received('session/load'), 'Observation loaded the native session'
            unknown_uid = page.evaluate('sid=>S.sessions.find(r=>r.sid===sid).uid',fixture.empty_sid)
            assert not page.evaluate('uid=>S.live.has(uid)',unknown_uid), 'Unbound ACP process marked another session live'
            assert not page.evaluate('uid=>S.liveSdk.has(uid)',uid), 'External observation granted managed SDK ownership'
            expect(page.locator('#a-session-toggle')).to_have_attribute('aria-disabled','true')
            batch = page.evaluate('async uid=>(await fetch("/api/messages/"+encodeURIComponent(uid))).json()',uid)
            assert any(m.get('tool_result') == '原生工具输出：证据完整。' for m in batch['messages']), batch
            result = page.evaluate('''async uid=>{const r=await fetch('/api/native-sdk/open',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({uid})});return {status:r.status,body:await r.text()}}''',uid)
            assert result['status'] >= 400 and '外部 ACP' in result['body'],result
            assert not fixture.received('session/load')
            expect(page.locator('#sdk-composer .sdk-status')).to_contain_text('外部只读')
            expect(page.locator('#sdk-input')).to_be_disabled()
            takeover = page.evaluate("""async uid=>{const r=await fetch('/api/term/takeover',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({uid})});return {status:r.status,body:await r.text()}}""",uid)
            assert takeover['status'] == 409 and '外部 ACP' in takeover['body'], takeover
            assert not any('fixture_cli_start' in r for r in fixture.received())
            page.keyboard.press('Escape')
            for width,theme in [(1280,'light'),(390,'dark'),(1280,'dark'),(390,'light')]:
                page.set_viewport_size({'width':width,'height':900});page.emulate_media(color_scheme=theme)
                if width == 390: page.evaluate('showMobileDetail()')
                if args.screenshots: page.screenshot(path=str(args.screenshots/f'kiro-external-{kind}-{width}-{theme}.png'),full_page=True)
            page.locator('.sdk-history .turn-process .fold-toggle').first.click()
            expect(page.locator('.sdk-history .turn-process')).not_to_have_class(__import__('re').compile(r'.*folded.*'))
            page.locator('.sdk-history .grp .fold-toggle').first.click()
            expect(page.get_by_text('原生工具输出：证据完整。',exact=True)).to_be_visible()
            if args.screenshots: page.screenshot(path=str(args.screenshots/f'kiro-external-{kind}-expanded.png'),full_page=True)
            page.close()
        # Same open browser follows new native messages without loading ACP.
        page = browser.new_page()
        page.goto(base,wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded && S.sessions.some(r=>r.source==="kiro")')
        uid = page.evaluate('sid=>S.sessions.find(r=>r.sid===sid).uid',fixture.sid)
        page.locator(f'#side .item[data-uid="{uid}"]').click()
        with messages.open('a') as f: f.write(record('a3','assistant',content='外部追加内容已同步。'))
        expect(page.locator('#msgs')).to_contain_text('外部追加内容已同步。',timeout=20000)
        shutil.rmtree(fixture.proc/'700')
        page.wait_for_function('uid=>!S.live?.has(uid)',arg=uid,timeout=20000)
        expect(page.locator('#msgs')).to_contain_text('外部追加内容已同步。')
        expect(page.locator('#sdk-input')).to_be_enabled()
        # PID reuse with old log content does not resurrect liveness.
        START[700] = 999999999999
        proc_pid(fixture.proc, 700, 'node', ['node','/private/kiro/acp-server.js'], 1)
        (fixture.proc/'700/fd/21').symlink_to(log)
        page.evaluate('refreshLive(true)')
        assert not page.evaluate('uid=>S.live.has(uid)',uid)
        assert not fixture.received('session/load')
        # Once the external owner is gone, an explicit user action may start SDK.
        page.locator('#sdk-input').click()
        assert not fixture.received('session/load')
        page.locator('#sdk-composer .composer-run-action').click()
        page.wait_for_function('SdkView.connected && !SdkView.readOnly')
        page.locator('#sdk-input').fill('Managed SDK after external exit')
        page.locator('#sdk-send').click()
        expect(page.locator('#msgs')).to_contain_text('Kiro ACP V3 final answer for: Managed SDK after external exit')
        assert len(fixture.received('session/load')) == 1
        page.evaluate('''async uid=>fetch('/api/native-sdk/stop',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({uid})})''',uid)
        print('Kiro external ACP node/Hub history, live, append, exit, conflict, explicit SDK handoff: PASS',flush=True)

if __name__ == '__main__':
    main()
