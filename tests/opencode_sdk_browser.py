#!/usr/bin/env python3
"""OpenCode native SDK through Chromium and a synthetic v2 native API.

Requires the shared native-sdk facade/UI integration and a built SessionDock
binary. No real OpenCode, provider, credentials, model call, or production home.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from browser_runtime import wait_for_async
from history_fixtures import BINARY, Corpus, isolated_server
from opencode_sdk_fixtures import FakeOpenCode, SID
from send_browser import initialize


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
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

                try:
                    uid = open_page()
                    listen(uid)
                    page.locator('#sdk-model').select_option('synthetic/fake-opencode')
                    page.locator('#sdk-effort').select_option('high')
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
                    assert not any(r[1].startswith('/api/session/') and r[0] == 'DELETE' for r in native.received)

                    # Linux's synthetic proc tree gives reproducible independent
                    # TUI conflict evidence without starting a second real CLI.
                    proc = root / 'proc' / '12345'
                    proc.mkdir()
                    (proc / 'cmdline').write_bytes(b'opencode\0--standalone\0--session\0' + SID.encode() + b'\0')
                    (proc / 'cwd').symlink_to(native.cwd)
                    refused = page.request.post(base + '/api/native-sdk/turn', data={
                        'uid': uid, 'text': 'must not reach native', 'request_id': 'independent-writer'})
                    assert not refused.ok
                    assert not any(r[0] == 'POST' and r[1].endswith('/prompt') and
                                   r[2]['text'] == 'must not reach native' for r in native.received)
                    (proc / 'cmdline').unlink()
                    (proc / 'cwd').unlink()
                    proc.rmdir()
                    if args.screenshot:
                        args.screenshot.parent.mkdir(parents=True, exist_ok=True)
                        for width in (1280,390,320):
                            page.set_viewport_size({'width':width,'height':900})
                            if width < 500 and not page.locator('#sdk-input').is_visible():
                                selected = page.evaluate('S.sel')
                                page.locator(f'#side .item[data-uid="{selected}"]').click()
                            expect(page.locator('#sdk-input')).to_be_visible()
                            for theme in ('light','dark'):
                                page.emulate_media(color_scheme=theme)
                                page.screenshot(path=str(args.screenshot.with_name(f'opencode-{width}-{theme}.png')),full_page=True)
                    assert not errors, errors
                    print('OpenCode SDK Chromium: native create/resume, streaming, tools, errors, dedup, interrupt, reconnect, approval/question and writer conflict passed')
                finally:
                    context.close()
                    browser.close()


if __name__ == '__main__':
    main()
