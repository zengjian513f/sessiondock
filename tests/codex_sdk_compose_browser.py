#!/usr/bin/env python3
"""SDK drafts and staged attachments through real node/Hub browser flows.

Synthetic app-server only: create without a native rollout, edit/upload/reload,
switch views and sessions, paste/drop/select, remove, fail SEND and retry. The
existing private conversation store and resumable upload routes own all bytes.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, REPO
from hub_fixtures import Hub, free_port
from media_browser import PNG
from new_session_model_browser import open_dialog
from node_auth_fixtures import TOKEN, node_env
from private_hosts import private_hosts
from path_completion_fixtures import create_paths, exercise_paths


def saved(page, uid, text):
    page.wait_for_function("""([uid,text]) => {
      const draft = composerDrafts.get(composerDraftOwner(uid));
      return draft?.text === text && !draft.loading && !draft.storageError
        && draft.editVersion === draft.savedVersion
        && draft.attachments.every(item => item.uploaded?.upload_id);
    }""", arg=[uid, text])


def attach(page, kind, name, mime, data):
    page.locator('#sdk-add').click()
    with page.expect_file_chooser() as chooser:
        page.locator(f'#sdk-attach-menu [data-attach="{kind}"]').click()
    chooser.value.set_files({'name': name, 'mimeType': mime, 'buffer': data})


def exercise(browser, base, fake, root, width, theme, screenshots, label):
    print(f'{label}: SDK draft/upload recovery, isolation and SEND', flush=True)
    context = browser.new_context(viewport={'width': width, 'height': 900 if width > 500 else 844},
                                  color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    errors, uploads = [], []
    page.on('pageerror', lambda error: errors.append(error.stack))
    page.on('request', lambda request: uploads.append(request.url) if request.method == 'POST'
            and urlsplit(request.url).path.endswith('/api/session/conversation/attachment') else None)
    page.set_default_timeout(15000)
    try:
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        open_dialog(page)
        page.locator('input[name="new-source"][value="codex"]').check()
        work = root / f'{label}-工作目录-with-a-realistic-project-name'
        work.mkdir()
        create_paths(work)
        page.locator('#new-cwd').fill(str(work))
        page.locator('#new-session-go').click()
        expect(page.locator('#sdk-input')).to_be_enabled()
        expect(page.locator('#sdk-add')).to_be_visible()
        uid = page.evaluate('S.sel')
        editor = page.locator('#sdk-input')
        editor.fill('查看 ~/home-n')
        editor.press('Tab')
        expect(editor).to_have_value('查看 ~/home-note.txt')
        exercise_paths(page, editor, work, screenshots / f'{label}-paths.png' if screenshots else None)
        saved(page, uid, '请检查 ./readme-one.md')
        page.reload(wait_until='domcontentloaded')
        expect(editor).to_have_value('请检查 ./readme-one.md')
        assert not fake.methods('thread/shellCommand'), 'completion or reload executed shell'
        rollout = fake.rollout
        assert not rollout.exists(), 'SDK create wrote a native history before SEND'
        text = f'approve {label} 请检查图片和任意文件，保留这份尚未发送的草稿。'
        page.locator('#sdk-input').fill(text)
        attach(page, 'image', '屏幕截图-with-realistic-long-name.png', 'image/png', base64.b64decode(PNG))
        attach(page, 'file', 'analysis-input-研究笔记.txt', 'text/plain', b'private fixture document\n')
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(2)
        expect(page.locator('#sdk-compose-items .draft-thumb img')).to_have_count(1)
        saved(page, uid, text)
        assert not rollout.exists(), 'draft/upload must not create native history'
        assert not list(work.rglob('sessiondock_attachments')), 'staging published into cwd before SEND'
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-draft.png'))

        # Reload reads actual server drafts and staged image bytes. It must not
        # re-upload a previously acknowledged attachment or send any message.
        before_uploads = len(uploads)
        page.reload(wait_until='domcontentloaded')
        expect(page.locator('#sdk-input')).to_have_value(text)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(2)
        expect(page.locator('#sdk-compose-items .draft-thumb img')).to_have_count(1)
        assert not rollout.exists() and len(uploads) == before_uploads
        assert not fake.methods('turn/start'), 'restoring a draft triggered SEND'

        print(f'{label}: persisted reload passed; following draft from another page', flush=True)
        follower_context = browser.new_context(viewport={'width':width, 'height':844 if width < 500 else 900},
                                               color_scheme=theme, service_workers='block')
        follower_context.route('**/*', lambda route: route.continue_()
            if route.request.url.startswith(base + '/') else route.abort())
        follower = follower_context.new_page()
        follower.goto(page.url, wait_until='domcontentloaded')
        follower.wait_for_function('T.listLoaded')
        expect(follower.locator('#sdk-input')).to_have_value(text)
        assert follower.evaluate('S.sel') == uid, 'fresh shared URL selected another session'
        text += '\n另一页面继续编辑。'
        follower.locator('#sdk-input').fill(text)
        saved(follower, uid, text)
        expect(page.locator('#sdk-input')).to_have_value(text)
        follower_context.close()

        print(f'{label}: shared draft follow passed; checking views and session isolation', flush=True)
        # The retained transitional view shares this editor's exact draft.
        page.locator('#a-chat').click()
        page.locator('#a-sdk').click()
        expect(page.locator('#sdk-input')).to_have_value(text)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(2)

        print(f'{label}: view/session isolation passed; clipboard/drop/quotes', flush=True)
        # Switch to the earlier native session. Its draft must remain empty;
        # reopening this pending session recovers its own files and text.
        if width < 500:
            page.locator('#detail .mobile-back').click()
        earlier = page.locator('#side .item').filter(has_text='earlier synthetic question')
        earlier.click()
        expect(page.locator('#sdk-input')).to_have_value('')
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(0)
        if width < 500:
            page.locator('#detail .mobile-back').click()
        page.locator(f'#side .item[data-uid="{uid}"]').click()
        expect(page.locator('#sdk-input')).to_have_value(text)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(2)

        # Browser paste/drop events reach the same handlers as desktop actions.
        # DataTransfer is constructed only to supply the browser clipboard and
        # drag payload; handlers and real streaming uploads remain unchanged.
        page.locator('#sdk-input').focus()
        page.locator('#sdk-input').evaluate("""(input, png) => {
          const data = new DataTransfer();
          const bytes = Uint8Array.from(atob(png), c => c.charCodeAt(0));
          data.items.add(new File([bytes], 'pasted-clipboard.png', {type:'image/png'}));
          input.dispatchEvent(new ClipboardEvent('paste', {clipboardData:data, bubbles:true, cancelable:true}));
        }""", PNG)
        page.locator('#sdk-composer').evaluate("""(zone) => {
          const data = new DataTransfer();
          data.items.add(new File(['dropped fixture'], 'dropped-project-file.csv', {type:'text/csv'}));
          zone.dispatchEvent(new DragEvent('drop', {dataTransfer:data, bubbles:true, cancelable:true}));
        }""")
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(4)
        saved(page, uid, text)
        removed = page.locator('#sdk-compose-items .draft-card').filter(has_text='pasted-clipboard.png')
        removed.get_by_role('button', name='移除附件', exact=True).click()
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(3)
        saved(page, uid, text)
        page.locator('#sdk-add').click()
        page.locator('#sdk-attach-menu [data-attach="quote"]').click()
        page.get_by_role('textbox', name='引用文字').fill('quoted research context from the shared draft')
        saved(page, uid, text)

        print(f'{label}: clipboard/drop/quotes passed; failed SEND and retained edits', flush=True)
        # Failed API calls keep all draft content and staged uploads. The next
        # user click is the only retry; it publishes the acknowledged uploads.
        page.route('**/api/codex-sdk/turn', lambda route: route.fulfill(status=503,
            content_type='application/json', body=json.dumps({'ok':False, 'error':'fixture unavailable'})), times=1)
        page.locator('#sdk-send').click()
        expect(page.locator('#sdk-status-menu')).to_contain_text('fixture unavailable')
        expect(page.locator('#sdk-input')).to_have_value(text)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(4)
        assert not rollout.exists()
        page.evaluate("""() => {
          window.composeFetch = window.fetch;
          window.fetch = async (...args) => {
            const response = await composeFetch(...args);
            if (String(args[0]).includes('/api/codex-sdk/turn') && args[1]?.method === 'POST')
              await new Promise(resolve => { window.releaseComposeSend = resolve; });
            return response;
          };
        }""")
        page.locator('#sdk-send').click()
        expect(page.locator('.sdk-request')).to_contain_text('需要批准执行命令')
        expect(page.locator('.sdk-queued')).to_have_count(0)
        params = fake.methods('turn/start')[-1]['params']
        native_input = params['input']
        assert all(item['type'] == 'text' for item in native_input), native_input
        assert list(work.rglob('屏幕截图-with-realistic-long-name.png'))
        prompt = '\n'.join(item.get('text', '') for item in native_input)
        assert '屏幕截图-with-realistic-long-name.png' in prompt and 'sessiondock_attachments' in prompt
        assert text in prompt and 'analysis-input-' in prompt and 'dropped-project-file.csv' in prompt, prompt
        assert 'pasted-clipboard.png' not in prompt, prompt
        assert 'quoted research context from the shared draft' in prompt, prompt
        new_text = '发送期间继续键入的下一条草稿应保留'
        page.locator('#sdk-input').fill(new_text)
        saved(page, uid, new_text)
        page.wait_for_function("() => typeof window.releaseComposeSend === 'function'")
        page.evaluate('() => { window.fetch = composeFetch; releaseComposeSend(); }')
        expect(page.locator('#sdk-input')).to_have_value(new_text)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(0)
        page.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
        expect(page.locator('.sdk-history')).to_contain_text('All done after approval')
        saved(page, uid, new_text)
        print(f'{label}: SEND retained new input; reload identity:',
              page.evaluate('() => ({uid:S.sel, stored:store.get("sel", ""), url:location.href})'), flush=True)
        page.reload(wait_until='domcontentloaded')
        expect(page.locator('#sdk-input')).to_have_value(new_text)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(0)

        # A pure attachment message is accepted without invented user text.
        page.locator('#sdk-input').fill('')
        attach(page, 'file', 'only-attachment.txt', 'text/plain', b'attachment-only request\n')
        saved(page, uid, '')
        before = len(fake.methods('turn/start'))
        with page.expect_response(lambda response: urlsplit(response.url).path.endswith('/api/codex-sdk/turn')) as sent:
            page.locator('#sdk-send').click()
        assert sent.value.status == 200, sent.value.text()
        fake.wait_for(lambda: len(fake.methods('turn/start')) == before + 1)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(0)
        expect(page.locator('#sdk-stop')).to_be_visible()
        page.locator('#sdk-stop').click()
        expect(page.locator('#sdk-stop')).to_be_hidden()
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-sent.png'))
        assert not errors, errors
    except BaseException:
        print('compose failure:', page.evaluate("""() => ({uid:S.sel, stored:store.get('sel', ''), url:location.href,
          notices:document.querySelector('#sdk-composer')?.innerText,
          draft:composerDrafts.get(composerDraftOwner(S.sel)) && {
            error:composerDrafts.get(composerDraftOwner(S.sel)).storageError,
            edits:composerDrafts.get(composerDraftOwner(S.sel)).editVersion,
            saved:composerDrafts.get(composerDraftOwner(S.sel)).savedVersion}})"""), flush=True)
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-failure.png'))
        raise
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    hub_binary = args.hub_binary or args.binary.with_name('sessiondock-hub')
    with tempfile.TemporaryDirectory(prefix='sd-sdk-compose-') as temporary:
        root = Path(temporary)
        corpus = build(root)
        for name in ('state', 'host', 'ledger', 'proc'):
            (root / name).mkdir(mode=0o700)
        (root/'proc/stat').write_text('btime 1700000000\n')
        config = root / 'launcher.json'
        config.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO/'target/debug/ptyhost'),
            'host_dir': str(root/'host'), 'profiles': [{'id': 'codex-cli', 'source': 'codex',
            'executable': sys.executable, 'args': [str(REPO/'tests/fake_model_cli.py')]}]}))
        config.chmod(0o600)
        subprocess.run([str(args.binary.resolve()), '--initialize-lifecycle', str(root/'ledger')],
                       check=True, capture_output=True, timeout=20)
        port = free_port()
        (root / 'home').mkdir()
        (root / 'home' / 'home-note.txt').write_text('home fixture')
        environment = {'HOME': str(root / 'home'), **node_env(root, port, '127.0.0.0/8'),
            'SESSIONDOCK_STATE_DIR': str(root/'state'), 'SESSIONDOCK_PTYHOST_DIR': str(root/'host'),
            'SESSIONDOCK_LIFECYCLE_DIR': str(root/'ledger'), 'SESSIONDOCK_LAUNCHER_CONFIG': str(config),
            'SESSIONDOCK_PROC_ROOT': str(root/'proc'),
            'SESSIONDOCK_FILE_ROOTS': str(root), 'SESSIONDOCK_FILE_WRITE_ROOTS': str(root)}
        with private_hosts(root), FakeAppServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake:
            with server(corpus, args.binary, root/'app.sock', environment) as base:
                hub_root = root/'hub'; hub_root.mkdir()
                hub = Hub(hub_binary.resolve(), hub_root, [SimpleNamespace(name='ComposeNode', token=TOKEN, port=port)])
                hub.start()
                try:
                    with sync_playwright() as pw:
                        options = {'headless': True}
                        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                        browser = pw.chromium.launch(**options)
                        try:
                            for mode, url, width, theme in [('node', base, 1280, 'light'),
                                    ('node', base, 390, 'dark'), ('hub', f'http://127.0.0.1:{hub.port}', 1280, 'dark'),
                                    ('hub', f'http://127.0.0.1:{hub.port}', 390, 'light')]:
                                # Each scripted server owns its current synthetic
                                # thread. Old native sessions stay in inventory.
                                fake.received.clear()
                                exercise(browser, url, fake, root, width, theme, args.screenshots,
                                         f'{mode}-{width}-{theme}')
                        finally:
                            browser.close()
                finally:
                    hub.stop()
    print('PASS SDK compose: node/Hub desktop/mobile light/dark, drafts, staged image/files, '
          'reload/view/session isolation, paste/drop/remove, failed SEND retry and attachment-only SEND')


if __name__ == '__main__':
    main()
