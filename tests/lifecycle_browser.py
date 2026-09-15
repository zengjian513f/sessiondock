#!/usr/bin/env python3
"""Opt-in isolated launch pipeline acceptance: fixed free shell, never model CLI."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.parse import urlsplit
from playwright.sync_api import sync_playwright, expect
from history_parity import REPO, BINARY, Corpus, codex_row, codex_message, isolated_server
from terminal_browser import SHELL_SCRIPT
from terminal_exit_browser import XTERM_TEXT


def main(bind_native=False, bare_shell=False):
    source = "shell" if bare_shell else "codex"
    if os.name != "posix":
        raise SystemExit("Real launch acceptance currently requires POSIX; no Windows/macOS claim.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-lifecycle-ui-") as temporary:
        root=Path(temporary)
        for name in ["host","work","ledger","claude","codex","grok"]:
            (root/name).mkdir(mode=0o700)
        corpus=Corpus(root)
        corpus.put("fixture","codex",[codex_row("session_meta",{"id":"fixture","cwd":str(root/"work")}),
            codex_message("user","Unchanged native history")],[])
        native=corpus.paths["fixture"].read_bytes()
        native_uid=corpus.uid("fixture")
        configuration=root/"launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({"host_binary":str(REPO/"target/debug/ptyhost"),
            "host_dir":str(root/"host"),"adapters":[{
                "id":"synthetic-shell-v1","source":source,"executable":str(Path("/bin/sh").resolve()),
                "args":["-c",('trap "" HUP\n' if bind_native else '')+'printf "START\\n" >> "$SESSIONDOCK_TEST_START_LOG"\n'+SHELL_SCRIPT],
                "env":{"PATH":"/usr/bin:/bin","TERM":"xterm-256color","SESSIONDOCK_TEST_START_LOG":str(root/"work/starts")}}]}))
        initialized=subprocess.run([str(BINARY),"--initialize-lifecycle",str(root/"ledger")],
            cwd=REPO,env={"PATH":"/usr/bin:/bin"},capture_output=True,timeout=15)
        assert initialized.returncode==0,initialized.stderr.decode()
        with sync_playwright() as playwright:
            options={"headless":True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"]=os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser=playwright.chromium.launch(**options)
            receipt=None
            try:
                for iteration in range(3 if bind_native else 2):
                    restarted=iteration>0
                    with isolated_server(corpus,BINARY,host_dir=root/"host",lifecycle_dir=root/"ledger",launcher_config=configuration) as (base,_):
                        context=browser.new_context(viewport={"width":1280,"height":900},service_workers="block")
                        context.route("**/*",lambda route:route.continue_() if route.request.url.startswith(base+"/") else route.abort())
                        page=context.new_page()
                        errors=[]
                        claims=[]
                        context.on("request",lambda request:claims.append(request.post_data_json)
                            if urlsplit(request.url).path=="/api/term/claim" else None)
                        page.on("pageerror",lambda error:errors.append(str(error)))
                        page.on("dialog",lambda dialog:dialog.accept())
                        page.goto(base,wait_until="networkidle")
                        if iteration==2:
                            denied=context.request.post(base+"/api/term/claim",data={"name":receipt["name"],"uid":native_uid,
                                "instance_id":receipt["instance_id"],"page":"after-cancel-restart","force":True})
                            assert denied.status==409,denied.text()
                            # The shell deliberately ignores HUP. Rejection must
                            # come from durable cancellation, not a dead socket.
                            with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as stream:
                                stream.settimeout(2)
                                stream.connect(str(root/"host"/(receipt["name"]+".sock")))
                                stream.sendall(b'{"op":"info"}\n')
                                data=b""
                                while b"\n" not in data:
                                    data+=stream.recv(8192)
                                    assert len(data)<65536
                                assert json.loads(data.split(b"\n")[0])["exited"] is False
                            context.close()
                            continue
                        expect(page.locator("#new-session")).to_be_visible()
                        if not restarted:
                            page.locator("#new-session").click()
                            if bare_shell:
                                page.set_viewport_size({"width":390,"height":844})
                            page.locator(f'input[name="new-source"][value="{source}"]').check()
                            if bare_shell:
                                for label in page.locator("#new-session-form .new-source label").all():
                                    bounds = label.bounding_box()
                                    assert bounds and bounds["x"] >= 0 and bounds["x"] + bounds["width"] <= 390, bounds
                            page.locator("#new-cwd").fill(str(root/"work"))
                            with page.expect_response(lambda response:urlsplit(response.url).path=="/api/term/create") as created:
                                page.locator("#new-session-go").click()
                            response=created.value
                            assert response.status==200,response.text()
                            receipt=response.json()
                            if bare_shell:
                                page.set_viewport_size({"width":1280,"height":900})
                            assert receipt["running"] and receipt["native_binding"]=="unbound",receipt
                            # A healthy pending page explains nothing: the terminal
                            # is simply usable, and the only actions are the ordinary
                            # console toggle and stop.
                            expect(page.locator(".new-session-wait")).to_have_text("")
                            expect(page.locator(".new-session-wait")).to_be_hidden()
                            assert sorted(page.evaluate("[...document.querySelectorAll('.dhead-actions button')].map(b => b.id || (b.hasAttribute('data-report-bug') ? 'report-bug' : ''))"))==["a-more","a-session-action","a-term","report-bug"]
                            if bare_shell:
                                assert receipt["source"] == "shell" and receipt["launch_kind"] == "fixed", receipt
                                assert not receipt.get("declared_sid"), receipt
                            original_request=response.request.post_data_json
                            repeated=context.request.post(base+"/api/term/create",data=original_request)
                            assert repeated.status==200 and repeated.json()["record_id"]==receipt["record_id"]
                            bad=context.request.post(base+"/api/term/create",data={**original_request,"cwd":str(root)})
                            assert bad.status in [400,409]
                            assert not any(key in receipt for key in ["uid","sid","argv","env","token","port","sock"])
                        elif bind_native:
                            # A confirmed binding is represented by the native
                            # row; the pending row is gone from the sidebar and the
                            # native console claims through the native lease.
                            expect(page.locator(f'#side .item[data-uid="tmux:{receipt["name"]}"]')).to_have_count(0)
                            page.locator(f'#side .item[data-uid="{native_uid}"]').click()
                            expect(page.locator("#a-term")).to_have_attribute("data-unavailable","false")
                            page.locator("#a-term").click()
                        else:
                            page.locator(f'#side .item[data-uid="tmux:{receipt["name"]}"]').click()
                        if not (bind_native and restarted):
                            if bare_shell:
                                expect(page.locator("#termpane")).to_be_visible()
                                expect(page.locator("#composer")).to_be_visible()
                                expect(page.locator("#cadd")).to_be_hidden()
                                expect(page.locator("#cesc")).to_be_hidden()
                                page.locator("#a-term").click()
                            else:
                                expect(page.locator("#termpane")).to_be_hidden()
                                page.locator("#a-term").click()
                        expect(page.locator("#termpane")).to_be_visible()
                        if not (bind_native and restarted):
                            expect(page.locator("#composer")).to_be_hidden()
                        page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                        page.wait_for_function("("+XTERM_TEXT+")().includes('RS_SHELL_READY')")
                        if bind_native and restarted:
                            assert claims[-1].get("uid")==native_uid and "record_id" not in claims[-1],claims[-1]
                        page.locator("#termpane .xterm-helper-textarea").press_sequentially("next" if restarted else "ping")
                        page.locator("#termpane .xterm-helper-textarea").press("Enter")
                        expected="RS_AFTER_RESTART" if restarted else "RS_PING_OK"
                        page.wait_for_function("text => ("+XTERM_TEXT+")().includes(text)",arg=expected)
                        assert (root/"work/starts").read_text().splitlines()==["START"]
                        assert corpus.paths["fixture"].read_bytes()==native
                        if bind_native and not restarted:
                            body={"record_id":receipt["record_id"],"instance_id":receipt["instance_id"],"uid":native_uid,"operator_confirmed":True}
                            assert context.request.post(base+"/api/term/bind",data={**body,"operator_confirmed":False}).status==400
                            ignored={**body,"instance_id":"0"*32,"sid":"forged-display-id"}
                            assert context.request.post(base+"/api/term/bind",data=ignored).status==409
                            page.evaluate("window.bindingSocket = T.ws")
                            # The operator path is the API alone; the page has no
                            # binding UI and learns of the binding from its own polling.
                            bound=context.request.post(base+"/api/term/bind",data=body)
                            assert bound.status==200,bound.text()
                            assert bound.json()["binding"]["state"]=="confirmed",bound.text()
                            # The page follows the confirmed binding by itself —
                            # the pending (launch-kind) socket is released, the native
                            # session opens and its console is claimed through the
                            # native lease; the pending row leaves the sidebar.
                            page.wait_for_function("uid => S.sel === uid",arg=native_uid)
                            try:
                                page.wait_for_function("T.ws && T.ws !== window.bindingSocket && T.ws.readyState === WebSocket.OPEN",timeout=10000)
                            except Exception:
                                print("FOLLOW_DIAGNOSTIC",json.dumps(page.evaluate("""name => ({
                                    selected:S.sel, uid:T.uid, name:T.name, ws:T.ws?.readyState,
                                    views:[...T.views].map(([key,v])=>({key,uid:v.bindingUid,retired:v.retired,ended:v.ended,revoked:v.revoked,ws:v.ws?.readyState,instance:v.instanceId})),
                                    pending:T.pending, sessions:T.list, errors:[...ConsoleUI.errors],
                                    visible:!document.querySelector('#termpane').classList.contains('hidden'),
                                    text:document.querySelector('#detail').innerText.slice(0,300),
                                })""",receipt["name"]),ensure_ascii=False))
                                raise
                            assert claims[-1].get("uid")==native_uid and "record_id" not in claims[-1],claims[-1]
                            assert page.evaluate("name => T.views.has(name) && T.views.get(name).bindingUid",receipt["name"])==native_uid
                            expect(page.locator(f'#side .item[data-uid="tmux:{receipt["name"]}"]')).to_have_count(0)
                            page.wait_for_function("("+XTERM_TEXT+")().includes('RS_SHELL_READY')")
                            page.locator("#termpane .xterm-helper-textarea").press_sequentially("ping")
                            page.locator("#termpane .xterm-helper-textarea").press("Enter")
                            page.wait_for_function("("+XTERM_TEXT+")().includes('RS_PING_OK')")
                            assert context.request.post(base+"/api/term/bind",data=body).json()["native_binding"]=="confirmed"
                            mixed=context.request.post(base+"/api/term/claim",data={"name":receipt["name"],"uid":native_uid,
                                "instance_id":receipt["instance_id"],"page":"cross-kind-force","force":True})
                            assert mixed.status in (200,409),mixed.text()
                            assert "sid" not in json.loads((root/"host"/(receipt["name"]+".json")).read_text())["meta"]
                            if mixed.status==200:
                                # Another page took the native console; ours reports it.
                                page.wait_for_function("uid => !T.ws || T.ws.readyState !== WebSocket.OPEN",arg=native_uid)
                        status=context.request.get(base+"/api/term/new-status",params={"record_id":receipt["record_id"],"instance_id":receipt["instance_id"]})
                        assert status.status==200 and status.json()["running"],status.text()
                        if restarted:
                            action_page=page
                            action_context=None
                            if bind_native:
                                assert status.json()["native_binding"]=="confirmed",status.text()
                                # The header action offers the ordinary stop of the
                                # bound session; the durable cancel itself is issued
                                # through `term/kill` (the shell ignores HUP, so the
                                # host must survive for the restart check below).
                                action=page.locator("#a-session-action")
                                if not action.is_visible():
                                    page.locator("#a-more").click()
                                expect(action).to_have_attribute("aria-label","停止会话")
                                page.keyboard.press("Escape")
                                before_cancel_claims=len(claims)
                                cancelled=context.request.post(base+"/api/term/kill",data={"record_id":receipt["record_id"],"instance_id":receipt["instance_id"]})
                                assert cancelled.status==200,cancelled.text()
                                assert cancelled.json()["state"]=="uncertain" and cancelled.json()["discardable"],cancelled.text()
                                # The derived native lease is retired: this page's
                                # console reports it and never reclaims by itself.
                                page.wait_for_function("name => { const v = T.views.get(name); return !v || v.retired || v.ended || v.revoked; }",arg=receipt["name"],timeout=15000)
                                page.wait_for_function("uid => !(T.list || []).some(row => row.uid === uid)",arg=native_uid,timeout=15000)
                                again=context.request.post(base+"/api/term/kill",data={"record_id":receipt["record_id"],"instance_id":receipt["instance_id"]})
                                assert again.status==200
                                page.wait_for_timeout(1200)
                                assert len(claims)==before_cancel_claims,"retired launch automatically reclaimed"
                                assert (root/"work/starts").read_text().splitlines()==["START"]
                            else:
                                action_page.set_viewport_size({"width":390,"height":844})
                                action_page.locator(f'#side .item[data-uid="tmux:{receipt["name"]}"]').click()
                                expect(action_page.locator("#a-term")).to_be_visible()
                                # Normal pending detail action lives in the session
                                # menu; keyboard access/click must retain its receipt.
                                action=action_page.locator("#a-session-action")
                                if not action.is_visible():
                                    action_page.locator("#a-more").click()
                                before_cancel_claims=len(claims)
                                with action_page.expect_response(lambda response:urlsplit(response.url).path=="/api/term/kill") as stopped:
                                    action.click()
                                assert stopped.value.status==200,stopped.value.text()
                                if bare_shell:
                                    action_page.wait_for_function("id => !T.pending.some(row=>row.record_id===id)",arg=receipt["record_id"])
                                    expect(action_page.locator(f'#side .item[data-uid="tmux:{receipt["name"]}"]')).to_have_count(0)
                                    final = context.request.get(base+"/api/term/new-status",params={"record_id":receipt["record_id"],"instance_id":receipt["instance_id"]})
                                    assert final.status == 200 and final.json()["state"] == "exited", final.text()
                                    replay = context.request.post(base+"/api/term/create",data=original_request)
                                    assert replay.status == 200 and replay.json()["record_id"] == receipt["record_id"] and not replay.json()["running"], replay.text()
                                else:
                                    action_page.wait_for_function("id => T.pending.some(row=>row.record_id===id && row.stale)",arg=receipt["record_id"])
                                expect(action_page.locator("#a-term")).to_have_attribute("data-unavailable","true")
                                again=context.request.post(base+"/api/term/kill",data={"record_id":receipt["record_id"],"instance_id":receipt["instance_id"]})
                                assert again.status==200
                                page.wait_for_timeout(1200)
                                assert len(claims)==before_cancel_claims,"retired launch automatically reclaimed"
                                assert (root/"work/starts").read_text().splitlines()==["START"]
                            if action_context: action_context.close()
                            if bare_shell:
                                natural = context.request.post(base+"/api/term/create",data={"source":"shell","cwd":str(root/"work"),"request_id":"shell-natural-exit"})
                                assert natural.status == 200 and natural.json()["running"], natural.text()
                                natural = natural.json()
                                page.evaluate("info => openPendingSession(info)",natural)
                                expect(page.locator("#termpane")).to_be_visible()
                                page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                                page.locator("#termpane .xterm-helper-textarea:visible").press_sequentially("quit")
                                page.locator("#termpane .xterm-helper-textarea:visible").press("Enter")
                                page.wait_for_function("name => T.views.get(name)?.ended",arg=natural["name"])
                                page.wait_for_function("async id => { await loadTermList(); return !T.pending.some(row => row.record_id === id); }",arg=natural["record_id"])
                                expect(page.locator(f'#side .item[data-uid="tmux:{natural["name"]}"]')).to_have_count(0)
                                final = context.request.get(base+"/api/term/new-status",params={"record_id":natural["record_id"],"instance_id":natural["instance_id"]})
                                assert final.status == 200 and final.json()["state"] == "exited", final.text()
                        assert not errors,errors
                        context.close()
                assert corpus.paths["fixture"].read_bytes()==native
            finally:
                browser.close()
                # Cleanup only explicitly created instances in this private
                # fixture, protected by their full immutable launch envelope.
                for path in (root/"host").glob("*.json"):
                    record=json.loads(path.read_text())
                    meta=record["meta"]
                    try:
                        with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as stream:
                            stream.settimeout(2)
                            stream.connect(str(root/"host"/(record["name"]+".sock")))
                            stream.sendall(json.dumps({"op":"launch_guard_v1","expected_source":meta["source"],
                                "expected_launch_id":meta["launch_id"],"expected_instance_id":meta["instance_id"],
                                "request":{"op":"kill","force":True}}).encode()+b"\n")
                    except OSError:
                        pass
                deadline=time.monotonic()+6
                while list((root/"host").glob("*.sock")) and time.monotonic()<deadline:
                    time.sleep(.05)
    print("PASS native binding browser: operator binding over the API only, page follows the confirmed binding to the native console, pending row leaves the sidebar, stop of the bound session, durable cancel across Web restart" if bind_native else
        "PASS lifecycle browser: explicit allowlist create, idempotency, pending xterm input, Web restart preserves one host, mobile cancellation retains receipt, native bytes unchanged")


if __name__=="__main__":
    import argparse
    parser=argparse.ArgumentParser()
    parser.add_argument("--native-binding",action="store_true")
    args = parser.parse_args()
    main(args.native_binding)
    if not args.native_binding:
        main(bare_shell=True)
