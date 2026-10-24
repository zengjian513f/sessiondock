#!/usr/bin/env python3
"""Kiro model failure backoff, cancellation and no input replay in Chromium."""
from __future__ import annotations
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import sys
import tempfile
import time
from types import SimpleNamespace
from kiro_sdk_fixtures import FakeKiroAcp, KiroFixture

CONTINUE = '重试：继续上次因模型不可用而中断的任务。保留已完成的工作，从未完成处继续，不要重复执行已完成的操作。'
ERROR = "The model 'claude-opus-5.5' is not available. Please select a different model and try again. (Request ID: fixture-model-error)"

class RetryPeer(FakeKiroAcp):
    def run_prompt(self, message):
        text = ' '.join(b.get('text', '') for b in message['params'].get('prompt', []))
        self.append_v3_message('user', text)
        with (self.sessions_root / 'attempts.jsonl').open('a') as f:
            f.write(json.dumps({'text': text, 'time': time.monotonic(), 'params': message['params']}) + '\n')
        if text.startswith('FAIL') or text == CONTINUE and not (self.sessions_root / 'recover').exists():
            self.update('tool_call', toolCallId='done-' + str(message['id']), title='Already executed work', kind='edit', status='completed')
            self.send({'jsonrpc':'2.0', 'id':message['id'], 'error':{
                'code':-32000, 'message':ERROR,
                'data':{'errorType':'InvalidModelError','retryErrorType':'CLIENT_ERROR'}}})
        elif text == 'OTHER_ERROR':
            self.error(message, -32000, 'Synthetic authentication failed')
        else:
            self.append_v3_message('assistant', 'Recovered without replaying original work')
            self.update('agent_message_chunk', content={'type':'text','text':'Recovered without replaying original work'})
            self.reply(message, {'stopReason':'end_turn'})

def fake_main():
    if 'acp' in sys.argv:
        peer = RetryPeer()
        for line in sys.stdin:
            if line.strip(): peer.handle(json.loads(line))
    else:
        from kiro_sdk_fixtures import main
        sys.argv.remove('--fake-model-retry'); main()

def exercise(browser, base, fixture, width, theme, shots, long=False):
    from playwright.sync_api import expect
    from sdk_ui_fixtures import start_sdk
    context = browser.new_context(viewport={'width':width,'height':900}, color_scheme=theme)
    page = context.new_page(); page.set_default_timeout(20000)
    errors=[]; page.on('pageerror', lambda e: errors.append(str(e)))
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    page.wait_for_function('async sid => {await loadSessions(true); return S.sessions.some(s=>s.sid===sid)}',arg=fixture.sid,polling=300)
    uid=page.evaluate('sid=>S.sessions.find(s=>s.sid===sid).uid',fixture.sid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.locator('#a-sdk').click(); start_sdk(page)
    page.wait_for_function('SdkView.connected && !SdkView.readOnly')
    def attempts():
        p=fixture.sessions/'attempts.jsonl'
        return [json.loads(x) for x in p.read_text().splitlines()] if p.exists() else []
    def send(text):
        page.locator('#sdk-input').fill(text);page.locator('#sdk-send').click()
        expect(page.locator('#sdk-input')).to_have_value('')
    def wait(delay):
        page.wait_for_function('d=>SdkView.raw?.retry?.delay_seconds===d',arg=delay,timeout=(delay+15)*1000)
        expect(page.locator('#sdk-stop')).to_be_visible()
        expect(page.locator('#sdk-status-menu')).to_contain_text(f'{delay} 秒后自动重试')
        expect(page.locator('.sdk-pane .sdk-status')).not_to_have_class('sdk-status blocked')
    def shot(label):
        if shots:
            page.locator('.sdk-pane .sdk-status').click()
            page.screenshot(path=str(shots/f'{width}-{theme}-{label}.png'),full_page=True)
            page.keyboard.press('Escape')
    def cancel():
        page.locator('#sdk-stop').click()
        page.wait_for_function('!SdkView.raw?.retry && !SdkView.running')
    retry_bubbles = page.locator('.sdk-pane .msg[data-role="user"]').filter(has_text=CONTINUE)
    previous_retries = retry_bubbles.count()
    (fixture.sessions/'recover').unlink(missing_ok=True)
    before=len(attempts());send('FAIL original input with completed work');wait(15);shot('waiting')
    page.locator('#sdk-input').fill('保留未发送的草稿')
    from codex_sdk_compose_browser import saved
    saved(page,uid,'保留未发送的草稿')
    page.reload(wait_until='domcontentloaded')
    wait(15);expect(page.locator('#sdk-input')).to_have_value('保留未发送的草稿')
    wait(30)
    expect(retry_bubbles).to_have_count(previous_retries + 1)
    retry_bubbles.last.scroll_into_view_if_needed()
    shot('retry-prefix')
    page.reload(wait_until='domcontentloaded')
    expect(retry_bubbles).to_have_count(previous_retries + 1)
    expect(page.locator('#sdk-input')).to_have_value('保留未发送的草稿')
    assert attempts()[before + 1]['text'] == CONTINUE
    if long:
        for delay in (60,120):
            print('waiting for actual backoff before',delay,flush=True)
            wait(delay)
        records=attempts()[before:]
        assert len(records)==4,records
        for prev,cur,delay in zip(records,records[1:],(15,30,60)):
            assert cur['time']-prev['time']>=delay-.2, records
        for r in records[1:]:
            assert r['text']==CONTINUE and set(r['params'])=={'sessionId','prompt'}, r
    if long: print('PASS actual 15/30/60-second backoff and 120-second next interval',flush=True)
    cancel();page.reload(wait_until='domcontentloaded')
    page.wait_for_function('SdkView.connected && !SdkView.raw.retry')
    send('FAIL reset delay');wait(15)
    send('New user input cancels the timer')
    page.wait_for_function('SdkView.raw.last_turn?.status==="completed" && !SdkView.raw.retry')
    send('FAIL change settings');wait(15)
    # Use the existing model selector, not a direct backend mutation.
    current=page.locator('#sdk-model').input_value()
    page.locator('#sdk-model').select_option('claude-haiku-4.5' if current=='auto' else 'auto')
    page.wait_for_function('!SdkView.raw.retry')
    send('OTHER_ERROR')
    page.wait_for_function('SdkView.raw.error?.includes("authentication failed")')
    assert page.evaluate('SdkView.raw.retry') is None
    shot('ordinary-error')
    if long:
        send('FAIL close all pages');wait(15)
        before=len(attempts());(fixture.sessions/'recover').touch()
        context.close()
        deadline=time.monotonic()+24
        while len(attempts())==before and time.monotonic()<deadline: time.sleep(.1)
        assert len(attempts())==before+1
        assert attempts()[-1]['text']==CONTINUE
    else:
        context.close()
    assert not errors,errors
    print(f'PASS Kiro retry {width}/{theme}: cancellation, settings, new input, unrelated errors, no original replay',flush=True)



def restart_case(browser, base, fixture, restart):
    from playwright.sync_api import expect
    page=browser.new_page()
    page.goto(base,wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    page.wait_for_function('async sid=>{await loadSessions(true);return S.sessions.some(s=>s.sid===sid)}',arg=fixture.sid)
    uid=page.evaluate('sid=>S.sessions.find(s=>s.sid===sid).uid',fixture.sid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.locator('#a-sdk').click()
    page.wait_for_function('SdkView.connected && !SdkView.readOnly')
    page.locator('#sdk-input').fill('FAIL restart must cancel historical retry')
    page.locator('#sdk-send').click()
    page.wait_for_function('SdkView.raw.retry?.delay_seconds===15')
    before=len(fixture.received('session/prompt'))
    page.goto('about:blank');base=restart()
    page.goto(base,wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    page.wait_for_function('async uid=>{await loadSessions(true);return S.sessions.some(s=>s.uid===uid)}',arg=uid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.locator('#a-sdk').click()
    page.wait_for_function('SdkView.connected && !SdkView.readOnly')
    assert page.evaluate('SdkView.raw.retry') is None
    expect(page.locator('#sdk-stop')).to_be_hidden()
    page.wait_for_timeout(16000)
    assert len(fixture.received('session/prompt'))==before, 'restart replayed a completed failure'
    page.locator('#sdk-input').fill('FAIL new failure after restart')
    page.locator('#sdk-send').click()
    page.wait_for_function('SdkView.raw.retry?.delay_seconds===15')
    page.locator('#sdk-stop').click()
    page.wait_for_function('!SdkView.raw.retry')
    page.close()
    print('PASS service restart cancels old failure, preserves host, new failure retries',flush=True)


def main():
    if '--fake-model-retry' in sys.argv:return fake_main()
    from playwright.sync_api import sync_playwright
    from codex_sdk_browser import server
    from history_fixtures import BINARY
    from hub_fixtures import Hub, free_port
    from node_auth_fixtures import TOKEN, node_env
    from send_browser import initialize
    from sdkhost_fixtures import stop_private_sdkhosts
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--hub-binary',type=Path,default=BINARY.with_name('sessiondock-hub'))
    parser.add_argument('--screenshots',type=Path)
    parser.add_argument('--skip-long',action='store_true',help='Reuse prior real-timer validation while checking later browser cases')
    args=parser.parse_args()
    if args.screenshots:args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-kiro-model-retry-') as tmp,sync_playwright() as pw:
        root=Path(tmp)
        for name in ('claude','codex','grok','state','trash'):(root/name).mkdir()
        fixture=KiroFixture(root)
        config=json.loads(fixture.launcher.read_text());config['profiles'][0]['args']=[str(Path(__file__).resolve()),'--fake-model-retry']
        fixture.launcher.write_text(json.dumps(config))
        initialize('--initialize-lifecycle',root/'ledger',args.binary.resolve())
        port=free_port()
        env={**fixture.env,**node_env(root,port,'127.0.0.0/8'),'SESSIONDOCK_NATIVE_SDKS':'auto',
             'SESSIONDOCK_STATE_DIR':str(root/'state'),'SESSIONDOCK_LIFECYCLE_DIR':str(root/'ledger'),
             'SESSIONDOCK_TRASH_DIR':str(root/'trash'),'SESSIONDOCK_PTYHOST_DIR':str(fixture.host_dir)}
        browser=pw.chromium.launch(headless=True)
        try:
            with ExitStack() as stack:
                node=ExitStack();stack.callback(lambda:node.close())
                base=node.enter_context(server(SimpleNamespace(root=root),args.binary,'off',env))
                def restart():
                    node.close()
                    return node.enter_context(server(SimpleNamespace(root=root),args.binary,'off',env))
                exercise(browser,base,fixture,1280,'light',args.screenshots,not args.skip_long)
                exercise(browser,base,fixture,390,'dark',args.screenshots)
                restart_case(browser,base,fixture,restart)
                hubroot=root/'hub';hubroot.mkdir()
                hub=Hub(args.hub_binary.resolve(),hubroot,[SimpleNamespace(name='KiroNode',token=TOKEN,port=port)])
                hub.start()
                try:
                    exercise(browser,f'http://127.0.0.1:{hub.port}',fixture,1280,'dark',args.screenshots)
                    exercise(browser,f'http://127.0.0.1:{hub.port}',fixture,390,'light',args.screenshots)
                finally:hub.stop()
        finally:
            browser.close();stop_private_sdkhosts(root/'state')

if __name__=='__main__':main()
