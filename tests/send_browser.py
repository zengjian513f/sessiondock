#!/usr/bin/env python3
"""Server drafts and one-shot conversation SEND against an isolated fake CLI.
Covers busy sends, lost responses, session isolation, uploads, cancellation,
and startup choice refusal/recovery without browser message persistence.
Older helper functions remain available to the terminal ownership suites.
"""
import hashlib
import json
import os
import socket
import subprocess
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect

from history_parity import REPO, BINARY, Corpus, isolated_server
from popups import on_popup  # noqa: E402

FAKE_CLI = REPO / "tests/fake_claude_cli.py"
SETTINGS = "/synthetic/bridge-settings.json"

XTERM_TEXT = """() => [...T.views.values()].map(view => {
  const buffer = view.term?.buffer?.active;
  if (!buffer) return '';
  const lines = [];
  for (let i = 0; i < buffer.length; i++) {
    const line = buffer.getLine(i);
    if (line) lines.push(line.translateToString(false).trimEnd());
  }
  return lines.join('\\n');
}).join('\\n')"""


def claude_uid(root, sid):
    path = root / "claude/project-history" / f"{sid}.jsonl"
    return "claude:" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


def xterm_includes(page, text, timeout=15000):
    page.wait_for_function("text => (" + XTERM_TEXT + ")().includes(text)", arg=text, timeout=timeout)


def initialize(flag, directory, binary=BINARY):
    with socket.socket() as occupied:
        occupied.bind(("127.0.0.1", 0))
        env = {"PATH": "/usr/bin:/bin", "SESSIONDOCK_BIND": "127.0.0.1:%d" % occupied.getsockname()[1]}
        done = subprocess.run([str(binary), flag, str(directory)], cwd=REPO, env=env, capture_output=True, timeout=20)
    assert done.returncode == 0, done.stderr.decode()


def create_claude(page, base, work, *, open_terminal=True):
    if not page.locator("#new-session").is_visible():
        page.locator("#header-more-btn").click()
    page.locator("#new-session").click()
    page.locator('input[name="new-source"][value="claude"]').check()
    page.locator("#new-cwd").fill(str(work / "claude-area"))
    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/create") as created:
        page.locator("#new-session-go").click()
    assert created.value.status == 200, created.value.text()
    receipt = created.value.json()
    assert receipt["running"] and receipt["launch_kind"] == "new_assigned", receipt
    expect(page.locator("#termpane")).to_be_hidden()
    expect(page.locator("#composer")).to_be_visible()
    expect(page.locator("#cadd")).to_be_visible()
    expect(page.locator("#cesc")).to_be_visible()
    assert page.evaluate("T.views.size === 0 && T.openViews.size === 0")
    if not open_terminal:
        return receipt
    page.locator("#a-term").click()
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    xterm_includes(page, "FAKE_CLAUDE_READY sid=[%s]" % receipt["declared_sid"])
    return receipt


def wait_history(page, text, timeout=20000):
    try:
        page.wait_for_function(
            "text => [...document.querySelectorAll('#msgs .msg:not(.queued-send)')].some(n => n.textContent.includes(text))",
            arg=text, timeout=timeout)
    except Exception:
        print("history timeout:", page.evaluate("() => ({uid: S.sel, text: document.querySelector('#msgs')?.innerText})"), flush=True)
        raise


def main():
    if os.name != "posix":
        raise SystemExit("Reliable-send browser acceptance currently requires POSIX.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-send-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "work", "work/claude-area", "ledger", "delivery", "state", "bin", "home", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        python = Path(subprocess.check_output(["/bin/sh", "-c", "command -v python3"]).decode().strip()).resolve()
        wrapper = root / "bin/fake-claude"
        wrapper.write_text("#!/bin/sh\nexec %s %s \"$@\"\n" % (python, REPO / "tests/fake_conversation_cli.py"))
        wrapper.chmod(0o700)
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [
                {"id": "claude-cli-v1", "source": "claude", "executable": str(wrapper),
                 "args": ["--settings", SETTINGS, "--reply", "--delay", "2500", "--busy-footer", "--collapse-paste"], "new_args": ["--session-id", "{session_id}"],
                 "resume_args": ["--resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color",
                         "LANG": "C.UTF-8", "SESSIONDOCK_TEST_CLAUDE_ROOT": str(root / "claude"),
                         "SESSIONDOCK_TEST_GATE":str(root / "gate"),"SESSIONDOCK_TEST_GATE_TRACE":str(root / "gate.trace"),
                         "SESSIONDOCK_TEST_PASTE_DELAY":str(root / "paste-delay"),
                         "SESSIONDOCK_TEST_CLAUDE_QUEUE":str(root / "claude-queue"),
                         "SESSIONDOCK_TEST_CLAUDE_ESC_RESTORE":str(root / "esc-restore")}}]}))
        initialize("--initialize-lifecycle", root / "ledger")
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, state_dir=root / "state",
                                     file_roots=(root / "work",), file_write_roots=(root / "work",)) as (base, _):
                    errors, dialogs, sends = [], [], []
                    dialog_action = {'accept': True}

                    def watch(context):
                        context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                        context.on("request", lambda request: sends.append(request.post_data_json)
                            if urlsplit(request.url).path == "/api/session/conversation/send" else None)
                        page = context.new_page()
                        page.on("pageerror", lambda error: errors.append(str(error)))

                        def on_dialog(dialog):
                            dialogs.append((dialog.type, dialog.message))
                            try:
                                dialog.accept() if dialog_action['accept'] else dialog.dismiss()
                            except Exception:
                                pass
                        on_popup(page, on_dialog)
                        page.goto(base, wait_until="networkidle")
                        return page

                    context = browser.new_context(viewport={"width":1280,"height":900},service_workers="block")
                    page=watch(context)
                    receipt=create_claude(page,base,root / 'work',open_terminal=False)
                    page.wait_for_function("composerUid && !composerDraft().loading && takenOver(composerUid)")
                    assert page.evaluate('T.views.size')==0
                    uid='tmux:'+receipt['name']
                    build=context.request.get(base+'/api/meta').json()['build']
                    def send(text, check_width=False):
                        page.fill('#cinput',text)
                        if check_width:
                            idle_width=page.locator('#csend').evaluate('el => el.getBoundingClientRect().width')
                            with page.expect_response(lambda r:urlsplit(r.url).path=='/api/session/conversation/send',timeout=20000) as response:
                                page.locator('#csend').click()
                                page.wait_for_function("document.querySelector('#csend').getAttribute('aria-busy') === 'true'")
                                busy=page.locator('#csend').evaluate('''el => {
                                    const after=getComputedStyle(el,'::after');
                                    return {width:el.getBoundingClientRect().width,text:el.textContent,
                                            busy:el.getAttribute('aria-busy'),label:el.getAttribute('aria-label'),
                                            spin:after.animationName};
                                }''')
                                assert busy['text']=='发送' and busy['busy']=='true' and busy['label']=='发送中',busy
                                assert abs(busy['width']-idle_width)<0.51,(idle_width,busy)
                                assert 'send-spin' in (busy['spin'] or ''),busy
                            sent=response.value
                        else:
                            with page.expect_response(lambda r:urlsplit(r.url).path=='/api/session/conversation/send',timeout=20000) as response:
                                page.locator('#csend').click()
                            sent=response.value
                        assert sent.status==200,sent.text()
                        assert sent.json()['state']=='sent'
                        expect(page.locator('#cinput')).to_have_value('')
                        return sends[-1]
                    # Empty legacy evidence must not inject text. Launch may already
                    # have bound the pending session identity into an empty draft.
                    response=context.request.post(base+'/api/session/conversation/import',data={'uid':uid,'value':{'text':'','attachments':[],'quotes':[]}})
                    assert response.status==200,response.text()
                    row=context.request.get(base+'/api/session/conversation?uid='+uid).json()['draft']
                    value=row.get('value') or {}
                    assert not value.get('text') and not value.get('quotes'),row
                    if row['revision']==0 and row['value'] is None:
                        # Repair the already-produced nullable shape without consuming any input.
                        response=context.request.post(base+'/api/session/conversation',data={'uid':uid,'revision':0,'value':{'text':None,'attachments':None,'quotes':None}})
                        assert response.status==200,response.text()
                        repaired=response.json()['draft']['value']
                        assert repaired['text']=='' and repaired['attachments']==[] and repaired['quotes']==[]
                    page.reload(wait_until='networkidle')
                    # With no native user turn yet, reopen the pending instance explicitly.
                    page.evaluate('async receipt => {await loadTermList();await openPendingSession(receipt)}',receipt)
                    page.wait_for_function('composerUid && !composerDraft().loading')
                    assert not page.evaluate('composerDraft().storageError || composerDraft().loadFailed')
                    first=send('first busy input',check_width=True)
                    # A successful write is not a native message yet: the CLI holds it
                    # in its own queue. It shows as a queued bubble at the tail and the
                    # Send button does not spin (BUG-20260928-113817-9301c2).
                    expect(page.locator('#csend')).to_have_attribute('aria-busy','false')
                    expect(page.locator('#csend')).to_be_enabled()
                    expect(page.locator('#queued-sends .msg.queued-send[data-role=user]').filter(has_text='first busy input')).to_have_count(1)
                    expect(page.locator('#queued-sends .queued-send-state').first).to_have_text('已发送，等待 CLI 处理')
                    assert page.locator('#msgs .msg[data-role=user]:not(.queued-send)').filter(has_text='first busy input').count()==0
                    page.locator('#cinput').fill('second busy input')
                    expect(page.locator('#csend')).to_have_attribute('aria-busy','false')
                    # The fake now records Claude's own enqueue for the next line.
                    (root/'claude-queue').touch()
                    second=send('second busy input')
                    # The sends sit in the server's CLI state object, in send order,
                    # for every page and device (docs/cli-state.md). The first echo
                    # lands 2.5 s after its send and may already have retired that row.
                    checked=context.request.post(base+'/api/session/conversation/check',data={'uid':uid,'name':receipt['name'],'_build':build}).json()
                    queued=[row['request_id'] for row in checked['cli']['queued']]
                    assert queued in ([first['request_id'],second['request_id']],[second['request_id']]),checked['cli']
                    assert all(row['state']=='queued' and row['echo_hash'] for row in checked['cli']['queued']),checked['cli']
                    assert checked['cli']['instance']['running'] is True,checked['cli']
                    expect(page.locator('#queued-sends .msg.queued-send').filter(has_text='second busy input')).to_have_count(1)
                    # Claude's enqueue record shows the send is held in the CLI's
                    # own queue until its step ends (BUG-20260928-231633-9a7610).
                    second_bubble=page.locator('#queued-sends .msg.queued-send').filter(has_text='second busy input')
                    expect(second_bubble.locator('.queued-send-state')).to_have_text('已进入 CLI 队列，当前步骤结束后处理',timeout=10000)
                    expect(second_bubble).to_have_attribute('data-cli-queued','1')
                    checked=context.request.post(base+'/api/session/conversation/check',data={'uid':uid,'name':receipt['name'],'_build':build}).json()
                    held=[row for row in checked['cli']['queued'] if row['request_id']==second['request_id']]
                    assert held and isinstance(held[0]['cli_queued_at'],(int,float)) and held[0]['state']=='queued',checked['cli']
                    (root/'claude-queue').unlink()
                    assert first['request_id']!=second['request_id']
                    replay=context.request.post(base+'/api/session/conversation/send',data=first)
                    assert replay.status==200,replay.text()
                    jsonl=root/'claude/project-history'/f"{receipt['declared_sid']}.jsonl"
                    deadline=time.monotonic()+15
                    while not jsonl.exists() or jsonl.read_text().count('second busy input')<2:
                        assert time.monotonic()<deadline
                        time.sleep(.1)
                    users=[json.loads(line)['message']['content'] for line in jsonl.read_text().splitlines() if json.loads(line)['type']=='user']
                    assert users==['first busy input','second busy input'],users
                    page.wait_for_function("S.sel && !S.sel.startsWith('tmux:')",timeout=20000)
                    native=page.evaluate('S.sel')
                    wait_history(page,'second busy input')
                    expect(page.locator('#csend')).to_have_attribute('aria-busy','false')
                    expect(page.locator('#queued-sends')).to_have_count(0)
                    # Older identical history must not retire a new send's queued bubble.
                    send('first busy input')
                    expect(page.locator('#queued-sends .msg.queued-send').filter(has_text='first busy input')).to_have_count(1)
                    page.wait_for_function("() => [...document.querySelectorAll('#msgs .msg[data-role=user]:not(.queued-send)')].filter(n => n.textContent.includes('first busy input')).length === 2")
                    expect(page.locator('#queued-sends')).to_have_count(0)
                    expect(page.locator('#csend')).to_have_attribute('aria-busy','false')
                    # A CLI that repaints the paste after SEND's 3 s wait still
                    # gets Enter, like Python; the draft must not be stranded in
                    # the CLI editor behind a refusal (BUG-20260927-112827-5aa96e).
                    (root/'paste-delay').write_text('3500')
                    dialog_count=len(dialogs)
                    send('继续')
                    (root/'paste-delay').unlink()
                    assert len(dialogs)==dialog_count,dialogs
                    wait_history(page,'继续')
                    users=[json.loads(line)['message']['content'] for line in jsonl.read_text().splitlines() if json.loads(line)['type']=='user']
                    assert users.count('继续')==1,users
                    expect(page.locator('#csend')).to_have_attribute('aria-busy','false')
                    # Esc right after Enter: Claude puts the prompt back into its
                    # editor and keeps the user record. The page must say so, and
                    # a SEND must not be appended to that text and submitted with
                    # it (BUG-20260928-235840-93d732).
                    (root/'esc-restore').touch()
                    send('哈希为什么')
                    wait_history(page,'哈希为什么')
                    expect(page.locator('#queued-sends')).to_have_count(0)
                    page.locator('#cesc').click()
                    status=page.locator('#composer-input-status')
                    expect(status).to_contain_text('上一条消息已被 Esc 退回终端输入框',timeout=10000)
                    expect(page.locator('#csend')).to_be_disabled()
                    returned=page.locator('#msgs .msg[data-role=user]:not(.queued-send)').filter(has_text='哈希为什么')
                    expect(returned.locator('.returned-to-cli')).to_have_text('已被 Esc 退回终端输入框，CLI 未处理')
                    page.fill('#cinput','我没说过研报上云')
                    expect(page.locator('#csend')).to_be_disabled()
                    # SEND refuses before any paste even when a client skips the gate.
                    refused=context.request.post(base+'/api/session/conversation/send',data={
                        'uid':native,'name':receipt['name'],'request_id':'append-to-returned',
                        'text':'我没说过研报上云','attachments':[],'quotes':[],'_build':build})
                    assert refused.status==409 and refused.json()['code']=='cli_input_pending',refused.text()
                    # Clearing the PTY editor lifts the block; text typed in the
                    # PTY blocks SEND the same way.
                    page.evaluate("uid => sendToSession(null, ['C-u'], uid)",native)
                    expect(page.locator('#csend')).to_be_enabled(timeout=10000)
                    expect(page.locator('.returned-to-cli')).to_have_count(0)
                    page.evaluate("uid => sendToSession(null, ['typed in the pty'], uid)",native)
                    expect(status).to_contain_text('终端输入框里已有未发送的文字',timeout=10000)
                    expect(page.locator('#csend')).to_be_disabled()
                    expect(page.locator('.returned-to-cli')).to_have_count(0)
                    page.evaluate("uid => sendToSession(null, ['C-u'], uid)",native)
                    (root/'esc-restore').unlink()
                    expect(page.locator('#csend')).to_be_enabled(timeout=10000)
                    expect(status).to_be_hidden()
                    send('我没说过研报上云')
                    wait_history(page,'我没说过研报上云')
                    users=[json.loads(line)['message']['content'] for line in jsonl.read_text().splitlines() if json.loads(line)['type']=='user']
                    assert users[-2:]==['哈希为什么','我没说过研报上云'],users
                    assert not any('typed in the pty' in text for text in users),users
                    holder_context=browser.new_context(service_workers='block')
                    holder=watch(holder_context)
                    holder.locator(f'#side .item[data-uid="{native}"]').click()
                    holder.locator('#a-term').click()
                    holder.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                    held_token=holder.evaluate('name => T.views.get(name)?.inputLease?.token',receipt['name'])
                    assert held_token
                    # A stale hook card must not veto a currently writable PTY.
                    prompt_dir=root/'state/claude-prompts'
                    prompt_dir.mkdir(exist_ok=True)
                    stale_prompt=prompt_dir/f"{receipt['declared_sid']}.json"
                    stale_prompt.write_text(json.dumps({'version':1,'id':'stale-hook','state':'waiting',
                        'questions':[{'question':'Old question','options':[{'label':'Yes'},{'label':'No'}]}]}))
                    check=context.request.post(base+'/api/session/conversation/check',data={'uid':native,'_build':build})
                    assert check.status==200 and check.json()['input']['state']=='ready',check.text()
                    # The report worker uses the same SEND outside this page.
                    # Its successful receipt must clear an already-open viewer.
                    page.fill('#cinput','server-owned first task')
                    page.evaluate('''async () => {
                        const draft=composerDraft();
                        draft.requestId='server-owned-task';
                        draft.requestText=JSON.stringify({text:draft.text,attachments:[],quotes:[]});
                        await persistComposerDraft();
                    }''')
                    row=context.request.get(base+'/api/session/conversation?uid='+native).json()['draft']
                    external=context.request.post(base+'/api/session/conversation/send',data={
                        'uid':native,'name':receipt['name'],'request_id':'server-owned-task',
                        'text':'server-owned first task','draft_revision':row['revision'],
                        'attachments':[],'quotes':[],'_build':build})
                    assert external.status==200,external.text()
                    assert holder.evaluate('name => T.views.get(name)?.inputLease?.token',receipt['name']) == held_token
                    assert holder.evaluate('T.ws?.readyState === WebSocket.OPEN')
                    holder_context.close()
                    stale_prompt.unlink()
                    expect(page.locator('#cinput')).to_have_value('',timeout=10000)
                    # Recreate the retained report metadata from a failed initial
                    # injection, then type a different message and press Enter.
                    # Assert native CLI bytes, not just the successful HTTP reply.
                    page.evaluate('''async () => {
                        const draft=composerDraft();
                        draft.text='original report description';
                        draft.report_text=draft.text;
                        draft.report_prompt='ORIGINAL REPORT TASK\\nMust not replace a follow-up';
                        draft.requestId='report-send:BUG-SYNTHETIC';
                        draft.requestText=JSON.stringify({text:draft.text,attachments:[],quotes:[]});
                        await persistComposerDraft();refreshComposerDraft(composerUid);
                    }''')
                    page.locator('#cinput').fill('new follow-up after failed report')
                    with page.expect_response(lambda r:urlsplit(r.url).path=='/api/session/conversation/send',timeout=20000) as followup:
                        page.locator('#cinput').press('Enter')
                    assert followup.value.status==200,followup.value.text()
                    expect(page.locator('#cinput')).to_have_value('')
                    deadline=time.monotonic()+10
                    while True:
                        users=[json.loads(line)['message']['content'] for line in jsonl.read_text().splitlines() if json.loads(line)['type']=='user']
                        if 'new follow-up after failed report' in users:break
                        assert time.monotonic()<deadline,users
                        time.sleep(.1)
                    assert users[-1]=='new follow-up after failed report',('follow-up replaced by stale report prompt',users[-1])
                    page.fill('#cinput','draft survives refresh');page.evaluate('async () => await composerDraftWrites')
                    server=context.request.get(base+'/api/session/conversation?uid='+native).json()['draft']
                    assert server['value']['text']=='draft survives refresh'
                    assert not page.evaluate("Object.keys(localStorage).some(k=>k.includes('composerDraft'))")
                    page.reload(wait_until='networkidle')
                    page.wait_for_function("composerUid && !composerDraft().loading")
                    expect(page.locator('#cinput')).to_have_value('draft survives refresh')
                    # CAS refuses another page's stale write without changing either input.
                    stale=context.request.post(base+'/api/session/conversation',data={'uid':native,'revision':server['revision']-1,'value':{'text':'stale other page'}})
                    assert stale.status==409,stale.text()
                    assert context.request.get(base+'/api/session/conversation?uid='+native).json()['draft']['value']['text']=='draft survives refresh'
                    other_reply=context.request.post(base+'/api/term/create',data={'source':'claude','cwd':str(root/'work/claude-area'),'request_id':'other-session','_build':build})
                    assert other_reply.status==200,other_reply.text()
                    other_receipt=other_reply.json();other='tmux:'+other_receipt['name']
                    context.request.post(base+'/api/session/conversation',data={'uid':other,'revision':0,'value':{'text':'separate session'}})
                    assert context.request.get(base+'/api/session/conversation?uid='+other).json()['draft']['value']['text']=='separate session'
                    assert context.request.get(base+'/api/session/conversation?uid='+native).json()['draft']['value']['text']=='draft survives refresh'
                    send('spinner session isolation')
                    expect(page.locator('#queued-sends .msg.queued-send').filter(has_text='spinner session isolation')).to_have_count(1)
                    page.evaluate('async receipt => {await loadTermList();await openPendingSession(receipt)}',other_receipt)
                    expect(page.locator('#csend')).to_have_attribute('aria-busy','false')
                    expect(page.locator('#queued-sends')).to_have_count(0)
                    page.locator(f'#side .item[data-uid="{native}"]').click()
                    wait_history(page,'spinner session isolation')
                    expect(page.locator('#queued-sends')).to_have_count(0)
                    expect(page.locator('#csend')).to_have_attribute('aria-busy','false')
                    # A lost HTTP reply is resolved by GET before any stale save or second SEND.
                    def lose_reply(route):
                        response=route.fetch()
                        assert response.status==200,response.text()
                        route.abort('failed')
                    page.route('**/api/session/conversation/send',lose_reply)
                    page.fill('#cinput','lost HTTP reply');page.locator('#csend').click()
                    page.wait_for_function('!composerSending')
                    expect(page.locator('#cinput')).to_have_value('lost HTTP reply')
                    count=len(sends)
                    page.unroute('**/api/session/conversation/send',lose_reply)
                    page.locator('#csend').click();page.wait_for_function('!composerSending')
                    expect(page.locator('#cinput')).to_have_value('')
                    assert len(sends)==count # Lookup only: no duplicate SEND.
                    wait_history(page,'lost HTTP reply')
                    expect(page.locator('#csend')).to_have_attribute('aria-busy','false')
                    page.fill('#cinput','draft survives refresh');page.evaluate('async () => await composerDraftWrites')
                    # Selection stages the bytes at once in private staging; no agent file yet.
                    staging=root/'state/conversations/conversation-uploads'
                    uploads=[]
                    context.on('request',lambda request:uploads.append(request.url) if '/conversation/attachment?' in request.url else None)
                    page.locator('#cadd').click()
                    with page.expect_file_chooser() as chooser:page.locator('#attach-menu [data-attach=file]').click()
                    with page.expect_response(lambda r:urlsplit(r.url).path=='/api/session/conversation/attachment') as staged:
                        chooser.value.set_files([{'name':'payload.txt','mimeType':'text/plain','buffer':b'private bytes'}])
                    assert staged.value.status==200,staged.value.text()
                    page.wait_for_function("composerDraft().attachments[0]?.uploaded?.upload_id && !composerDraft().attachments[0].staging")
                    page.evaluate('async () => await composerDraftWrites')
                    assert len(uploads)==1 and not (root/'work/claude-area/sessiondock_attachments').exists()
                    assert len(list(staging.iterdir()))==1
                    # The staged reference survives a reload: the File is gone, SEND still works.
                    page.reload(wait_until='networkidle')
                    # The declared Claude identity now owns the sidebar row; its draft is the same record.
                    page.locator(f'#side .item[data-uid="{claude_uid(root,receipt["declared_sid"])}"]').first.click()
                    page.wait_for_function("composerUid && !composerDraft().loading && takenOver(composerUid)")
                    expect(page.locator('#cinput')).to_have_value('draft survives refresh')
                    assert page.evaluate("composerDraft().attachments[0].uploaded.upload_id") and not page.evaluate('composerDraft().attachments[0].file instanceof File')
                    # A failed staging keeps the File on the card with a retry; SEND retries too.
                    def unavailable(route):route.fulfill(status=503,content_type='application/json',body='{"error":"upload unavailable"}')
                    page.route('**/api/session/conversation/attachment?*',unavailable)
                    page.locator('#cadd').click()
                    with page.expect_file_chooser() as chooser:page.locator('#attach-menu [data-attach=file]').click()
                    chooser.value.set_files([{'name':'second.txt','mimeType':'text/plain','buffer':b'second bytes'}])
                    expect(page.locator('#compose-items .draft-card.failed')).to_contain_text('upload unavailable')
                    expect(page.locator('#compose-items .draft-card.failed .draft-retry')).to_be_visible()
                    count=len(sends);page.locator('#csend').click()
                    page.wait_for_function('!composerSending')
                    assert len(sends)==count and page.evaluate('composerDraft().attachments[1].file instanceof File')
                    expect(page.locator('#cinput')).to_have_value('draft survives refresh')
                    page.unroute('**/api/session/conversation/attachment?*',unavailable)
                    page.locator('#compose-items .draft-card.failed .draft-retry').click()
                    page.wait_for_function("composerDraft().attachments.length===2 && composerDraft().attachments.every(a => a.uploaded?.upload_id && !a.staging)")
                    page.evaluate('async () => await composerDraftWrites')
                    assert len(list(staging.iterdir()))==2
                    # Removing a staged attachment releases its private bytes once the draft is saved.
                    with page.expect_response(lambda r:urlsplit(r.url).path=='/api/session/conversation/attachment/discard') as discarded:
                        page.locator('#compose-items .draft-card').nth(1).locator('.draft-remove').click()
                    assert discarded.value.status==200 and discarded.value.json()['removed'] is True,discarded.value.text()
                    assert len(list(staging.iterdir()))==1
                    body=send('attachment send')
                    assert body['text']=='attachment send' and body['attachments'][0]['upload_id']
                    published=list((root/'work/claude-area/sessiondock_attachments').glob('*/payload.txt'))
                    assert len(published)==1 and published[0].read_bytes()==b'private bytes'
                    assert not list(staging.iterdir()) # Published bytes leave staging.
                    wait_history(page,'attachment send')
                    expect(page.locator('#csend')).to_have_attribute('aria-busy','false')
                    # A paste of more than five files (or over 50 MB) asks first:
                    # dismissed stages nothing, accepted stages every file.
                    paste_files="""files => {
                      const transfer = new DataTransfer();
                      for (const [name, type, text] of files) transfer.items.add(new File([text], name, {type}));
                      document.querySelector('#cinput').dispatchEvent(new ClipboardEvent('paste', {clipboardData: transfer, bubbles: true, cancelable: true}));
                    }"""
                    six=[[f'shot-{i}.png','image/png',f'png {i}'] for i in range(6)]
                    dialog_action['accept']=False
                    page.evaluate(paste_files,six)
                    assert dialogs[-1]==('confirm','粘贴了 6 个文件，共 1 KB。继续？'),dialogs[-1]
                    page.wait_for_timeout(200)
                    assert page.evaluate('composerDraft().attachments.length')==0
                    dialog_action['accept']=True
                    page.evaluate(paste_files,six)
                    page.wait_for_function("composerDraft().attachments.length===6 && composerDraft().attachments.every(a => a.uploaded?.upload_id && !a.staging)")
                    page.evaluate(paste_files,six[:5])
                    page.wait_for_function('composerDraft().attachments.length===11 && composerDraft().attachments.every(a => a.uploaded?.upload_id && !a.staging)')
                    assert dialogs[-1][1].startswith('粘贴了 6 个文件'),dialogs[-1]   # five files asked nothing
                    for remaining in range(10,-1,-1):
                        page.locator('#compose-items .draft-card').first.locator('.draft-remove').click()
                        page.wait_for_function(f'composerDraft().attachments.length==={remaining}')
                    page.evaluate('async () => await composerDraftWrites')
                    page.wait_for_function('!document.querySelector("#compose-items .draft-card")')
                    deadline=time.monotonic()+10
                    while list(staging.iterdir()) and time.monotonic()<deadline:page.wait_for_timeout(100)
                    assert not list(staging.iterdir()),list(staging.iterdir())
                    # The same upload ID cannot overwrite another session's staging bytes.
                    for owner,content in [('report:a',b'a'),('report:b',b'b')]:
                        response=context.request.post(base+'/api/session/conversation/attachment?uid='+owner+'&id=same&name=x',data=content,headers={'Content-Type':'text/plain'})
                        assert response.status==200,response.text()
                    conflict=context.request.post(base+'/api/session/conversation/attachment?uid=report:a&id=same&name=x',data=b'changed',headers={'Content-Type':'text/plain'})
                    assert conflict.status==409,conflict.text()
                    # Reading staged bytes back follows the same session key, and
                    # a non-image stays opaque bytes whatever was uploaded.
                    for owner,content in [('report:a',b'a'),('report:b',b'b')]:
                        served=context.request.get(base+'/api/session/conversation/attachment?uid='+owner+'&id=same')
                        assert served.status==200 and served.body()==content,(owner,served.status)
                        assert served.headers['content-type']=='application/octet-stream',served.headers
                    # Published bytes are gone from staging, so the editor gets a 404.
                    assert context.request.get(base+'/api/session/conversation/attachment?uid='+uid+'&id='+body['attachments'][0]['upload_id']).status==404
                    # Disconnect an incomplete streaming upload; no partial staging files survive.
                    address=urlsplit(base)
                    with socket.create_connection((address.hostname,address.port)) as connection:
                        connection.sendall(b'POST /api/session/conversation/attachment?uid=report:a&id=partial&name=partial HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1000000\r\nContent-Type: text/plain\r\n\r\nabc')
                    time.sleep(.3)
                    assert not list((root/'state/conversations/conversation-uploads').glob('*.upload'))
                    # A startup choice disables the chat sender, and backend rejects bypasses.
                    # Rendered like Claude Code 2.1: every blank cell is a cursor-forward move, never a space
                    # (BUG-20260917-012213-bfffee), so the styled capture carries `\x1b[C` instead of spaces.
                    (root/'gate').write_text(' Accessing workspace:\n\n Quick safety check: Is this a project you created or one you trust?\n\n ❯ No, exit\n   Yes, I trust this folder\n\n Enter to confirm · Esc to cancel'.replace(' ','\x1b[C'))
                    gated=create_claude(page,base,root/'work',open_terminal=False)
                    gated_uid='tmux:'+gated['name']
                    page.wait_for_function("composerDraft()?.inputStatus?.state === 'blocked'",timeout=15000)
                    expect(page.locator('#csend')).to_be_disabled()
                    page.fill('#cinput','must not answer trust')
                    page.evaluate('async () => await composerDraftWrites')
                    refused=context.request.post(base+'/api/session/conversation/send',data={'uid':gated_uid,'text':'must not answer trust','request_id':'choice-refusal','_build':build})
                    assert refused.status==409 and refused.json()['code']=='cli_question',refused.text()
                    assert not (root/'gate.trace').exists() # No terminal bytes.
                    assert context.request.get(base+'/api/session/conversation?uid='+gated_uid).json()['draft']['value']['text']=='must not answer trust'
                    expect(page.locator('#cinput')).to_have_value('must not answer trust')
                    stopped=context.request.post(base+'/api/term/kill',data={'record_id':gated['record_id'],'instance_id':gated['instance_id']})
                    assert stopped.status==200,stopped.text()
                    # CLI exit/restart keeps the same draft and stable restart request.
                    restarted=context.request.post(base+'/api/session/conversation/restart',data={'uid':gated_uid,'request_id':'restart-same','_build':build})
                    assert restarted.status==200,restarted.text()
                    new_receipt=restarted.json();next_uid='tmux:'+new_receipt['name']
                    assert context.request.get(base+'/api/session/conversation?uid='+next_uid).json()['draft']['value']['text']=='must not answer trust'
                    again=context.request.post(base+'/api/session/conversation/restart',data={'uid':gated_uid,'request_id':'restart-same','_build':build})
                    assert again.status==200 and again.json()['name']==new_receipt['name'],again.text()
                    context.request.post(base+'/api/term/kill',data={'record_id':new_receipt['record_id'],'instance_id':new_receipt['instance_id']})
                    assert not errors,errors
                    # Clean up private test hosts only.
                    for item in [receipt,other_receipt,gated]:
                        context.request.post(base+'/api/term/kill',data={'record_id':item['record_id'],'instance_id':item['instance_id']})
                    context.close()
            finally:
                browser.close()
    print('PASS send_browser: server drafts/CAS/isolation, busy SEND, deduplication, failed-report follow-up native text, metadata-only selection, private uploads/publication, interrupted stream, startup choice refusal, no browser outbox')

if __name__ == '__main__':
    main()
