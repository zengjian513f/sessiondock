#!/usr/bin/env python3
"""OpenCode native SDK through Chromium and a synthetic v2 native API.

Requires the shared native-sdk facade/UI integration and a built SessionDock
binary. No real OpenCode, provider, credentials, model call, or production home.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from sdk_usage_fixtures import assert_inline_usage, toggle_usage_details

from browser_runtime import wait_for_async
from history_fixtures import BINARY, Corpus, isolated_server
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from opencode_sdk_fixtures import FakeOpenCode, SID
from media_browser import PNG
from send_browser import initialize



def exercise_hub_fork(browser, native, node_port, binary, root, screenshot):
    hub_root = root / 'hub'
    hub_root.mkdir()
    hub = Hub(binary.resolve(strict=True), hub_root,
              [SimpleNamespace(name='OpenCodeSdkNode', token=TOKEN, port=node_port)])
    hub.start()
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    base = f'http://127.0.0.1:{hub.port}'
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    try:
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=SID)
        parent = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', SID)
        assert parent.startswith('opencode:') and '~' in parent, parent
        page.locator(f'#side .item[data-uid="{parent}"]').click()
        expect(page.locator('#sdk-input')).to_be_visible()
        page.locator('#sdk-more').click()
        with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/fork')) as forked:
            page.locator('[data-sdk-action="fork"]').click()
        assert forked.value.ok, forked.value.text()
        receipt = forked.value.json()
        uid = receipt['uid']
        assert uid.startswith('opencode:') and '~' in uid and uid != parent, receipt
        assert receipt['session']['uid'] == uid, receipt
        assert receipt['session_id'] in native.sessions
        page.wait_for_function('uid => S.sel === uid', arg=uid)
        expect(page.locator('#sdk-input')).to_be_enabled()
        page.locator('#sdk-input').fill('Hub fork native input')
        page.locator('#sdk-send').click()
        expect(page.locator('.sdk-live')).to_contain_text('native reply completed: Hub fork native input', timeout=20000)
        assert any(row.get('text') == 'Hub fork native input' for row in native.messages[receipt['session_id']])
        assert not any(row.get('text') == 'Hub fork native input' for row in native.messages[SID])
        count = native.forks
        repeated = page.request.post(base + '/api/native-sdk/fork', data=forked.value.request.post_data_json)
        assert repeated.ok and repeated.json()['uid'] == uid, repeated.text()
        assert repeated.json()['session']['uid'] == uid and native.forks == count
        if screenshot:
            page.screenshot(path=str(screenshot.with_name('opencode-hub-fork.png')), full_page=True)
        assert not errors, errors
        print('OpenCode SDK Hub Chromium: fork namespace, scoped reopening/SSE/SEND, successful receipt retry passed', flush=True)
    finally:
        context.close()
        hub.stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path, help='also validate native fork namespace through the Hub')
    parser.add_argument('--host-binary', type=Path, default=BINARY.parent / 'ptyhost')
    parser.add_argument('--screenshot', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-opencode-sdk-') as temporary:
        root = Path(temporary)
        for name in ('claude', 'codex', 'grok', 'host', 'state', 'proc'):
            (root / name).mkdir()
        (root / 'proc' / 'stat').write_text('btime 1700000000\n')
        with FakeOpenCode(root) as native:
            launcher = root / 'launcher.json'
            launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(args.host_binary.resolve(strict=True)),
                                            'host_dir': str(root / 'host'), 'profiles': [native.profile()]}))
            initialize('--initialize-lifecycle', root / 'ledger', args.binary.resolve(strict=True))
            env = {'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_OPENCODE_DB': str(native.db),
                   'SESSIONDOCK_OPENCODE_ROOT': str(root / 'mirror'),
                   'SESSIONDOCK_PROC_ROOT': str(root / 'proc')}
            node_port = free_port() if args.hub_binary else None
            if node_port:
                env.update(node_env(root, node_port, '127.0.0.0/8'))
            with isolated_server(Corpus(root), args.binary, host_dir=root / 'host',
                                 state_dir=root / 'state', lifecycle_dir=root / 'ledger', launcher_config=launcher,
                                 extra_env=env) as (base, _), sync_playwright() as pw:
                options = {'headless': True}
                if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                    options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                browser = pw.chromium.launch(**options)
                context = browser.new_context(service_workers='block')
                context.route('**/*', lambda route: route.continue_()
                              if route.request.url.startswith(base + '/') else route.abort())
                page = context.new_page()
                page.on('response', lambda response: print('SDK HTTP error', response.status, response.url, response.text(), flush=True) if '/api/native-sdk/' in response.url and response.status >= 400 else None)
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.on('console', lambda message: print('console',message.type,message.text,flush=True) if message.type=='error' else None)
                assert not native.received, 'SDK initialization must not start a native API request'

                def post(operation, data):
                    response = page.request.post(base + '/api/native-sdk/' + operation, data=data)
                    assert response.ok, (operation, response.status, response.text())
                    return response.json()

                def open_page():
                    page.goto(base, wait_until='domcontentloaded')
                    page.wait_for_function('sid => S.sessions.some(s => s.sid === sid)', arg=SID)
                    uid = page.evaluate('sid => S.sessions.find(s => s.sid === sid).uid', SID)
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    page.locator('#a-sdk').click()
                    expect(page.locator('#sdk-input')).to_be_visible()
                    return uid

                def listen(uid):
                    page.evaluate('''uid => {
                        window.sdkProbe?.close(); window.sdkSnapshots = [];
                        window.sdkProbe = new EventSource('/api/native-sdk/events?uid=' + encodeURIComponent(uid));
                        window.sdkProbe.addEventListener('sdk', event => {
                            const value = JSON.parse(event.data);
                            if (value.type === 'snapshot') window.sdkSnapshots.push(value);
                        });
                    }''', uid)
                    page.wait_for_function('() => window.sdkSnapshots.length > 0')

                def snapshot(predicate):
                    script = '() => { const s = window.sdkSnapshots.at(-1); return s && (' + predicate + ')(s.raw) ? s : null; }'
                    wait_for_async(page, script, timeout=20000, polling=50)
                    return page.evaluate(script)

                def send(text):
                    page.locator('#sdk-input').fill(text)
                    page.locator('#sdk-send').click()

                def action(method):
                    page.locator('#sdk-more').click()
                    page.locator(f'[data-sdk-action="{method}"]').click()

                def return_to_seed():
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    if not page.locator('#sdk-input').is_visible():
                        page.locator('#a-sdk').click()
                    listen(uid)

                try:
                    uid = open_page()
                    listen(uid)
                    expect(page.locator('#sdk-model option[value="synthetic/catalog-tail"]')).to_have_count(1)
                    print('OpenCode SDK: native catalog beyond 600 KB pipe boundary fully reaches browser picker', flush=True)
                    page.locator('#sdk-model').select_option('synthetic/fake-opencode')
                    page.locator('#sdk-effort').select_option('high')
                    expect(page.locator('#sdk-send')).to_be_enabled()
                    assert native.sessions[SID]['model']['variant'] == 'high'
                    assert not any(path.endswith('/prompt') for _,path,_ in native.received), 'selection submitted a prompt'
                    send('first SDK turn')
                    try:
                        snapshot("r => r.items.some(m => m.content === 'partial native reply')")
                    except Exception:
                        print('DEBUG',page.locator('.sdk-status').inner_text(),errors,native.received[-12:],flush=True)
                        raise
                    expect(page.locator('.sdk-live')).to_contain_text('partial native reply')
                    native.stream_release.set()
                    assert native.sessions[SID]['model']['variant'] == 'high'
                    final = snapshot("r => !r.running && r.items.some(m => m.content === 'native reply completed: first SDK turn')")
                    assert any(m.get('error') == 'synthetic tool error' and
                               m.get('input') == {'path': 'synthetic-file'} for m in final['raw']['items'])

                    # Native totals and estimated latest-provider context remain
                    # distinct; opening a detail menu exercises the actual UI.
                    assert final['raw']['usage']['tokens']['total_tokens'] == 2100
                    assert final['raw']['usage']['tokens']['input'] == 1200
                    assert final['raw']['usage']['tokens']['cache']['read'] == 600
                    assert final['raw']['usage']['cost_usd'] == 0.0125
                    assert final['raw']['usage']['context']['used_tokens'] == 2100
                    toggle_usage_details(page)
                    expect(page.locator('#native-usage-detail')).to_contain_text('1,200')
                    expect(page.locator('#native-usage-detail')).to_contain_text('0.0125')
                    expect(page.locator('#native-usage-detail')).to_contain_text('上下文')
                    toggle_usage_details(page)

                    # Queue entries are native durable inbox items. Steering only
                    # changes delivery, while cancel removes undelivered input.
                    send('slow queue blocker')
                    snapshot("r => r.running && r.items.some(m => m.content === 'partial native reply')")
                    with native.lock:
                        native.inbox[SID].append({'id': 'msg_shell_steer', 'sessionID': SID,
                                                  'type': 'synthetic', 'delivery': 'steer',
                                                  'payload': {'text': 'Native shell output waiting for guidance'}})
                    send('queue cancel from browser')
                    queued = snapshot("r => r.queue?.some(q => q.text === 'queue cancel from browser')")
                    entry = next(q for q in queued['raw']['queue'] if q['text'] == 'queue cancel from browser')
                    assert entry['delivery'] == 'queue'
                    shell_card = page.locator('.sdk-queue [data-queue-id="msg_shell_steer"]')
                    expect(shell_card).to_contain_text('等待引导')
                    expect(shell_card.get_by_role('button', name='立即引导', exact=True)).to_have_count(0)
                    expect(shell_card.get_by_role('button', name='撤销', exact=True)).to_be_enabled()
                    if args.screenshot:
                        args.screenshot.parent.mkdir(parents=True, exist_ok=True)
                        for width, theme in ((1280, 'light'), (390, 'dark')):
                            page.set_viewport_size({'width': width, 'height': 900})
                            page.emulate_media(color_scheme=theme)
                            if width < 500 and not page.locator('#sdk-input').is_visible():
                                page.locator(f'#side .item[data-uid="{uid}"]').click()
                            expect(page.locator('.sdk-queue')).to_contain_text('queue cancel from browser')
                            page.screenshot(path=str(args.screenshot.with_name(f'opencode-queue-{width}-{theme}.png')), full_page=True)
                        page.set_viewport_size({'width': 1280, 'height': 900})
                        page.emulate_media(color_scheme='light')
                    card = page.locator(f'.sdk-queue [data-queue-id="{entry["id"]}"]')
                    shell_card.get_by_role('button', name='撤销', exact=True).click()
                    snapshot("r => !r.queue?.some(q => q.id === 'msg_shell_steer')")
                    card.get_by_role('button', name='撤销', exact=True).click()
                    snapshot('r => r.queue?.length === 0')
                    send('queue steer from browser')
                    queued = snapshot("r => r.queue?.some(q => q.text === 'queue steer from browser')")
                    entry = next(q for q in queued['raw']['queue'] if q['text'] == 'queue steer from browser')
                    card = page.locator(f'.sdk-queue [data-queue-id="{entry["id"]}"]')
                    card.get_by_role('button', name='立即引导', exact=True).click()
                    snapshot("r => r.queue?.some(q => q.delivery === 'steer')")
                    expect(card).to_contain_text('等待引导')
                    expect(card.get_by_role('button', name='立即引导', exact=True)).to_have_count(0)
                    card.get_by_role('button', name='撤销', exact=True).click()
                    snapshot('r => r.queue?.length === 0')
                    page.locator('#sdk-stop').click()
                    snapshot('r => !r.running')
                    assert not any(row.get('text', '').startswith('queue ') for row in native.messages[SID])

                    # Same request_id while running admits exactly one native input.
                    post('turn', {'uid': uid, 'text': 'slow', 'request_id': 'dedup-inflight',
                                  'model': 'synthetic/fake-opencode', 'effort': 'low'})
                    post('turn', {'uid': uid, 'text': 'slow', 'request_id': 'dedup-inflight'})
                    snapshot('r => r.running')
                    page.locator('#sdk-stop').click()
                    stopped = snapshot('r => !r.running')
                    assert stopped['raw']['model']['variant'] == 'low'
                    prompts = [r for r in native.received if r[0] == 'POST' and r[1].endswith('/prompt') and r[2]['text'] == 'slow']
                    assert len(prompts) == 1, prompts
                    assert native.sessions[SID]['model']['variant'] == 'low'

                    send('approval')
                    snapshot("r => r.requests.some(q => q.key === 'permission:permission_native')")
                    # Navigating away drops both browser SSE subscribers. Native
                    # work and the permission must remain until an explicit reply.
                    page.goto('about:blank')
                    uid = open_page()
                    listen(uid)
                    pending = snapshot("r => r.requests.some(q => q.type === 'approval')")
                    assert pending['raw']['running']
                    page.locator('.sdk-request').get_by_role('button',name='允许一次',exact=True).click()
                    snapshot("r => !r.running && r.items.some(m => m.content === 'native reply completed: approval')")
                    replies = [r for r in native.received if '/permission/' in r[1] and r[1].endswith('/reply')]
                    assert len(replies) == 1 and replies[0][2] == {'decision': 'once'}

                    send('question')
                    snapshot("r => r.requests.some(q => q.type === 'question')")
                    page.locator('.sdk-request').get_by_label('Second',exact=True).check()
                    page.locator('.sdk-request').get_by_role('button',name='提交',exact=True).click()
                    snapshot("r => !r.running && r.items.some(m => m.content === 'native reply completed: question')")
                    form_reply = next(r for r in native.received if '/form/' in r[1] and r[1].endswith('/reply'))
                    assert form_reply[2] == {'answer': {'choice': 'second'}}

                    send('error')
                    snapshot("r => !r.running && r.items.some(m => m.content === 'synthetic provider failure')")

                    # Image uploads follow the same text-path contract as ordinary files.
                    page.locator('#sdk-add').click()
                    with page.expect_file_chooser() as chooser:
                        page.locator('#sdk-attach-menu [data-attach="image"]').click()
                    chooser.value.set_files({'name': 'sdk-image.png', 'mimeType': 'image/png',
                                            'buffer': base64.b64decode(PNG)})
                    expect(page.locator('#sdk-compose-items')).to_contain_text('sdk-image.png')
                    send('image attachment')
                    snapshot("r => !r.running && r.items.some(m => m.content?.startsWith('native reply completed: image attachment'))")
                    image_prompt = next(r[2] for r in native.received if r[1].endswith('/prompt')
                                        and r[0] == 'POST' and r[2]['text'].startswith('image attachment'))
                    assert 'files' not in image_prompt and 'images' not in image_prompt, image_prompt
                    assert 'sdk-image.png' in image_prompt['text'] and 'sessiondock_attachments' in image_prompt['text']
                    assert list(native.cwd.rglob('sdk-image.png'))

                    page.locator('#sdk-more').click()
                    page.locator('#native-sdk-native_agent').select_option('plan')
                    snapshot("r => r.agent === 'plan'")
                    page.locator('#sdk-more').click()
                    action('permissions')
                    permissions = [{'action': 'shell', 'resource': '*', 'effect': 'ask'}]
                    page.locator('textarea[aria-label="权限规则 JSON"]').fill(json.dumps(permissions))
                    page.locator('#sdk-action-dialog button[type="submit"]').click()
                    snapshot("r => r.permissions?.some(rule => rule.action === 'shell' && rule.effect === 'ask')")
                    assert native.sessions[SID]['permissions'] == permissions
                    action('command')
                    page.locator('select[aria-label="原生命令"]').select_option('synthetic-command')
                    page.locator('textarea[aria-label="命令参数"]').fill('browser parameters')
                    page.locator('#sdk-action-dialog button[type="submit"]').click()
                    snapshot("r => r.items.some(m => m.content === 'native command: synthetic-command browser parameters')")
                    command = [r for r in native.received if r[0] == 'POST' and r[1].endswith('/command')][-1]
                    assert command[2] == {'name': 'synthetic-command', 'text': 'browser parameters', 'delivery': 'queue'}

                    action('compact')
                    snapshot("r => r.items.some(m => m.content === 'OpenCode 上下文压缩完成') && r.usage.context === null")
                    compaction = [r for r in native.received if r[0] == 'POST' and r[1].endswith('/compact')]
                    assert compaction[-1][2]['delivery'] == 'steer'
                    action('shell')
                    page.locator('textarea[aria-label="命令"]').fill('printf synthetic-shell')
                    page.locator('#sdk-action-dialog button[type="submit"]').click()
                    snapshot("r => r.items.some(m => m.tool_name === 'shell' && m.output === 'synthetic shell output')")
                    assert not any(r[0] == 'POST' and r[1].endswith('/prompt') and 'printf synthetic-shell' in r[2]['text'] for r in native.received)

                    # Revert is reversible and conversation-only. It stages a
                    # native message boundary rather than editing transcript files.
                    send('revert this message')
                    before_revert = snapshot("r => !r.running && r.items.some(m => m.content === 'native reply completed: revert this message')")
                    boundary = next(m['native_id'] for m in before_revert['raw']['items'] if m.get('role') == 'user' and m.get('content') == 'revert this message')
                    action('revert')
                    page.locator('select[aria-label="回退消息"]').select_option(boundary)
                    page.locator('#sdk-action-dialog button[type="submit"]').click()
                    snapshot('r => !!r.revert?.messageID')
                    staged = [r for r in native.received if r[1].endswith('/revert/stage')][-1]
                    assert staged[2] == {'messageID': boundary, 'files': False}
                    action('unrevert')
                    snapshot('r => r.revert === null')

                    # Native full-history and per-message forks receive their
                    # own identity and must not steal the source SID.
                    action('fork')
                    page.wait_for_function('uid => S.sel !== uid && S.sel?.startsWith("opencode:")', arg=uid)
                    assert native.forks == 1
                    return_to_seed()
                    send('fork before this message')
                    snapshot("r => !r.running && r.items.some(m => m.content === 'native reply completed: fork before this message')")
                    bubble = page.locator('.sdk-live .msg').filter(has=page.locator('.mb')).filter(has_text='fork before this message').filter(has=page.locator('.sdk-bubble-toggle')).last
                    bubble.get_by_role('button', name='消息操作', exact=True).click()
                    page.get_by_role('button', name='从此处之前分支', exact=True).click()
                    page.wait_for_function('uid => S.sel !== uid && S.sel?.startsWith("opencode:")', arg=uid)
                    assert native.forks == 2
                    forks = [r for r in native.received if r[0] == 'POST' and r[1].endswith('/fork')]
                    assert forks[0][2] == {} and forks[1][2].get('before')
                    return_to_seed()

                    send('commit this conversation boundary')
                    before_commit = snapshot("r => !r.running && r.items.some(m => m.content === 'native reply completed: commit this conversation boundary')")
                    boundary = next(m['native_id'] for m in before_commit['raw']['items'] if m.get('role') == 'user' and m.get('content') == 'commit this conversation boundary')
                    action('revert')
                    page.locator('select[aria-label="回退消息"]').select_option(boundary)
                    page.locator('#sdk-action-dialog button[type="submit"]').click()
                    snapshot('r => !!r.revert?.messageID')
                    action('revert_commit')
                    page.get_by_role('dialog').get_by_role('button', name='提交回退', exact=True).click()
                    snapshot('r => r.revert === null')
                    assert all(record['id'] != boundary for record in native.messages[SID])
                    assert any(r[0] == 'POST' and r[1].endswith('/revert/commit') for r in native.received)

                    # A native operation can succeed while its CLI loses the
                    # HTTP acknowledgement. Same-ID retries must not execute it
                    # again, including fork which has no native request-ID slot.
                    native.lose_ack = 'shell'
                    action('shell')
                    page.locator('textarea[aria-label="命令"]').fill('lost-ack-shell')
                    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/shell')) as lost:
                        page.locator('#sdk-action-dialog button[type="submit"]').click()
                    assert not lost.value.ok
                    shell_retry = lost.value.request.post_data_json
                    retry = page.request.post(base + '/api/native-sdk/shell', data=shell_retry)
                    assert not retry.ok and '不会重复' in retry.text()
                    native_shells = [r for r in native.received if r[0] == 'POST' and r[1].endswith('/shell')
                                     and r[2]['command'] == 'lost-ack-shell']
                    assert len(native_shells) == 1, native_shells
                    native.lose_ack = 'fork'
                    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/fork')) as lost:
                        action('fork')
                    assert not lost.value.ok
                    forks_before_retry = native.forks
                    fork_retry = lost.value.request.post_data_json
                    retry = page.request.post(base + '/api/native-sdk/fork', data=fork_retry)
                    assert not retry.ok and '不会重复' in retry.text()
                    assert native.forks == forks_before_retry

                    created = post('create', {'source': 'opencode', 'cwd': str(native.cwd),
                                             'request_id': 'create-native', 'model': 'synthetic/fake-opencode', 'effort': 'high'})
                    again = post('create', {'source': 'opencode', 'cwd': str(native.cwd), 'request_id': 'create-native'})
                    assert again['session_id'] == created['session_id']
                    assert created['session']['source'] == 'opencode'
                    assert created['uid'].startswith('opencode:') and created['cwd'] == str(native.cwd)
                    assert created['capabilities']['discard'] is False
                    post('open', {'uid': created['uid']})
                    models = post('models', {'uid': created['uid']})
                    assert any(m['id'] == 'synthetic/fake-opencode' for m in models['models'])
                    assert not any(r[1].startswith('/api/session/') and r[1].count('/') == 3 and r[0] == 'DELETE' for r in native.received)

                    # Linux's synthetic proc tree gives reproducible independent
                    # TUI conflict evidence without starting a second real CLI.
                    proc = root / 'proc' / '12345'
                    proc.mkdir()
                    (proc / 'cmdline').write_bytes(b'opencode\0--standalone\0--session\0' + SID.encode() + b'\0')
                    (proc / 'cwd').symlink_to(native.cwd)
                    send('must not reach native')
                    page.wait_for_function("NativeSdkView.raw.input_queue?.some(row=>row.text==='must not reach native' && row.state==='failed')")
                    refused = page.evaluate("NativeSdkView.raw.input_queue.find(row=>row.text==='must not reach native')")
                    assert refused['error'], refused
                    assert not any(r[0] == 'POST' and r[1].endswith('/prompt') and
                                   r[2]['text'] == 'must not reach native' for r in native.received)
                    (proc / 'cmdline').unlink()
                    (proc / 'cwd').unlink()
                    proc.rmdir()
                    if args.screenshot:
                        args.screenshot.parent.mkdir(parents=True, exist_ok=True)
                        for width in (1280,390,320):
                            page.set_viewport_size({'width':width,'height':900})
                            page.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
                            expect(page.locator('#sdk-input:visible, #side .item.sel:visible').first).to_be_visible()
                            if width < 500 and not page.locator('#sdk-input').is_visible():
                                selected = page.evaluate('S.sel')
                                row=page.locator(f'#side .item[data-uid="{selected}"]')
                                if row.is_visible():row.click()
                                if not page.locator('#sdk-input').is_visible():page.locator('#a-sdk').click()
                            expect(page.locator('#sdk-input')).to_be_visible()
                            for theme in ('light','dark'):
                                page.emulate_media(color_scheme=theme)
                                assert_inline_usage(page)
                                page.screenshot(path=str(args.screenshot.with_name(f'opencode-{width}-{theme}.png')),full_page=True)
                                for toggle, label in (('#sdk-usage-toggle', 'usage'), ('#sdk-more', 'actions')):
                                    if label == 'usage': toggle_usage_details(page)
                                    else: page.locator(toggle).click()
                                    page.screenshot(path=str(args.screenshot.with_name(f'opencode-{width}-{theme}-{label}.png')), full_page=True)
                                    if label == 'usage': toggle_usage_details(page)
                                    else: page.locator(toggle).click()
                    if args.hub_binary:
                        exercise_hub_fork(browser, native, node_port, args.hub_binary, root, args.screenshot)
                    assert not errors, errors
                    print('OpenCode SDK Chromium: native lifecycle, streaming, tools/errors, dedup/interrupt, queue cancel/steer, usage/context, image upload, compact/shell and lost-ACK safety, reversible conversation revert, full/message forks, approvals/forms and writer conflict passed')
                except Exception:
                    print('OpenCode browser diagnostics', page.locator('.sdk-status').all_text_contents(),
                          errors, native.received[-16:], flush=True)
                    if args.screenshot:
                        args.screenshot.parent.mkdir(parents=True, exist_ok=True)
                        page.screenshot(path=str(args.screenshot.with_name('opencode-failure.png')), full_page=True)
                    raise
                finally:
                    context.close()
                    browser.close()


if __name__ == '__main__':
    main()
