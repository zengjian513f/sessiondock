#!/usr/bin/env python3
"""Claude native SDK browser contract, using a private fake CLI only.

Requires already built SessionDock, Hub and ptyhost binaries. This suite never
builds Rust, calls a real provider or reads a real CLI home. Mode restoration
uses a private PTY running a fixed synthetic shell loop.
"""
from __future__ import annotations

import argparse
from sdk_usage_fixtures import assert_inline_usage, toggle_usage_details
import base64
import json
from pathlib import Path
import re
import tempfile
import time

from claude_sdk_fixtures import ClaudeFixture, MonotonicClock, managed_console, native_uid, project_key
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


def open_sdk(page, fixture, expect, uid=None):
    uid = uid or fixture.uid
    card = page.locator(f'#side .item[data-uid="{uid}"]')
    if not card.is_visible() and page.locator('#detail .mobile-back').is_visible():
        page.locator('#detail .mobile-back').click()
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator("#a-sdk")).to_be_visible()
    page.locator("#a-sdk").click()
    expect(page.locator("#sdk-input")).to_be_visible()
    expect(page.locator("#sdk-send")).to_be_enabled()
    page.wait_for_function("uid => NativeSdkView.connected && NativeSdkView.uid === uid",arg=uid)


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
        toggle_usage_details(page)
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
        toggle_usage_details(page)
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
        form = page.locator('[data-sdk-request]').filter(has_text='Synthetic MCP form')
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
        form = page.locator('[data-sdk-request]').filter(has_text='Synthetic MCP form')
        expect(form.get_by_role('link', name='打开授权页面')).to_have_attribute('href', 'https://example.invalid/authorize')
        form.get_by_role('button', name='取消', exact=True).click()
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final elicitation-url')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])

        send(page, 'dialog')
        dialog = page.locator('[data-sdk-request]').filter(has_text='Synthetic fallback choice')
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
        prompt = fixture.frames('user')[-1]['message']['content']
        assert isinstance(prompt, str), prompt
        assert 'synthetic-image.png' in prompt and 'sessiondock_attachments' in prompt, prompt
        assert list(fixture.cwd.rglob('synthetic-image.png'))

        send(page, 'retry')
        wait_snapshot(page, fixture.uid, lambda raw: (raw.get('retry') or {}).get('attempt') == 1)
        expect(page.locator('.sdk-pane #sdk-status-menu')).to_contain_text('重试')
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
        toggle_usage_details(page)
        page.locator('#native-usage-refresh').click()
        degraded = wait_snapshot(page, fixture.uid, lambda raw: bool(raw.get('usage_errors')) and not raw['running'])['raw']
        assert degraded['usage']['context']['used_tokens'] == 200 and not degraded['error'], degraded
        expect(page.locator('#native-usage-detail')).to_contain_text('synthetic experimental usage unavailable')
        capture(page, 'claude-sdk-mobile-dark-usage-unavailable.png')
        toggle_usage_details(page)
        send(page, "error")
        error = wait_snapshot(page, fixture.uid, lambda raw: bool(raw.get("error")))
        assert "synthetic native execution failure" in error["raw"]["error"]
        assert error['raw']['usage']['tokens'] == degraded['usage']['tokens'], error['raw']['usage']
        assert error['raw']['usage']['cost_usd'] == degraded['usage']['cost_usd'], error['raw']['usage']
        assert error['raw']['error_kind'] == 'turn'

        # Forking is a native lazy resume, frozen at a real chain UUID. The
        # provider writes the branch on its first actual browser input.
        source_text = fixture.path.read_text()
        source_rows = [json.loads(line) for line in source_text.splitlines()]
        page.locator('#sdk-more').click()
        with page.expect_response(lambda response: '/api/native-sdk/fork' in response.url) as forked:
            page.locator('[data-sdk-action=fork]').click()
        full = receipt(forked.value.json())
        full_request = forked.value.request.post_data_json
        full_path = fixture.path.with_name(full['session_id'] + '.jsonl')
        assert full['uid'] != fixture.uid and not full_path.exists(), full
        assert full['session']['sdk_fork']['resume_at'] == source_rows[-1]['uuid'], full
        page.wait_for_function('uid => NativeSdkView.connected && NativeSdkView.uid === uid', arg=full['uid'])
        send(page, 'browser full native fork')
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final browser full native fork')
        wait_snapshot(page, full['uid'], lambda raw: not raw['running'])
        assert fixture.path.read_text() == source_text
        full_rows = [json.loads(line) for line in full_path.read_text().splitlines()]
        assert all(row['sessionId'] == full['session_id'] for row in full_rows)
        assert [row['uuid'] for row in full_rows[:len(source_rows)]] == [row['uuid'] for row in source_rows]
        assert receipt(post(page, 'fork', full_request))['uid'] == full['uid']
        expect(page.locator('.sdk-history')).to_contain_text('earlier synthetic Claude answer')
        capture(page, 'claude-sdk-mobile-dark-full-fork.png')

        open_sdk(page, fixture, expect)
        first_user = next(row for row in source_rows if row.get('message', {}).get('content') == 'hello from browser')
        bubble = page.locator(f'.sdk-live [data-sdk-item="{first_user["uuid"]}"]')
        bubble.locator('.sdk-bubble-toggle').click()
        with page.expect_response(lambda response: '/api/native-sdk/fork' in response.url) as forked:
            bubble.get_by_role('button', name='从此处之前分支', exact=True).click()
        partial = receipt(forked.value.json())
        assert partial['session']['sdk_fork']['resume_at'] == first_user['parentUuid'], partial
        page.wait_for_function('uid => NativeSdkView.connected && NativeSdkView.uid === uid', arg=partial['uid'])
        send(page, 'browser native fork before message')
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final browser native fork before message')
        wait_snapshot(page, partial['uid'], lambda raw: not raw['running'])
        partial_rows = [json.loads(line) for line in fixture.path.with_name(partial['session_id'] + '.jsonl').read_text().splitlines()]
        assert any(row.get('uuid') == 'seed-u' for row in partial_rows)
        assert not any(row.get('uuid') == first_user['uuid'] for row in partial_rows)
        assert fixture.path.read_text() == source_text
        assert len([row for row in fixture.frames('spawn') if '--fork-session' in row['args']]) == 2
        open_sdk(page, fixture, expect)

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
        page.reload(wait_until='networkidle')
        open_sdk(page, fixture, expect, created['uid'])
        first = next(row for row in snapshot(page, created['uid'])['raw']['items'] if row['role'] == 'user')
        bubble = page.locator(f'.sdk-live [data-sdk-item="{first["id"]}"]')
        bubble.locator('.sdk-bubble-toggle').click()
        with page.expect_response(lambda response: '/api/native-sdk/fork' in response.url) as forked:
            bubble.get_by_role('button', name='从此处之前分支', exact=True).click()
        empty = receipt(forked.value.json())
        assert empty['session']['sdk_fork']['empty'] is True, empty
        page.wait_for_function('uid => NativeSdkView.connected && NativeSdkView.uid === uid', arg=empty['uid'])
        send(page, 'browser native fork before first message')
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final browser native fork before first message')
        wait_snapshot(page, empty['uid'], lambda raw: not raw['running'])
        empty_rows = [json.loads(line) for line in path.with_name(empty['session_id'] + '.jsonl').read_text().splitlines()]
        assert not any(row.get('message', {}).get('content') == 'created in SDK' for row in empty_rows)

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


def idle_check(fixture, base, clock):
    from playwright.sync_api import sync_playwright, expect
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True, args=['--no-sandbox'])
        control = browser.new_context(service_workers='block')

        def wake():
            assert control.request.get(base + '/api/health').ok
            time.sleep(.1)

        def open_page():
            context = browser.new_context(viewport={'width':1280, 'height':900}, service_workers='block')
            page = context.new_page()
            page.goto(base, wait_until='networkidle')
            open_sdk(page, fixture, expect)
            return context, page

        def loaded():
            return fixture.uid in control.request.get(base + '/api/live').json()['sdk_uids']

        context, page = open_page()
        page.locator('#sdk-more').click()
        page.locator('#native-sdk-permission_mode').select_option('plan')
        expect(page.locator('#native-sdk-permission_mode')).to_have_value('plan')
        page.locator('#native-sdk-fast_mode').select_option('true')
        expect(page.locator('#native-sdk-fast_mode')).to_have_value('true')
        page.locator('#sdk-more').click()
        send(page, 'hold')
        wait_snapshot(page, fixture.uid, lambda raw: raw['running'])
        first_pid = fixture.frames('spawn')[-1]['pid']
        context.close()
        clock.advance(650)
        wake()
        assert loaded(), 'active native turn was reaped without a subscriber'
        context, page = open_page()
        page.locator('#sdk-stop').click()
        expect(page.locator('.sdk-live')).to_contain_text('Claude interrupted')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])

        send(page, 'approval')
        expect(page.locator('[data-sdk-request]')).to_be_visible()
        context.close()
        clock.advance(650)
        wake()
        assert loaded(), 'pending native approval was reaped'
        context, page = open_page()
        page.locator('[data-sdk-request]').get_by_role('button', name='拒绝', exact=True).click()
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final approval')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])

        send(page, 'background')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'] and bool(raw['background_tasks']))
        context.close()
        clock.advance(650)
        wake()
        assert loaded(), 'native background task was reaped after the main result'
        context, page = open_page()
        send(page, 'finish-background')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'] and not raw['background_tasks'])
        clock.advance(650)
        wake()
        assert loaded(), 'observed native runtime was reaped'
        assert len(fixture.frames('spawn')) == 1
        context.close()

        # The first tick begins the subscriber-free idle period. The second
        # jump crosses the unchanged production ten-minute deadline.
        clock.advance(31)
        wake()
        clock.advance(601)
        deadline = time.monotonic() + 8
        while loaded() and time.monotonic() < deadline:
            wake()
        assert not loaded(), 'inactive native runtime was not reclaimed after ten minutes'
        context, page = open_page()
        assert len(fixture.frames('spawn')) == 2
        resumed = fixture.frames('spawn')[-1]
        assert resumed['pid'] != first_pid and resumed['sid'] == fixture.sid
        assert '--resume=' + str(fixture.path) in resumed['args']
        send(page, 'browser resumed after idle')
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final browser resumed after idle')
        resumed = wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])['raw']
        assert resumed['settings']['permission_mode'] == 'plan' and resumed['settings']['fast_mode_state'] == 'on', resumed['settings']
        context.close()
        control.close()
        browser.close()


def fork_restart_check(fixture, binary):
    from playwright.sync_api import sync_playwright, expect
    failure = fixture.root / 'fork-initialize-fail-once'
    failure.write_text('synthetic transport failure')
    launcher = json.loads(fixture.launcher.read_text())
    launcher['profiles'][0]['env']['SESSIONDOCK_FAKE_FORK_FAIL_ONCE'] = str(failure)
    fixture.launcher.write_text(json.dumps(launcher))
    original = fixture.path.read_text()
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True, args=['--no-sandbox'])
        with fixture.server(binary) as base:
            context = browser.new_context(service_workers='block')
            page = context.new_page()
            page.goto(base, wait_until='networkidle')
            open_sdk(page, fixture, expect)
            page.locator('#sdk-more').click()
            with page.expect_response(lambda response: '/api/native-sdk/fork' in response.url) as failed:
                page.locator('[data-sdk-action=fork]').click()
            assert not failed.value.ok
            count = len(fixture.frames('spawn'))
            retried = post(page, 'fork', failed.value.request.post_data_json, ok=False)
            assert 'already attempted' in json.dumps(retried), retried
            assert len(fixture.frames('spawn')) == count, 'unknown native fork was dispatched twice'
            expect(page.locator('.sdk-pane #sdk-status-menu')).to_contain_text('操作失败')
            page.locator('#sdk-more').click()
            with page.expect_response(lambda response: '/api/native-sdk/fork' in response.url) as forked:
                page.locator('[data-sdk-action=fork]').click()
            assert forked.value.ok, forked.value.text()
            created = receipt(forked.value.json())
            request = forked.value.request.post_data_json
            path = fixture.path.with_name(created['session_id'] + '.jsonl')
            page.wait_for_function('uid => NativeSdkView.connected && NativeSdkView.uid === uid', arg=created['uid'])
            assert not path.exists(), 'pending fork inserted a placeholder transcript'
            context.close()
        with fixture.server(binary) as base:
            context = browser.new_context(service_workers='block')
            page = context.new_page()
            page.goto(base, wait_until='networkidle')
            open_sdk(page, fixture, expect, created['uid'])
            assert not path.exists()
            before = len(fixture.frames('spawn'))
            assert receipt(post(page, 'fork', request))['uid'] == created['uid']
            assert len(fixture.frames('spawn')) == before, 'persisted fork receipt created another child'
            resumed = fixture.frames('spawn')[-1]
            assert resumed['sid'] == created['session_id'] and '--fork-session' in resumed['args']
            assert '--resume=' + str(fixture.path) in resumed['args']
            send(page, 'browser pending fork restored after restart')
            expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final browser pending fork restored after restart')
            wait_snapshot(page, created['uid'], lambda raw: not raw['running'])
            rows = [json.loads(line) for line in path.read_text().splitlines()]
            assert any(row.get('uuid') == 'seed-u' for row in rows)
            assert fixture.path.read_text() == original
            context.close()
        browser.close()


def startup_hooks_check(fixture, binary, foreign_kind):
    from playwright.sync_api import sync_playwright, expect
    launcher = json.loads(fixture.launcher.read_text())
    launcher['profiles'][0]['env']['SESSIONDOCK_FAKE_STARTUP_HOOKS'] = '1'
    fixture.launcher.write_text(json.dumps(launcher))
    with fixture.server(binary) as base, sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True, args=['--no-sandbox'])
        page = browser.new_page(service_workers='block')
        page.goto(base, wait_until='networkidle')
        open_sdk(page, fixture, expect)
        assert '--resume=' + str(fixture.path) in fixture.frames('spawn')[-1]['args']
        send(page, 'browser startup hook resume')
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final browser startup hook resume')
        wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])
        assert 'startup-hook-must-not-project' not in page.locator('.sdk-pane').inner_text()
        send(page, 'foreign-identity-' + foreign_kind)
        expect(page.locator('.sdk-pane #sdk-status-menu')).to_contain_text('native stream session_id differs')
        raw = wait_snapshot(page, fixture.uid, lambda raw: raw.get('error_kind') == 'transport')['raw']
        assert raw['session_id'] == fixture.sid
        assert not any('FOREIGN_NATIVE_CONTENT' in json.dumps(row) for row in raw['items'])
        assert not raw['requests'], 'foreign approval crossed the native SID guard'
        expect(page.locator('[data-sdk-request]')).to_have_count(0)
        browser.close()


def startup_failure_check(fixture, binary, screenshots=None):
    from playwright.sync_api import sync_playwright, expect
    launcher = json.loads(fixture.launcher.read_text())
    launcher['profiles'][0]['env']['SESSIONDOCK_FAKE_STARTUP_FAILURE'] = '1'
    fixture.launcher.write_text(json.dumps(launcher))
    with fixture.server(binary) as base, sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True, args=['--no-sandbox'])
        page = browser.new_page(service_workers='block')
        page.goto(base, wait_until='networkidle')
        page.locator(f'#side .item[data-uid="{fixture.uid}"]').click()
        page.locator('#a-sdk').click()
        expect(page.locator('.sdk-pane #sdk-status-menu')).to_contain_text('command not found')
        expect(page.locator('.sdk-pane #sdk-status-menu')).to_contain_text('[redacted]')
        assert 'synthetic-startup-secret' not in page.locator('.sdk-pane .sdk-status').inner_text()
        assert not page.evaluate('NativeSdkView.connected')
        assert not fixture.frames('user'), 'startup failure submitted a model prompt'
        if screenshots:
            screenshots.mkdir(parents=True, exist_ok=True)
            page.screenshot(path=str(screenshots / 'claude-sdk-startup-error.png'), full_page=True)
        browser.close()


def view_mode_check(fixture, base, screenshots, label):
    """Restore actual user choices without opening a competing SDK runtime."""
    from playwright.sync_api import sync_playwright, expect
    other_sid = '7335eb0f-98c5-421b-b72a-d1abae633f80'
    other_path = fixture.path.with_name(other_sid + '.jsonl')
    other_path.write_text(fixture.path.read_text().replace(fixture.sid, other_sid)
                          .replace('earlier synthetic Claude prompt', '另一条 Claude SDK 会话 · independent preference'))
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for width, theme in [(1280, 'light'), (390, 'dark'), (1280, 'dark'), (390, 'light')]:
                print(f'Claude view restore: {label} {width} {theme}', flush=True)
                context = browser.new_context(viewport={'width': width, 'height': 900},
                                              color_scheme=theme, service_workers='block')
                page = context.new_page()
                errors, opens = [], []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.on('request', lambda request: opens.append(request.post_data_json)
                        if request.method == 'POST' and '/api/native-sdk/open' in request.url else None)
                # Keep console discovery pending until the user has opened the
                # row: a slow first list must not trigger SDK auto-connection.
                pending_lists = []
                page.route('**/api/term/list*', lambda route: pending_lists.append(route))
                page.goto(base, wait_until='domcontentloaded')
                row = page.locator('#side .item[data-uid^="claude:"]').filter(has_text='earlier synthetic Claude prompt')
                row.click()
                expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
                assert not opens, opens
                for route in pending_lists:
                    route.continue_()
                page.unroute('**/api/term/list*')
                expect(page.locator('#a-term')).to_have_attribute('aria-pressed', 'true')
                page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                uid = page.evaluate('S.sel')
                assert ('~' in uid) == (label == 'hub'), uid
                assert not opens, opens

                def capture(mode):
                    if screenshots:
                        screenshots.mkdir(parents=True, exist_ok=True)
                        page.screenshot(path=str(screenshots / f'claude-view-{label}-{width}-{theme}-{mode}.png'))

                def choose(target):
                    if page.locator('#detail .mobile-back').is_visible():
                        page.locator('#detail .mobile-back').click()
                    page.locator(f'#side .item[data-uid="{target}"]').click()

                # Explicit terminal choice survives refresh without SDK open.
                page.locator('#a-term').click()
                page.reload(wait_until='networkidle')
                expect(page.locator('#a-term')).to_have_attribute('aria-pressed', 'true')
                capture('terminal')
                assert not opens, opens
                page.locator('#a-chat').click()
                page.reload(wait_until='networkidle')
                expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
                expect(page.locator('#msgs')).to_contain_text('earlier synthetic Claude answer')
                capture('chat')
                assert not opens, opens

                if page.locator('#detail .mobile-back').is_visible():
                    page.locator('#detail .mobile-back').click()
                page.locator('#side .item[data-uid^="claude:"]').filter(has_text='另一条 Claude SDK 会话').click()
                expect(page.locator('#sdk-input')).to_be_visible()
                other_uid = page.evaluate('S.sel')
                page.wait_for_function('NativeSdkView.connected')
                page.reload(wait_until='networkidle')
                expect(page.locator('#sdk-input')).to_be_enabled()
                capture('sdk')
                choose(uid)
                expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
                page.locator('#a-term').click()
                expect(page.locator('#a-term')).to_have_attribute('aria-pressed', 'true')
                choose(other_uid)
                expect(page.locator('#sdk-input')).to_be_visible()
                choose(uid)
                expect(page.locator('#a-term')).to_have_attribute('aria-pressed', 'true')
                assert not [body for body in opens if body['uid'] == uid], opens
                assert not errors, errors
                context.close()
        finally:
            browser.close()


def view_modes(binary, screenshots):
    from hub_fixtures import Hub, free_port
    from node_auth_fixtures import TOKEN, node_env
    from types import SimpleNamespace
    for label in ('node', 'hub'):
        with tempfile.TemporaryDirectory(prefix='sessiondock-claude-views-') as temporary:
            root = Path(temporary)
            fixture = ClaudeFixture(root)
            with managed_console(fixture):
                port = free_port()
                with fixture.server(binary, node_env(root, port, '127.0.0.0/8')) as base:
                    if label == 'node':
                        view_mode_check(fixture, base, screenshots, label)
                    else:
                        hub_root = root / 'hub'; hub_root.mkdir()
                        hub = Hub(binary.resolve().with_name('sessiondock-hub'), hub_root,
                                  [SimpleNamespace(name='ClaudeViews', token=TOKEN, port=port)])
                        hub.start()
                        try:
                            view_mode_check(fixture, f'http://127.0.0.1:{hub.port}', screenshots, label)
                        finally:
                            hub.stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    view_modes(args.binary, args.screenshots)
    for kind in ('user', 'approval'):
        with tempfile.TemporaryDirectory(prefix='sessiondock-claude-sdk-startup-hooks-') as temporary:
            startup_hooks_check(ClaudeFixture(Path(temporary)), args.binary, kind)
    with tempfile.TemporaryDirectory(prefix='sessiondock-claude-sdk-startup-') as temporary:
        startup_failure_check(ClaudeFixture(Path(temporary)), args.binary, args.screenshots)
    with tempfile.TemporaryDirectory(prefix="sessiondock-claude-sdk-") as temporary:
        fixture = ClaudeFixture(Path(temporary))
        with fixture.server(args.binary) as base:
            browser_check(fixture, base, args.screenshots)
    with tempfile.TemporaryDirectory(prefix='sessiondock-claude-sdk-idle-') as temporary:
        fixture = ClaudeFixture(Path(temporary))
        clock = MonotonicClock(Path(temporary))
        with fixture.server(args.binary, extra_env=clock.env) as base:
            idle_check(fixture, base, clock)
    with tempfile.TemporaryDirectory(prefix='sessiondock-claude-sdk-fork-restart-') as temporary:
        fork_restart_check(ClaudeFixture(Path(temporary)), args.binary)
    print("PASS Claude native SDK: node/Hub view restoration, browser send/stop, usage/settings/compact/image/requests, native fork boundaries/source preservation/deduplication, ten-minute idle reclaim with active/request/task/subscriber guards and same-SID resume")


if __name__ == "__main__":
    main()
