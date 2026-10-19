#!/usr/bin/env python3
"""Grok native SDK through real HTTP/SSE and Chromium, using a fake ACP CLI.

Requires the main integration's /api/native-sdk routes and SDK controls. Never
builds or calls a real Grok/model. Run with --binary after the shared build.
"""
from __future__ import annotations

import argparse
import re
import base64
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from sdk_usage_fixtures import assert_inline_usage, toggle_usage_details

from codex_sdk_browser import server
from grok_sdk_fixtures import GrokFixture
from history_fixtures import BINARY
from send_browser import initialize


def api(page, method, body):
    result = page.evaluate("""async ({method,body}) => {
      const r = await fetch('/api/native-sdk/' + method, {method:'POST',
        headers:{'Content-Type':'application/json'}, body:JSON.stringify(body)});
      return {status:r.status, body:await r.json()};
    }""", {"method": method, "body": body})
    assert result["status"] < 300 and result["body"].get("ok") is not False, result
    return result["body"]


def select(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.locator("#a-sdk").click()
    expect(page.locator("#sdk-input")).to_be_visible()


def snapshot(page, uid):
    return api(page, "open", {"uid": uid})["raw"]


def watch(page, uid):
    page.evaluate("""uid => {
      window.__grokEvents?.close(); window.__grokSnapshot = null;
      window.__grokSeqs = [];
      const es = window.__grokEvents = new EventSource('/api/native-sdk/events?uid=' + encodeURIComponent(uid));
      es.addEventListener('sdk', e => {
        const event=JSON.parse(e.data);
        if(event.type==='snapshot') {window.__grokSnapshot=event.raw;window.__grokSeqs.push(event.seq);}
        else if(event.type==='input_queue' && window.__grokSnapshot) Object.assign(window.__grokSnapshot,event.raw);
      });
    }""", uid)
    page.wait_for_function("window.__grokSnapshot !== null")


def capture_state(page, screenshots, name, target=None, popover=False):
    if not screenshots:
        return
    for width in (1280,390,320):
        if popover and page.locator("#sdk-usage-menu").is_visible():
            toggle_usage_details(page)
        page.set_viewport_size({"width":width,"height":900})
        # Let the media-query handler choose the mobile list/detail scene.
        page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
        if width < 500 and not page.evaluate("document.body.classList.contains('mobile-detail')"):
            selected=page.evaluate("S.sel")
            page.locator(f'#side .item[data-uid="{selected}"]').click()
        expect(page.locator("#sdk-input")).to_be_visible()
        if popover:
            toggle_usage_details(page)
        if target is not None:
            target.scroll_into_view_if_needed()
        for theme in ("light","dark"):
            page.emulate_media(color_scheme=theme)
            page.screenshot(path=str(screenshots / f"grok-{name}-{width}-{theme}.png"),full_page=True)
    page.set_viewport_size({"width":1280,"height":900})
    page.emulate_media(color_scheme="light")


def exercise(browser, base, fixture, screenshots):
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
    page = context.new_page()
    page.on('response', lambda response: print('SDK HTTP error', response.status, response.url, response.text(), flush=True) if '/api/native-sdk/' in response.url and response.status >= 400 else None)
    page.set_default_timeout(15000)
    failures, sends, operations = [], [], []
    page.on("request", lambda request: operations.append((request.url.rsplit("/",1)[-1],request.post_data_json))
            if "/api/native-sdk/" in request.url and request.method == "POST" else None)
    page.on("pageerror", lambda error: failures.append(str(error)))
    page.on('console', lambda message: print('console',message.type,message.text,flush=True) if message.type=='error' else None)
    page.on("request", lambda request: sends.append(request.post_data_json)
            if "/api/native-sdk/turn" in request.url and request.method == "POST" else None)
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    rows = page.evaluate("S.sessions")
    uid = next(row["uid"] for row in rows if row.get("session_id", row.get("sid")) == fixture.sid
               or row.get("title") == "Synthetic Grok SDK")
    # The real installation has no models_cache.json. New-session controls must
    # use initialize.modelState without creating/loading any native session.
    assert not (fixture.home / "models_cache.json").exists()
    page.locator("#new-session").click()
    page.locator('label:has(input[name=new-source][value=grok])').click()
    expect(page.locator("#new-model")).to_be_enabled()
    assert not fixture.received("session/new") and not fixture.received("session/load")
    assert not fixture.received("session/prompt")
    page.locator("#new-model").click()
    page.locator('#new-model-options [title="fake-other"]').click()
    page.locator("#new-effort").select_option("low")
    page.locator("#new-cwd").fill(str(fixture.work))
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/create')) as created:
        page.locator("#new-session-go").click()
    assert created.value.ok, created.value.text()
    create_input = created.value.request.post_data_json
    assert create_input["model"] == "fake-other" and create_input["effort"] == "low", create_input
    receipt = created.value.json()
    assert receipt["session_id"] != fixture.sid, receipt
    expect(page.locator("#sdk-input")).to_be_visible()
    created_raw = api(page,"open",{"uid":receipt["uid"]})["raw"]
    options = {row["id"]:row["currentValue"] for row in created_raw["configOptions"]}
    assert options == {"model":"fake-other","reasoning_effort":"low"}, options
    configured = fixture.received("session/set_config_option")
    assert [row["params"]["value"] for row in configured[-2:]] == ["fake-other","low"]
    assert len(fixture.received("session/new")) == 1 and not fixture.received("session/prompt")
    if screenshots:
        page.screenshot(path=str(screenshots / "grok-native-catalog-create.png"),full_page=True)
    select(page, uid)
    opened = api(page, "open", {"uid": uid})
    assert opened["session_id"] == fixture.sid and opened["cwd"] == str(fixture.work), opened
    assert opened["capabilities"]["send"] and opened["capabilities"]["interrupt"]
    assert opened["capabilities"]["respond"] and opened["capabilities"]["question"]
    assert not opened["capabilities"]["shared_tui"]
    assert not any("Replay is ordinary history" in item.get("content", "") for item in opened["raw"]["items"])
    watch(page, uid)
    before = len(fixture.received("session/load"))
    api(page, "open", {"uid": uid})
    assert len(fixture.received("session/load")) == before, "open reloaded a live native session"

    # Real UI send -> ACP tools and one explicit native permission outcome.
    page.locator("#sdk-input").fill("approval allow through native ACP")
    page.locator("#sdk-send").click()
    try:
        page.wait_for_function("window.__grokSnapshot?.requests.length === 1")
    except Exception:
        print('DEBUG', page.locator('.sdk-status').inner_text(), failures, sends, fixture.received()[-12:], flush=True)
        print(page.evaluate('({value:NativeSdkView.input?.value, pane:!!NativeSdkView.pane, agent:NativeSdkView.agent, generation:NativeSdkView.generation})'),flush=True)
        raise
    pending = page.evaluate("window.__grokSnapshot")
    assert pending["running"] and pending["requests"][0]["type"] == "approval"
    native_tool = next(item for item in pending["items"] if item["role"] == "tool")
    assert native_tool["status"] == "in_progress" and native_tool["tool_result"]["exit_code"] == 0
    assert native_tool["result"]["text"] == "partial native stdout", "partial native output was lost"
    tool_card = page.locator('.sdk-live .tool-msg').filter(has_text='Synthetic command')
    expect(tool_card.locator('.tool-status')).to_have_text('运行中…')
    expect(tool_card).not_to_contain_text('✓ 完成')
    expect(tool_card.locator('.tool-out')).to_contain_text('partial native stdout')
    if screenshots:
        page.screenshot(path=str(screenshots / 'grok-tool-in-progress.png'),full_page=True)
    assert not [m for m in fixture.received() if isinstance(m.get("id"), str) and "method" not in m], "permission was auto-answered"
    assert sends and sends[-1].get("request_id"), "UI must retain submission identity"
    prompt_count = len(fixture.received("session/prompt"))
    api(page, "turn", sends[-1])
    assert len(fixture.received("session/prompt")) == prompt_count, "duplicate request_id sent a second prompt"
    page.get_by_role("button", name="Allow once", exact=True).click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running")
    done = page.evaluate("window.__grokSnapshot")
    assert not done["requests"]
    tools = [item for item in done["items"] if item["role"] == "tool"]
    assert tools[-1]["tool_input"] == {"command": "printf synthetic"}
    assert tools[-1]["tool_result"] == {"output": "Grok native tool result"}
    assert tools[-1]["result"]["error"] is False
    expect(tool_card.locator('.tool-status')).to_contain_text('✓ 完成')
    expect(tool_card.locator('.tool-out')).to_contain_text('Grok native tool result')
    expect(page.locator(".sdk-live")).to_contain_text("Grok final answer")
    if screenshots:
        for width in (1280,390,320):
            page.set_viewport_size({'width':width,'height':900})
            if width < 500 and not page.locator('#sdk-input').is_visible():
                selected = page.evaluate('S.sel')
                page.locator(f'#side .item[data-uid="{selected}"]').click()
            expect(page.locator('#sdk-input')).to_be_visible()
            for theme in ('light','dark'):
                page.emulate_media(color_scheme=theme)
                page.screenshot(path=str(screenshots / f"grok-sdk-{width}-{theme}.png"),full_page=True)
        page.set_viewport_size({'width':1280,'height':900})
        page.emulate_media(color_scheme='light')

    page.locator("#sdk-input").fill("approval deny through native ACP")
    page.locator("#sdk-send").click()
    page.get_by_role("button", name="Reject", exact=True).click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running && window.__grokSnapshot.items.some(i=>i.error)")
    assert not page.evaluate("window.__grokSnapshot.requests.length")

    # Images use the ordinary published attachment path even when ACP advertises image:true.
    page.locator("#sdk-add").click()
    with page.expect_file_chooser() as chooser:
        page.locator('#sdk-attach-menu [data-attach=image]').click()
    image = base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=')
    chooser.value.set_files({"name":"grok-synthetic-image.png","mimeType":"image/png","buffer":image})
    expect(page.locator("#sdk-compose-items")).to_contain_text("grok-synthetic-image.png")
    page.locator("#sdk-input").fill("Grok image attachment")
    page.locator("#sdk-send").click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running && window.__grokSnapshot.items.some(i=>i.content?.includes('Grok image attachment'))")
    structured = fixture.received("session/prompt")[-1]["params"]["prompt"]
    assert len(structured) == 1 and structured[0]["type"] == "text", structured
    assert "grok-synthetic-image.png" in structured[0]["text"] and "sessiondock_attachments" in structured[0]["text"], structured
    assert list(fixture.work.rglob("grok-synthetic-image.png"))

    # Native question labels and multi-select arrays survive a real form submit.
    page.locator("#sdk-input").fill("question")
    page.locator("#sdk-send").click()
    question_card = page.locator(".sdk-request").filter(has_text="Grok 需要回答")
    expect(question_card).to_be_visible()
    question_card.get_by_label("Postgres",exact=True).check()
    question_card.get_by_label("Search",exact=True).check()
    question_card.get_by_label("Cache",exact=True).check()
    capture_state(page,screenshots,"question",question_card)
    question_card.get_by_role("button",name="提交",exact=True).click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running")
    expect(page.locator(".sdk-live")).to_contain_text("Native interaction accepted")
    answered = [row for row in fixture.received() if str(row.get("id","")).startswith("interaction-") and "method" not in row]
    assert answered[-1]["result"]["answers"]["Which features?"] == ["Search","Cache"],answered[-1]

    # Plan exit remains an explicit native approval, never auto-approved.
    page.locator("#sdk-input").fill("plan-approval")
    page.locator("#sdk-send").click()
    plan_card = page.locator(".sdk-request").filter(has_text="Grok 计划批准")
    expect(plan_card).to_contain_text("Synthetic native plan")
    capture_state(page,screenshots,"plan",plan_card)
    plan_card.get_by_role("button",name="批准计划",exact=True).click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running")
    answered = [row for row in fixture.received() if str(row.get("id","")).startswith("interaction-") and "method" not in row]
    assert answered[-1]["result"] == {"outcome":"approved"},answered[-1]
    # Prompt partial cost is hidden rather than shown as a zero or full bill.
    assert "costUsdTicks" not in snapshot(page,uid)["usage"]["last_turn"]

    # Read the native process ledger and account billing through the toolbar.
    toggle_usage_details(page)
    page.locator("#native-usage-refresh").click()
    expect(page.locator("#native-usage-detail")).to_contain_text("1,500")
    expect(page.locator(".native-sdk-pane .sdk-gauges")).to_have_attribute("title",re.compile("42.5"))
    assert fixture.received("x.ai/session/usage") and fixture.received("x.ai/billing")
    capture_state(page,screenshots,"usage",popover=True)
    page.keyboard.press("Escape")
    page.locator("#sdk-more").click()
    page.locator("#native-sdk-mode").select_option("plan")
    page.wait_for_function("window.__grokSnapshot?.mode === 'plan'")
    assert fixture.received("session/set_mode")[-1]["params"]["modeId"] == "plan"
    page.keyboard.press("Escape")

    # Native compaction and direct shell use their own protocol operations.
    page.locator("#sdk-more").click()
    with page.expect_response(lambda response: response.url.endswith("/api/native-sdk/compact") and response.request.method == "POST") as compact_response:
        page.locator('[data-sdk-action="compact"]').click()
    assert compact_response.value.status < 300
    compact_body = next(body for method,body in reversed(operations) if method == "compact")
    count = len(fixture.received("x.ai/compact_conversation"))
    api(page,"compact",compact_body)
    assert len(fixture.received("x.ai/compact_conversation")) == count == 1
    page.locator("#sdk-more").click()
    page.locator('[data-sdk-action="shell"]').click()
    page.get_by_role("textbox",name="命令",exact=True).fill("printf direct-native-shell")
    page.get_by_role("button",name="运行",exact=True).click()
    expect(page.locator(".sdk-live")).to_contain_text("Grok direct shell output")
    shell = fixture.received("session/prompt")[-1]["params"]["prompt"][0]
    assert shell["_meta"] == {"bash_command":"printf direct-native-shell"}
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running")

    # Only native rewind points are selectable; file rewind is never requested.
    page.locator("#sdk-more").click()
    page.locator('[data-sdk-action="rewind"]').click()
    page.get_by_role("combobox",name="回退消息",exact=True).select_option("0")
    with page.expect_response(lambda response: response.url.endswith("/api/native-sdk/rewind") and response.request.method == "POST") as rewind_response:
        page.get_by_role("button",name="回退",exact=True).click()
    assert rewind_response.value.status < 300
    rewind = fixture.received("x.ai/rewind/execute")[-1]["params"]
    assert rewind["mode"] == "conversation_only" and rewind["force"] is False
    rewind_body = next(body for method,body in reversed(operations) if method == "rewind")
    api(page,"rewind",rewind_body)
    assert len(fixture.received("x.ai/rewind/execute")) == 1

    # Native fork returns its new exact identity and the UI navigates there.
    page.locator("#sdk-more").click()
    page.locator('[data-sdk-action="fork"]').click()
    page.wait_for_function("uid => S.sel !== uid",arg=uid)
    fork_body = next(body for method,body in reversed(operations) if method == "fork")
    forked = api(page,"fork",fork_body)
    assert forked["uid"] != uid and forked["session_id"] != fixture.sid
    assert page.evaluate("S.sel") == forked["uid"]
    assert len(fixture.received("x.ai/session/fork")) == 1
    assert fixture.path.is_dir() and (fixture.path/"summary.json").exists()
    select(page,uid)
    watch(page,uid)

    # Closing every EventSource/page does not cancel an accepted native turn.
    page.locator("#sdk-input").fill("detached")
    page.locator("#sdk-send").click()
    page.wait_for_function("window.__grokSnapshot?.running === true")
    count = len(fixture.received("session/prompt"))
    page.goto("about:blank")
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    select(page, uid)
    watch(page, uid)
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running")
    assert len(fixture.received("session/prompt")) == count
    assert not fixture.received("session/cancel"), "view detach cancelled native work"

    # Stop is a native notification, leaving the ACP runtime reusable.
    page.locator("#sdk-input").fill("wait-for-cancel")
    page.locator("#sdk-send").click()
    page.wait_for_function("window.__grokSnapshot?.running === true")
    page.locator("#sdk-input").fill("FIFO queued while active")
    page.locator("#sdk-send").click()
    page.wait_for_function("window.__grokSnapshot?.input_queue?.some(i=>i.text==='FIFO queued while active' && i.state==='queued')")
    expect(page.locator("#sdk-input")).to_have_value("")
    page.locator("#sdk-stop").click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running && window.__grokSnapshot.items.some(i=>i.content==='FIFO queued while active')")
    prompts = fixture.received("session/prompt")
    assert [row["params"]["prompt"][0]["text"] for row in prompts[-2:]] == ["wait-for-cancel", "FIFO queued while active"]
    assert fixture.received("session/cancel")[-1]["params"]["sessionId"] == fixture.sid
    assert "id" not in fixture.received("session/cancel")[-1]

    count = len(fixture.received("session/prompt"))
    api(page, "turn", {"uid": uid, "request_id": "cancel-before-prompt", "text": "never-native-prompt", "effort": "high"})
    api(page, "interrupt", {"uid": uid})
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running")
    assert len(fixture.received("session/prompt")) == count, "stop overtook setup and later allowed the prompt"

    api(page, "turn", {"uid": uid, "request_id": "native-error", "text": "native-error"})
    page.wait_for_function("window.__grokSnapshot?.error?.includes('grok_sdk_native_error')")
    assert not snapshot(page, uid)["running"]
    failed_card=page.locator('[data-queue-origin="bridge"]').filter(has_text="native-error")
    failed_card.get_by_role("button",name="撤销",exact=True).click()

    # Native create identity, retry cache and model/effort without a PTY.
    create = {"source": "grok", "cwd": str(fixture.work), "request_id": "grok-create-retry",
              "model": "fake-other", "effort": "high"}
    create_count = len(fixture.received("session/new"))
    created = api(page, "create", create)
    retried = api(page, "create", create)
    assert created["uid"] == retried["uid"] and created["session_id"] == retried["session_id"]
    assert len(fixture.received("session/new")) == create_count + 1
    new_sid = created["session_id"]
    assert created["session"]["source"] == "grok" and created["cwd"] == str(fixture.work)
    native = fixture.path.parent / new_sid
    assert native.is_dir() and not (native / "chat_history.jsonl").exists(), "bridge invented native history"
    assert created["raw"]["model"] == "fake-other" and created["raw"]["effort"] == "high"
    models = api(page, "models", {"uid": created["uid"]})
    assert models["models"] and models["configOptions"]
    receipt = api(page, "turn", {"uid": created["uid"], "text": "new native session", "request_id": "new-first"})
    assert receipt["accepted"]
    page.wait_for_function("""async uid => {
      const r=await fetch('/api/native-sdk/open',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({uid})});
      return !(await r.json()).raw.running;
    }""", arg=created["uid"])
    assert (native / "chat_history.jsonl").is_file()
    assert any(row["params"].get("sessionId") == new_sid for row in fixture.received("session/prompt"))
    api(page, "discard", {"uid": created["uid"]})
    assert native.is_dir(), "discard deleted native files"

    # The new native process models installed builds that advertise image:false.
    fixture.capability_marker.write_text(json.dumps({"image":False}))
    # Disconnect recovery must load the same SID and retain accepted ids.
    api(page, "turn", {"uid": uid, "request_id": "disconnect-once", "text": "disconnect"})
    page.wait_for_function("window.__grokSnapshot?.error?.includes('grok_sdk_disconnected')")
    recovered = api(page, "open", {"uid": uid})
    assert recovered["session_id"] == fixture.sid
    count = len(fixture.received("session/prompt"))
    api(page, "turn", {"uid": uid, "request_id": "disconnect-once", "text": "disconnect"})
    assert len(fixture.received("session/prompt")) == count
    assert "images" not in recovered["capabilities"]

    # An image:false handshake preserves the established file-path send flow.
    # SessionDock attachments remain available without a permanent protocol note.
    # HTTP does not manufacture ACP image support; the file is published once.
    page.reload(wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    select(page,uid)
    watch(page,uid)
    unknown_card=page.locator('[data-queue-origin="bridge"]').filter(has_text="disconnect")
    unknown_card.get_by_role("button",name="移除记录",exact=True).click()
    page.get_by_role("dialog").get_by_role("button",name="移除记录",exact=True).click()
    expect(unknown_card).to_have_count(0)
    expect(page.locator("#native-media-note")).to_have_count(0)
    page.locator("#sdk-add").click()
    with page.expect_file_chooser() as chooser:
        page.locator('#sdk-attach-menu [data-attach=image]').click()
    chooser.value.set_files({"name":"grok-path-only.png","mimeType":"image/png","buffer":image})
    expect(page.locator("#sdk-compose-items")).to_contain_text("grok-path-only.png")
    page.locator("#sdk-input").fill("Grok image path fallback")
    page.locator("#sdk-send").click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running && window.__grokSnapshot.items.some(i=>i.content?.includes('Grok image path fallback'))")
    fallback = fixture.received("session/prompt")[-1]["params"]["prompt"]
    assert len(fallback) == 1 and fallback[0]["type"] == "text",fallback
    assert "grok-path-only.png" in fallback[0]["text"] and "sessiondock_attachments" in fallback[0]["text"],fallback
    assert any(path.name == "grok-path-only.png" for path in fixture.work.rglob("grok-path-only.png"))
    expect(page.locator("#sdk-input")).to_have_value("")
    expect(page.locator("#native-media-note")).to_have_count(0)
    capture_state(page,screenshots,"image-attachment")

    # Old/native builds report unsupported extensions honestly; refresh errors
    # stay visible and the next open removes the corresponding capability.
    page.locator("#sdk-input").fill("disable-extensions")
    page.locator("#sdk-send").click()
    page.wait_for_function("window.__grokSnapshot && !window.__grokSnapshot.running && window.__grokSnapshot.items.some(i=>i.content==='disable-extensions')")
    toggle_usage_details(page)
    page.locator("#native-usage-refresh").click()
    expect(page.locator("#native-usage-detail")).to_contain_text("grok_sdk_native_error")
    unsupported = api(page,"open",{"uid":uid})
    assert unsupported["capabilities"]["usage"] is False
    assert unsupported["raw"]["extension_support"]["usage"] is False
    page.keyboard.press("Escape")

    # A synthetic independently owned TUI is refused and never signalled.
    pid = 987654
    process = fixture.proc / str(pid)
    process.mkdir()
    (process / "stat").write_text(f"{pid} (grok) S 1 " + "0 " * 30 + "\n")
    fixture.active.write_text(json.dumps({"sessions": [{"id": fixture.sid, "pid": pid}]}))
    count=len(fixture.received("session/prompt"))
    page.locator("#sdk-input").fill("external writer")
    page.locator("#sdk-send").click()
    blocked=page.locator('[data-queue-origin="bridge"]').filter(has_text="external writer")
    expect(blocked).to_contain_text("grok_sdk_session_busy")
    assert len(fixture.received("session/prompt"))==count,"queue admission bypassed native writer protection"
    assert (process / "stat").exists()
    fixture.active.unlink()
    assert not failures, failures
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-grok-sdk-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok"):
            (root / name).mkdir()
        fixture = GrokFixture(root)
        initialize('--initialize-lifecycle', root / 'ledger', args.binary.resolve(strict=True))
        extra = {**fixture.env, 'SESSIONDOCK_NATIVE_SDKS':'auto',
            'SESSIONDOCK_LIFECYCLE_DIR':str(root/'ledger'),
            'SESSIONDOCK_PTYHOST_DIR':str(root/'unused-host'),
            'SESSIONDOCK_STATE_DIR':str(root/'state')}
        (root/'state').mkdir()
        with server(SimpleNamespace(root=root), args.binary, "off", extra) as base:
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch(headless=True)
                exercise(browser, base, fixture, args.screenshots)
                browser.close()
    print("Grok native ACP SDK browser: PASS")


if __name__ == "__main__":
    main()
