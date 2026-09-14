#!/usr/bin/env python3
"""Legacy console routed by native UID and instance, using only a synthetic shell."""
import os
from pathlib import Path
import tempfile
import uuid
from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, codex_row, codex_message, isolated_server
from host_identity import host


def main():
    with tempfile.TemporaryDirectory(prefix="sessiondock-managed-ui-") as temporary:
        root=Path(temporary)
        for name in ["host","work","claude","codex","grok"]:
            (root/name).mkdir(mode=0o700)
        corpus=Corpus(root)
        sid="synthetic-native-sid"
        corpus.put(sid,"codex",[codex_row("session_meta",{"id":sid,"cwd":str(root/"work")}),codex_message("user","Synthetic managed console")],[])
        uid=corpus.uid(sid)
        native=corpus.paths[sid].read_bytes()
        instance="synthetic-"+uuid.uuid4().hex
        with host(root,instance,uid=uid) as (process,record), sync_playwright() as playwright:
            launch={"headless":True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"]=os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser=playwright.chromium.launch(**launch)
            try:
                for restart in [False,True]:
                    with isolated_server(corpus,BINARY,host_dir=root/"host") as (base,_):
                        context=browser.new_context(viewport={"width":1280,"height":900},service_workers="block")
                        context.route("**/*",lambda route:route.continue_() if route.request.url.startswith(base+"/") else route.abort())
                        errors=[]
                        context.on("page",lambda page:page.on("pageerror",lambda error:errors.append(str(error))))
                        page=context.new_page()
                        page.goto(base,wait_until="networkidle")
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                        expect(page.locator("#msgs")).to_contain_text("Synthetic managed console")
                        expect(page.locator("#new-session")).to_be_hidden()
                        expect(page.locator("#a-term")).to_be_visible()
                        expect(page.locator("#a-term")).to_have_attribute("data-unavailable","false")
                        expect(page.locator("#composer")).to_be_hidden()
                        page.locator("#a-term").click()
                        expect(page.locator("#termpane")).to_be_visible()
                        page.wait_for_function("T.ws && T.ws.readyState === WebSocket.OPEN")
                        # Read the actual imported xterm buffer only to assert
                        # rendered output; all controls use normal user events.
                        page.wait_for_function("[...T.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_SHELL_READY'))")
                        page.locator("#termpane .xterm-helper-textarea").press_sequentially("next" if restart else "ping")
                        page.locator("#termpane .xterm-helper-textarea").press("Enter")
                        expected="RS_AFTER_RESTART" if restart else "RS_PING_OK"
                        page.wait_for_function("expected=>[...T.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes(expected))",arg=expected)
                        page.set_viewport_size({"width":390,"height":844})
                        # The responsive layout returns to the sidebar; open
                        # the same session with the normal mobile navigation.
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                        expect(page.locator("#a-term")).to_be_visible()
                        if not restart:
                            # A second real page claims the same bound instance.
                            # Normal confirmation must revoke the old browser,
                            # not destroy or relaunch the shell.
                            revoked=[]
                            page.on("dialog",lambda dialog:(revoked.append(dialog.message),dialog.accept()))
                            second_context=browser.new_context(viewport={"width":1280,"height":900},service_workers="block")
                            second_context.route("**/*",lambda route:route.continue_() if route.request.url.startswith(base+"/") else route.abort())
                            second=second_context.new_page()
                            second.on("pageerror",lambda error:errors.append(str(error)))
                            second.on("dialog",lambda dialog:dialog.accept())
                            second.goto(base,wait_until="networkidle")
                            second.locator(f'#side .item[data-uid="{uid}"]').click()
                            expect(second.locator("#a-term")).to_have_attribute("data-unavailable","false")
                            second.locator("#a-term").click()
                            second.wait_for_function("T.ws && T.ws.readyState === WebSocket.OPEN")
                            expect(page.locator("#termpane")).to_be_hidden()
                            assert any("抢占" in message and "本页面的终端已关闭" in message for message in revoked),revoked
                            second.wait_for_function("[...T.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_SHELL_READY'))")
                            second.bring_to_front()
                            second.locator("#termpane .xterm-helper-textarea").press_sequentially("next")
                            second.locator("#termpane .xterm-helper-textarea").press("Enter")
                            second.wait_for_function("[...T.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_AFTER_RESTART'))")
                            second_context.close()
                        assert not errors,errors
                        context.close()
                    assert process.poll() is None
                assert corpus.paths[sid].read_bytes()==native
            finally:
                browser.close()
    print("PASS managed console browser: exact UID/instance, normal console click/xterm keyboard, second-page force/revoke, no new CLI action, mobile, Web restart, native fixture unchanged")


if __name__=="__main__":
    main()
