#!/usr/bin/env python3
"""Synthetic Rust lazy-image rendering and controlled GET-error recovery.

Only private generated native records and a loopback Rust server are used.
--synthetic-descriptors can exercise the frontend before lazy backend wiring;
--reproduce-broken records the old UI failure without requiring its new handlers.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile
from datetime import datetime, timezone

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, codex_row, encoded, isolated_server
from history_pages_browser import HOOK
from media_browser import PNG, image, native_bytes, uid


def build(root):
    corpus=Corpus(root)
    for source in ("claude","codex","grok"):
        (root/source).mkdir()
    records=[codex_row("session_meta",{"id":"codex-lazy","cwd":"/synthetic/lazy"})]
    for index in range(1400):
        content=[{"type":"input_text","text":f"LAZY ROW {index:04d}"}]
        if index in (0,1399):
            content.append(image("codex",PNG))
        records.append(codex_row("response_item",{"type":"message","role":"user","content":content}))
    corpus.put("codex-lazy","codex",records,[])
    corpus.put("codex-other","codex",[codex_row("session_meta",{"id":"codex-other","cwd":"/synthetic/other"}),
        codex_row("response_item",{"type":"message","role":"user","content":[{"type":"input_text","text":"OTHER VIEW ONLY"}]})],[])
    corpus.put("codex-short","codex",[codex_row("session_meta",{"id":"codex-short","cwd":"/synthetic/short"}),
        codex_row("response_item",{"type":"message","role":"user","content":[{"type":"input_text","text":"SHORT IMAGE"},image("codex",PNG)]})],[])
    return corpus


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=Path,default=BINARY)
    parser.add_argument("--synthetic-descriptors",action="store_true")
    parser.add_argument("--reproduce-broken",action="store_true")
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-media-lazy-") as temporary:
        corpus=build(Path(temporary));before=native_bytes(corpus.root)
        with isolated_server(corpus,args.binary,extra_env={"SESSIONDOCK_HISTORY_PAGE_EVENTS":"200"}) as (base,_),sync_playwright() as playwright:
            options={"headless":True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"]=os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser=playwright.chromium.launch(**options)
            try:
                context=browser.new_context(viewport={"width":1280,"height":900},service_workers="block")
                context.add_init_script(HOOK)
                requests=[];errors=[];media_requests=[];held=[]
                mode={"status":503,"hold_diagnostic":False}
                context.route("**/*",lambda route:route.continue_() if route.request.url.startswith(base+"/") else route.abort())
                page=context.new_page()
                page.on("request",lambda request:requests.append(request.url))
                page.on("pageerror",lambda error:errors.append(str(error)))
                if args.synthetic_descriptors:
                    def capability_script(route):
                        response=route.fetch()
                        script=response.text()+"\n globalThis.AgentHubCapabilities=Object.freeze({...AgentHubCapabilities,config:Object.freeze({...AgentHubCapabilities.config,media_lazy:true})});"
                        route.fulfill(response=response,body=script)
                    def metadata(route):
                        data=route.fetch().json();data["capabilities"]["media_lazy"]=True
                        route.fulfill(status=200,json=data)
                    def descriptors(route):
                        response=route.fetch()
                        if response.status!=200:
                            route.fulfill(response=response);return
                        data=response.json()
                        for message in data.get("messages",[]):
                            for media in message.get("media",[]):
                                if "src" in media:
                                    for field in ("width","height","mime"):
                                        media.pop(field,None)
                                    media["lazy"]=True
                        route.fulfill(status=200,json=data)
                    page.route("**/api/meta",metadata)
                    page.route("**/capabilities.js*",capability_script)
                    page.route("**/api/messages/**",descriptors)
                def media_route(route):
                    diagnostic=route.request.headers.get("accept")=="application/json"
                    media_requests.append((route.request.url,diagnostic))
                    if diagnostic and mode["hold_diagnostic"]:
                        mode["hold_diagnostic"]=False
                        held.append(route)
                    elif mode["status"]:
                        status=mode["status"]
                        route.fulfill(status=status,json={"error":f"Synthetic media failure {status}","code":"media_test"})
                    else:
                        route.continue_()
                page.route("**/api/media/*",media_route)
                page.goto(base,wait_until="networkidle")
                page.evaluate("HISTORY_PAGE_CHAIN=false")   # batch 44: one page per click here
                page.locator(f'#side .item[data-uid="{uid(corpus,"codex-lazy")}"]').click()
                expect(page.locator("#msgs")).to_contain_text("LAZY ROW 1399")
                page.wait_for_function("[...document.querySelectorAll('#msgs img')].some(i=>i.complete&&!i.naturalWidth)")
                if args.reproduce_broken:
                    assert page.locator(".media-load-error").count()==0
                    print("REPRO: token GET503; rendered img complete=true naturalWidth=0; no visible HTTP reason or retry control")
                    return
                assert page.evaluate("AgentHubCapabilities.config.media_lazy===true"),"real backend must declare media_lazy"
                assert page.evaluate("cache.get(viewKey(S.sel,S.agent)).msgs.flatMap(m=>m.media||[]).every(m=>m.lazy===true&&!('mime' in m)&&!('width' in m)&&!('height' in m))")
                expect(page.locator(".media-load-error")).to_contain_text("HTTP 503")
                expect(page.locator(".media-load-error")).to_contain_text("Synthetic media failure 503")
                expect(page.locator(".media-load-retry")).to_be_visible()
                expect(page.locator("#a-term")).to_be_visible()
                expect(page.locator("#a-term")).to_be_enabled()
                images=page.locator("#msgs img")
                expect(images).to_have_count(2)
                head_src=images.nth(0).get_attribute("src");tail_src=images.nth(1).get_attribute("src")
                assert not any(url==head_src for url,_ in media_requests),"offscreen head image fetched before scrolling"
                assert sum(url==tail_src for url,_ in media_requests)==2,"one image failure plus one diagnostic only"
                page.wait_for_timeout(300)
                assert sum(url==tail_src for url,_ in media_requests)==2,"automatic retry loop"

                def state():
                    return page.evaluate("""(() => {const key=viewKey(S.sel,S.agent),e=cache.get(key);return {
                      text:e.msgs.map(m=>m.text),end:e.end,anchor:e.anchor,cursor:S.cursors.get(key),partial:e.partial};})()""")

                original=state();mode["status"]=None
                page.locator(".media-load-retry").click()
                page.wait_for_function("document.querySelectorAll('#msgs img')[1].naturalWidth===2 && document.querySelectorAll('#msgs img')[1].naturalHeight===3")
                expect(page.locator(".media-load-error")).to_have_count(0)
                assert state()==original,"same-token retry mutated history/live cursor"
                assert sum(url==tail_src for url,_ in media_requests)==3
                images.nth(0).scroll_into_view_if_needed()
                page.wait_for_function("document.querySelector('#msgs img').naturalWidth===2")
                assert sum(url==head_src for url,_ in media_requests)==1

                # Native JSON/Markdown paths and remote descriptors never become
                # requests, even if a malformed descriptor sets lazy=true.
                assert page.evaluate("""() => ['https://media.example.invalid/private.png','file:///private/x.png',
                  '/native/private.png','data:image/png;base64,AAAA'].every(src=>imageHtml({src,lazy:true})==='')""")

                for status in (404,409):
                    mode["status"]=status
                    images.nth(1).scroll_into_view_if_needed()
                    start=len(requests);previous=state()
                    images.nth(1).evaluate("img=>{const src=img.src;img.removeAttribute('src');setTimeout(()=>{img.src=src},30)}")
                    expect(page.locator(".media-load-error")).to_contain_text(f"HTTP {status}")
                    expect(page.locator(".media-load-reload")).to_be_visible()
                    assert state()==previous
                    assert not any('/api/messages/' in url for url in requests[start:]),"error auto-reloaded history"
                    mode["status"]=None;start=len(requests)
                    page.evaluate("window.__deferNextPageRender=true")
                    page.locator(".media-load-reload").click()
                    page.wait_for_function("window.__heldPageRender!==null")
                    page.evaluate("window.__mediaResetSeq=renderSeq;window.__resetMessages=cache.get(viewKey(S.sel,S.agent)).msgs")
                    assert page.evaluate("document.querySelectorAll('#msgs .msg').length===0 && renderSeq===window.__heldRenderSeq")
                    if status==404:
                        with corpus.paths["codex-lazy"].open("ab") as stream:
                            stream.write(encoded(codex_row("response_item",{"type":"message","role":"user",
                                "content":[{"type":"input_text","text":"LAZY SSE AFTER RELOAD"}]})))
                        page.wait_for_function("cache.get(viewKey(S.sel,S.agent)).msgs.at(-1).text==='LAZY SSE AFTER RELOAD'")
                    activity=codex_row("event_msg",{"type":"task_complete","error":status==404,"turn_id":"lazy-reset-turn"})
                    activity["timestamp"]=datetime.now(timezone.utc).isoformat()
                    with corpus.paths["codex-lazy"].open("ab") as stream:
                        stream.write(encoded(activity))
                    before=native_bytes(corpus.root)
                    page.wait_for_function("state=>cache.get(viewKey(S.sel,S.agent)).activity?.state===state",arg='failed' if status==404 else 'idle')
                    if status==409:
                        assert page.evaluate("cache.get(viewKey(S.sel,S.agent)).msgs===window.__resetMessages")
                    live=state()["cursor"]
                    assert page.evaluate("renderSeq===window.__mediaResetSeq"),"another render superseded the held reset"
                    assert 'LAZY ROW 1399' not in page.locator('#msgs').inner_text()
                    page.evaluate("window.__heldPageRender();window.__heldPageRender=null")
                    page.wait_for_function("historyPageRequests.size===0")
                    if status==404:
                        body=page.locator('#msgs').inner_text()
                        assert body.count('LAZY SSE AFTER RELOAD')==1
                        assert body.index('LAZY SSE AFTER RELOAD')>body.index('LAZY ROW 1399'),"reset render placed SSE before its older history"
                        expect(page.locator('#activity')).to_contain_text('执行失败')
                    else:
                        expect(page.locator('#activity')).to_have_count(0)
                    assert state()["cursor"]==live,"reset render rolled back accepted SSE cursor"
                    expect(page.locator(".media-load-error")).to_have_count(0)
                    assert any('/api/messages/' in url and 'window=1' in url for url in requests[start:])
                    assert len(state()["text"])==(601 if status==404 else 600)
                    images=page.locator("#msgs img")
                    page.wait_for_function("document.querySelectorAll('#msgs img')[1].naturalWidth===2")

                # A held diagnostic belongs to its old element/view. Releasing
                # it after selection changes must not paint errors in the new UI.
                mode["status"]=503;mode["hold_diagnostic"]=True
                images.nth(1).evaluate("img=>{const src=img.src;img.removeAttribute('src');setTimeout(()=>{img.src=src},30)}")
                page.wait_for_function("mediaDiagnosticActive===1")
                page.locator(f'#side .item[data-uid="{uid(corpus,"codex-other")}"]').click()
                expect(page.locator("#msgs")).to_contain_text("OTHER VIEW ONLY")
                assert len(held)==1
                held.pop().fulfill(status=503,json={"error":"STALE DIAGNOSTIC MUST NOT APPEAR"})
                page.wait_for_function("mediaDiagnosticActive===0")
                expect(page.locator(".media-load-error")).to_have_count(0)
                expect(page.locator("#detail")).not_to_contain_text("STALE DIAGNOSTIC")

                # Recovery is available even in a complete, non-windowed-gap
                # session; no dependency on partial/cursor grants is allowed.
                mode["status"]=409
                page.locator(f'#side .item[data-uid="{uid(corpus,"codex-short")}"]').click()
                expect(page.locator('.media-load-error')).to_contain_text('HTTP 409')
                assert state()["partial"] is None
                mode["status"]=None
                page.locator('.media-load-reload').click()
                page.wait_for_function("historyPageRequests.size===0")
                expect(page.locator('.media-load-error')).to_have_count(0)
                page.wait_for_function("document.querySelector('#msgs img').naturalWidth===2")
                expect(page.locator('#msgs')).to_contain_text('SHORT IMAGE')

                mode["status"]=None
                page.set_viewport_size({"width":390,"height":844})
                if page.locator('.mobile-back').is_visible():
                    page.locator('.mobile-back').click()
                page.locator(f'#side .item[data-uid="{uid(corpus,"codex-lazy")}"]').click()
                expect(page.locator("#msgs")).to_contain_text("LAZY ROW 1399")
                gap=page.locator('.history-gap');gap.scroll_into_view_if_needed()
                top=gap.bounding_box()["y"];previous=state()["cursor"]
                page.locator('.history-gap-load').click()
                page.wait_for_function("historyPageRequests.size===0")
                page.wait_for_timeout(100)
                assert abs(page.locator('.history-gap').bounding_box()["y"]-top)<12
                assert state()["cursor"]==previous
                expect(page.locator('#a-term')).to_be_visible()
                expect(page.locator('#a-term')).to_be_enabled()
                assert page.evaluate("document.documentElement.scrollWidth<=innerWidth")
                assert not errors,errors
                assert all(url.startswith(base+'/') for url in requests)
                assert all('window=1' in url or '/page?' in url or 'start=' in url for url in requests if '/api/messages/' in url)
                print("PASS lazy media: offscreen zero GET; scroll decode; one bounded error diagnostic; manual503 retry and404/409 window reload; stale view isolation; mobile gap/console; no remote/full-history/native mutations")
            finally:
                assert native_bytes(corpus.root)==before
                browser.close()


if __name__=="__main__":
    main()
