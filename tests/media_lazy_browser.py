#!/usr/bin/env python3
"""Synthetic Rust lazy-image rendering and controlled GET-error recovery.

Only private generated native records and a loopback Rust server are used.
--synthetic-descriptors can exercise the frontend before lazy backend wiring;
--reproduce-broken records the old UI failure without requiring its new handlers.
"""
from __future__ import annotations

import argparse
import base64
import json
import html
import re
import os
from pathlib import Path
import tempfile
from datetime import datetime, timezone

from browser_runtime import js
from browser_race_assets import install_small_render_batches
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
    inline_dir = root / 'inline'
    inline_dir.mkdir(mode=0o700)
    (inline_dir / 'inline.png').write_bytes(base64.b64decode(PNG))
    corpus.put('codex-inline','codex',[
        codex_row('session_meta',{'id':'codex-inline','cwd':str(inline_dir)}),
        codex_row('response_item',{'type':'message','role':'user','content':[
            {'type':'input_text','text':'INLINE IMAGE\n' + 'INLINE FOLD FILLER\n' * 240
                + '\nINLINE FULL IMAGE ![inline](inline.png)'}]})],[])
    return corpus


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=Path,default=BINARY)
    parser.add_argument("--synthetic-descriptors",action="store_true")
    parser.add_argument("--reproduce-broken",action="store_true")
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-media-lazy-") as temporary:
        corpus=build(Path(temporary));before=native_bytes(corpus.root)
        with isolated_server(corpus,args.binary,extra_env={"SESSIONDOCK_HISTORY_PAGE_EVENTS":"200",
                "SESSIONDOCK_FILE_ROOTS":str(corpus.root / 'inline')}) as (base,_),sync_playwright() as playwright:
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
                    def capability_html(route):
                        response = route.fetch()
                        def enable(match):
                            config = json.loads(html.unescape(match.group(3)))
                            config['media_lazy'] = True
                            return match.group(1) + match.group(2) + html.escape(json.dumps(config), quote=True) + match.group(2)
                        script = re.sub(r"(<meta\b[^>]*name=['\"]sessiondock-capabilities['\"][^>]*content=)(['\"])(.*?)\2",
                                        enable, response.text())
                        route.fulfill(response=response, body=script)
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
                    page.route(base + "/", capability_html)
                    page.route("**/api/meta",metadata)
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
                # Keep the render-race choreography when the configured history
                # window contains fewer than the normal 250 render groups.
                install_small_render_batches(page, context)
                page.goto(base,wait_until="networkidle")
                page.evaluate(js('HISTORY_PAGE_CHAIN=false', 'runtime.core.history.timing.chain=false'))   # one page per click here
                page.locator(f'#side .item[data-uid="{uid(corpus,"codex-lazy")}"]').click()
                expect(page.locator("#msgs")).to_contain_text("LAZY ROW 1399")
                page.wait_for_function("[...document.querySelectorAll('#msgs img')].some(i=>i.complete&&!i.naturalWidth)")
                if args.reproduce_broken:
                    assert page.locator(".media-load-error").count()==0
                    print("REPRO: token GET503; rendered img complete=true naturalWidth=0; no visible HTTP reason or retry control")
                    return
                assert page.evaluate(js('SessionDockCapabilities.config.media_lazy===true', 'JSON.parse(document.querySelector(\'meta[name="sessiondock-capabilities"]\').content).media_lazy===true')),"real backend must declare media_lazy"
                assert page.evaluate(js("cache.get(viewKey(S.sel,S.agent)).msgs.flatMap(m=>m.media||[]).every(m=>m.lazy===true&&!('mime' in m)&&!('width' in m)&&!('height' in m))", 'runtime.core.cache.cache.get((runtime.core.state.selection.agent ? runtime.core.state.selection.sel + "::" + runtime.core.state.selection.agent : runtime.core.state.selection.sel)).msgs.flatMap(m=>m.media||[]).every(m=>m.lazy===true&&!(\'mime\' in m)&&!(\'width\' in m)&&!(\'height\' in m))'))
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
                    return page.evaluate(js(r"""(() => {const key=viewKey(S.sel,S.agent),e=cache.get(key);return {
                      text:e.msgs.map(m=>m.text),end:e.end,anchor:e.anchor,cursor:S.cursors.get(key),partial:e.partial};})()""", r"""(() => {const key=(runtime.core.state.selection.agent ? runtime.core.state.selection.sel + "::" + runtime.core.state.selection.agent : runtime.core.state.selection.sel),e=runtime.core.cache.cache.get(key);return {
                      text:e.msgs.map(m=>m.text),end:e.end,anchor:e.anchor,cursor:runtime.core.state.unread.cursors.get(key),partial:e.partial};})()"""))

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
                assert page.evaluate(js(r"""() => ['https://media.example.invalid/private.png','file:///private/x.png',
                  '/native/private.png','data:image/png;base64,AAAA'].every(src=>imageHtml({src,lazy:true})==='')""", r"""() => ['https://media.example.invalid/private.png','file:///private/x.png',
                  '/native/private.png','data:image/png;base64,AAAA'].every(src=>runtime.mediaRuntime.imageHtml({src,lazy:true})==='')"""))

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
                    page.evaluate(js('() => {window.__mediaResetSeq=renderSeq;window.__resetMessages=cache.get(viewKey(S.sel,S.agent)).msgs}', '() => {window.__mediaResetSeq=runtime.conversationRenderer.renderSeq;window.__resetMessages=runtime.core.cache.cache.get((runtime.core.state.selection.agent ? runtime.core.state.selection.sel + "::" + runtime.core.state.selection.agent : runtime.core.state.selection.sel)).msgs}'))
                    assert page.evaluate(js("document.querySelectorAll('#msgs .msg').length===0 && renderSeq===window.__heldRenderSeq", "document.querySelectorAll('#msgs .msg').length===0 && runtime.conversationRenderer.renderSeq===window.__heldRenderSeq"))
                    if status==404:
                        with corpus.paths["codex-lazy"].open("ab") as stream:
                            stream.write(encoded(codex_row("response_item",{"type":"message","role":"user",
                                "content":[{"type":"input_text","text":"LAZY SSE AFTER RELOAD"}]})))
                        page.wait_for_function(js("cache.get(viewKey(S.sel,S.agent)).msgs.at(-1).text==='LAZY SSE AFTER RELOAD'", 'runtime.core.cache.cache.get((runtime.core.state.selection.agent ? runtime.core.state.selection.sel + "::" + runtime.core.state.selection.agent : runtime.core.state.selection.sel)).msgs.at(-1).text===\'LAZY SSE AFTER RELOAD\''))
                    activity=codex_row("event_msg",{"type":"task_complete","error":status==404,"turn_id":"lazy-reset-turn"})
                    activity["timestamp"]=datetime.now(timezone.utc).isoformat()
                    with corpus.paths["codex-lazy"].open("ab") as stream:
                        stream.write(encoded(activity))
                    before=native_bytes(corpus.root)
                    page.wait_for_function(js('state=>cache.get(viewKey(S.sel,S.agent)).activity?.state===state', 'state=>runtime.core.cache.cache.get((runtime.core.state.selection.agent ? runtime.core.state.selection.sel + "::" + runtime.core.state.selection.agent : runtime.core.state.selection.sel)).activity?.state===state'),arg='failed' if status==404 else 'idle')
                    if status==409:
                        assert page.evaluate(js('cache.get(viewKey(S.sel,S.agent)).msgs===window.__resetMessages', 'runtime.core.cache.cache.get((runtime.core.state.selection.agent ? runtime.core.state.selection.sel + "::" + runtime.core.state.selection.agent : runtime.core.state.selection.sel)).msgs===window.__resetMessages'))
                    live=state()["cursor"]
                    assert page.evaluate(js('renderSeq===window.__mediaResetSeq', 'runtime.conversationRenderer.renderSeq===window.__mediaResetSeq')),"another render superseded the held reset"
                    assert 'LAZY ROW 1399' not in page.locator('#msgs').inner_text()
                    page.evaluate("window.__heldPageRender();window.__heldPageRender=null")
                    page.wait_for_function(js('historyPageRequests.size===0', 'runtime.core.history.historyPageRequests.size===0'))
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
                    assert len(state()["text"])==len(original["text"])+(1 if status==404 else 0)
                    images=page.locator("#msgs img")
                    page.wait_for_function("document.querySelectorAll('#msgs img')[1].naturalWidth===2")

                # A held diagnostic belongs to its old element/view. Releasing
                # it after selection changes must not paint errors in the new UI.
                mode["status"]=503;mode["hold_diagnostic"]=True
                images.nth(1).evaluate("img=>{const src=img.src;img.removeAttribute('src');setTimeout(()=>{img.src=src},30)}")
                page.wait_for_function(js('(globalThis.SessionDockOverlays ? SessionDockOverlays.mediaState.diagnosticActive : mediaDiagnosticActive)===1', 'runtime.mediaRuntime.diagnosticActive===1'))
                page.locator(f'#side .item[data-uid="{uid(corpus,"codex-other")}"]').click()
                expect(page.locator("#msgs")).to_contain_text("OTHER VIEW ONLY")
                assert len(held)==1
                held.pop().fulfill(status=503,json={"error":"STALE DIAGNOSTIC MUST NOT APPEAR"})
                page.wait_for_function(js('(globalThis.SessionDockOverlays ? SessionDockOverlays.mediaState.diagnosticActive : mediaDiagnosticActive)===0', 'runtime.mediaRuntime.diagnosticActive===0'))
                expect(page.locator(".media-load-error")).to_have_count(0)
                expect(page.locator("#detail")).not_to_contain_text("STALE DIAGNOSTIC")

                # Recovery is available even in a complete, non-windowed-gap
                # session; no dependency on partial/cursor grants is allowed.
                mode["status"]=409
                page.locator(f'#side .item[data-uid="{uid(corpus,"codex-short")}"]').click()
                expect(page.locator('.media-load-error')).to_contain_text('HTTP 409')
                assert state()["partial"] is None
                mode["status"]=None
                # The first SSE snapshot can arrive during this explicit
                # reload. The documented guard keeps it and asks for another
                # manual click; only that specific refusal permits a retry.
                for attempt in range(3):
                    page.locator('.media-load-reload').click()
                    page.wait_for_function(js('historyPageRequests.size===0', 'runtime.core.history.historyPageRequests.size===0'))
                    notice = page.locator('.media-load-error')
                    if not notice.count():
                        break
                    expect(notice).to_contain_text('实时历史已更新；已保留新内容，请再次手动重新载入')
                    expect(page.locator('#msgs')).to_contain_text('SHORT IMAGE')
                    assert state()["text"] == ['SHORT IMAGE'], state()
                    assert state()["partial"] is None
                    page.wait_for_function(js('_es?.readyState === EventSource.OPEN', 'runtime.core.sync.watching?.readyState === EventSource.OPEN'))
                expect(page.locator('.media-load-error')).to_have_count(0)
                page.wait_for_function("document.querySelector('#msgs img').naturalWidth===2")
                expect(page.locator('#msgs')).to_contain_text('SHORT IMAGE')

                # Markdown uses the established inner HTML fragment rather than
                # gallery components; its recovery controls must work too.
                mode['status']=503
                page.locator(f'#side .item[data-uid="{uid(corpus,"codex-inline")}"]').click()
                inline_toggle=page.locator('#msgs .msg[data-role="user"] > .more[aria-expanded]')
                expect(inline_toggle).to_have_attribute('aria-expanded','false')
                expect(page.locator('#msgs img')).to_have_count(0)
                inline_toggle.click()
                inline_image=page.locator('#msgs img')
                expect(inline_image).to_have_count(1)
                page.locator('#msgs .media-load').scroll_into_view_if_needed()
                assert inline_image.get_attribute('data-vue-media') is None
                expect(page.locator('.media-load-error')).to_contain_text('HTTP 503')
                expect(page.locator('.media-load-retry')).to_be_visible()
                inline_snapshot=state()
                inline_src=inline_image.get_attribute('src')
                assert [(url,diagnostic) for url,diagnostic in media_requests if url==inline_src]==[
                    (inline_src,False),(inline_src,True)],'one inline image failure and one diagnostic'

                # Full-text disclosure repaints Markdown in both entries. The old
                # error panel must disappear, and the replacement must recover
                # through the same actual retry/reload controls.
                failed_wrapper=inline_image.locator('xpath=ancestor::span[contains(@class,"media-load")]').element_handle()
                inline_toggle.click()
                expect(inline_toggle).to_have_attribute('aria-expanded','false')
                expect(page.locator('#msgs img')).to_have_count(0)
                expect(page.locator('.media-load-error')).to_have_count(0)
                assert failed_wrapper.evaluate('wrapper=>!wrapper.isConnected')
                assert state()==inline_snapshot
                inline_toggle.click()
                expect(inline_toggle).to_have_attribute('aria-expanded','true')
                expect(inline_image).to_have_count(1)
                page.locator('#msgs .media-load').scroll_into_view_if_needed()
                expect(page.locator('.media-load-error')).to_have_count(1)
                expect(page.locator('.media-load-error')).to_contain_text('HTTP 503')
                expect(page.locator('.media-load-retry')).to_be_visible()
                assert state()==inline_snapshot
                mode['status']=None
                retry_requests=list(media_requests)
                page.locator('.media-load-retry').click()
                page.wait_for_function("document.querySelector('#msgs img').naturalWidth===2")
                expect(page.locator('.media-load-error')).to_have_count(0)
                assert state()==inline_snapshot
                assert media_requests[len(retry_requests):]==[(inline_src,False)],'manual retry must issue one same-token image GET'
                mode['status']=404
                inline_image.evaluate("img=>{const src=img.src;img.removeAttribute('src');setTimeout(()=>img.src=src,30)}")
                expect(page.locator('.media-load-error')).to_contain_text('HTTP 404')
                mode['status']=None
                reload_requests=len(requests)
                for attempt in range(3):
                    page.locator('.media-load-reload').click()
                    page.wait_for_function(js('historyPageRequests.size===0', 'runtime.core.history.historyPageRequests.size===0'))
                    panel=page.locator('.media-load-error')
                    if not panel.count():
                        break
                    expect(panel).to_contain_text('实时历史已更新；已保留新内容，请再次手动重新载入')
                    page.wait_for_function(js('_es?.readyState === EventSource.OPEN', 'runtime.core.sync.watching?.readyState === EventSource.OPEN'))
                expect(page.locator('.media-load-error')).to_have_count(0)
                assert any('/api/messages/' in url and 'window=1' in url for url in requests[reload_requests:])
                if inline_toggle.get_attribute('aria-expanded')=='false':
                    inline_toggle.click()
                inline_image.scroll_into_view_if_needed()
                page.wait_for_function("document.querySelector('#msgs img').naturalWidth===2")
                mode['status']=503;mode['hold_diagnostic']=True
                inline_image.evaluate("img=>{const src=img.src;img.removeAttribute('src');setTimeout(()=>img.src=src,30)}")
                page.wait_for_function(js('(globalThis.SessionDockOverlays ? SessionDockOverlays.mediaState.diagnosticActive : mediaDiagnosticActive)===1', 'runtime.mediaRuntime.diagnosticActive===1'))
                expect(page.locator('.media-load-error')).to_have_count(1)
                expect(page.locator('.media-load-error')).to_contain_text('正在读取错误说明')
                page.locator(f'#side .item[data-uid="{uid(corpus,"codex-other")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('OTHER VIEW ONLY')
                expect(page.locator('.media-load-error')).to_have_count(0)
                expect(page.locator('.media-load-retry')).to_have_count(0)
                expect(page.locator('.media-load-reload')).to_have_count(0)
                assert len(held)==1
                held.pop().fulfill(status=503,json={'error':'STALE INLINE DIAGNOSTIC'})
                page.wait_for_function(js('(globalThis.SessionDockOverlays ? SessionDockOverlays.mediaState.diagnosticActive : mediaDiagnosticActive)===0', 'runtime.mediaRuntime.diagnosticActive===0'))
                expect(page.locator('.media-load-error')).to_have_count(0)
                expect(page.locator('#detail')).not_to_contain_text('STALE INLINE DIAGNOSTIC')

                mode["status"]=None
                page.set_viewport_size({"width":390,"height":844})
                if page.locator('.mobile-back').is_visible():
                    page.locator('.mobile-back').click()
                page.locator(f'#side .item[data-uid="{uid(corpus,"codex-lazy")}"]').click()
                expect(page.locator("#msgs")).to_contain_text("LAZY ROW 1399")
                gap=page.locator('.history-gap');gap.scroll_into_view_if_needed()
                page.wait_for_function("document.querySelector('#msgs img').naturalWidth===2")
                gap.scroll_into_view_if_needed()
                top=gap.bounding_box()["y"];previous=state()["cursor"]
                page.locator('.history-gap-load').click()
                page.wait_for_function(js('historyPageRequests.size===0', 'runtime.core.history.historyPageRequests.size===0'))
                expect(page.locator('#msgs > .msg[data-role="user"]')).to_have_count(len(state()["text"]))
                page.wait_for_timeout(100)
                assert abs(page.locator('.history-gap').bounding_box()["y"]-top)<12
                assert state()["cursor"]==previous
                expect(page.locator('#a-term')).to_be_visible()
                expect(page.locator('#a-term')).to_be_enabled()
                assert page.evaluate("document.documentElement.scrollWidth<=innerWidth")
                assert not errors,errors
                assert all(url.startswith(base+'/') for url in requests)
                assert all('window=1' in url or '/page?' in url or 'start=' in url for url in requests if '/api/messages/' in url)
                print("PASS lazy media: offscreen zero GET; scroll decode; one bounded error diagnostic; inline fold/unfold cleanup; manual503 retry and404/409 window reload; stale view isolation; mobile gap/console; no remote/full-history/native mutations")
            finally:
                assert native_bytes(corpus.root)==before
                browser.close()


if __name__=="__main__":
    main()
