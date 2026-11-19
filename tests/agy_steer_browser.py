#!/usr/bin/env python3
"""Agy running-turn steering, queue_steer promotion, and restart recovery via Node/Hub Chromium UI."""
from __future__ import annotations

import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from types import SimpleNamespace
from unittest.mock import patch
from urllib.parse import quote

sys.dont_write_bytecode = True
from playwright.sync_api import expect, sync_playwright
from agy_history_browser import execute, record, system_record, write_transcript
from agy_sdk_browser import resize_sdk, send
from agy_sdk_fixtures import AgyFixture, SID, TITLE
from claude_sdk_browser import open_sdk, wait_snapshot
from history_fixtures import BINARY, REPO, Corpus, claude_row, isolated_server
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN, free_port, node_env
from sdk_selection_fixtures import snapshots
from sdk_send_gesture_fixtures import exercise_idle_send_gestures, exercise_send_gestures
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize


def capture(page, shots: Path | None, name: str):
    if not shots:
        return
    page.screenshot(path=str(shots / f'{name}-full.png'), full_page=True)
    pane = page.locator('.native-sdk-pane')
    if pane.is_visible():
        pane.screenshot(path=str(shots / f'{name}-pane.png'))


def check_foreground_fifo(page, base, fixture, uid, shots, label, restart_server):
    root = fixture.root
    status = root / 'foreground-status'
    status.write_text('RUNNING')
    (root / 'native-echo-store').touch()
    child = 'a6000000-0000-4000-8000-000000000060'
    fixture.seed(child)
    execute(fixture.native / 'conversation_summaries.db',
            'UPDATE conversation_summaries SET parent_conversation_id=?, title=? WHERE conversation_id=?',
            (SID, 'Training monitor', child))

    def timer(index, delay, prompt, created_at=None):
        value = record(index, 'GENERIC',
                       'Created At: 2026-10-04T00:00:00Z\n'
                       f'Tool is running as a background task with task id: {child}/task-{index}\n'
                       f'Task Description: Timer: {delay}, Prompt: {prompt}\n'
                       'Task logs are available at: file:///private/timer.log')
        value['status'] = 'RUNNING'
        if created_at:
            value['created_at'] = created_at
        return value

    training = '核对训练进度，检查每个种子的最后一个分段，完成后导出预测并报告异常。'
    replay = '确认所有历史回放已运行完毕。'
    now = datetime.now(timezone.utc).isoformat()
    older = '旧巡检 8：核对上一次训练与导出日志。'
    elapsed = [timer(index, '2m30s', f'旧巡检 {index}：核对上一次训练与导出日志。')
               for index in range(8, 20)]
    tasks = [timer(2, '5m', 'already completed'), timer(3, '35m', training, now), timer(4, '1h 30m', replay, now),
             system_record(5, 'Timer fired', sender=f'{child}/task-2')]
    write_transcript(fixture.native, child, [*tasks, *elapsed])
    notice = page.locator('#sdk-composer .sdk-status')
    send(page, '__UNRECOVERED_STREAM__')
    expect(notice).to_have_text('错误')
    send(page, 'hold:foreground')
    wait_snapshot(page, uid, lambda raw: any(
        item.get('native_echo') and item.get('content') == 'hold:foreground'
        for item in raw['items']))
    pid = fixture.inputs('hold:foreground')[0]['pid']
    details = page.locator('#sdk-status-menu')
    expect(notice).to_have_text('处理中')
    status.write_text('IDLE')
    expect(notice).to_have_text('后台定时任务等待中', timeout=20000)
    for width, theme in ((1280, 'light'), (390, 'dark')):
        page.emulate_media(color_scheme=theme)
        resize_sdk(page, uid, width)
        expect(notice).to_be_enabled()
        if width == 1280:
            notice.click()
        else:
            notice.press('Enter')
        expect(details).to_be_visible()
        expect(details).to_contain_text('Agy 前台已空闲')
        expect(details).to_contain_text('35 分')
        expect(details).to_contain_text(training)
        expect(details).to_contain_text('1 小时 30 分')
        expect(details).to_contain_text(replay)
        expect(details).not_to_contain_text('already completed')
        expect(details).not_to_contain_text('file:///')
        expect(details.locator('.sdk-timer-body > .sdk-timer-list > li')).to_have_count(2)
        expect(details.get_by_text(older, exact=True)).to_have_count(0)
        bounds = details.bounding_box()
        assert bounds['x'] >= 7 and bounds['x'] + bounds['width'] <= width - 7, bounds
        if width == 1280:
            assert abs(bounds['x'] - notice.bounding_box()['x']) <= 2, bounds
        capture(page, shots, f'{label}-foreground-timers-{width}-{theme}')
        page.keyboard.press('Escape')
        expect(details).to_be_hidden()
    expiring = '即将到期的巡检任务。'
    many = [timer(20, '15s', expiring, datetime.now(timezone.utc).isoformat()),
            *(timer(step, '2h10m', f'第 {step} 项巡检：核对训练节点与导出任务进度。', now)
              for step in range(21, 30))]
    notice.click()
    write_transcript(fixture.native, child, [*tasks, *elapsed, *many])
    expect(details.get_by_text(expiring, exact=True)).to_be_visible(timeout=12000)
    assert details.bounding_box()['height'] <= 421
    assert details.locator('.sdk-timer-body').evaluate('body => body.scrollHeight > body.clientHeight')
    details.get_by_text('第 29 项巡检：核对训练节点与导出任务进度。', exact=True).scroll_into_view_if_needed()
    expect(details.get_by_role('heading', name='后台定时任务')).to_be_in_viewport()
    capture(page, shots, f'{label}-foreground-timers-scroll-390-dark')
    expect(details.get_by_text(expiring, exact=True)).to_have_count(0, timeout=18000)
    expect(details.get_by_text(older, exact=True)).to_have_count(0)
    page.keyboard.press('Escape')
    page.emulate_media(color_scheme='light')
    resize_sdk(page, uid, 1280)
    notice.click()
    write_transcript(fixture.native, child, [*tasks, *elapsed, *many,
        system_record(30, 'Timer fired', sender=f'{child}/task-3'),
        system_record(31, 'Timer fired', sender=f'{child}/task-4'),
        *(system_record(step + 12, 'Timer fired', sender=f'{child}/task-{step}') for step in range(21, 30))])
    expect(notice).to_have_text('等待回执', timeout=20000)
    expect(details).to_be_visible()
    expect(details).not_to_contain_text(training)
    expect(details).not_to_contain_text(replay)
    page.keyboard.press('Escape')
    status.write_text('RUNNING')
    expect(notice).to_have_text('处理中', timeout=10000)
    echo = root / 'steer-echo-delay'
    echo.touch()
    consume = root / 'steer-consume-delay'
    consume.touch()
    first = 'foreground FIFO first ' + label
    second = 'foreground FIFO second ' + label
    send(page, first)
    send(page, second)
    queued = page.locator('.sdk-queued').filter(has_text=second)
    expect(queued).to_be_visible()
    capture(page, shots, f'{label}-foreground-running-queue')
    assert not fixture.steers(first) and not fixture.steers(second)

    status.write_text('IDLE')
    wait_snapshot(page, uid, lambda raw: any(
        item.get('content') == first and item.get('fifo_control') and not item.get('native_echo')
        for item in raw['items']))
    assert not fixture.steers(second), 'a later FIFO input overtook the native echo'
    assert not (root / 'release/foreground').exists()
    if restart_server:
        base = restart_server()
        page.goto(base, wait_until='domcontentloaded')
        open_sdk(page, fixture, expect, uid=uid)
        expect(queued).to_be_visible()
        assert len(fixture.steers(first)) == 1
    consume.unlink()
    wait_snapshot(page, uid, lambda raw: any(
        item.get('content') == second and item.get('native_echo') and item.get('steered') is False
        for item in raw['items']))
    for text in (first, second):
        deliveries = fixture.steers(text)
        assert len(deliveries) == 1 and deliveries[0]['pid'] == pid
        assert not fixture.inputs(text)
    assert not (root / 'release/foreground').exists()
    expect(queued).to_have_count(0)
    capture(page, shots, f'{label}-foreground-fifo-delivered')

    def finish_foreground():
        fixture.release('foreground')
        deadline = time.monotonic() + 5
        while not (root / 'foreground-result-written').exists():
            assert time.monotonic() < deadline, 'fake Agy did not emit the final receipt'
            time.sleep(0.02)
        echo.unlink()

    if restart_server:
        # Rebuild both FIFO echoes from a burst of unkeyed native output
        # followed by the stale ERROR receipt after the service stopped.
        base = restart_server(finish_foreground)
        page.goto(base, wait_until='domcontentloaded')
        open_sdk(page, fixture, expect, uid=uid)
    else:
        finish_foreground()
    snapshot = wait_snapshot(page, uid, lambda raw:
                             (raw.get('last_turn') or {}).get('status') in ('completed', 'failed')
                             and any(item.get('content') == 'hold:foreground' for item in raw['items']))
    raw = snapshot['raw']
    assert raw['last_turn']['status'] == 'completed' and raw['error'] is None, raw['error']
    assert 'prior turn; current native response completed' in raw['diagnostics']
    expect(notice).to_have_text('就绪', timeout=20000)
    messages = page.locator('.sdk-msgs')
    expect(messages.get_by_text(second, exact=True)).to_have_count(1)
    expect(messages).to_contain_text('Agy steered: '+second)
    assert all(len(fixture.steers(text)) == 1 for text in (first, second))
    capture(page, shots, f'{label}-foreground-fifo-completed')
    status.unlink()
    (root / 'native-echo-store').unlink()
    return base


def check(page, base: str, fixture: AgyFixture, shots: Path | None, label: str,
          restart_server=None, stream_recovery_only=False):
    errors = []
    page.set_default_timeout(15000)
    page.on('pageerror', lambda error: (print('PAGEERROR', error, flush=True), errors.append(error.stack or str(error))))
    page.on('response', lambda response: print('SDK HTTP error', response.status, response.url, response.text(), flush=True) if '/api/native-sdk/' in response.url and response.status >= 400 else None)
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    row = page.locator('#side .item[data-uid]').filter(has_text=TITLE).first
    expect(row).to_be_visible()
    uid = row.get_attribute('data-uid')
    assert ('~' in uid) == (label == 'hub')
    open_sdk(page, fixture, expect, uid=uid)
    box = page.locator('#sdk-input')
    button = page.locator('#sdk-send')
    messages = page.locator('.sdk-msgs')
    expect(button).to_have_text('发送')

    def idle_done(text: str):
        expect(messages).to_contain_text('Synthetic Agy reply: ' + text)
        wait_snapshot(page, uid, lambda raw: not raw['running'])
        assert len(fixture.inputs(text)) == 1
        assert not fixture.steers(text)

    if not stream_recovery_only:
        exercise_idle_send_gestures(page, 'agy', label, idle_done)

    send(page, 'hold:steer')
    wait_snapshot(page, uid, lambda raw: raw['running'])
    expect(button).to_have_text('排队')

    # Recall-edit check on a queued message, then FIFO + queue_steer conversion preserving draft.
    recall_text = 'recall edit ' + label
    recalled = page.locator('.sdk-queued').filter(has_text=recall_text)
    with snapshots() as stalled:
        # The browser receives an initial SDK snapshot, then the stream stays
        # open but silent. The real node still journals the real SEND request.
        stalled.packet = {'type': 'snapshot', 'seq': 1000000, 'raw': page.evaluate('NativeSdkView.raw')}
        page.route('**/api/native-sdk/events?*', lambda route: route.continue_(url=stalled.url), times=1)
        page.locator('#side .item[data-uid^="claude:"]').click()
        row.click()
        page.wait_for_function('NativeSdkView.seq === 1000000')
        calls = []

        def record_call(request):
            if request.url.endswith(('/api/native-sdk/open', '/api/native-sdk/turn')):
                calls.append(request.url.rsplit('/', 1)[1])

        page.on('request', record_call)
        box.fill(recall_text)
        with page.expect_response('**/api/native-sdk/turn') as sent:
            button.click()
        receipt = sent.value.json()['raw']
        assert receipt['accepted'] and receipt['state'] == 'queued', receipt
        expect(box).to_have_value('')
        expect(recalled).to_be_visible(timeout=8000)
        expect(recalled.locator('.queued-send-state')).to_have_text('排队中')
        expect(messages.get_by_text(recall_text, exact=True)).to_have_count(1)
        page.remove_listener('request', record_call)
        assert calls == ['turn'], calls
        assert not fixture.inputs(recall_text) and not fixture.steers(recall_text)
        capture(page, shots, f'{label}-recovered-queue-wide-light')
    if stream_recovery_only:
        fixture.release('steer')
        idle_done(recall_text)
        expect(recalled).to_have_count(0)
        expect(messages.get_by_text(recall_text, exact=True)).to_have_count(1)
        assert not errors, errors
        return
    recalled.get_by_role('button', name='撤回编辑', exact=True).click()
    expect(recalled).to_have_count(0)
    expect(box).to_have_value('recall edit ' + label)
    box.fill('')

    send(page, 'ordinary FIFO ' + label)
    fifo = page.locator('.sdk-queued').filter(has_text='ordinary FIFO ' + label)
    expect(fifo).to_be_visible()
    assert not fixture.steers('ordinary FIFO ' + label)

    send(page, 'converted steer ' + label)
    converted = page.locator('.sdk-queued').filter(has_text='converted steer ' + label)
    expect(converted).to_be_visible()
    expect(converted.get_by_role('button', name='立即引导', exact=True)).to_be_visible()
    capture(page, shots, f'{label}-queued-actions-wide-light')

    # Selecting an attached Agy session observes it without calling /open.
    # The initial event snapshot must still expose the durable queue actions.
    opens = []

    def record_open(request):
        if request.url.endswith('/api/native-sdk/open'):
            opens.append(request.url)

    page.on('request', record_open)
    page.locator('#side .item[data-uid^="claude:"]').click()
    row.click()
    page.wait_for_function('uid => NativeSdkView.connected && NativeSdkView.uid === uid', arg=uid)
    assert not opens, opens
    converted.scroll_into_view_if_needed()
    capture(page, shots, f'{label}-switched-queue-wide-light')
    expect(converted.get_by_role('button', name='立即引导', exact=True)).to_be_visible()

    page.reload(wait_until='domcontentloaded')
    page.wait_for_function('uid => NativeSdkView.connected && NativeSdkView.uid === uid', arg=uid)
    assert not opens, opens
    page.remove_listener('request', record_open)
    for width, theme in ((1280, 'light'), (390, 'dark')):
        page.emulate_media(color_scheme=theme)
        resize_sdk(page, uid, width)
        action = converted.get_by_role('button', name='立即引导', exact=True)
        expect(action).to_be_enabled()
        action.scroll_into_view_if_needed()
        capture(page, shots, f'{label}-reopened-queue-{width}-{theme}')
    page.emulate_media(color_scheme='light')
    resize_sdk(page, uid, 1280)

    queue_id = converted.get_attribute('data-queue-id')
    box.fill('preserve unrelated draft')
    gate = fixture.root / 'steer-delay'
    gate.touch()
    with page.expect_request(lambda request: request.url.endswith('/api/native-sdk/queue_steer')):
        converted.get_by_role('button', name='立即引导', exact=True).click()
    expect(converted.get_by_role('button', name='立即引导', exact=True)).to_be_disabled()
    gate.unlink()
    expect(converted).to_have_count(0)
    expect(box).to_have_value('preserve unrelated draft')
    expect(messages).to_contain_text('Agy steered: converted steer ' + label)
    expect(messages.get_by_text('converted steer ' + label, exact=True)).to_have_count(1)
    repeated = page.request.post(base + '/api/native-sdk/queue_steer', data={'uid': uid, 'id': queue_id, 'request_id': 'repeat-conversion'})
    assert repeated.ok, repeated.text()
    assert len(fixture.steers('converted steer ' + label)) == 1
    expect(fifo).to_be_visible()
    assert not fixture.steers('ordinary FIFO ' + label)

    # Ctrl+click, mobile 500ms touch-hold, and short-tap queueing on the held turn.
    resize_sdk(page, uid, 390)
    exercise_send_gestures(page, 'agy', label, shots)
    wait_snapshot(page, uid, lambda raw: any('Agy steered: Ctrl click ' + label in (item.get('content') or '') for item in raw['items']))
    wait_snapshot(page, uid, lambda raw: any('Agy steered: touch hold ' + label in (item.get('content') or '') for item in raw['items']))
    resize_sdk(page, uid, 1280)
    expect(messages).to_contain_text('hold:steer')
    transcript = page.evaluate('''() => [...document.querySelectorAll('.sdk-history > *, .sdk-live > *')].map(node => ({
        cls: node.className,
        id: node.dataset.sdkItem || node.dataset.toolGroupKey || node.dataset.turnId || '',
        text: (node.innerText || '').slice(0, 80),
    }))''')
    hold_idx = next(i for i, row in enumerate(transcript) if 'hold:steer' in row['text'])
    assert not any('sdk-tool-group' in row['cls'] or 'view_file' in row['text'] for row in transcript[hold_idx:]), transcript

    def steer(text: str):
        box.fill(text)
        with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/steer')) as response:
            box.press('Control+Enter')
        expect(button).to_be_enabled()
        return response.value

    accepted = steer('direct steering ' + label)
    assert accepted.ok, accepted.text()
    expect(box).to_have_value('')
    expect(messages).to_contain_text('Agy steered: direct steering ' + label)
    expect(messages.get_by_text('direct steering ' + label, exact=True)).to_have_count(1)
    repeat = page.request.post(accepted.url, data=accepted.request.post_data_json)
    assert repeat.ok, repeat.text()
    assert len(fixture.steers('direct steering ' + label)) == 1
    assert page.evaluate('NativeSdkView.running')

    # Session action menu ("引导当前任务") steers composer draft directly during running turn.
    box.fill('menu steering ' + label)
    action = page.get_by_role('button', name='引导当前任务', exact=True)
    if not action.is_visible():
        page.locator('#sdk-more').click()
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/steer')) as menu_resp:
        action.click()
    assert menu_resp.value.ok, menu_resp.value.text()
    expect(box).to_have_value('')
    expect(messages).to_contain_text('Agy steered: menu steering ' + label)
    assert len(fixture.steers('menu steering ' + label)) == 1

    # Composer attachment + Ctrl+Enter steer and queued attachment + 立即引导.
    page.locator('#sdk-file').set_input_files({'name': 'agy-steer-context.txt', 'mimeType': 'text/plain', 'buffer': b'isolated agy steer context'})
    att_resp = steer('attachment steering ' + label)
    assert att_resp.ok, att_resp.text()
    expect(box).to_have_value('')
    expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(0)
    assert any('attachment steering ' + label in event['text'] and 'agy-steer-context.txt' in event['text'] for event in fixture.steers())

    page.locator('#sdk-file').set_input_files({'name': 'agy-queued-context.txt', 'mimeType': 'text/plain', 'buffer': b'queued agy steer context'})
    send(page, 'queued attachment ' + label)
    attached = page.locator('.sdk-queued').filter(has_text='queued attachment ' + label)
    expect(attached.get_by_role('button', name='立即引导', exact=True)).to_be_visible()
    attached.get_by_role('button', name='立即引导', exact=True).click()
    expect(attached).to_have_count(0)
    expect(messages).to_contain_text('Agy steered: queued attachment ' + label)
    assert any('queued attachment ' + label in event['text'] and 'agy-queued-context.txt' in event['text'] for event in fixture.steers())

    # Native RPC acknowledgement is distinct from step_update consumption: single user bubble + compact receipt.
    consume_gate = fixture.root / 'steer-consume-delay'
    consume_gate.touch()
    acknowledged = steer('ack pending ' + label)
    assert acknowledged.ok, acknowledged.text()
    expect(box).to_have_value('')
    wait_snapshot(page, uid, lambda raw: any(item.get('status') == 'native_queued' and item.get('text') == 'ack pending ' + label for item in raw.get('steers', [])))
    expect(page.locator('.native-sdk-pane').get_by_text('ack pending ' + label, exact=True)).to_have_count(1)
    expect(page.locator('.sdk-queue .native-message-state').filter(has_text='原生已接收引导')).to_be_visible()
    capture(page, shots, f'{label}-ack-pending-wide-light')
    consume_gate.unlink()
    wait_snapshot(page, uid, lambda raw: any(item.get('native_echo') and item.get('content') == 'ack pending ' + label for item in raw['items']) and not any(item.get('text') == 'ack pending ' + label for item in raw.get('steers', [])))
    expect(page.locator('.native-sdk-pane').get_by_text('ack pending ' + label, exact=True)).to_have_count(1)
    expect(messages).to_contain_text('Agy steered: ack pending ' + label)

    # Deterministic rejection keeps draft, records failure without retry button, and deduplicates request_id.
    rejected = steer('reject steer')
    assert not rejected.ok or rejected.json().get('accepted') is False
    expect(box).to_have_value('reject steer')
    failure = page.locator('.sdk-queue [data-queue-origin="bridge"]').filter(has_text='reject steer')
    expect(failure).to_contain_text('HTTP 503')
    expect(failure.get_by_role('button', name='重试', exact=True)).to_have_count(0)
    count = len(fixture.steers('reject steer'))
    steer('reject steer')
    assert len(fixture.steers('reject steer')) == count == 1
    failure.get_by_role('button', name='撤销', exact=True).click()
    expect(failure).to_have_count(0)
    expect(page.locator('.native-sdk-pane')).not_to_contain_text('reject steer')
    assert page.evaluate('NativeSdkView.running')

    # In-flight HTTP delay: single user bubble + 正在发送 state across wide light & narrow dark.
    gate.touch()
    box.fill('delayed steer ' + label)
    with page.expect_request(lambda request: request.url.endswith('/api/native-sdk/steer')):
        box.press('Control+Enter')
    pending = page.locator('.sdk-queue .sdk-queued').filter(has_text='delayed steer ' + label)
    expect(pending).to_be_visible()
    expect(pending).to_have_count(1)
    expect(pending).to_contain_text('正在发送')
    expect(page.locator('.native-sdk-pane .msgs').get_by_text('delayed steer ' + label, exact=True)).to_have_count(1)
    for width, theme in ((1280, 'light'), (390, 'dark')):
        page.emulate_media(color_scheme=theme)
        resize_sdk(page, uid, width)
        expect(pending).to_be_visible()
        capture(page, shots, f'{label}-pending-{width}-{theme}')
    page.emulate_media(color_scheme='light')
    resize_sdk(page, uid, 1280)
    box.fill('newer draft ' + label)
    gate.unlink()
    expect(button).to_be_enabled()
    expect(box).to_have_value('newer draft ' + label)
    wait_snapshot(page, uid, lambda raw: any(item.get('native_echo') and item.get('content') == 'delayed steer ' + label for item in raw['items']))
    expect(page.locator('.native-sdk-pane').get_by_text('delayed steer ' + label, exact=True)).to_have_count(1)

    if restart_server is not None:
        steer_count_before = len(fixture.steers())
        base = restart_server()
        page.goto(base, wait_until='networkidle')
        open_sdk(page, fixture, expect, uid=uid)
        assert page.evaluate('NativeSdkView.running')
        expect(messages.get_by_text('direct steering ' + label, exact=True)).to_have_count(1)
        expect(messages).to_contain_text('Agy steered: direct steering ' + label)
        expect(messages.get_by_text('delayed steer ' + label, exact=True)).to_have_count(1)
        assert len(fixture.steers()) == steer_count_before
    else:
        assert page.evaluate('uid => persistComposerDraft(uid)', uid)
        page.reload(wait_until='networkidle')
        open_sdk(page, fixture, expect, uid=uid)
        expect(box).to_have_value('newer draft ' + label)
        assert len(fixture.steers('delayed steer ' + label)) == 1
        expect(page.locator('.native-sdk-pane').get_by_text('direct steering ' + label, exact=True)).to_have_count(1)
        assert page.evaluate('NativeSdkView.running')

    # Release held turn: ordinary FIFO drains automatically as the next turn, and native transcript history aligns.
    fifo = page.locator('.sdk-queued').filter(has_text='ordinary FIFO ' + label)
    expect(fifo).to_be_visible()
    fixture.release('steer')
    expect(messages).to_contain_text('Synthetic Agy reply: ordinary FIFO ' + label)
    wait_snapshot(page, uid, lambda raw: not raw['running'])
    assert len(fixture.inputs('ordinary FIFO ' + label)) == 1
    capture(page, shots, f'{label}-completed-wide-light')

    # Reload after completion: history + live deduplicate by native_id and /api/messages preserves steered order.
    page.reload(wait_until='networkidle')
    open_sdk(page, fixture, expect, uid=uid)
    expect(page.locator('.native-sdk-pane .msgs').get_by_text('converted steer ' + label, exact=True)).to_have_count(1)
    msgs_text = page.locator('.native-sdk-pane .msgs').inner_text()
    assert msgs_text.index('hold:steer') < msgs_text.index('converted steer ' + label) < msgs_text.index('Agy steered: converted steer ' + label) < msgs_text.index('finished steer') < msgs_text.index('ordinary FIFO ' + label)
    api_msgs = page.request.get(f"{base}/api/messages/{quote(uid, safe='')}").json()
    raw_history = json.dumps(api_msgs.get('messages', []), ensure_ascii=False)
    assert raw_history.index('hold:steer') < raw_history.index('converted steer ' + label) < raw_history.index('Agy steered: converted steer ' + label) < raw_history.index('finished steer') < raw_history.index('ordinary FIFO ' + label)

    base = check_foreground_fifo(page, base, fixture, uid, shots, label, restart_server)

    # Uncertain transport loss before response body is retained as unknown and never replayed.
    send(page, 'hold:uncertain')
    wait_snapshot(page, uid, lambda raw: raw['running'])
    uncertain = steer('unknown steer')
    expect(box).to_have_value('unknown steer')
    unknown = page.locator('.sdk-queue [data-queue-origin="bridge"]').filter(has_text='unknown steer')
    expect(unknown).to_be_visible()
    expect(unknown.get_by_role('button', name='重新发送', exact=True)).to_have_count(0)
    wait_snapshot(page, uid, lambda raw: any(item.get('state') == 'unknown' for item in raw.get('input_queue', [])))
    page.request.post(uncertain.url, data=uncertain.request.post_data_json)
    assert len(fixture.steers('unknown steer')) == 1
    page.reload(wait_until='networkidle')
    open_sdk(page, fixture, expect, uid=uid)
    expect(page.locator('.sdk-queue [data-queue-origin="bridge"]').filter(has_text='unknown steer')).to_be_visible()
    expect(box).to_have_value('unknown steer')
    assert len(fixture.steers('unknown steer')) == 1
    capture(page, shots, f'{label}-unknown-wide-light')
    page.locator('#sdk-stop').click()
    wait_snapshot(page, uid, lambda raw: not raw['running'])
    page.request.post(base + '/api/native-sdk/stop', data={'uid': uid})
    assert all(event['delivery_strategy'] == 'MESSAGE_DELIVERY_STRATEGY_NEXT_INVOCATION' for event in fixture.steers())
    assert not errors, errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--ptyhost', type=Path, default=REPO / 'target/debug/ptyhost')
    parser.add_argument('--frontend', type=Path)
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-agy-steer-browser'))
    parser.add_argument('--stream-recovery-only', action='store_true',
                        help='Exercise a silent browser SDK stream during ordinary FIFO submission')
    args = parser.parse_args()
    os.environ.setdefault('PLAYWRIGHT_BROWSERS_PATH', str(Path.home() / '.cache/ms-playwright'))
    if args.frontend:
        os.environ['SESSIONDOCK_TEST_WEB_DIR'] = str(args.frontend.resolve())
    binary = args.binary.resolve(strict=True)
    ptyhost = args.ptyhost.resolve(strict=True)
    with sync_playwright() as pw:
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        try:
            for hub_mode in (False, True):
                with tempfile.TemporaryDirectory(prefix='sessiondock-agy-steer-') as directory, ExitStack() as stack:
                    root = Path(directory).resolve()
                    stack.callback(stop_private_sdkhosts, root)
                    fixture = AgyFixture(root, ptyhost)
                    corpus = Corpus(root)
                    corpus.put('navigation-history', 'claude', [
                        claude_row('navigation-history', 'user', 'navigation-user', None, '辅助导航会话。'),
                    ], [])
                    stack.enter_context(patch.dict(os.environ, fixture.env))
                    initialize('--initialize-lifecycle', root / 'ledger', binary)
                    node_port = free_port()
                    extra_env = dict(fixture.extra_env)
                    if hub_mode:
                        extra_env.update(node_env(root, node_port, '127.0.0.0/8'))
                    server_box = {'cm': None}

                    def start_node():
                        cm = isolated_server(
                            corpus, binary,
                            host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                            state_dir=root / 'state',
                            file_roots=(root,), file_write_roots=(fixture.work,),
                            launcher_config=fixture.launcher, extra_env=extra_env,
                        )
                        base_url, _ = cm.__enter__()
                        server_box['cm'] = cm
                        return base_url

                    def stop_node():
                        cm = server_box.pop('cm', None)
                        if cm is not None:
                            cm.__exit__(None, None, None)

                    stack.callback(stop_node)
                    base = start_node()
                    restart_cb = None
                    if hub_mode:
                        hub_root = root / 'hub'
                        hub_root.mkdir()
                        hub = Hub(binary.with_name('sessiondock-hub'), hub_root, [SimpleNamespace(name='AgySteerNode', token=TOKEN, port=node_port)])
                        hub.start()
                        stack.callback(hub.stop)
                        base = f'http://127.0.0.1:{hub.port}'
                    else:
                        def restart_cb(while_stopped=None):
                            stop_node()
                            if while_stopped:
                                while_stopped()
                            return start_node()

                    context = browser.new_context(viewport={'width': 1280, 'height': 900}, color_scheme='light', service_workers='block')
                    try:
                        label = 'hub' if hub_mode else 'node'
                        shots = (args.screenshots / label) if args.screenshots else None
                        if shots:
                            shots.mkdir(parents=True, exist_ok=True)
                        check(context.new_page(), base, fixture, shots, label, restart_server=restart_cb,
                              stream_recovery_only=args.stream_recovery_only)
                    finally:
                        context.close()
        finally:
            browser.close()
    print(f"PASS Agy {'stalled SDK stream' if args.stream_recovery_only else 'steer'} Chromium Node + Hub", flush=True)


if __name__ == '__main__':
    main()
