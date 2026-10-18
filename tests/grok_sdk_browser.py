#!/usr/bin/env python3
"""Grok native SDK through real HTTP/SSE and Chromium, using a fake ACP CLI.

Requires the main integration's /api/native-sdk routes and SDK controls. Never
builds or calls a real Grok/model. Run with --binary after the shared build.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

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
      });
    }""", uid)
    page.wait_for_function("window.__grokSnapshot !== null")


def exercise(browser, base, fixture, screenshots):
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
    page = context.new_page()
    page.on('response', lambda response: print('SDK HTTP error', response.status, response.url, response.text(), flush=True) if '/api/native-sdk/' in response.url and response.status >= 400 else None)
    page.set_default_timeout(15000)
    failures, sends = [], []
    page.on("pageerror", lambda error: failures.append(str(error)))
    page.on('console', lambda message: print('console',message.type,message.text,flush=True) if message.type=='error' else None)
    page.on("request", lambda request: sends.append(request.post_data_json)
            if "/api/native-sdk/turn" in request.url and request.method == "POST" else None)
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    rows = page.evaluate("S.sessions")
    uid = next(row["uid"] for row in rows if row.get("session_id", row.get("sid")) == fixture.sid
               or row.get("title") == "Synthetic Grok SDK")
    select(page, uid)
    opened = api(page, "open", {"uid": uid})
    assert opened["session_id"] == fixture.sid and opened["cwd"] == str(fixture.work), opened
    assert opened["capabilities"]["send"] and opened["capabilities"]["interrupt"]
    assert opened["capabilities"]["respond"] and not opened["capabilities"]["question"]
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
    expect(page.locator("#sdk-input")).to_have_value("FIFO queued while active")
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

    # Native create identity, retry cache and model/effort without a PTY.
    create = {"source": "grok", "cwd": str(fixture.work), "request_id": "grok-create-retry",
              "model": "fake-other", "effort": "high"}
    created = api(page, "create", create)
    retried = api(page, "create", create)
    assert created["uid"] == retried["uid"] and created["session_id"] == retried["session_id"]
    assert len(fixture.received("session/new")) == 1
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

    # Disconnect recovery must load the same SID and retain accepted ids.
    api(page, "turn", {"uid": uid, "request_id": "disconnect-once", "text": "disconnect"})
    page.wait_for_function("window.__grokSnapshot?.error?.includes('grok_sdk_disconnected')")
    recovered = api(page, "open", {"uid": uid})
    assert recovered["session_id"] == fixture.sid
    count = len(fixture.received("session/prompt"))
    api(page, "turn", {"uid": uid, "request_id": "disconnect-once", "text": "disconnect"})
    assert len(fixture.received("session/prompt")) == count

    # A synthetic independently owned TUI is refused and never signalled.
    pid = 987654
    process = fixture.proc / str(pid)
    process.mkdir()
    (process / "stat").write_text(f"{pid} (grok) S 1 " + "0 " * 30 + "\n")
    fixture.active.write_text(json.dumps({"sessions": [{"id": fixture.sid, "pid": pid}]}))
    result = page.evaluate("""async uid => {
      const r=await fetch('/api/native-sdk/turn',{method:'POST',headers:{'Content-Type':'application/json'},
        body:JSON.stringify({uid,text:'external writer',request_id:'busy'})});
      return {status:r.status,body:await r.json()};
    }""", uid)
    assert result["status"] >= 400 and "grok_sdk_session_busy" in json.dumps(result)
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
