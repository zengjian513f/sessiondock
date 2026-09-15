#!/usr/bin/env python3
"""Server drafts and one-shot conversation SEND against an isolated fake CLI.
Covers busy sends, lost responses, session isolation, uploads, cancellation,
and startup choice refusal/recovery without browser message persistence.
Older helper functions remain available to the terminal ownership suites.
"""
import base64
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
from media_browser import PNG

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


def initialize(flag, directory):
    with socket.socket() as occupied:
        occupied.bind(("127.0.0.1", 0))
        env = {"PATH": "/usr/bin:/bin", "SESSIONDOCK_BIND": "127.0.0.1:%d" % occupied.getsockname()[1]}
        done = subprocess.run([str(BINARY), flag, str(directory)], cwd=REPO, env=env, capture_output=True, timeout=20)
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
    assert page.evaluate("T.views.size === 0 && T.openViews.size === 0")
    if not open_terminal:
        return receipt
    page.locator("#a-term").click()
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    xterm_includes(page, "FAKE_CLAUDE_READY sid=[%s]" % receipt["declared_sid"])
    return receipt


def outbox_labels(page):
    return page.evaluate("() => [...document.querySelectorAll('#msgs .client-outbox .client-pending-state')].map(n => n.textContent)")


def user_messages(page):
    return page.evaluate("() => [...document.querySelectorAll('#msgs .msg.user:not(.client-outbox) .text, #msgs .msg.user:not(.client-outbox) .body')].map(n => n.textContent.trim())")


def wait_history(page, text, timeout=20000):
    try:
        page.wait_for_function(
            "text => [...document.querySelectorAll('#msgs .msg:not(.client-outbox)')].some(n => n.textContent.includes(text))"
            " && !document.querySelector('#msgs .client-outbox')",
            arg=text, timeout=timeout)
    except Exception:
        print("history timeout:", page.evaluate("() => ({uid: S.sel, text: document.querySelector('#msgs')?.innerText, outbox: [...document.querySelectorAll('.client-pending-state')].map(n => n.textContent)})"), flush=True)
        raise


def send_from_composer(page, text):
    ta = page.locator("#cinput")
    expect(ta).to_be_visible()
    ta.fill(text)
    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/send", timeout=20000) as sent:
        ta.press("Enter")
    return sent.value


def send_attachments(page, root, label, sends, *, fail_first=False):
    """Real chooser, raw uploads and delivery, with a recoverable upload error."""
    payloads = [
        {"name": f"{label} 数据.json", "mimeType": "application/json", "buffer": b'{"data":"' + b'x' * 20000 + b'"}'},
        {"name": f"{label} 截图.png", "mimeType": "image/png", "buffer": base64.b64decode(PNG)},
    ]
    page.locator("#cadd").click()
    with page.expect_file_chooser() as chooser:
        page.locator('#attach-menu [data-attach="file"]').click()
    chooser.value.set_files(payloads)
    expect(page.locator("#compose-items .draft-card")).to_have_count(2)
    page.locator("#cinput").fill(label)
    if fail_first:
        def unavailable(route):
            route.fulfill(status=503, content_type="application/json", body='{"error":"synthetic upload unavailable"}')
        page.route("**/api/session/attachment?*", unavailable)
        count = len(sends)
        page.locator("#csend").click()
        expect(page.locator("#compose-items .draft-card.failed")).to_contain_text("synthetic upload unavailable")
        expect(page.locator("#cinput")).to_have_value(label)
        assert len(sends) == count, sends
        page.unroute("**/api/session/attachment?*", unavailable)
    with page.expect_response(lambda r: urlsplit(r.url).path == "/api/session/attachment") as uploaded:
        with page.expect_response(lambda r: urlsplit(r.url).path == "/api/session/send", timeout=20000) as sent:
            page.locator("#csend").click()
    assert uploaded.value.status == 200, uploaded.value.text()
    assert sent.value.status == 200, sent.value.text()
    batch = uploaded.value.json()["attachment_id"]
    expected = label + "\n\n" + "\n".join(
        f"附件{i}: ./sessiondock_attachments/{batch}/{payload['name']}"
        for i, payload in enumerate(payloads, 1))
    assert sends[-1]["text"] == expected, sends[-1]
    for payload in payloads:
        path = root / "work/claude-area/sessiondock_attachments" / batch / payload["name"]
        assert path.read_bytes() == payload["buffer"]
    # The renderer replaces attachment path lines with file/image cards.
    wait_history(page, label)
    native_users = [json.loads(line)["message"]["content"]
        for history in (root / "claude/project-history").glob("*.jsonl")
        for line in history.read_text().splitlines() if json.loads(line)["type"] == "user"]
    assert expected in native_users, native_users
    for payload in payloads:
        expect(page.locator("#msgs")).to_contain_text(payload["name"])
    expect(page.locator("#compose-items .draft-card")).to_have_count(0)
    expect(page.locator("#cinput")).to_have_value("")


def wait_server_outbox_empty(context, base, uid, timeout=15.0):
    deadline = time.monotonic() + timeout
    while True:
        listed = context.request.get(base + "/api/session/outbox?uid=" + uid).json()
        if not listed["outbox"]:
            return listed
        assert time.monotonic() < deadline, ("server outbox never emptied", listed)
        time.sleep(0.2)


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
                 "args": ["--settings", SETTINGS, "--reply", "--delay", "2500", "--busy-footer"], "new_args": ["--session-id", "{session_id}"],
                 "resume_args": ["--resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color",
                         "LANG": "C.UTF-8", "SESSIONDOCK_TEST_CLAUDE_ROOT": str(root / "claude"),
                         "SESSIONDOCK_TEST_GATE":str(root / "gate"),"SESSIONDOCK_TEST_GATE_TRACE":str(root / "gate.trace")}}]}))
        initialize("--initialize-lifecycle", root / "ledger")
        initialize("--initialize-delivery", root / "delivery")
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, delivery_dir=root / "delivery", state_dir=root / "state",
                                     file_roots=(root / "work",), file_write_roots=(root / "work",)) as (base, _):
                    errors, dialogs, sends = [], [], []

                    def watch(context):
                        context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                        context.on("request", lambda request: sends.append(request.post_data_json)
                            if urlsplit(request.url).path == "/api/session/conversation/send" else None)
                        page = context.new_page()
                        page.on("pageerror", lambda error: errors.append(str(error)))

                        def on_dialog(dialog):
                            dialogs.append((dialog.type, dialog.message))
                            try:
                                dialog.accept()
                            except Exception:
                                pass
                        page.on("dialog", on_dialog)
                        page.goto(base, wait_until="networkidle")
                        return page

                    context = browser.new_context(viewport={"width":1280,"height":900},service_workers="block")
                    page=watch(context)
                    receipt=create_claude(page,base,root / 'work',open_terminal=False)
                    page.wait_for_function("composerUid && !composerDraft().loading && takenOver(composerUid)")
                    assert page.evaluate('T.views.size')==0
                    uid='tmux:'+receipt['name']
                    build=context.request.get(base+'/api/meta').json()['build']
                    def send(text):
                        page.fill('#cinput',text)
                        with page.expect_response(lambda r:urlsplit(r.url).path=='/api/session/conversation/send',timeout=20000) as response:
                            page.locator('#csend').click()
                        assert response.value.status==200,response.value.text()
                        assert response.value.json()['state']=='sent'
                        expect(page.locator('#cinput')).to_have_value('')
                        assert not page.locator('.client-outbox,.draft-saved').count()
                        return sends[-1]
                    # Empty legacy evidence must leave a fresh server draft untouched.
                    response=context.request.post(base+'/api/session/conversation/import',data={'uid':uid,'value':{'text':'','attachments':[],'quotes':[]}})
                    assert response.status==200,response.text()
                    row=context.request.get(base+'/api/session/conversation?uid='+uid).json()['draft']
                    assert row['revision']==0 and row['value'] is None,row
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
                    first=send('first busy input')
                    started=time.monotonic();second=send('second busy input')
                    assert time.monotonic()-started<2.5 # No JSONL confirmation wait.
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
                    page.fill('#cinput','draft survives refresh');page.evaluate('async () => await composerDraftWrites')
                    # Selection saves metadata, with no upload or published agent file.
                    uploads=[]
                    context.on('request',lambda request:uploads.append(request.url) if '/conversation/attachment?' in request.url else None)
                    page.locator('#cadd').click()
                    with page.expect_file_chooser() as chooser:page.locator('#attach-menu [data-attach=file]').click()
                    chooser.value.set_files([{'name':'payload.txt','mimeType':'text/plain','buffer':b'private bytes'}])
                    page.evaluate('async () => await composerDraftWrites')
                    assert not uploads and not (root/'work/claude-area/sessiondock_attachments').exists()
                    # A failed upload keeps the original File and text; no SEND.
                    def unavailable(route):route.fulfill(status=503,content_type='application/json',body='{"error":"upload unavailable"}')
                    page.route('**/api/session/conversation/attachment?*',unavailable)
                    count=len(sends);page.locator('#csend').click()
                    page.wait_for_function('!composerSending')
                    assert len(sends)==count and page.evaluate('composerDraft().attachments[0].file instanceof File')
                    expect(page.locator('#cinput')).to_have_value('draft survives refresh')
                    page.unroute('**/api/session/conversation/attachment?*',unavailable)
                    body=send('attachment send')
                    assert body['text']=='attachment send' and body['attachments'][0]['upload_id']
                    published=list((root/'work/claude-area/sessiondock_attachments').glob('*/payload.txt'))
                    assert len(published)==1 and published[0].read_bytes()==b'private bytes'
                    # The same upload ID cannot overwrite another session's staging bytes.
                    for owner,content in [('report:a',b'a'),('report:b',b'b')]:
                        response=context.request.post(base+'/api/session/conversation/attachment?uid='+owner+'&id=same&name=x',data=content,headers={'Content-Type':'text/plain'})
                        assert response.status==200,response.text()
                    conflict=context.request.post(base+'/api/session/conversation/attachment?uid=report:a&id=same&name=x',data=b'changed',headers={'Content-Type':'text/plain'})
                    assert conflict.status==409,conflict.text()
                    # Disconnect an incomplete streaming upload; no partial staging files survive.
                    address=urlsplit(base)
                    with socket.create_connection((address.hostname,address.port)) as connection:
                        connection.sendall(b'POST /api/session/conversation/attachment?uid=report:a&id=partial&name=partial HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1000000\r\nContent-Type: text/plain\r\n\r\nabc')
                    time.sleep(.3)
                    assert not list((root/'state/conversations/conversation-uploads').glob('*.upload'))
                    # A startup choice disables the chat sender, and backend rejects bypasses.
                    (root/'gate').write_text('Do you trust this directory?\n❯ 1. Yes\n  2. No\nPress Enter to confirm')
                    gated=create_claude(page,base,root/'work',open_terminal=False)
                    gated_uid='tmux:'+gated['name']
                    page.wait_for_function('composerDraft()?.cliQuestion === true',timeout=15000)
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
    print('PASS send_browser: server drafts/CAS/isolation, busy SEND, deduplication, metadata-only selection, private uploads/publication, interrupted stream, startup choice refusal, no browser outbox')

if __name__ == '__main__':
    main()
