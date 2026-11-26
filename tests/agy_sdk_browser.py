#!/usr/bin/env python3
"""Native Agy SDK through generic SessionDock HTTP/UI and a private fake CLI.

Requires the reviewer's integrated native-sdk routes/UI and a prebuilt binary.
No build, real CLI, credential, model, production home, or PTY is used here.
The test clicks SDK/SEND/STOP, types real input, reloads during accepted work,
and checks the native fixture's independently recorded input/argv/signal log.
"""
from __future__ import annotations

import argparse
from contextlib import ExitStack
from sdkhost_fixtures import stop_private_sdkhosts
import re
import base64
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
from unittest.mock import patch

sys.dont_write_bytecode = True
from playwright.sync_api import expect, sync_playwright
from sdk_usage_fixtures import assert_inline_usage, assert_usage_outside_menu, close_more, refresh_usage
from agy_sdk_fixtures import AgyFixture, MODEL, SID, TITLE
from history_fixtures import BINARY, REPO, Corpus, claude_row, isolated_server
from new_session_model_browser import open_dialog
from send_browser import initialize
from media_browser import PNG


WATCH = r'''uid => {
  window.__agyEvents?.close();
  window.__agySnapshots = [];
  window.__agyEvents = new EventSource('/api/native-sdk/events?uid='+encodeURIComponent(uid));
  window.__agyEvents.addEventListener('sdk', event => {
    const snapshot=JSON.parse(event.data);
    if(snapshot.type==='snapshot') {
      window.__agySnapshots.push(snapshot);
      window.__agySnapshot=snapshot;
    }
  });
}'''


def watch(page, uid):
    page.evaluate(WATCH, uid)
    page.wait_for_function('window.__agySnapshots.length > 0')


def idle(page):
    page.wait_for_function('''window.__agySnapshot?.raw?.running === false &&
        NativeSdkView.raw?.running === false &&
        !(NativeSdkView.raw.input_queue || []).some(row =>
            ['preparing', 'queued', 'sending', 'accepted'].includes(row.state))''')


def post(page, base, method, body, *, ok=True):
    response = page.request.post(base + '/api/native-sdk/' + method, data=body)
    assert response.ok == ok, (method, response.status, response.text())
    return response.json()


def select_sdk(page, uid=None, connected=True):
    row = page.locator('#side .item[data-uid]').filter(has_text=TITLE).first if uid is None else page.locator(f'#side .item[data-uid="{uid}"]')
    expect(row).to_be_visible()
    uid = row.get_attribute('data-uid')
    row.click()
    expect(page.locator('#a-sdk')).to_be_visible()
    page.locator('#a-sdk').click()
    expect(page.locator('#sdk-input')).to_be_visible()
    page.locator('#sdk-input').click()
    if connected:
        from claude_sdk_browser import start_sdk_if_needed
        start_sdk_if_needed(page)
        page.wait_for_function('NativeSdkView.connected')
    return uid


def send(page, text):
    page.locator('#sdk-input').fill(text)
    with page.expect_response(lambda response: '/api/native-sdk/turn' in response.url and response.request.method == 'POST') as event:
        page.locator('#sdk-send').click()
    assert event.value.ok, event.value.text()
    receipt = event.value.json()
    page.wait_for_function('''id =>
        (NativeSdkView.raw?.input_queue || []).some(row => row.request_id === id) ||
        (NativeSdkView.raw?.items || []).some(item => item.request_id === id)''',
        arg=receipt['request_id'])
    return receipt


def resize_sdk(page, uid, width):
    page.set_viewport_size({'width':width,'height':900})
    # Let the responsive media-query handler finish before deciding whether
    # the mobile list needs an actual session-row click.
    page.evaluate('() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
    if width <= 720 and not page.locator('.native-sdk-pane').is_visible():
        page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator('.native-sdk-pane')).to_be_visible()


def check_startup_interrupt(page, fixture, uid, screenshots=None, *, reject_first=False):
    watch(page, uid)
    cancel_count = sum(event['kind'] == 'cancel' for event in fixture.events())
    start = 'hold:interrupt-before-input'
    follow_up = 'after native interrupt'
    mode = fixture.root / 'interrupt-mode'
    if reject_first:
        mode.write_text('reject')
    send(page, start)
    stop = page.locator('#sdk-stop')
    notice = page.locator('#sdk-composer .sdk-status')
    with page.expect_response('**/api/native-sdk/interrupt') as stopped:
        stop.click()
    receipt = stopped.value.json()
    assert stopped.value.ok and receipt['pending'] and not receipt['interrupted'], receipt
    expect(notice).to_have_text('等待接收输入')
    expect(stop).to_be_disabled()
    assert page.evaluate('NativeSdkView.raw.cancel_requested === false')
    assert sum(event['kind'] == 'cancel' for event in fixture.events()) == cancel_count
    if screenshots:
        expect(notice).to_be_enabled()
        notice.press('Enter')
        details = page.locator('#sdk-status-menu')
        expect(details).to_be_visible()
        expect(details).to_contain_text('Agy 接收本轮输入后，SessionDock 会发送停止请求。')
        page.screenshot(path=str(screenshots / 'agy-interrupt-start.png'), full_page=True)
        page.keyboard.press('Escape')
        expect(details).to_be_hidden()
    send(page, follow_up)
    assert not fixture.inputs(follow_up)
    fixture.release('interrupt-input-ready')
    if reject_first:
        expect(notice).to_have_text('中断失败')
        expect(stop).to_be_enabled()
        assert not page.evaluate('NativeSdkView.raw.cancel_pending || NativeSdkView.raw.cancel_requested')
        mode.unlink()
        with page.expect_response('**/api/native-sdk/interrupt') as retried:
            stop.click()
        assert retried.value.ok and retried.value.json()['interrupted'], retried.value.text()
        cancel_count += 1
    idle(page)
    expect(page.locator('.native-sdk-pane .msgs')).to_contain_text('Synthetic Agy reply: ' + follow_up)
    assert page.evaluate("window.__agySnapshots.some(({raw}) => raw.last_turn?.status === 'interrupted')")
    assert page.evaluate('NativeSdkView.raw.cancel_pending === false')
    assert sum(event['kind'] == 'cancel' for event in fixture.events()) == cancel_count + 1
    assert len(fixture.inputs(start)) == len(fixture.inputs(follow_up)) == 1
    pid = fixture.inputs(start)[0]['pid']
    assert fixture.inputs(follow_up)[0]['pid'] == pid
    assert any(event['kind'] == 'turn_cancelled' and event['pid'] == pid for event in fixture.events())
    assert not any(event['kind'] == 'signal' and event['pid'] == pid for event in fixture.events())
    return pid


def check_writer_admission(page, fixture, screenshots=None):
    fixture.occupy()
    before = len(fixture.inputs())
    blocked = send(page, 'must not take over')['request_id']
    page.wait_for_function("NativeSdkView.raw.input_queue?.some(row=>row.text==='must not take over' && row.state==='failed')")
    refused = page.evaluate("NativeSdkView.raw.input_queue.find(row=>row.text==='must not take over')")
    assert 'agy_sdk_session_busy' in refused['error'], refused
    assert len(fixture.inputs()) == before
    row = page.locator(f'[data-queue-id="{refused["id"]}"]')
    if screenshots:
        page.screenshot(path=str(screenshots / 'agy-writer-busy.png'), full_page=True)
    fixture.unoccupy()
    row.get_by_role('button', name='撤销', exact=True).click()
    expect(row).to_have_count(0)
    warm = send(page, 'after external writer exits')['request_id']
    expect(page.locator('.sdk-msgs')).to_contain_text('Synthetic Agy reply: after external writer exits')
    idle(page)
    assert len(fixture.inputs('after external writer exits')) == 1
    assert fixture.inputs()[-1]['pid'] == fixture.inputs()[before - 1]['pid']
    if screenshots:
        page.screenshot(path=str(screenshots / 'agy-writer-released.png'), full_page=True)
    return blocked, warm


def exercise_writer_admission(browser, base, fixture, screenshots):
    with browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block') as context:
        context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
        page = context.new_page()
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        uid = select_sdk(page)
        watch(page, uid)
        send(page, 'warm Agy session')
        expect(page.locator('.sdk-msgs')).to_contain_text('Synthetic Agy reply: warm Agy session')
        idle(page)
        return check_writer_admission(page, fixture, screenshots)


def exercise(browser, base, fixture, screenshots=None):
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.on('response', lambda response: print('SDK HTTP error', response.status, response.url, response.text(), flush=True) if '/api/native-sdk/' in response.url and response.status >= 400 else None)
    page.set_default_timeout(15000)
    errors = []
    page.on('pageerror', lambda error: errors.append(error.stack))
    try:
        # A slow native query must not hide the shared persisted quota.
        quota_dir = fixture.root / 'state/quotas'
        quota_dir.mkdir(parents=True, exist_ok=True)
        (quota_dir / 'agy.json').write_text(json.dumps({'source':'agy',
            'captured_at':'2020-10-17T03:02:16Z','buckets':[{'id':'standard',
            'group':'Agy Standard','window':'5h','remaining_percent':40}],'error':None}))
        (fixture.root / 'quota-hold').touch()
        def quota_calls():
            return [event for event in fixture.events() if event['kind']=='launch' and '-p' in event['argv']]
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        uid = select_sdk(page)
        expect(page.locator('.native-sdk-pane .sdk-status-row > .sdk-gauges')).to_contain_text('60%')
        page.wait_for_function('NativeSdkView.loadingUsage')
        # Settings opened concurrently joins the same Agy query.
        quotas = context.new_page()
        quotas.goto(base + '/quotas.html')
        expect(quotas.locator('section[data-source=agy]')).to_contain_text('60%')
        expect(quotas.locator('#quota-refresh')).to_be_disabled()
        if screenshots:
            screenshots.mkdir(parents=True, exist_ok=True)
            page.screenshot(path=str(screenshots / 'agy-sdk-cached-loading.png'), full_page=True)
        (fixture.root / 'quota-hold').unlink()
        expect(page.locator('.native-sdk-pane .sdk-status-row > .sdk-gauges')).to_contain_text('28%')
        expect(quotas.locator('section[data-source=agy]')).to_contain_text('28%')
        expect(quotas.locator('#quota-refresh')).to_be_enabled()
        page.wait_for_function('!NativeSdkView.loadingUsage')
        assert len(quota_calls()) == 1, 'settings and SDK must share the in-flight query'
        quotas.reload()
        expect(quotas.locator('#quota-refresh')).to_be_enabled()
        page.reload()
        expect(page.locator('.native-sdk-pane .sdk-status-row > .sdk-gauges')).to_contain_text('28%')
        page.wait_for_function('!NativeSdkView.loadingUsage')
        assert len(quota_calls()) == 1, 'fresh quota must survive page reloads without a CLI query'
        # Explicit refresh updates the shared cache, including failure cooldown.
        (fixture.root / 'quota-remaining').write_text('0.61')
        refresh_usage(page)
        expect(page.locator('.native-sdk-pane .sdk-status-row > .sdk-gauges')).to_contain_text('39%')
        page.wait_for_function('!NativeSdkView.loadingUsage')
        quotas.reload()
        expect(quotas.locator('section[data-source=agy]')).to_contain_text('39%')
        expect(quotas.locator('#quota-refresh')).to_be_enabled()
        assert len(quota_calls()) == 2
        (fixture.root / 'quota-fail').touch()
        quotas.locator('#quota-refresh').click()
        expect(quotas.locator('section[data-source=agy]')).to_contain_text('查询失败')
        expect(quotas.locator('section[data-source=agy]')).to_contain_text('39%')
        page.reload()
        expect(page.locator('.native-sdk-pane .sdk-status-row > .sdk-gauges')).to_contain_text('39%')
        page.wait_for_function('!NativeSdkView.loadingUsage')
        assert_inline_usage(page)
        expect(page.locator('.native-sdk-pane .sdk-gauges')).to_have_attribute('title',re.compile('查询失败'))
        assert len(quota_calls()) == 3, 'automatic reads must reuse a recent failed attempt'
        (fixture.root / 'quota-fail').unlink()
        (fixture.root / 'quota-remaining').unlink()
        refresh_usage(page)
        expect(page.locator('.native-sdk-pane .sdk-status-row > .sdk-gauges')).to_contain_text('28%')
        page.wait_for_function('!NativeSdkView.loadingUsage')
        page.keyboard.press('Escape')
        quotas.close()
        expect(page.locator('#native-media-note')).to_have_count(0)
        opened = post(page, base, 'open', {'uid': uid})
        assert opened['session_id'] == SID and opened['cwd'] == str(fixture.work)
        assert opened['capabilities']['send'] and opened['capabilities']['models']
        assert opened['capabilities']['respond'] is False
        watch(page, uid)
        models = post(page, base, 'models', {'uid': uid})
        assert MODEL in [row['id'] for row in models['models']], models
        assert not fixture.inputs(), 'open/model discovery must not submit a prompt'
        page.locator('#sdk-model').select_option('synthetic-agy')
        expect(page.locator('#sdk-model')).to_be_enabled()
        expect(page.locator('#sdk-effort')).to_be_visible()
        page.locator('#sdk-effort').select_option('high')
        if not page.locator('#native-sdk-mode').is_visible():page.locator('#sdk-more').click()
        page.locator('#native-sdk-mode').select_option('plan')
        expect(page.locator('#sdk-send')).to_be_enabled()
        assert not fixture.inputs(), 'selection must save settings without submitting a prompt'
        page.wait_for_function('model => NativeSdkView.nativeModel === model && NativeSdkView.nativeEffort === null', arg='synthetic-agy-high')
        page.keyboard.press('Escape')
        print('Agy: click SDK, type/send, sparse tool result, idempotent retry', flush=True)
        receipt = send(page, 'SDK 中文\nsecond line')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: SDK 中文')
        expect(page.locator('.sdk-live')).to_contain_text('synthetic tool output')
        idle(page)
        post(page, base, 'turn', {'uid': uid, 'text': 'SDK 中文\nsecond line', 'request_id': receipt['request_id']})
        assert len(fixture.inputs('SDK 中文\nsecond line')) == 1
        assert fixture.inputs('SDK 中文\nsecond line')[0]['model'] == 'synthetic-agy-high'
        assert fixture.inputs('SDK 中文\nsecond line')[0]['effort'] is None
        assert fixture.inputs('SDK 中文\nsecond line')[0]['mode'] == 'plan'
        footer = page.locator('.native-sdk-pane .sdk-status-row > .sdk-gauges')
        expect(footer).to_have_attribute('title', re.compile('累计 Token：12,800'))
        assert_usage_outside_menu(page)
        # A long native diagnostic stays in the footer description; /usage clears it again.
        def expanded_usage(route):
            response = route.fetch()
            data = response.json()
            payload = data.get('raw', data)
            payload.setdefault('quota', {})['error'] = 'Delayed native quota diagnostic. ' * 80
            route.fulfill(response=response, json=data)
        page.route('**/api/native-sdk/usage', expanded_usage, times=1)
        before = len(fixture.inputs())
        refresh_usage(page)
        expect(footer).to_have_attribute('title', re.compile('72%'))
        expect(footer).to_have_attribute('title', re.compile('Delayed native quota diagnostic.'))
        expect(page.locator('#sdk-more-menu')).to_be_hidden()
        refresh_usage(page)
        expect(footer).not_to_have_attribute('title', re.compile('Delayed native quota diagnostic.'))
        assert len(fixture.inputs()) == before, 'slash query must never reach NDJSON user input'
        page.locator('#sdk-input').fill('/unknown-native-command')
        page.locator('#sdk-send').click()
        expect(page.locator('.native-sdk-pane #sdk-status-menu')).to_contain_text('尚未映射')
        expect(page.locator('#sdk-input')).to_have_value('/unknown-native-command')
        assert len(fixture.inputs()) == before
        page.locator('#sdk-input').fill('')
        snapshot = page.evaluate('window.__agySnapshot')
        tool = next(item for item in snapshot['raw']['items'] if item['role'] == 'tool')
        assert tool['input']['AbsolutePath'] == str(fixture.work / 'synthetic.txt')
        assert tool['output'] == 'synthetic tool output'
        ids = [item['id'] for item in snapshot['raw']['items']]
        assert len(ids) == len(set(ids)), ids

        first_pid=fixture.inputs('SDK 中文\nsecond line')[0]['pid']
        initial_notices=page.locator('.sdk-live').inner_text().count('Native runtime restarted')
        send(page,'__ARTIFACT__');idle(page)
        assert fixture.inputs('__ARTIFACT__')[0]['pid']==first_pid, 'ordinary next turn restarted the native runtime'
        assert page.locator('.sdk-live').inner_text().count('Native runtime restarted')==initial_notices
        document=page.locator('.sdk-live a[data-file-ref^="file://"]').last
        expect(document).to_have_text('SessionDock 全量代码审计报告 (codebase_audit_report.md)')
        if screenshots:
            document.scroll_into_view_if_needed()
            page.screenshot(path=str(screenshots/'agy-artifact-link-wide.png'),full_page=True)
            resize_sdk(page,uid,390)
            page.emulate_media(color_scheme='dark')
            expect(page.locator('.native-sdk-pane')).to_be_visible()
            document.scroll_into_view_if_needed()
            page.screenshot(path=str(screenshots/'agy-artifact-link-narrow.png'),full_page=True)
            resize_sdk(page,uid,1280)
            page.emulate_media(color_scheme='light')
        # The real referenced native transcript must grant this file before
        # the same user-facing link opens its built-in preview.
        page.wait_for_function('''uid=>{
          if(!window.__artifactHistoryPending){
            window.__artifactHistoryPending=true;
            fetch('/api/messages/'+encodeURIComponent(uid)+'?window=1').then(r=>r.json())
              .then(view=>window.__artifactHistoryReady=view.messages.some(m=>(m.text||'').includes('完整报告：')))
              .finally(()=>window.__artifactHistoryPending=false);
          }
          return window.__artifactHistoryReady===true;
        }''',arg=uid,polling=100)
        with context.expect_page() as opened:document.click()
        preview=opened.value
        expect(preview.locator('#file-content')).to_contain_text('完整报告正文：保留运行中的子代理，修复文档链接。')
        if screenshots:preview.screenshot(path=str(screenshots/'agy-artifact-preview.png'),full_page=True)
        preview.close()
        print('Agy: consecutive rounds retain PID/background runtime; file URI artifact opens the actual document',flush=True)
        send(page,'__BACKGROUND__');idle(page)
        before_background=len(fixture.inputs())
        fixture.release('background')
        expect(page.locator('.sdk-live')).to_contain_text('后台子代理已完成审计，运行时未重启。')
        expect(page.locator('.sdk-live')).to_contain_text('回合结束后补齐的原生系统正文。')
        assert len(fixture.inputs())==before_background and fixture.inputs()[-1]['pid']==first_pid

        # Only an explicit setting change rebuilds the native argv/runtime.
        if not page.locator('#sdk-effort').is_visible():page.locator('#sdk-more').click()
        page.locator('#sdk-effort').select_option('low')
        page.wait_for_function('NativeSdkView.nativeModel === "synthetic-agy-low" && NativeSdkView.nativeEffort === null')
        page.keyboard.press('Escape')
        send(page,'after explicit effort change');idle(page)
        changed=fixture.inputs('after explicit effort change')[0]
        assert changed['pid']!=first_pid and changed['model']==MODEL and changed['effort'] is None
        notices=page.locator('.sdk-live').inner_text().count('Native runtime restarted')
        send(page,'same runtime after explicit setting');idle(page)
        assert fixture.inputs('same runtime after explicit setting')[0]['pid']==changed['pid']
        assert page.locator('.sdk-live').inner_text().count('Native runtime restarted')==notices

        print('Agy: metadata-only notifications hydrate native system/thinking/reply text without empty bubbles', flush=True)
        send(page, '__METADATA_ONLY__')
        expect(page.locator('.sdk-live')).to_contain_text('服务重启后，请核对子代理状态再继续审计。')
        expect(page.locator('.sdk-live')).to_contain_text('已接续原会话，正在核对各项审计结果。')
        expect(page.locator('.sdk-live')).to_contain_text('全部子代理状态已核对。')
        expect(page.locator('.sdk-live')).not_to_contain_text('"step_type"')
        expect(page.locator('.sdk-live')).not_to_contain_text('附加信息 · system_message')
        page.wait_for_function("window.__agySnapshot.raw.items.some(item=>item.role==='thinking' && item.content==='核对已有子代理的状态与历史记录。')")
        assert page.evaluate("window.__agySnapshot.raw.running"), 'body must appear before final result'
        assert page.locator('.sdk-live .msg[data-role="assistant"]').evaluate_all('(nodes)=>nodes.every(node=>node.textContent.trim().length>0)')
        if screenshots:
            for width, theme in ((1280,'light'),(390,'light'),(390,'dark')):
                resize_sdk(page,uid,width)
                page.emulate_media(color_scheme=theme)
                expect(page.locator('.native-sdk-pane')).to_be_visible()
                page.screenshot(path=str(screenshots / f'agy-metadata-{width}-{theme}.png'), full_page=True)
            resize_sdk(page,uid,1280)
            page.emulate_media(color_scheme='light')
        page.reload(wait_until='domcontentloaded')
        select_sdk(page, uid);watch(page,uid)
        expect(page.locator('.sdk-live')).to_contain_text('已接续原会话，正在核对各项审计结果。')
        assert len(fixture.inputs('__METADATA_ONLY__')) == 1
        fixture.release('metadata-only');idle(page)
        assert page.locator('.sdk-live .msg[data-role="assistant"]').filter(has_text='已接续原会话，正在核对各项审计结果。').count() == 1

        print('Agy: running request retry, FIFO, detach/reopen without cancellation', flush=True)
        held = send(page, 'hold:detached')
        expect(page.locator('.sdk-live')).to_contain_text('Agy running detached')
        # BUG-20261020-131530-fda8ec: navigation reconnects the browser
        # subscription, not the native runtime. Drop the test observer too so
        # it cannot accidentally keep accepted work alive during the switch.
        detached_pid = fixture.inputs('hold:detached')[0]['pid']
        native_launches = [event for event in fixture.events()
                           if event['kind'] == 'launch' and '--input-format' in event['argv']]
        page.evaluate('''() => {
            window.__agyEvents.close();
            window.__navigationStream = NativeSdkView.es;
        }''')
        page.locator('#sdk-input').fill('未发送的切换草稿')
        other = page.locator(f'#side .item[data-uid]:not([data-uid="{uid}"])').first
        expect(other).to_be_visible()
        other_uid = other.get_attribute('data-uid')
        other.click()
        page.wait_for_function('uids => S.sel === uids.other && SdkView.uid !== uids.agy',
                               arg={'other': other_uid, 'agy': uid})
        assert page.evaluate('window.__navigationStream.readyState === EventSource.CLOSED')
        select_sdk(page, uid)
        page.wait_for_function('NativeSdkView.connected && NativeSdkView.running')
        expect(page.locator('.sdk-live')).to_contain_text('Agy running detached')
        expect(page.locator('#sdk-input')).to_have_value('未发送的切换草稿')
        assert page.evaluate('NativeSdkView.es !== window.__navigationStream')
        assert len(fixture.inputs('hold:detached')) == 1
        assert [event for event in fixture.events()
                if event['kind'] == 'launch' and '--input-format' in event['argv']] == native_launches
        assert not any(event['kind'] == 'signal' for event in fixture.events())
        if screenshots:
            page.screenshot(path=str(screenshots / 'agy-navigation-attached.png'), full_page=True)
        page.locator('#sdk-input').fill('')
        watch(page, uid)
        post(page, base, 'turn', {'uid': uid, 'request_id': held['request_id'], 'text': 'hold:detached'})
        post(page, base, 'turn', {'uid': uid, 'request_id': 'queued-once', 'text': 'queued after detach'})
        # Closing this document also closes every EventSource; the accepted turn must survive.
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        fixture.release('detached')
        select_sdk(page, uid)
        watch(page, uid)
        expect(page.locator('.sdk-live')).to_contain_text('finished detached')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: queued after detach')
        idle(page)
        assert len(fixture.inputs('hold:detached')) == len(fixture.inputs('queued after detach')) == 1
        assert fixture.inputs('queued after detach')[0]['pid'] == detached_pid
        assert all(row['sid'] == SID for row in fixture.inputs())

        print('Agy: native turn cancellation retains the same runtime', flush=True)
        interrupted_pid = check_startup_interrupt(page, fixture, uid, screenshots)

        # Cancellation failure or slow native completion never escalates to
        # process signals. A repeated click while pending is idempotent.
        for mode in ('reject', 'pending'):
            (fixture.root / 'interrupt-mode').write_text(mode)
            token = 'interrupt-' + mode
            send(page, 'hold:' + token)
            expect(page.locator('.sdk-live')).to_contain_text('Agy running ' + token)
            with page.expect_response(lambda response: '/api/native-sdk/interrupt' in response.url) as stopped:
                page.locator('#sdk-stop').click()
            if mode == 'reject':
                assert not stopped.value.ok and '原生接口拒绝 CancelCascadeInvocation' in stopped.value.text()
                expect(page.locator('#sdk-stop')).to_be_enabled()
                assert page.evaluate('!NativeSdkView.raw.cancel_requested')
            else:
                expect(page.locator('#sdk-composer .sdk-status')).to_have_text('停止已受理')
                expect(page.locator('#sdk-stop')).to_be_disabled()
                assert post(page, base, 'interrupt', {'uid': uid})['raw']['interrupted']
            # The former five-second SIGKILL deadline must no longer exist.
            page.wait_for_timeout(5500)
            assert page.evaluate('NativeSdkView.running')
            assert not any(event['kind'] == 'signal' for event in fixture.events())
            assert len([event for event in fixture.events()
                        if event['kind'] == 'cancel' and event['mode'] == mode]) == 1
            fixture.release(token)
            idle(page)
            assert page.evaluate('NativeSdkView.raw.cancel_requested === false')
            (fixture.root / 'interrupt-mode').unlink()
            send(page, 'after interrupt-' + mode)
            expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: after interrupt-' + mode)
            idle(page)
            assert fixture.inputs('after interrupt-' + mode)[0]['pid'] == interrupted_pid

        print('Agy: unsupported responses, stderr redaction, independent writer rejection', flush=True)
        post(page, base, 'respond', {'uid': uid, 'key': 'not-a-native-request', 'result': 'approve'}, ok=False)
        send(page, '__ERROR__')
        idle(page)
        expect(page.locator('.sdk-live')).to_contain_text('synthetic native error')
        diagnostic = page.evaluate('window.__agySnapshot.raw.diagnostics')
        assert 'synthetic transport diagnostic' in diagnostic
        assert fixture.env['AGY_SDK_FAKE_KEY'] not in diagnostic and '[redacted]' in diagnostic
        # A native execution error after admission completes the send
        # (docs/native-sdk.md): no visible queue record, nothing to remove.
        page.wait_for_function("!(NativeSdkView.raw.input_queue||[]).some(row=>row.text==='__ERROR__')")
        send(page, 'continue after native error')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: continue after native error')
        idle(page)
        assert fixture.inputs('__ERROR__')[-1]['pid'] == fixture.inputs('continue after native error')[-1]['pid']
        assert len(fixture.inputs('__ERROR__')) == 1

        print('Agy: recovered stream with stale ERROR receipt is not a failed send', flush=True)
        for marker, conflict in [('__HISTORICAL_ERROR__', False), ('好，请在独立工作区修复审计发现的问题，完成复审与验证；不要重复执行已经完成的任务，并保留当前会话。', True), ('__UNRECOVERED_STREAM__', False), ('__HISTORICAL_WITH_CURRENT_ERROR__', False)]:
            send(page, marker)
            idle(page)
            # Native admission completes the send; the execution outcome stays
            # in runtime state, never as a failed/unknown queue record.
            page.wait_for_function("text=>!(NativeSdkView.raw.input_queue||[]).some(row=>row.text===text)", arg=marker)
            raw = page.evaluate('NativeSdkView.raw')
            assert raw['error_kind'] == ('uncertain' if conflict else 'turn'), raw['error']
            if conflict:
                expect(page.locator('.sdk-live')).to_contain_text('最终结果：所有指定检查通过')
                expect(page.locator('.native-sdk-pane .sdk-status')).to_have_text('结果待核对')
                expect(page.locator('.native-sdk-pane .sdk-status')).to_have_attribute('data-state', 'warning')
                assert not any(item.get('id', '').startswith('agy:error:') and item.get('content', '').startswith('agy_sdk_result_conflict:') for item in raw['items'])
                assert 'result.status=ERROR' in raw['diagnostics']
                if screenshots:
                    for width, theme in [(1440, 'dark'), (675, 'light')]:
                        page.set_viewport_size({'width':width,'height':900})
                        page.emulate_media(color_scheme=theme)
                        page.screenshot(path=str(screenshots/f'agy-result-conflict-{width}-{theme}.png'),full_page=True)
                    page.set_viewport_size({'width':1440,'height':900})
                    page.emulate_media(color_scheme='dark')
            assert len(fixture.inputs(marker)) == 1
            if not conflict:
                followup = 'continue after ' + marker
                send(page, followup)
                expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: ' + followup)
                idle(page)
                assert fixture.inputs(marker)[-1]['pid'] == fixture.inputs(followup)[-1]['pid']
                assert len(fixture.inputs(marker)) == 1
            if conflict:
                send(page, 'after conflicting result')
                expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: after conflicting result')
                idle(page)
                assert fixture.inputs(marker)[-1]['pid'] == fixture.inputs('after conflicting result')[-1]['pid']
                assert len(fixture.inputs(marker)) == 1
                print('Agy: completed native turn ignores proven earlier-turn ERROR and keeps its runtime', flush=True)
                receipt = send(page, '__HISTORICAL_ERROR__')
                expect(page.locator('.sdk-live')).to_contain_text('本轮完整回复：历史错误未污染本轮。')
                idle(page)
                raw = page.evaluate('NativeSdkView.raw')
                assert not raw.get('error'), raw.get('error')
                assert not any(row['request_id'] == receipt['request_id'] for row in raw['input_queue'])
                assert 'prior turn; current native response completed' in raw['diagnostics']
                assert fixture.inputs(marker)[-1]['pid'] == fixture.inputs('__HISTORICAL_ERROR__')[-1]['pid']
                if screenshots:
                    for width, theme in [(1440, 'dark'), (675, 'light')]:
                        page.set_viewport_size({'width':width,'height':900})
                        page.emulate_media(color_scheme=theme)
                        page.screenshot(path=str(screenshots/f'agy-historical-result-{width}-{theme}.png'),full_page=True)
                    page.set_viewport_size({'width':1440,'height':900})
                    page.emulate_media(color_scheme='dark')
                send(page, 'next round after historical error')
                expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: next round after historical error')
                idle(page)
                assert fixture.inputs(marker)[-1]['pid'] == fixture.inputs('next round after historical error')[-1]['pid']
        blocked_input, warm_input = check_writer_admission(page, fixture, screenshots)

        print('Agy: create through the real picker, native SID receipt and first SEND', flush=True)
        open_dialog(page)
        page.locator('input[name="new-source"][value="agy"]').check()
        page.locator('#new-cwd').fill(str(fixture.work))
        with page.expect_response(lambda response: '/api/native-sdk/create' in response.url) as created_event:
            page.locator('#new-session-go').click()
        assert created_event.value.ok, created_event.value.text()
        created = created_event.value.json()
        assert created['session_id'] != SID
        assert (fixture.native / 'conversations' / (created['session_id'] + '.db')).is_file()
        assert created['session']['source'] == 'agy'
        assert created['uid'].startswith('agy:')
        assert not [row for row in fixture.inputs() if row['sid'] == created['session_id']]
        expect(page.locator('#sdk-input')).to_be_visible()
        watch(page, created['uid'])
        send(page, 'created native conversation')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: created native conversation')
        expect(page.locator('#sdk-input')).to_have_value('')
        idle(page)
        assert fixture.inputs('created native conversation')[0]['sid'] == created['session_id']
        page.reload(wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        select_sdk(page, created['uid'])
        expect(page.locator('#sdk-input')).to_have_value('')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: created native conversation')
        # Shared SessionDock attachments work even without native image content.
        watch(page, created['uid'])
        page.locator('#sdk-add').click()
        with page.expect_file_chooser() as chooser:
            page.locator('#sdk-attach-menu [data-attach=file]').click()
        chooser.value.set_files({'name': 'agy-attachment.png', 'mimeType': 'image/png', 'buffer': base64.b64decode(PNG)})
        expect(page.locator('#sdk-compose-items')).to_contain_text('agy-attachment.png')
        expect(page.locator('#native-media-note')).to_have_count(0)
        send(page, 'shared attachment through SDK')
        expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: shared attachment through SDK')
        idle(page)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(0)
        assert list(fixture.work.rglob('agy-attachment.png')), 'shared upload was not published'
        assert any('agy-attachment.png' in row['text'] for row in fixture.inputs())
        assert not list((fixture.root / 'host').glob('sessiondock-*.json')), 'SDK must not create a PTY'
        if screenshots:
            screenshots.mkdir(parents=True, exist_ok=True)
            for width in (1280, 390, 320):
                resize_sdk(page,page.evaluate('S.sel'),width)
                expect(page.locator('#sdk-input')).to_be_visible()
                for theme in ('light', 'dark'):
                    page.emulate_media(color_scheme=theme)
                    assert_inline_usage(page)
                    expect(page.locator('#native-media-note')).to_have_count(0)
                    page.screenshot(path=str(screenshots / f'agy-sdk-{width}-{theme}.png'), full_page=True)
                    assert_usage_outside_menu(page)
                    if page.locator('#sdk-more').is_visible():
                        page.locator('#sdk-more').click()
                        page.screenshot(path=str(screenshots / f'agy-sdk-settings-{width}-{theme}.png'), full_page=True)
                        close_more(page)
        assert not errors, errors
        return blocked_input, warm_input
    except Exception:
        if screenshots:page.screenshot(path=str(screenshots/'agy-failure.png'),full_page=True)
        raise
    finally:
        context.close()


def exercise_rewind_echo(browser, base, fixture, screenshots):
    sid = 'a6400000-0000-4000-8000-000000000064'
    title = 'Agy rewind echo'
    fixture.seed(sid)
    with sqlite3.connect(fixture.native / 'conversation_summaries.db') as db:
        db.execute('UPDATE conversation_summaries SET title=? WHERE conversation_id=?', (title, sid))
    with sqlite3.connect(fixture.native / 'conversations' / f'{sid}.db') as db:
        db.execute('CREATE TABLE steps (idx INTEGER PRIMARY KEY,step_type INTEGER,status INTEGER,step_format INTEGER,step_payload BLOB,metadata BLOB)')
        db.executemany('INSERT INTO steps (idx,step_type,status) VALUES (?,?,3)', [(0, 14), (1, 1)])
    (fixture.root / 'native-echo-store').touch()
    fixture.release('rewind-echo')
    prompt = 'hold:rewind-echo'
    abandoned = '这条旧消息已回退'
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    try:
        page = context.new_page()
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        row = page.locator('#side .item[data-uid]').filter(has_text=title)
        uid = row.get_attribute('data-uid', timeout=20000)
        select_sdk(page, uid)
        watch(page, uid)
        history = page.locator('.sdk-history')
        live = page.locator('.sdk-live')
        composer = page.locator('#sdk-input')
        send(page, prompt)
        idle(page)
        send(page, abandoned)
        idle(page)
        old = history.locator('.msg[data-role="user"]').filter(has_text=abandoned)
        expect(old).to_be_visible(timeout=20000)
        if screenshots:
            page.screenshot(path=str(screenshots / 'agy-rewind-before.png'), full_page=True)
        old.get_by_role('button', name='回退到此处之前', exact=True).click()
        with page.expect_response('**/api/native-sdk/revert') as reverted:
            page.locator('dialog.app-popup[open]').get_by_role('button', name='回退', exact=True).click()
        assert reverted.value.ok, reverted.value.text()
        expect(composer).to_have_value(abandoned)
        expect(old).to_have_count(0)
        expect(live).not_to_contain_text(abandoned)
        # Repeat a message already present in retained history. The new
        # delivery must use the reused index, even without a stream echo.
        (fixture.root / 'rewind-reply-delay').touch()
        receipt = send(page, prompt)
        old_reply = history.locator('.msg[data-role="assistant"]').filter(has_text=abandoned)
        new_reply = history.locator('.msg[data-role="assistant"]').filter(has_text='回退后的新回复。')
        users = history.locator('.msg[data-role="user"]').filter(has_text=prompt)
        expect(users).to_have_count(2, timeout=20000)
        page.wait_for_function('''id => window.__agySnapshot?.raw?.items.some(
            item => item.request_id === id && item.role === 'user' && item.native_echo)''',
            arg=receipt['request_id'])
        # Cross a history poll with the new user step and the old reply still
        # in both SQLite and JSONL.
        page.wait_for_timeout(2200)
        assert any(event['kind'] == 'rewind-reply-pending' and event['sid'] == sid for event in fixture.events())
        expect(old_reply).to_have_count(0)
        expect(live).not_to_contain_text(abandoned)
        expect(new_reply).to_have_count(0)
        if screenshots:
            page.locator('.sdk-msgs').evaluate('box => box.scrollTop = box.scrollHeight')
            page.screenshot(path=str(screenshots / 'agy-rewind-pending.png'), full_page=True)
        fixture.release('rewind-reply-db')
        # The replacement in SQLite now has a new creation time while JSONL
        # still has only the old line.
        page.wait_for_timeout(2200)
        assert any(event['kind'] == 'rewind-reply-in-db' and event['sid'] == sid for event in fixture.events())
        expect(old_reply).to_have_count(0)
        expect(new_reply).to_have_count(0)
        fixture.release('rewind-reply')
        idle(page)
        expect(new_reply).to_have_count(1, timeout=20000)
        assert new_reply.evaluate('(reply, user) => !!(user.compareDocumentPosition(reply) & Node.DOCUMENT_POSITION_FOLLOWING)',
                                  users.last.element_handle())
        assert not page.evaluate('''([id, abandoned]) => window.__agySnapshots.some(
            ({raw}) => raw.items.some(item => item.request_id === id &&
                item.role === 'assistant' && item.content.includes(abandoned)))''',
            [receipt['request_id'], abandoned]), 'bridge hydrated the abandoned reply'
        if screenshots:
            page.screenshot(path=str(screenshots / 'agy-rewind-after-wide.png'), full_page=True)
        items = page.evaluate('''id => window.__agySnapshot.raw.items.filter(
            item => item.request_id === id)''', receipt['request_id'])
        item = next(item for item in items if item['role'] == 'user')
        assert item.get('native_echo') is True, ('the bridge left the persisted input unbound', item)
        assert [item['role'] for item in items] == ['user', 'assistant']
        assert item['native']['step_index'] == next(
            row['first_removed'] for row in fixture.events() if row['kind'] == 'revert' and row['sid'] == sid)
        expect(live.locator('.msg[data-role="user"]')).to_have_count(0)
        expect(page.locator('.sdk-msgs')).not_to_contain_text(abandoned)
        expect(composer).to_have_value('')
        with sqlite3.connect(fixture.root / 'state/native-sdk-inputs.sqlite') as db:
            stored, = db.execute('SELECT row FROM inputs WHERE id=?', (receipt['queue_id'],)).fetchone()
        assert json.loads(stored)['consumption'] == 'observed'
        assert len({row['pid'] for row in fixture.inputs() if row['sid'] == sid}) == 1
        assert len([row for row in fixture.inputs(prompt) if row['sid'] == sid]) == 2
        page.reload(wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        select_sdk(page, uid)
        resize_sdk(page, uid, 390)
        page.emulate_media(color_scheme='dark')
        expect(history.locator('.msg[data-role="user"]').filter(has_text=prompt)).to_have_count(2)
        expect(live.locator('.msg[data-role="user"]')).to_have_count(0)
        if screenshots:
            page.screenshot(path=str(screenshots / 'agy-rewind-after-narrow.png'), full_page=True)
        print('Agy: rewind, reused stdin step, history join and reload verified', flush=True)
    finally:
        context.close()


def diagnostic_events(fixture):
    """Read the real service's persisted trace after its normal shutdown flush."""
    records = [json.loads(line)
               for path in sorted((fixture.root / 'state/runtime-logs').glob('*.jsonl'))
               for line in path.read_text().splitlines()]
    return [row for row in records if row.get('event', '').startswith(('agy.sdk.', 'native_sdk.input_'))]


def check_diagnostic_logs(fixture, events):
    assert events, 'SDK diagnostics were not persisted'
    assert all(row.get('aggregation') == 'none' and 'repeat_count' not in row for row in events)
    encoded = json.dumps(events, ensure_ascii=False)
    for private in (fixture.env['AGY_SDK_FAKE_KEY'], '__UNRECOVERED_STREAM__',
                    '最终结果：所有指定检查通过', '历史错误未污染本轮', 'synthetic native error',
                    'The stream was interrupted. Please continue the task you were working on.'):
        assert private not in encoded, f'private native content escaped into metadata logs: {private}'
    results = [row for row in events if row['event'] == 'agy.sdk.result']
    for conflict, reason in [(True, 'recovered_response_with_stale_error'),
                             (False, 'missing_final_response')]:
        matches = [row for row in results if row['evidence']['reason'] == reason]
        assert matches, (reason, results)
        result = matches[0]
        delivery = result['delivery_id']
        trace = [row for row in events if row.get('delivery_id') == delivery]
        assert result['conflict'] == conflict and result['status'] == 'ERROR', result
        evidence = result['evidence']
        assert evidence['user_step'] < evidence['error_steps'][0] <= evidence['last_step'], evidence
        assert result['response_bytes'] > 0 and result['stdout_offset'] > 0
        assert result['native_pid'] != result['pid'], 'native PID was overwritten by the service logger'
        assert any(row['pid'] == result['native_pid'] for row in fixture.inputs())
        assert any(row['event'] == 'agy.sdk.input_written' and row['native_pid'] == result['native_pid'] for row in trace)
        assert any(row['event'] == 'agy.sdk.write_receipt' and row['written'] and not row['reused'] for row in trace)
        assert any(row['event'] == 'agy.sdk.step' and row['kind'] == 'user_input' for row in trace)
        finished = next(row for row in trace if row['event'] == 'agy.sdk.turn_finished')
        assert finished['runtime_retained'], finished
        # The send completes on native admission; the ERROR is a runtime outcome.
        transition = next(row for row in trace if row['event'] == 'native_sdk.input_transition' and row['to'] == 'completed')
        assert transition['attempts'] == 0
        assert not any(row['event'] == 'native_sdk.input_transition' and row['to'] in ('unknown', 'failed') for row in trace), trace
        assert transition['request_id'] and transition['uid'] and transition['source'] == 'agy'
        assert all(row['runtime_id'] == result['runtime_id'] for row in trace if 'runtime_id' in row)
        if conflict:
            assert evidence['response_suffix_matches'] and evidence['final_response_bytes'] > 0
            assert evidence['last_step'] > evidence['error_steps'][-1]
        assert not any(row['event'] in ('agy.sdk.interrupt', 'agy.sdk.terminate',
                                       'agy.sdk.kill_requested', 'agy.sdk.kill_signal') for row in trace)
    assert any(row['status'] == 'SUCCESS' for row in results), 'normal result tracing missing'
    inherited = next(row for row in results if row['evidence']['reason'] == 'historical_error_after_completed_response')
    assert inherited['status'] == 'ERROR' and inherited['effective_status'] == 'SUCCESS'
    assert inherited['historical_error'] and not inherited['conflict']
    assert inherited['evidence']['error_steps'] == []
    assert inherited['evidence']['prior_error_step'] < inherited['evidence']['user_step'] < inherited['evidence']['last_step']
    assert any(row['event'] == 'native_sdk.input_transition' and row['delivery_id'] == inherited['delivery_id']
               and row['to'] == 'completed' and row['consumption'] == 'observed' for row in events)
    assert any(row['evidence']['reason'] == 'missing_native_error' and row['effective_status'] == 'ERROR' for row in results)
    assert any(row['evidence']['reason'] == 'different_current_error' and row['effective_status'] == 'ERROR' for row in results)
    assert any(row['event'] == 'agy.sdk.interrupt' and row['sent']
               and row['method'] == 'CancelCascadeInvocation'
               and row['kill_background_tasks'] is False for row in events)
    cancel_pid = fixture.inputs('hold:interrupt-before-input')[0]['pid']
    cancel = next(row for row in events if row['event'] == 'agy.sdk.interrupt'
                  and row['native_pid'] == cancel_pid)
    assert any(row['event'] == 'agy.sdk.step' and row['kind'] == 'user_input'
               and row['delivery_id'] == cancel['delivery_id']
               and row['stdout_offset'] <= cancel['stdout_offset'] for row in events)
    assert not any(row['event'] in ('agy.sdk.kill_requested', 'agy.sdk.kill_signal')
                   and row.get('native_pid') == cancel_pid for row in events), 'turn cancellation killed the runtime'
    print('Agy: persisted request/delivery/runtime/step/result/queue trace and metadata-only logging verified', flush=True)


def check_writer_logs(events, blocked_input, warm_input):
    for delivery, available, finish in [(blocked_input, False, 'native_sdk.input_transition'),
                                         (warm_input, True, 'agy.sdk.input_written')]:
        def index(event, state=None):
            return next(i for i, row in enumerate(events) if row['event'] == event
                        and row.get('delivery_id') == delivery
                        and (state is None or row['to'] == state))

        queued = index('native_sdk.input_transition', 'queued')
        sending = index('native_sdk.input_transition', 'sending')
        finished = index(finish, None if available else 'failed')
        checks = [(i, row) for i, row in enumerate(events[queued:finished], queued)
                  if row['event'] == 'agy.sdk.writer_check' and row['session_id'] == SID]
        assert len(checks) == 1 and sending < checks[0][0] < finished, checks
        assert checks[0][1]['available'] is available, checks
    assert not any(row['event'] == 'agy.sdk.turn_started' and row['delivery_id'] == blocked_input
                   for row in events)
    print('Agy: one writer check per warm send, explicit occupancy failure and same-process recovery verified', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--ptyhost', type=Path, default=REPO / 'target/debug/ptyhost')
    parser.add_argument('--frontend', type=Path)
    parser.add_argument('--screenshots', type=Path)
    scope = parser.add_mutually_exclusive_group()
    scope.add_argument('--rewind-echo-only', action='store_true',
                       help='Exercise Agy rewind and the next input without a headless echo')
    scope.add_argument('--writer-check-only', action='store_true',
                       help='Exercise warm sends and external writer refusal')
    args = parser.parse_args()
    os.environ.setdefault('PLAYWRIGHT_BROWSERS_PATH', str(Path.home() / '.cache/ms-playwright'))
    if args.frontend:
        os.environ['SESSIONDOCK_TEST_WEB_DIR'] = str(args.frontend.resolve())
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-sdk-') as tmp, ExitStack() as cleanup:
        root = Path(tmp).resolve()
        cleanup.callback(stop_private_sdkhosts, root)
        fixture = AgyFixture(root, args.ptyhost.resolve(strict=True))
        corpus = Corpus(root)
        corpus.put('navigation-history', 'claude', [
            claude_row('navigation-history', 'user', 'navigation-user', None,
                       '另一个会话的历史，用于验证 SDK 切换。'),
        ], [])
        with patch.dict(os.environ, fixture.env):
            initialize('--initialize-lifecycle', root / 'ledger', args.binary.resolve(strict=True))
            with isolated_server(corpus, args.binary, host_dir=root / 'host',
                    lifecycle_dir=root / 'ledger', state_dir=root / 'state',
                    launcher_config=fixture.launcher, extra_env=fixture.extra_env) as (base, _), sync_playwright() as pw:
                options = {'headless': True}
                if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                    options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                browser = pw.chromium.launch(**options)
                try:
                    if args.screenshots:
                        args.screenshots.mkdir(parents=True, exist_ok=True)
                    if args.writer_check_only:
                        blocked_input, warm_input = exercise_writer_admission(browser, base, fixture, args.screenshots)
                    elif not args.rewind_echo_only:
                        blocked_input, warm_input = exercise(browser, base, fixture, args.screenshots)
                    if not args.writer_check_only:
                        exercise_rewind_echo(browser, base, fixture, args.screenshots)
                finally:
                    browser.close()
        if not args.rewind_echo_only:
            events = diagnostic_events(fixture)
            check_writer_logs(events, blocked_input, warm_input)
            if not args.writer_check_only:
                check_diagnostic_logs(fixture, events)
    print('PASS Agy native SDK browser', flush=True)


if __name__ == '__main__':
    main()
