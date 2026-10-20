#!/usr/bin/env python3
"""Operator-only real Kiro SDK/CLI continuity through Chromium.

Requires an explicitly supplied, already authenticated temporary environment.
No credentials/configuration are copied. The model must be explicitly selected
as auto (the operator-authorized model for this acceptance). --prepare-only
opens the model picker without creating a native session or sending a prompt.
The default validation sweep excludes this *_real_browser suite.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import socket
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import server
from history_fixtures import BINARY
from kiro_sdk_browser import watch
from private_hosts import private_hosts
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize


def stop_from_toolbar(page):
    uid = page.evaluate('S.sel')
    button = page.locator('#a-session-toggle[aria-label="停止会话"]')
    if not button.is_visible():
        page.locator('#a-more').click()
    with page.expect_response(lambda r: r.request.method == 'POST' and r.url.endswith(('/api/session/stop', '/api/native-sdk/stop'))) as response:
        button.click()
        popup = page.locator('dialog.app-popup')
        if popup.is_visible():
            popup.get_by_role('button', name='确定', exact=True).click()
    assert response.value.ok, response.value.text()
    page.wait_for_function('uid => !ConsoleUI.busy.has(uid)', arg=uid)


def exercise(page, base, work, evidence, prepare_only):
    page.set_default_timeout(30000)
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    page.locator('#new-session').click()
    page.locator('label:has(input[name="new-source"][value="kiro"])').click()
    page.locator('#new-model').click()
    option = page.locator('#new-model-options [title="auto"]')
    expect(option).to_be_visible()
    page.screenshot(path=str(evidence / 'real-model-catalog.png'), full_page=True)
    option.click()
    page.locator('#new-cwd').fill(str(work))
    if prepare_only:
        return {'prepared': True, 'model': 'auto', 'prompt_submissions': 0}

    with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/create')) as created:
        page.locator('#new-session-go').click()
    assert created.value.ok, created.value.text()
    receipt = created.value.json()
    uid, sid = receipt['uid'], receipt['session']['sid']
    watch(page, uid)
    page.wait_for_function('window.__kiroSnapshot?.model === "auto" && window.__kiroSnapshot?.configOptions?.some(o => (o.id === "model" || o.category === "model") && o.currentValue === "auto")')
    expect(page.locator('#sdk-model')).to_have_value('auto')
    (evidence / 'native-config.json').write_text(json.dumps(page.evaluate('({model:__kiroSnapshot.model, configOptions:__kiroSnapshot.configOptions, session_id:__kiroSnapshot.session_id})'), indent=2))
    prompt = '你好，可以用一句话说说你能怎样帮我整理一个小项目吗？'
    page.locator('#sdk-input').click()
    page.locator('#sdk-input').fill(prompt)
    page.locator('#sdk-send').click()
    page.wait_for_function('!window.__kiroSnapshot?.running && window.__kiroSnapshot?.items?.some(i => i.role === "assistant" && i.content?.trim())', timeout=120000)
    reply = page.evaluate('window.__kiroSnapshot.items.filter(i => i.role === "assistant" && i.content?.trim()).at(-1).content.trim()')
    assert reply and reply != prompt, 'Expected a nonempty assistant answer, not the user prompt'
    expect(page.locator('.sdk-live')).to_contain_text(reply)
    page.screenshot(path=str(evidence / 'real-sdk-answer.png'), full_page=True)
    stop_from_toolbar(page)

    with page.expect_response(lambda r: r.url.endswith('/api/term/takeover')) as takeover:
        page.locator('#a-term').click()
    assert takeover.value.ok, takeover.value.text()
    terminal_receipt = takeover.value.json()
    assert terminal_receipt.get('ok') and not terminal_receipt.get('needs_confirm'), terminal_receipt
    page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
    # The real native terminal must replay the SDK answer on the same SID.
    page.wait_for_function(r'''reply => Array.from({length:T.term?.buffer?.active?.length||0}, (_,i)=>T.term.buffer.active.getLine(i)?.translateToString(true)||"").join("\n").replace(/\s+/g, '').includes(reply.replace(/\s+/g, ''))''', arg=reply, timeout=60000)
    terminal_text = page.evaluate('Array.from({length:T.term.buffer.active.length}, (_,i)=>T.term.buffer.active.getLine(i)?.translateToString(true)||"").join("\\n")')
    assert 'auto' in terminal_text.lower(), terminal_text
    page.screenshot(path=str(evidence / 'real-cli-resume.png'), full_page=True)
    stop_from_toolbar(page)

    page.locator('#a-sdk').click()
    with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/open')) as opened:
        page.locator('#sdk-input').click()
    assert opened.value.ok, opened.value.text()
    loaded = opened.value.json()
    assert loaded['session_id'] == sid, loaded
    assert any(i.get('role') == 'assistant' and ''.join(reply.split()) == ''.join((i.get('content') or '').split()) for i in loaded['raw']['items']), loaded
    page.screenshot(path=str(evidence / 'real-sdk-replay.png'), full_page=True)
    stop_from_toolbar(page)
    return {'passed': True, 'uid': uid, 'sid': sid, 'model': 'auto', 'reply_excerpt': reply[:240], 'prompt_submissions': 1}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--auth-environment', type=Path, required=True)
    parser.add_argument('--kiro', type=Path, required=True)
    parser.add_argument('--model', choices=['auto'], required=True)
    parser.add_argument('--proxy-environment', action='store_true', help='Pass only standard SSH proxy variables, without saving their values')
    parser.add_argument('--prepare-only', action='store_true')
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    config = json.loads(args.auth_environment.read_text())
    assert config['machine'] == socket.gethostname(), 'Authentication environment belongs to another machine'
    auth_root = Path(config['root']).resolve(strict=True)
    assert auth_root.is_relative_to(Path(tempfile.gettempdir()).resolve()), 'Require an explicitly isolated temporary login'
    env = dict(config['env'])
    for key in ['HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'KIRO_HOME']:
        assert Path(env[key]).resolve().is_relative_to(auth_root), key
    work = Path(config['cwd']).resolve(strict=True)
    assert work.is_relative_to(auth_root)
    kiro = args.kiro.resolve(strict=True)
    env['PATH'] = str(kiro.parent) + os.pathsep + env.get('PATH', '/usr/bin:/bin')
    if args.proxy_environment:
        env.update({k: os.environ[k] for k in ['HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','NO_PROXY','http_proxy','https_proxy','all_proxy','no_proxy'] if k in os.environ})
    evidence = args.screenshots.resolve()
    evidence.mkdir(parents=True, exist_ok=True, mode=0o700)
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-kiro-real-') as temporary, sync_playwright() as playwright:
        root = Path(temporary)
        for name in ['claude','codex','grok','state','trash','host']:
            (root / name).mkdir(mode=0o700)
        initialize('--initialize-lifecycle', root / 'ledger', binary)
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema':2, 'host_binary':str(binary.with_name('ptyhost')), 'host_dir':str(root/'host'), 'profiles':[{'id':'kiro','source':'kiro','label':'Kiro','executable':str(kiro),'args':[], 'new_args':['chat','--agent-engine=v3','--model','auto'], 'resume_args':['chat','--agent-engine=v3','--model','auto','--resume-id','{sid}'], 'env':{k:v for k,v in env.items() if k in config['env']}}]}))
        launcher.chmod(0o600)
        extra = {'SESSIONDOCK_KIRO_ROOT':str(auth_root/'account-scope'), 'SESSIONDOCK_LAUNCHER_CONFIG':str(launcher), 'SESSIONDOCK_NATIVE_SDKS':'auto', 'SESSIONDOCK_STATE_DIR':str(root/'state'), 'SESSIONDOCK_LIFECYCLE_DIR':str(root/'ledger'), 'SESSIONDOCK_PTYHOST_DIR':str(root/'host'), 'SESSIONDOCK_TRASH_DIR':str(root/'trash')}
        chromium_path = playwright.chromium.executable_path
        try:
            with patch.dict(os.environ, env, clear=True), private_hosts(root), server(SimpleNamespace(root=root),binary,'off',extra) as base:
                browser = playwright.chromium.launch(headless=True, executable_path=chromium_path)
                context = browser.new_context(viewport={'width':1280,'height':900},service_workers='block')
                context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base+'/') else route.abort())
                page = context.new_page()
                try:
                    result = exercise(page,base,work,evidence,args.prepare_only)
                    (evidence/'result.json').write_text(json.dumps(result,indent=2))
                    print(json.dumps(result), flush=True)
                except BaseException:
                    page.screenshot(path=str(evidence/'failure.png'),full_page=True)
                    raise
                finally:
                    browser.close()
        finally:
            stop_private_sdkhosts(root)


if __name__ == '__main__':
    main()
