#!/usr/bin/env python3
"""Claude native SDK browser contract, using a private fake CLI only.

Requires the primary reviewer's generic /api/native-sdk HTTP/UI integration
and an already built SessionDock binary. This suite never builds Rust, calls
a real provider, reads a real CLI home, or starts a PTY.
"""
from __future__ import annotations

import argparse
import base64
import json
from pathlib import Path
import re
import tempfile
import time

from claude_sdk_fixtures import ClaudeFixture, native_uid, project_key
from history_fixtures import BINARY


def post(page, method, body, *, ok=True):
    result = page.evaluate("""async ({method,body}) => {
      const response = await fetch('/api/native-sdk/' + method, {
        method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body)});
      return {status:response.status, body:await response.json()};
    }""", {"method": method, "body": body})
    if ok:
        assert 200 <= result["status"] < 300 and result["body"].get("ok") is not False, result
    return result["body"]


def receipt(body):
    return body["raw"] if "uid" not in body and isinstance(body.get("raw"), dict) and "uid" in body["raw"] else body


def snapshot(page, uid):
    return page.evaluate(r"""async uid => {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), 10000);
      try {
        const response = await fetch('/api/native-sdk/events?uid=' + encodeURIComponent(uid), {signal:controller.signal});
        if (!response.ok) throw Error('SSE HTTP ' + response.status);
        const reader = response.body.getReader(), decoder = new TextDecoder();
        let buffer = '';
        while (true) {
          const {done,value} = await reader.read();
          if (done) throw Error('SSE closed before snapshot');
          buffer += decoder.decode(value,{stream:true}).replace(/\r/g,'');
          let end;
          while ((end = buffer.indexOf('\n\n')) >= 0) {
            const event = buffer.slice(0,end); buffer = buffer.slice(end+2);
            const data = event.split('\n').filter(s => s.startsWith('data:')).map(s=>s.slice(5).trim()).join('\n');
            if (data) {
              const frame = JSON.parse(data);
              if (frame.type === 'snapshot') { await reader.cancel(); return frame; }
            }
          }
        }
      } finally { clearTimeout(timer); controller.abort(); }
    }""", uid)


def wait_snapshot(page, uid, predicate):
    deadline = time.monotonic() + 15
    last = None
    while time.monotonic() < deadline:
        last = snapshot(page, uid)
        if predicate(last["raw"]):
            return last
        page.wait_for_timeout(50)
    raise AssertionError("native snapshot condition not reached: " + json.dumps(last, ensure_ascii=False))


def open_sdk(page, fixture, expect):
    card = page.locator(f'#side .item[data-uid="{fixture.uid}"]')
    if not card.is_visible() and page.locator('#detail .mobile-back').is_visible():
        page.locator('#detail .mobile-back').click()
    page.locator(f'#side .item[data-uid="{fixture.uid}"]').click()
    expect(page.locator("#a-sdk")).to_be_visible()
    page.locator("#a-sdk").click()
    expect(page.locator("#sdk-input")).to_be_visible()
    expect(page.locator("#sdk-send")).to_be_enabled()
    page.wait_for_function("uid => NativeSdkView.connected && NativeSdkView.uid === uid",arg=fixture.uid)


def send(page, text):
    page.locator("#sdk-input").fill(text)
    page.locator("#sdk-send").click()


def browser_check(fixture, base, screenshots):
    from playwright.sync_api import sync_playwright, expect
    def capture(page, name):
        if screenshots:
            screenshots.mkdir(parents=True, exist_ok=True)
            page.screenshot(path=str(screenshots / name), full_page=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True, args=["--no-sandbox"])
        context = browser.new_context(viewport={"width": 1280, "height": 900}, color_scheme='light', service_workers="block")
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        turns = []
        settings = []
        page.on("request", lambda request: turns.append(request.post_data_json)
                if request.method == "POST" and "/api/native-sdk/turn" in request.url else None)
        page.on("request", lambda request: settings.append(request.post_data_json)
                if request.method == "POST" and "/api/native-sdk/settings" in request.url else None)
        page.goto(base, wait_until="networkidle")
        open_sdk(page, fixture, expect)
        assert len(fixture.frames("spawn")) == 1
        send(page, "hello from browser")
        expect(page.locator(".sdk-live")).to_contain_text("Claude synthetic final hello from browser")
        final = wait_snapshot(page, fixture.uid, lambda raw: not raw["running"])
        assert final["raw"]["session_id"] == fixture.sid
        ids = [item["id"] for item in final["raw"]["items"]]
        assert len(ids) == len(set(ids)), ids
        assert len([frame for frame in fixture.frames("user") if frame["message"]["content"] == "hello from browser"]) == 1
        post(page, "turn", turns[-1])
        assert len([frame for frame in fixture.frames("user") if frame["message"]["content"] == "hello from browser"]) == 1
        page.locator('#sdk-usage-toggle').click()
        page.locator('#native-usage-refresh').click()
        expect(page.locator('#native-usage-detail')).to_contain_text('输入 Token：100')
        expect(page.locator('#native-usage-detail')).to_contain_text('缓存读取：30')
        usage = wait_snapshot(page, fixture.uid, lambda raw: raw.get('usage', {}).get('tokens', {}).get('total_tokens') == 160)['raw']['usage']
        assert usage['tokens'] == {'input': 100, 'output': 20, 'reasoning': 5, 'cache': {'read': 30, 'write': 10}, 'total_tokens': 160}, usage
        assert usage['cost_usd'] == .01 and usage['context']['used_tokens'] == 1200, usage
        assert usage['context']['max_tokens'] == 200000 and usage['context']['percent'] == .6, usage
        capture(page, 'claude-sdk-wide-light-usage.png')
        # Multiple assistant blocks and a repeated HTTP acknowledgement never
        # add the cumulative native modelUsage a second time.
        page.locator('#sdk-usage-toggle').click()
        page.locator('#sdk-more').click()
        page.locator('#native-sdk-permission_mode').select_option('plan')
        wait_snapshot(page, fixture.uid, lambda raw: raw.get('settings', {}).get('permission_mode') == 'plan')
        expect(page.locator('#native-sdk-permission_mode')).to_have_value('plan')
        page.locator('#native-sdk-fast_mode').select_option('true')
        wait_snapshot(page, fixture.uid, lambda raw: raw.get('settings', {}).get('fast_mode') is True)
        expect(page.locator('#native-sdk-fast_mode')).to_have_value('true')
        configured = dict(settings[-1])
        before = len([f for f in fixture.frames('control_request') if f['request']['subtype'] == 'apply_flag_settings'])
        post(page, 'settings', configured)
        assert len([f for f in fixture.frames('control_request') if f['request']['subtype'] == 'apply_flag_settings']) == before
        capture(page, 'claude-sdk-wide-light-settings.png')
        page.locator('#sdk-more').click()

        send(page, "hold")
        expect(page.locator(".sdk-live")).to_contain_text("streaming Claude progress")
        wait_snapshot(page, fixture.uid, lambda raw: raw["running"])
        accepted = dict(turns[-1])
        post(page, "turn", accepted)
        # Browser and individual SSE consumers can leave while work continues.
        context.close()
        context = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme='dark', service_workers="block")
        page = context.new_page()
        page.on('pageerror', lambda error: errors.append(str(error)))
        compacts = []
        page.on('request', lambda request: compacts.append(request.post_data_json)
                if request.method == 'POST' and '/api/native-sdk/compact' in request.url else None)
        page.goto(base, wait_until="networkidle")
        open_sdk(page, fixture, expect)
        assert snapshot(page, fixture.uid)["raw"]["running"] is True
        assert len(fixture.frames("spawn")) == 1, "reload launched another independent runtime"
        assert len([frame for frame in fixture.frames("user") if frame["message"]["content"] == "hold"]) == 1
        page.locator("#sdk-stop").click()
        expect(page.locator(".sdk-live")).to_contain_text("Claude interrupted")
        wait_snapshot(page, fixture.uid, lambda raw: not raw["running"])
        assert any(frame["request"]["subtype"] == "interrupt" for frame in fixture.frames("control_request"))

        send(page, "approval")
        request = wait_snapshot(page, fixture.uid, lambda raw: bool(raw["requests"]))["raw"]["requests"][0]
        assert request["type"] == "approval" and request["input"]["command"] == "printf synthetic"
        page.reload(wait_until="networkidle")
        open_sdk(page, fixture, expect)
        assert snapshot(page, fixture.uid)["raw"]["requests"][0]["key"] == request["key"]
        # Native request answers use exact keys and explicit decisions; generic
        # UI rendering is owned by the primary reviewer's shared browser suite.
        post(page, "respond", {"uid": fixture.uid, "key": request["key"], "result": {"behavior": "deny"}})
        expect(page.locator(".sdk-live")).to_contain_text("Claude synthetic final approval")
        denied = wait_snapshot(page, fixture.uid, lambda raw: not raw["running"])
        tools = [row for row in denied["raw"]["items"] if row["role"] == "tool"]
        assert any(row.get("result", {}).get("error") is True and "denied" in row["result"]["text"] for row in tools), tools

        send(page, "question")
        request = wait_snapshot(page, fixture.uid, lambda raw: bool(raw["requests"]))["raw"]["requests"][0]
        assert request["type"] == "question" and request["questions"][0]["multiSelect"] is False
        post(page, "respond", {"uid": fixture.uid, "key": request["key"], "result": {"answers": {"Choose a synthetic answer?": "Beta"}}})
        expect(page.locator(".sdk-live")).to_contain_text("Claude synthetic final question")
        wait_snapshot(page, fixture.uid, lambda raw: not raw["running"])
        assert any("Beta" in json.dumps(frame) for frame in fixture.frames("control_response"))

        send(page, 'elicitation')
        form = page.locator('.sdk-request').filter(has_text='Synthetic MCP form')
        expect(form).to_be_visible()
        form.get_by_label('Synthetic choice', exact=True).select_option('Beta')
        expect(form.get_by_label('Synthetic enabled', exact=True)).to_be_checked()
        form.get_by_label('Synthetic enabled', exact=True).uncheck()
        form.get_by_label('Synthetic count', exact=True).fill('3')
        capture(page, 'claude-sdk-mobile-dark-elicitation.png')
        form.get_by_role('button', name='提交', exact=True).click()
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final elicitation')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])
        elicited = [f['response'].get('response') for f in fixture.frames('control_response') if f['response'].get('response', {}).get('action') == 'accept']
        assert {'action': 'accept', 'content': {'choice': 'Beta', 'enabled': False, 'count': 3}} in elicited, elicited

        send(page, 'elicitation-url')
        form = page.locator('.sdk-request').filter(has_text='Synthetic MCP form')
        expect(form.get_by_role('link', name='打开授权页面')).to_have_attribute('href', 'https://example.invalid/authorize')
        form.get_by_role('button', name='取消', exact=True).click()
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final elicitation-url')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])

        send(page, 'dialog')
        dialog = page.locator('.sdk-request').filter(has_text='Synthetic fallback choice')
        expect(dialog).to_be_visible()
        dialog.get_by_role('button', name='修改输入', exact=True).click()
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final dialog')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])
        assert any(f['response'].get('response') == {'behavior': 'completed', 'result': 'edit_prompt'} for f in fixture.frames('control_response'))

        page.locator('#sdk-add').click()
        with page.expect_file_chooser() as chooser:
            page.locator('#sdk-attach-menu [data-attach=image]').click()
        image = base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=')
        chooser.value.set_files({'name': 'synthetic-image.png', 'mimeType': 'image/png', 'buffer': image})
        expect(page.locator('#sdk-compose-items')).to_contain_text('synthetic-image.png')
        send(page, 'image from browser')
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final image from browser')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])
        structured = [f for f in fixture.frames('user') if isinstance(f['message']['content'], list)]
        assert any(block == {'type': 'image', 'source': {'type': 'base64', 'media_type': 'image/png', 'data': base64.b64encode(image).decode()}}
            for f in structured for block in f['message']['content']), structured

        send(page, 'retry')
        wait_snapshot(page, fixture.uid, lambda raw: (raw.get('retry') or {}).get('attempt') == 1)
        expect(page.locator('.sdk-status')).to_contain_text('重试')
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final retry')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])

        send(page, 'rate-limit')
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final rate-limit')
        wait_snapshot(page, fixture.uid, lambda raw: raw.get('usage', {}).get('rate_limit_event', {}).get('status') == 'allowed_warning' and not raw['running'])
        page.locator('#sdk-more').click()
        page.locator('[data-sdk-action=compact]').click()
        compact = wait_snapshot(page, fixture.uid, lambda raw: (raw.get('compact') or {}).get('state') == 'success' and not raw['running'])
        assert compact['raw']['compact']['state'] == 'success'
        wait_snapshot(page, fixture.uid, lambda raw: raw.get('usage', {}).get('context', {}).get('used_tokens') == 200)
        assert len([f for f in fixture.frames('user') if f['message']['content'] == '/compact']) == 1
        post(page, 'compact', compacts[-1])
        assert len([f for f in fixture.frames('user') if f['message']['content'] == '/compact']) == 1, 'retried compact ran twice'

        send(page, 'usage-unavailable')
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final usage-unavailable')
        page.locator('#sdk-usage-toggle').click()
        page.locator('#native-usage-refresh').click()
        degraded = wait_snapshot(page, fixture.uid, lambda raw: bool(raw.get('usage_errors')) and not raw['running'])['raw']
        assert degraded['usage']['context']['used_tokens'] == 200 and not degraded['error'], degraded
        expect(page.locator('#native-usage-detail')).to_contain_text('synthetic experimental usage unavailable')
        capture(page, 'claude-sdk-mobile-dark-usage-unavailable.png')
        page.locator('#sdk-usage-toggle').click()
        send(page, "error")
        error = wait_snapshot(page, fixture.uid, lambda raw: bool(raw.get("error")))
        assert "synthetic native execution failure" in error["raw"]["error"]
        assert error['raw']['usage']['tokens'] == degraded['usage']['tokens'], error['raw']['usage']
        assert error['raw']['usage']['cost_usd'] == degraded['usage']['cost_usd'], error['raw']['usage']

        # Creation has a native UUID and real path-derived UID, but no placeholder.
        created = receipt(post(page, "create", {"source": "claude", "request_id": "create-one", "cwd": str(fixture.cwd),
            "model": "fake-claude", "effort": "low"}))
        path = fixture.projects / project_key(fixture.cwd) / (created["session_id"] + ".jsonl")
        assert created["uid"] == native_uid(path) and not path.exists(), created
        again = receipt(post(page, "create", {"source": "claude", "request_id": "create-one", "cwd": str(fixture.cwd),
            "model": "fake-claude", "effort": "low"}))
        assert again["uid"] == created["uid"]
        post(page, "open", {"uid": created["uid"]})
        assert not path.exists(), "open/initialize wrote a fake native history"
        post(page, "turn", {"uid": created["uid"], "request_id": "new-first-turn", "text": "created in SDK"})
        wait_snapshot(page, created["uid"], lambda raw: not raw["running"] and bool(raw["items"]))
        assert path.is_file()
        assert json.loads(path.read_text().splitlines()[0])["sessionId"] == created["session_id"]

        # Simulate an independently running TUI in the private proc tree. It is
        # observed and rejected, with no signals or replacement process.
        independent = fixture.root / "proc/424242"
        independent.mkdir()
        (independent / "cmdline").write_bytes(("claude\0--resume\0" + created["session_id"] + "\0").encode())
        (independent / "stat").write_text("424242 (claude) S 1 " + " ".join(["0"] * 17) + " 100\n")
        conflict = post(page, "turn", {"uid": created["uid"], "request_id": "busy", "text": "must not send"}, ok=False)
        assert "busy" in json.dumps(conflict) or "independent" in json.dumps(conflict), conflict
        assert not any(frame["message"]["content"] == "must not send" for frame in fixture.frames("user"))
        assert (independent / "stat").is_file(), "independent writer was modified"

        capture(page, 'claude-sdk-mobile.png')
        assert not errors, errors
        assert not list((fixture.root / "host").glob("*.session")), "SDK path started a PTY"
        context.close()
        browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-claude-sdk-") as temporary:
        fixture = ClaudeFixture(Path(temporary))
        with fixture.server(args.binary) as base:
            browser_check(fixture, base, args.screenshots)
    print("PASS Claude native SDK: browser send/stop, cumulative usage/context, settings, native compact, structured image, elicitation/dialog, retry/limits/degraded usage, reconnect/deduplication/create/writer refusal")


if __name__ == "__main__":
    main()
