#!/usr/bin/env python3
"""Grok explicit interject and ten-minute idle lifecycle through Chromium.

The preload clock accelerates only this temporary HTTP service's monotonic
clock. Production has no timeout override, and fake native peers keep real time.
"""
from __future__ import annotations
import argparse
import base64
from pathlib import Path
import json
import os
import subprocess
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import expect,sync_playwright
from codex_sdk_browser import server
from grok_sdk_browser import api,select,snapshot,watch,capture_state
from grok_sdk_fixtures import GrokFixture
from history_fixtures import BINARY
from send_browser import initialize
from sdkhost_fixtures import stop_private_sdkhosts


def alive(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(") ",1)[1][0] != "Z"
    except FileNotFoundError:
        return False


def native_pid(fixture):
    return [row["fixture_start"]["pid"] for row in fixture.received() if "fixture_start" in row][-1]


def page_for(context,base,fixture):
    page=context.new_page();page.set_default_timeout(15000)
    page.goto(base,wait_until="domcontentloaded");page.wait_for_function("T.listLoaded")
    uid=next(row["uid"] for row in page.evaluate("S.sessions") if row.get("session_id",row.get("sid"))==fixture.sid)
    select(page,uid);watch(page,uid)
    return page,uid


def advance(observer,marker,seconds):
    value=int(marker.read_text())+seconds
    pending=marker.with_suffix(".next");pending.write_text(str(value));pending.replace(marker)
    # Wake the service reactor without opening/subscribing a native session.
    for _ in range(4):
        observer.evaluate("async () => (await fetch('/api/health')).json()")
    # HTTP completion is not a Tokio timer/stream-drop barrier. Let the private
    # server process the due timers before advancing its monotonic clock again.
    time.sleep(.1)


def wait_for_process(observer,pid,expected):
    deadline=time.monotonic()+12
    while alive(pid)!=expected and time.monotonic()<deadline:
        observer.evaluate("async () => (await fetch('/api/health')).json()")
        time.sleep(.03)
    assert alive(pid)==expected,(pid,expected)


def steer_checks(browser,base,fixture,marker,screenshots):
    context=browser.new_context(viewport={"width":1280,"height":900})
    page,uid=page_for(context,base,fixture)
    sent=[]
    page.on("request",lambda request:sent.append(request.post_data_json) if request.url.endswith("/api/native-sdk/steer") and request.method=="POST" else None)
    page.locator("#sdk-input").fill("steer-active");page.locator("#sdk-send").click()
    page.wait_for_function("window.__grokSnapshot?.running && window.__grokSnapshot.items.some(i=>i.content==='Grok streaming ')")
    page.locator("#sdk-input").fill("ordinary draft stays here")
    page.locator("#sdk-more").click();page.locator('[data-sdk-action="steer"]').click()
    page.get_by_role("textbox",name="引导内容",exact=True).fill("Use the native safe point")
    page.get_by_role("button",name="发送引导",exact=True).click()
    page.wait_for_function("window.__grokSnapshot?.steers?.some(r=>r.status==='native_received' && r.accepted===true)")
    expect(page.locator("#sdk-input")).to_have_value("ordinary draft stays here")
    receipt=page.evaluate("window.__grokSnapshot.steers[0]")
    assert receipt["accepted"] is True and receipt["native_received"] is True and receipt["consumed"] is None,receipt
    assert "Native safe point consumed" not in page.locator(".sdk-live").inner_text()
    native=fixture.received("x.ai/interject")
    assert len(native)==1 and native[0]["method"]=="_x.ai/interject"
    assert native[0]["params"]["interjectionId"]==sent[-1]["request_id"]
    retry=api(page,"steer",sent[-1]);assert retry["request_id"]==receipt["request_id"]
    assert len(fixture.received("x.ai/interject"))==1,"lost ACK replayed the native interjection"
    # Select/upload through the real composer. The API accepts the authenticated
    # staging grant, never a caller-supplied native image block or file path.
    page.locator("#sdk-add").click()
    with page.expect_file_chooser() as chooser:
        page.locator('#sdk-attach-menu [data-attach=image]').click()
    picture=base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=')
    chooser.value.set_files({"name":"steer-native-image.png","mimeType":"image/png","buffer":picture})
    expect(page.locator("#sdk-compose-items")).to_contain_text("steer-native-image.png")
    attachments=page.evaluate("""async () => {
      const payload=await CodexSdkCompose.collect(NativeSdkView);
      CodexSdkCompose.failed(NativeSdkView);return payload.attachments;
    }""")
    media={"uid":uid,"request_id":"trusted-steer-image","text":"Image at the safe point","attachments":attachments}
    api(page,"steer",media)
    params=fixture.received("x.ai/interject")[-1]["params"]
    assert "content" not in params and "steer-native-image.png" in params["text"] and "sessiondock_attachments" in params["text"],params
    expect(page.locator("#native-media-note")).to_have_count(0)
    api(page,"steer",media)
    assert len(fixture.received("x.ai/interject"))==2,"image retry repeated native interject"
    capture_state(page,screenshots,"steer-pending",page.locator('[data-queue-origin="steer"]').first)
    fixture.safe_point.write_text("allow native drain")
    expect(page.locator(".sdk-live")).to_contain_text("Native safe point consumed: Use the native safe point")
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running")
    assert snapshot(page,uid)["steers"][0]["consumed"] is None,"fixture model output cannot manufacture protocol consumption proof"
    pid=native_pid(fixture)
    page.close()
    observer=context.new_page();observer.goto(base+"/api/health")
    advance(observer,marker,20);advance(observer,marker,620)
    assert alive(pid),"native accepted steer with no consumption proof was reclaimed"
    reopened,again=page_for(context,base,fixture)
    assert again==uid and snapshot(reopened,uid)["steers"][0]["request_id"]==receipt["request_id"]
    assert len(fixture.received("x.ai/interject"))==2
    context.close()


def idle_checks(browser,base,fixture,marker,screenshots):
    context=browser.new_context(viewport={"width":1280,"height":900})
    page,uid=page_for(context,base,fixture)
    pid=native_pid(fixture)
    observer=context.new_page();observer.goto(base+"/api/health")
    advance(observer,marker,20);advance(observer,marker,620)
    assert alive(pid),"a subscribed idle runtime was reclaimed"
    page.locator("#sdk-more").click();page.locator("#native-sdk-mode").select_option("plan")
    page.wait_for_function("window.__grokSnapshot?.mode==='plan'")
    page.keyboard.press("Escape")
    page.locator("#sdk-input").fill("wait-for-cancel");page.locator("#sdk-send").click()
    page.wait_for_function("window.__grokSnapshot?.running")
    page.close();advance(observer,marker,620)
    assert alive(pid),"active task was reclaimed after the browser left"
    page,uid=page_for(context,base,fixture)
    page.locator("#sdk-stop").click();page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running")
    page.locator("#sdk-input").fill("approval idle protection");page.locator("#sdk-send").click()
    expect(page.get_by_role("button",name="Allow once",exact=True)).to_be_visible()
    page.close();advance(observer,marker,620)
    assert alive(pid),"a pending native permission was reclaimed"
    page,uid=page_for(context,base,fixture)
    expect(page.get_by_role("button",name="Allow once",exact=True)).to_be_visible()
    page.get_by_role("button",name="Allow once",exact=True).click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running && !window.__grokSnapshot.requests.length")
    page.locator("#sdk-input").fill("background-task");page.locator("#sdk-send").click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running && window.__grokSnapshot.native_tasks?.length>0")
    page.close();advance(observer,marker,20);advance(observer,marker,620)
    assert alive(pid),"a native background task was reclaimed after the prompt ended"
    page,uid=page_for(context,base,fixture)
    page.locator("#sdk-input").fill("clear-background");page.locator("#sdk-send").click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running && !window.__grokSnapshot.native_tasks?.length")
    page.wait_for_function("window.__grokSnapshot && Array.isArray(window.__grokSnapshot.input_queue) && window.__grokSnapshot.input_queue.length===0")
    prompts=len(fixture.received("session/prompt"))
    page.close()
    # The first tick lets the closed SSE stream observe its failed heartbeat;
    # the next tick can start the no-receiver idle deadline.
    advance(observer,marker,20)
    advance(observer,marker,20)
    assert alive(pid),"idle runtime was released before ten minutes"
    advance(observer,marker,620);wait_for_process(observer,pid,False)
    fixture.capability_marker.write_text(json.dumps({"image":True,"omit_settings":True}))
    page,again=page_for(context,base,fixture)
    assert again==uid and native_pid(fixture)!=pid
    assert fixture.received("session/load")[-1]["params"]["sessionId"]==fixture.sid
    assert len(fixture.received("session/prompt"))==prompts,"reopen replayed a native prompt"
    raw=snapshot(page,uid)
    assert not raw["items"] and not raw["requests"],raw
    assert raw["configOptions"] is None and raw["modes"] is None and raw["mode"] is None,"reopen retained unconfirmed old native settings"
    assert not isinstance(raw["usage"],dict) or raw["usage"].get("last_turn") is None,raw
    capture_state(page,screenshots,"idle-reopened")
    context.close()


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--binary",type=Path,default=BINARY);parser.add_argument("--screenshots",type=Path)
    args=parser.parse_args()
    if args.screenshots:args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-grok-lifecycle-") as temporary:
        parent=Path(temporary);library=parent/"private-clock.so"
        subprocess.run(["cc","-shared","-fPIC","-O2",str(Path(__file__).with_name("grok_sdk_idle_clock.c")),"-o",str(library),"-ldl"],check=True,timeout=30)
        with sync_playwright() as playwright:
            browser=playwright.chromium.launch(headless=True)
            for name,exercise in (("steer",steer_checks),("steer-path",steer_checks),("idle",idle_checks)):
                root=parent/name;root.mkdir()
                for source in ("claude","codex","grok"):(root/source).mkdir()
                fixture=GrokFixture(root);marker=root/"clock-seconds";marker.write_text("0")
                if name=="steer-path":fixture.capability_marker.write_text(json.dumps({"image":False}))
                initialize("--initialize-lifecycle",root/"ledger",args.binary.resolve(strict=True))
                (root/"state").mkdir()
                extra={**fixture.env,"SESSIONDOCK_NATIVE_SDKS":"auto","SESSIONDOCK_LIFECYCLE_DIR":str(root/"ledger"),"SESSIONDOCK_PTYHOST_DIR":str(root/"unused-host"),"SESSIONDOCK_STATE_DIR":str(root/"state"),"LD_PRELOAD":str(library),"SESSIONDOCK_TEST_CLOCK_FILE":str(marker)}
                try:
                    with server(SimpleNamespace(root=root),args.binary,"off",extra) as base:exercise(browser,base,fixture,marker,args.screenshots)
                finally:
                    stop_private_sdkhosts(root)
            browser.close()
    print("PASS Grok explicit native interject, receipt boundaries, subscribed/active/approval protection, ten-minute idle release and same-SID reopen")

if __name__=="__main__":main()
