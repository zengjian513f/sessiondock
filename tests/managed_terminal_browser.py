#!/usr/bin/env python3
"""Legacy console routed by native UID and instance, using only a synthetic shell."""
from browser_runtime import js
import os
from pathlib import Path
import tempfile
import uuid
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from history_parity import REPO, BINARY, Corpus, codex_row, codex_message, isolated_server
from host_identity import host
from hub_http_suite import Hub, free_port, scoped
from popups import on_popup  # noqa: E402


def live_rotation(browser, hub_mode):
    """A live Codex rewind changes rollout UID, never the console instance."""
    with tempfile.TemporaryDirectory(prefix="sessiondock-rotation-ui-") as temporary:
        root = Path(temporary)
        for name in ["host", "work", "claude", "codex", "grok", "hub"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = "synthetic-native-sid"
        corpus.put("original", "codex", [codex_row("session_meta", {
            "id": sid, "cwd": str(root / "work"), "timestamp": "2026-09-11T08:00:00Z"}, 0),
            codex_message("user", "Original managed conversation", 1)], [])
        guard_uid = corpus.uid("original")
        native = corpus.paths["original"].read_bytes()
        node = SimpleNamespace(nid="a" * 32, name="RotationNode", port=free_port(), token="f" * 64)
        extra = {}
        if hub_mode:
            for name, value in [("node-token", node.token), ("node-id", node.nid)]:
                path = root / name
                path.touch(mode=0o600)
                path.write_text(value)
            extra = {"SESSIONDOCK_NODE_BIND": f"127.0.0.1:{node.port}",
                     "SESSIONDOCK_NODE_TOKEN_FILE": str(root / "node-token"),
                     "SESSIONDOCK_NODE_ID_FILE": str(root / "node-id"),
                     "SESSIONDOCK_NODE_PEERS": "127.0.0.0/8"}
        view_uid = lambda uid: scoped(node.nid, uid) if hub_mode else uid
        instance = "rotation-" + uuid.uuid4().hex
        with host(root, instance, uid=guard_uid) as (process, _), \
             isolated_server(corpus, BINARY, host_dir=root / "host", extra_env=extra) as (base, _):
            hub = Hub(REPO / "target/debug/sessiondock-hub", root / "hub", [node]) if hub_mode else None
            if hub:
                hub.start()
                base = f"http://127.0.0.1:{hub.port}"
            context = browser.new_context(viewport={"width":1280,"height":900}, service_workers="block")
            try:
                context.add_init_script("localStorage.setItem('sessiondock.consoleRenderer', JSON.stringify('%s'))"
                                        % ("grid" if hub_mode else "xterm"))
                context.route("**/*", lambda route: route.continue_()
                              if route.request.url.startswith(base + "/") else route.abort())
                page = context.new_page()
                errors, controls = [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("request", lambda request: controls.append(request.url)
                        if "/api/term/takeover" in request.url or "/api/term/claim" in request.url else None)
                page.goto(base, wait_until="networkidle")
                page.locator(f'#side .item[data-uid="{view_uid(guard_uid)}"]').click()
                page.locator("#a-term").click()
                page.wait_for_function(js("T.ws?.readyState === WebSocket.OPEN", 'runtime.terminal.state.ws?.readyState === WebSocket.OPEN'))
                expect(page.locator("#a-term")).to_have_attribute("aria-label", "切换到对话")
                original = page.evaluate(js("({name:T.name, instance:T.views.get(T.name).instanceId})", '({name:runtime.terminal.state.name, instance:runtime.terminal.state.views.get(runtime.terminal.state.name).instanceId})'))
                initial_controls = list(controls)
                # Retain object identities only for assertions; all view changes
                # below use the real header button and native fixture updates.
                page.evaluate(js("window.rotationSocket = T.ws; window.rotationView = T.views.get(T.name)", 'window.rotationSocket = runtime.terminal.state.ws; window.rotationView = runtime.terminal.state.views.get(runtime.terminal.state.name)', body=True))
                page.evaluate("window.rotationNode = document.querySelector('#xterm'); window.rotationHost = rotationView.host")
                def preserved(selected_uid):
                    assert page.evaluate("""() => document.querySelector('#xterm') === rotationNode
                      && rotationHost.isConnected && rotationHost.parentElement === rotationNode
                      && document.querySelector('#xterm > .xterm-view:not([hidden])') === rotationHost""")
                    assert page.evaluate(js("uid => S.sel === uid && T.uid === uid", 'uid => runtime.core.state.selection.sel === uid && runtime.terminal.state.uid === uid'), selected_uid)
                    expect(page.locator("#termpane")).to_be_visible()
                # Real sidebar/header updates must retain the mounted terminal,
                # even when the selected conversation is filtered out of the list.
                page.locator("#q").fill("absent-migration-filter")
                expect(page.locator("#side .item")).to_have_count(0)
                preserved(view_uid(guard_uid))
                page.locator("#q").press("Escape")
                expect(page.locator(f'#side .item.sel[data-uid="{view_uid(guard_uid)}"]')).to_be_visible()
                preserved(view_uid(guard_uid))
                page.locator('#view button[data-v="date"]').click()
                expect(page.locator('#view button[data-v="date"]')).to_have_class("on")
                preserved(view_uid(guard_uid))
                nested = page.locator("#nest-toggle").get_attribute("aria-pressed")
                page.locator("#nest-toggle").click()
                expect(page.locator("#nest-toggle")).to_have_attribute("aria-pressed", "false" if nested == "true" else "true")
                preserved(view_uid(guard_uid))
                for generation in [1, 2]:
                    key = f"rotation-{generation}"
                    corpus.put(key, "codex", [codex_row("session_meta", {
                        "id": sid, "cwd": str(root / "work"),
                        "timestamp": f"2026-09-11T{8 + generation:02}:00:00Z",
                        "history_base": {"thread_id": sid, "end_byte_offset": len(native),
                                         "end_ordinal_exclusive": 2}}, 2),
                        codex_message("user", f"Rotated conversation {generation}", 3)], [])
                    uid = view_uid(corpus.uid(key))
                    # Fetch both real catalogs; no injected frontend state or
                    # mock terminal response can establish the new association.
                    page.evaluate(js("async () => { await loadSessions(true); await loadTermList(); }", 'async () => { await runtime.core.list.loadSessions(true); await runtime.terminal.loadTermList(); }'))
                    page.wait_for_function(js("uid => S.sel === uid && T.uid === uid", 'uid => runtime.core.state.selection.sel === uid && runtime.terminal.state.uid === uid'), arg=uid)
                    expect(page.locator("#a-term")).to_have_attribute("aria-label", "切换到对话")
                    page.locator("#a-term").click()
                    expect(page.locator("#msgs")).to_be_visible()
                    expect(page.locator("#msgs")).to_contain_text(f"Rotated conversation {generation}")
                    expect(page.locator("#a-term")).to_have_attribute("aria-label", "切换到终端")
                    page.locator("#a-term").click()
                    expect(page.locator("#a-term")).to_have_attribute("aria-label", "切换到对话")
                    assert page.evaluate(js("T.ws === window.rotationSocket && T.views.get(T.name) === window.rotationView", 'runtime.terminal.state.ws === window.rotationSocket && runtime.terminal.state.views.get(runtime.terminal.state.name) === window.rotationView'))
                    assert page.evaluate(js("({name:T.name, instance:T.views.get(T.name).instanceId})", '({name:runtime.terminal.state.name, instance:runtime.terminal.state.views.get(runtime.terminal.state.name).instanceId})')) == original
                    assert controls == initial_controls, controls
                    preserved(uid)
                page.locator("#termpane .xterm-helper-textarea").press_sequentially("ping")
                page.locator("#termpane .xterm-helper-textarea").press("Enter")
                page.wait_for_function(js("Array.from({length:T.term.buffer.active.length}, (_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_PING_OK')", "Array.from({length:runtime.terminal.state.term.buffer.active.length}, (_,i)=>runtime.terminal.state.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_PING_OK')"))
                assert process.poll() is None
                assert corpus.paths["original"].read_bytes() == native
                assert not errors, errors
                print(f"PASS live rollout rotation ({'Hub' if hub else 'node'}): two generations, chat/terminal clicks, same socket/instance, no takeover or claim", flush=True)
            finally:
                context.close()
                if hub:
                    hub.stop()


def main():
    with tempfile.TemporaryDirectory(prefix="sessiondock-managed-ui-") as temporary:
        root=Path(temporary)
        for name in ["host","work","claude","codex","grok"]:
            (root/name).mkdir(mode=0o700)
        corpus=Corpus(root)
        sid="synthetic-native-sid"
        corpus.put(sid,"codex",[codex_row("session_meta",{"id":sid,"cwd":str(root/"work"),
            "timestamp":"2026-09-11T08:00:00Z"},0),codex_message("user","Synthetic managed console",1)],[])
        guard_uid=corpus.uid(sid)
        corpus.put("rotation", "codex", [codex_row("session_meta", {"id":sid,
            "cwd":str(root/"work"), "timestamp":"2026-09-11T09:00:00Z",
            "history_base":{"thread_id":sid,"end_byte_offset":corpus.paths[sid].stat().st_size,
                            "end_ordinal_exclusive":2}},2),
            codex_message("user","Rotated managed console",3)],[])
        uid=corpus.uid("rotation")
        native=corpus.paths[sid].read_bytes()
        instance="synthetic-"+uuid.uuid4().hex
        with host(root,instance,uid=guard_uid) as (process,record), \
             host(root,"waiting-"+uuid.uuid4().hex,uid=uid,name="synthetic-waiting-resume"), \
             sync_playwright() as playwright:
            launch={"headless":True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"]=os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser=playwright.chromium.launch(**launch)
            try:
                live_rotation(browser, False)
                live_rotation(browser, True)
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
                        listed=context.request.get(base+"/api/term/list?force=1").json()
                        original=next(row for row in listed["sessions"] if row["name"]=="synthetic-identity-host")
                        assert original["uid"]==guard_uid and original["current_uid"]==uid, original
                        assert len(listed["sessions"])==2, listed
                        page.locator("#a-term").click()
                        expect(page.locator("#termpane")).to_be_visible()
                        page.wait_for_function(js("T.name === 'synthetic-identity-host'", "runtime.terminal.state.name === 'synthetic-identity-host'"))
                        page.wait_for_function(js("T.ws && T.ws.readyState === WebSocket.OPEN", 'runtime.terminal.state.ws && runtime.terminal.state.ws.readyState === WebSocket.OPEN'))
                        # Read the actual imported xterm buffer only to assert
                        # rendered output; all controls use normal user events.
                        page.wait_for_function(js("[...T.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_SHELL_READY'))", "[...runtime.terminal.state.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_SHELL_READY'))"))
                        page.locator("#termpane .xterm-helper-textarea").press_sequentially("next" if restart else "ping")
                        page.locator("#termpane .xterm-helper-textarea").press("Enter")
                        expected="RS_AFTER_RESTART" if restart else "RS_PING_OK"
                        page.wait_for_function(js("expected=>[...T.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes(expected))", "expected=>[...runtime.terminal.state.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes(expected))"),arg=expected)
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
                            on_popup(page, lambda dialog:(revoked.append(dialog.message),dialog.accept()))
                            second_context=browser.new_context(viewport={"width":1280,"height":900},service_workers="block")
                            second_context.route("**/*",lambda route:route.continue_() if route.request.url.startswith(base+"/") else route.abort())
                            second=second_context.new_page()
                            second.on("pageerror",lambda error:errors.append(str(error)))
                            on_popup(second, lambda dialog:dialog.accept())
                            second.goto(base,wait_until="networkidle")
                            second.locator(f'#side .item[data-uid="{uid}"]').click()
                            expect(second.locator("#a-term")).to_have_attribute("data-unavailable","false")
                            second.locator("#a-term").click()
                            second.wait_for_function(js("T.ws && T.ws.readyState === WebSocket.OPEN", 'runtime.terminal.state.ws && runtime.terminal.state.ws.readyState === WebSocket.OPEN'))
                            expect(page.locator("#termpane")).to_be_hidden()
                            assert any("抢占" in message and "本页面的终端已关闭" in message for message in revoked),revoked
                            second.wait_for_function(js("[...T.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_SHELL_READY'))", "[...runtime.terminal.state.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_SHELL_READY'))"))
                            second.bring_to_front()
                            second.locator("#termpane .xterm-helper-textarea").press_sequentially("next")
                            second.locator("#termpane .xterm-helper-textarea").press("Enter")
                            second.wait_for_function(js("[...T.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_AFTER_RESTART'))", "[...runtime.terminal.state.views.values()].some(v=>v.term?.buffer?.active && Array.from({length:v.term.buffer.active.length},(_,i)=>v.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('RS_AFTER_RESTART'))"))
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
