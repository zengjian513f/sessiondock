#!/usr/bin/env python3
"""Real bounded Rust history pages, SSE races and explicit recovery in Chromium.

Only owned synthetic native records are appended/rewritten. Network deferrals
retain real server responses; injected 404/409/410 cases test recovery controls.
No Python service, native home, CLI or paid call is used.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
from datetime import datetime, timezone
from urllib.parse import urlsplit, parse_qs

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, codex_row, encoded, get_json, isolated_server
from media_browser import PNG, image, native_bytes, uid


def row(text, picture=False):
    content = [{"type":"input_text","text":text}]
    if picture:
        content.append(image("codex",PNG))
    return codex_row("response_item",{"type":"message","role":"user","content":content})


def build(root):
    corpus = Corpus(root)
    for source in ("claude","codex","grok"):
        (root/source).mkdir()
    corpus.put("codex-pages","codex",[codex_row("session_meta",{"id":"codex-pages","cwd":"/synthetic/pages"}),
        *[row(f"PAGE ROW {index:04d}",index==150) for index in range(1400)]],[])
    corpus.put("codex-other","codex",[codex_row("session_meta",{"id":"codex-other","cwd":"/synthetic/other"}),row("OTHER VIEW ONLY")],[])
    corpus.put("codex-page-agent","codex",[codex_row("session_meta",{"id":"codex-page-agent","session_id":"codex-pages",
        "thread_source":"subagent","parent_thread_id":"codex-pages","forked_from_id":"codex-pages","cwd":"/synthetic/pages"}),
        *[row(f"AGENT ROW {index:04d}") for index in range(750)]],[])
    activity_rows=[codex_row("session_meta",{"id":"codex-page-activity","cwd":"/synthetic/activity"}),
        *[row(f"ACTIVITY ROW {index:04d}") for index in range(1100)]]
    activity_rows[-1]["payload"]["turn_id"]="activity-turn"
    activity_rows.append(codex_row("response_item",{"type":"function_call","name":"shell_command",
        "call_id":"synthetic-activity-call","turn_id":"activity-turn",
        "arguments":json.dumps({"command":"synthetic recorded tool; never executed"})}))
    activity_rows.append(codex_row("response_item",{"type":"message","role":"assistant","turn_id":"activity-turn",
        "phase":"commentary","content":[{"type":"output_text","text":"COVERED ACTIVITY PROGRESS"}]}))
    started=codex_row("event_msg",{"type":"task_started","turn_id":"activity-turn"})
    started["timestamp"]=datetime.now(timezone.utc).isoformat()
    corpus.put("codex-page-activity","codex",[*activity_rows,started],[])
    return corpus


HOOK = """(() => {
  const timeout = window.setTimeout;
  window.__deferNextPageRender = false;
  window.__heldPageRender = null;
  window.setTimeout = (callback, delay, ...args) => {
    const stack = new Error().stack.split('\\n');
    if (delay === 0 && window.__deferNextPageRender && stack[3]?.includes('new Promise')
        && stack[4]?.includes('renderSession')) {
      window.__deferNextPageRender = false;
      window.__heldRenderStack = stack;
      window.__heldRenderSeq = renderSeq;
      window.__heldPageRender = () => timeout(callback,0,...args);
      return 0;
    }
    return timeout(callback,delay,...args);
  };
})();"""


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-history-pages-") as temporary:
        corpus=build(Path(temporary))
        expected_native=native_bytes(corpus.root)
        # Pages default to 2000 events and the gap button chains pages;
        # this choreography checks one 200-event page per click.
        with isolated_server(corpus,args.binary,extra_env={"SESSIONDOCK_HISTORY_PAGE_EVENTS":"200"}) as (base,opener),sync_playwright() as playwright:
            assert get_json(opener,base,"/api/meta")["capabilities"]["history_pages"] is True
            options={"headless":True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"]=os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser=playwright.chromium.launch(**options)
            try:
                context=browser.new_context(viewport={"width":1280,"height":900},service_workers="block")
                context.route("**/*",lambda request:request.continue_() if request.request.url.startswith(base+"/") else request.abort())
                context.add_init_script(HOOK)
                page=context.new_page()
                errors,requests,held=[],[],[]
                mode={"hold":False,"error":None,"invalid":False}
                page.on("pageerror",lambda error:errors.append(str(error)))
                page.on("request",lambda request:requests.append(request.url))

                def page_route(request):
                    if mode["error"]:
                        code=mode["error"]
                        mode["error"]=None
                        request.fulfill(status=code,json={"error":f"Synthetic page failure {code}","code":"history_page_test"})
                    elif mode["invalid"]:
                        mode["invalid"]=False
                        response=request.fetch()
                        invalid=response.json()
                        invalid["messages"][0]=None
                        request.fulfill(status=200,json=invalid)
                    elif mode["hold"]:
                        mode["hold"]=False
                        held.append((request,request.fetch()))
                    else:
                        request.continue_()
                page.route("**/api/messages/*/page?*",page_route)
                page.goto(base,wait_until="networkidle")
                page.evaluate("HISTORY_PAGE_CHAIN=false")

                def snapshot():
                    return page.evaluate("""(() => {const e=cache.get(viewKey(S.sel,S.agent));return {
                      uid:S.sel,agent:S.agent,text:e.msgs.map(m=>m.text),partial:e.partial,
                      end:e.end,anchor:e.anchor,version:e.version,cursor:S.cursors.get(viewKey(S.sel,S.agent))};})()""")

                def select(name,marker):
                    if page.locator(".mobile-back").is_visible():
                        page.locator(".mobile-back").click()
                    page.locator(f'#side .item[data-uid="{uid(corpus,name)}"]').click()
                    expect(page.locator("#msgs")).to_contain_text(marker)
                    expect(page.locator("#a-term")).to_be_visible()
                    expect(page.locator("#a-term")).to_be_enabled()

                def append(text):
                    path=corpus.paths["codex-pages"]
                    with path.open("ab") as stream:
                        stream.write(encoded(row(text)))
                    expected_native[str(path.relative_to(corpus.root))]=hashlib.sha256(path.read_bytes()).hexdigest()
                    page.wait_for_function("text=>cache.get(viewKey(S.sel,S.agent)).msgs.some(m=>m.text===text)",arg=text)

                def status(state,failed=False):
                    path=corpus.paths["codex-pages"]
                    event=codex_row("event_msg",{"type":state,"turn_id":"pages-status-turn","error":failed})
                    event["timestamp"]=datetime.now(timezone.utc).isoformat()
                    with path.open("ab") as stream:
                        stream.write(encoded(event))
                    expected_native[str(path.relative_to(corpus.root))]=hashlib.sha256(path.read_bytes()).hexdigest()

                def release():
                    assert len(held)==1
                    request,response=held.pop()
                    request.fulfill(response=response)

                def click_page():
                    button=page.locator(".history-gap-load")
                    button.scroll_into_view_if_needed()
                    button.click()

                def settled():
                    page.wait_for_function("historyPageRequests.size===0")

                select("codex-pages","PAGE ROW 1399")
                first=snapshot()
                assert len(first["text"])==600 and first["partial"]["omitted"]==800
                page.wait_for_function("_es && _es.readyState===EventSource.OPEN")
                page.evaluate("window.__watchBefore=_es")

                # A real valid page response is delayed until an ordinary SSE
                # append updates the same entry and real-time cursor.
                mode["hold"]=True
                click_page()
                page.wait_for_function("historyPageRequests.size===1")
                append("APPEND DURING PAGE HTTP")
                live=snapshot()
                release();settled()
                after=snapshot()
                assert after["partial"]["head"]==300 and len(after["text"])==801
                assert after["text"][-1]=="APPEND DURING PAGE HTTP" and after["text"].count("APPEND DURING PAGE HTTP")==1
                assert after["cursor"]==live["cursor"] and after["end"]==live["end"] and after["anchor"]==live["anchor"]
                assert page.evaluate("_es===window.__watchBefore")

                # Activity-only SSE changes no messages array. Final page DOM
                # publication must nevertheless retain the newer idle state.
                status("task_complete",failed=True)
                page.wait_for_function("cache.get(viewKey(S.sel,S.agent)).activity?.state==='failed'")
                expect(page.locator("#activity")).to_contain_text("执行失败")
                page.evaluate("window.__deferNextPageRender=true")
                click_page()
                page.wait_for_function("window.__heldPageRender!==null")
                assert page.evaluate("document.querySelectorAll('#msgs .msg').length===0 && renderSeq===window.__heldRenderSeq")
                page.evaluate("window.__activityMessages=cache.get(viewKey(S.sel,S.agent)).msgs")
                status("task_complete")
                page.wait_for_function("cache.get(viewKey(S.sel,S.agent)).activity?.state==='idle'")
                assert page.evaluate("window.__activityMessages===cache.get(viewKey(S.sel,S.agent)).msgs")
                assert page.evaluate("renderSeq===window.__heldRenderSeq")
                page.evaluate("window.__heldPageRender();window.__heldPageRender=null")
                settled()
                expect(page.locator("#activity")).to_have_count(0)
                expect(page.locator("#msgs img")).to_have_count(1)

                # Pause the real renderer at its existing batch yield. SSE now
                # updates both cache and its temporary DOM before final publish.
                page.evaluate("window.__deferNextPageRender=true")
                click_page()
                page.wait_for_function("window.__heldPageRender!==null")
                assert page.evaluate("document.querySelectorAll('#msgs .msg').length===0 && renderSeq===window.__heldRenderSeq")
                append("APPEND DURING PAGE RENDER")
                live=snapshot()
                assert page.evaluate("renderSeq===window.__heldRenderSeq")
                assert 'PAGE ROW 1399' not in page.locator('#msgs').inner_text()
                page.evaluate("window.__heldPageRender();window.__heldPageRender=null")
                settled()
                after=snapshot()
                assert after["partial"]["head"]==700 and len(after["text"])==1202
                assert after["cursor"]==live["cursor"]
                assert page.locator("#msgs").inner_text().count("APPEND DURING PAGE RENDER")==1
                assert page.locator("#msgs").inner_text().count("APPEND DURING PAGE HTTP")==1
                assert page.evaluate("_es===window.__watchBefore")

                # A view switch invalidates a captured response even if its
                # original cache entry still exists in the LRU.
                old=snapshot();mode["hold"]=True;click_page()
                select("codex-other","OTHER VIEW ONLY")
                release();settled()
                expect(page.locator("#msgs")).not_to_contain_text("PAGE ROW")
                select("codex-pages","APPEND DURING PAGE RENDER")
                assert snapshot()["partial"]["head"]==old["partial"]["head"]

                # Likewise, an actual native prefix rewrite causes an SSE
                # window reset. A previously valid held page must not reappear.
                mode["hold"]=True;click_page()
                path=corpus.paths["codex-pages"]
                changed=path.read_bytes().replace(b"PAGE ROW 0000",b"EDIT ROW 0000")
                path.write_bytes(changed)
                expected_native[str(path.relative_to(corpus.root))]=hashlib.sha256(changed).hexdigest()
                page.wait_for_function("cache.get(viewKey(S.sel,S.agent)).msgs[0].text==='EDIT ROW 0000'")
                reset=snapshot()
                assert reset["partial"] and len(reset["text"])<=600
                release();settled()
                assert snapshot()["text"]==reset["text"] and snapshot()["partial"]==reset["partial"]

                # Error UX uses deterministic transport responses. No implicit
                # full-history retry or snapshot deletion is allowed.
                previous=snapshot();mode["invalid"]=True;click_page();settled()
                assert snapshot()["text"]==previous["text"] and snapshot()["partial"]==previous["partial"]
                expect(page.locator(".history-page-error")).to_contain_text("历史分页响应与当前缺口不匹配")
                expect(page.locator("#msgs")).to_contain_text("APPEND DURING PAGE RENDER")
                expect(page.locator("#a-term")).to_be_enabled()
                for code in (404,409,410):
                    previous=snapshot();start=len(requests);mode["error"]=code
                    click_page();settled()
                    expect(page.locator(".history-page-error")).to_contain_text(f"Synthetic page failure {code}")
                    assert snapshot()["text"]==previous["text"] and snapshot()["cursor"]==previous["cursor"]
                    expect(page.locator(".history-gap-reload")).to_be_visible()
                    expect(page.locator("#a-term")).to_be_enabled()
                    assert all("/page?" in url for url in requests[start:] if "/api/messages/" in url)
                start=len(requests)
                page.evaluate("window.__deferNextPageRender=true")
                page.locator(".history-gap-reload").click()
                page.wait_for_function("window.__heldPageRender!==null")
                assert page.evaluate("document.querySelectorAll('#msgs .msg').length===0 && renderSeq===window.__heldRenderSeq")
                append("APPEND DURING GAP RELOAD RENDER")
                live=snapshot()["cursor"]
                assert page.evaluate("renderSeq===window.__heldRenderSeq")
                assert 'EDIT ROW 0000' not in page.locator('#msgs').inner_text()
                page.evaluate("window.__heldPageRender();window.__heldPageRender=null")
                settled()
                body=page.locator('#msgs').inner_text()
                assert body.count('APPEND DURING GAP RELOAD RENDER')==1
                assert body.index('APPEND DURING GAP RELOAD RENDER')>body.index('APPEND DURING PAGE RENDER')
                assert snapshot()["cursor"]==live
                expect(page.locator(".history-page-error")).to_have_count(0)
                assert any(parse_qs(urlsplit(url).query).get("window")==["1"] for url in requests[start:] if "/api/messages/" in url)

                # An explicit window reload is a reset, not an append-safe gap
                # splice. Delay its real response and accept a newer SSE first.
                mode["error"]=410;click_page();settled()
                reload_pattern="**/api/messages/*?window=1"
                def hold_reload(request):
                    held.append((request,request.fetch()))
                page.route(reload_pattern,hold_reload)
                page.locator(".history-gap-reload").click()
                append("APPEND DURING WINDOW RELOAD")
                live=snapshot();release();settled()
                assert snapshot()["text"]==live["text"] and snapshot()["cursor"]==live["cursor"]
                expect(page.locator(".history-page-error")).to_contain_text("实时历史已更新")
                expect(page.locator("#msgs")).to_contain_text("APPEND DURING WINDOW RELOAD")
                page.unroute(reload_pattern,hold_reload)
                page.locator(".history-gap-reload").click();settled()
                expect(page.locator(".history-page-error")).to_have_count(0)

                # Agent pages use the exact agent scope and never import the
                # main view's newly appended tail or its page cursor.
                page.locator("#a-view-switch").click()
                page.locator('#session-view-menu button[data-agent="codex-page-agent"]').click()
                expect(page.locator("#msgs")).to_contain_text("AGENT ROW 0749")
                start=len(requests);click_page();settled()
                assert snapshot()["partial"] is None and len(snapshot()["text"])==750
                assert all(text.startswith("AGENT ROW") for text in snapshot()["text"])
                assert any(parse_qs(urlsplit(url).query).get("agent")==["codex-page-agent"] for url in requests[start:] if "/page?" in url)
                page.locator("#a-view-switch").click()
                page.locator('#session-view-menu button[data-agent=""]').click()
                expect(page.locator("#msgs")).to_contain_text("APPEND DURING PAGE RENDER")

                # Mobile sequential fills preserve the gap's viewport position
                # and retain the original tail and both concurrent SSE appends.
                page.set_viewport_size({"width":390,"height":844})
                select("codex-pages","APPEND DURING PAGE RENDER")
                while snapshot()["partial"]:
                    gap=page.locator(".history-gap")
                    gap.scroll_into_view_if_needed()
                    top=gap.bounding_box()["y"]
                    click_page();settled()
                    if snapshot()["partial"]:
                        page.wait_for_timeout(80)
                        assert abs(page.locator(".history-gap").bounding_box()["y"]-top)<12
                final=snapshot()
                assert len(final["text"])==1404 and len(set(final["text"]))==1404
                assert final["text"]==["EDIT ROW 0000",*[f"PAGE ROW {i:04d}" for i in range(1,1400)],"APPEND DURING PAGE HTTP","APPEND DURING PAGE RENDER","APPEND DURING GAP RELOAD RENDER","APPEND DURING WINDOW RELOAD"]
                expect(page.locator(".history-gap-load")).to_have_count(0)
                expect(page.locator("#a-term")).to_be_visible()
                assert page.evaluate("document.documentElement.scrollWidth<=innerWidth")

                # Exercise the real covered-activity handler in Chromium with a
                # synthetic duplicate packet. It deliberately changes a message
                # in-place, without a native rewrite/reset or a new msgs array.
                page.set_viewport_size({"width":1280,"height":900})
                select("codex-page-activity","COVERED ACTIVITY PROGRESS")
                page.evaluate("window.__deferNextPageRender=true")
                click_page();page.wait_for_function("window.__heldPageRender!==null")
                assert page.evaluate("document.querySelectorAll('#msgs .msg').length===0 && renderSeq===window.__heldRenderSeq")
                page.evaluate("""(() => {
                  const e=cache.get(viewKey(S.sel,S.agent));window.__coveredMessages=e.msgs;
                  applyCoveredActivity(S.sel,S.agent,e,{activity_changed:true,activity:{
                    state:'aborted',turn_id:'activity-turn',reason:'SYNTHETIC COVERED INTERRUPT',
                    ts:new Date(Date.now()+1000).toISOString()}});
                })()""")
                assert page.evaluate("window.__coveredMessages===cache.get(viewKey(S.sel,S.agent)).msgs && window.__coveredMessages.at(-1).interrupted===true")
                page.evaluate("window.__heldPageRender();window.__heldPageRender=null")
                settled()
                expect(page.locator("#msgs .native-interrupted")).to_contain_text("COVERED ACTIVITY PROGRESS")
                expect(page.locator("#msgs .native-interrupted")).to_have_count(1)
                expect(page.locator('#msgs .turn-process[data-interrupted="true"]')).to_have_count(1)
                assert not errors,errors
                assert all(url.startswith(base+"/") for url in requests),requests
                assert all("/page?" in url or "window=1" in url or "start=" in url for url in requests if "/api/messages/" in url), "unbounded full-history fetch occurred"
                assert native_bytes(corpus.root)==expected_native,"native bytes changed outside owned test mutations"
                print("PASS history pages: bounded ordered pages, exact agent scope, HTTP/render/reload SSE races, activity-only/covered interruption, invalid payload preservation, stale view/reset responses discarded, explicit404/409/410 recovery, mobile gap anchor, console, no full-history fetch")
            finally:
                browser.close()


if __name__=="__main__":
    main()
