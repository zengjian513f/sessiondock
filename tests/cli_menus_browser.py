#!/usr/bin/env python3
"""Source-derived CLI menus through real Chromium, CHECK and private PTYs.

Fixtures retain native key expectations. The fake CLI accepts exactly those
bytes and records the semantic outcome; it never starts a model request.
"""
import argparse
import json
import os
from pathlib import Path
import re
import select
import sys
import tempfile
import termios
import time
import tty
from urllib.parse import urlsplit


def key_bytes(keys):
    named = {'Enter': '\r', 'Escape': '\x1b', 'Tab': '\t', 'BTab': '\x1b[Z',
             'BSpace': '\x7f', 'Backspace': '\x7f', 'Space': ' ',
             'Home': '\x1b[H', 'End': '\x1b[F', 'Up': '\x1b[A',
             'Down': '\x1b[B', 'Right': '\x1b[C', 'Left': '\x1b[D',
             'PPage': '\x1b[5~', 'PageUp': '\x1b[5~',
             'NPage': '\x1b[6~', 'PageDown': '\x1b[6~',
             'Delete': '\x1b[3~', 'DC': '\x1b[3~', 'Insert': '\x1b[2~'}
    for number, sequence in enumerate(['OP', 'OQ', 'OR', 'OS', '[15~', '[17~',
                                       '[18~', '[19~', '[20~', '[21~', '[23~', '[24~'], 1):
        named[f'F{number}'] = named[f'f{number}'] = '\x1b' + sequence
    parts = []
    for key in keys:
        if key in named:
            parts.append(named[key].encode())
        elif key.lower().startswith(('ctrl-', 'c-')) and len(key.rsplit('-', 1)[1]) == 1:
            parts.append(bytes([ord(key[-1].lower()) & 31]))
        else:
            parts.append(key.encode())
    return b''.join(parts)


def fake():
    if 'api' in sys.argv:
        from fake_opencode_composer import api
        raise SystemExit(api(sys.argv[sys.argv.index('api') + 1:]))
    screen = Path(os.environ['SESSIONDOCK_TEST_SCREEN'])
    original = termios.tcgetattr(0)
    tty.setraw(0)
    previous, received = None, b''
    try:
        while True:
            state = screen.read_text()
            if state != previous:
                record = json.loads(state)
                received = b''
                screen.with_suffix('.trace').write_bytes(b'')
                screen.with_suffix('.outcome').write_text('')
                text = record['screen'].replace('\n', '\r\n')
                sys.stdout.write('\x1b[?1l\x1b[?2004h\x1b[0m\x1b[2J\x1b[H' + text)
                sys.stdout.flush()
                screen.with_suffix('.shown').write_text(str(record['sequence']))
                previous = state
            if not select.select([0], [], [], .01)[0]:
                continue
            received += os.read(0, 8192)
            screen.with_suffix('.trace').write_bytes(received)
            expected = bytes.fromhex(record.get('expected_bytes', ''))
            if received == expected and expected:
                screen.with_suffix('.outcome').write_text(record['outcome'])
            elif expected and not expected.startswith(received):
                screen.with_suffix('.outcome').write_text('WRONG:' + received.hex())
    finally:
        termios.tcsetattr(0, termios.TCSANOW, original)


def subset(actual, expected, path='prompt'):
    if isinstance(expected, dict):
        assert isinstance(actual, dict), (path, actual, expected)
        for key, value in expected.items():
            assert key in actual, (path, key, actual)
            subset(actual[key], value, path + '.' + key)
    elif isinstance(expected, list):
        assert isinstance(actual, list) and len(actual) == len(expected), (path, actual, expected)
        for index, value in enumerate(expected):
            subset(actual[index], value, path + f'[{index}]')
    else:
        assert actual == expected, (path, actual, expected)


def main():
    from playwright.sync_api import expect, sync_playwright
    from history_parity import REPO, BINARY, Corpus, isolated_server
    import send_browser
    from host_identity import request as host_request
    from popups import on_popup
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', nargs='+', default=['claude', 'codex', 'grok', 'opencode'])
    parser.add_argument('--cases', nargs='+')
    args = parser.parse_args()
    send_browser.BINARY = args.binary.resolve()
    cases, exercised = 0, 0
    with tempfile.TemporaryDirectory(prefix='sessiondock-cli-menus-') as temporary:
        root = Path(temporary).resolve()
        for name in ['host', 'work', 'ledger', 'state', 'home', 'claude', 'codex', 'grok', 'opencode']:
            (root / name).mkdir(mode=0o700)
        screens = {source: root / (source + '-screen') for source in args.sources}
        sequence = 0

        def frame(source, text, expected=b'', outcome='choice'):
            nonlocal sequence
            sequence += 1
            pending = screens[source].with_suffix('.pending')
            pending.write_text(json.dumps({'sequence': sequence, 'screen': text,
                'expected_bytes': expected.hex(), 'outcome': outcome}))
            pending.replace(screens[source])
            return sequence

        for source in args.sources:
            frame(source, 'Private CLI menu test. No provider configured.')
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': source + '-cli-v1', 'source': source,
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(Path(__file__).resolve()), '--fake'],
                'new_args': [] if source == 'codex' else ['--session-id', '{session_id}'],
                'resume_args': ['--resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                        'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                        'SESSIONDOCK_TEST_SCREEN': str(screens[source]),
                        'SESSIONDOCK_TEST_OPENCODE_DB': str(root / 'opencode/opencode.db')}}
                for source in args.sources]}))
        launcher.chmod(0o600)
        send_browser.initialize('--initialize-lifecycle', root / 'ledger')
        with isolated_server(Corpus(root), args.binary.resolve(), host_dir=root / 'host',
                lifecycle_dir=root / 'ledger', launcher_config=launcher, state_dir=root / 'state',
                file_roots=(root / 'work',), file_write_roots=(root / 'work',),
                extra_env={'SESSIONDOCK_OPENCODE_DB': str(root / 'opencode/opencode.db'),
                           'SESSIONDOCK_OPENCODE_ROOT': str(root / 'opencode/mirror')}) as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(service_workers='block')

            def route(request):
                if not request.request.url.startswith(base + '/'):
                    request.abort()
                else:
                    request.continue_()

            context.route('**/*', route)
            page = context.new_page()
            errors, dialogs, writes = [], [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
            page.on('request', lambda request: writes.append(request.post_data_json)
                    if urlsplit(request.url).path == '/api/term/send' else None)
            try:
                page.goto(base, wait_until='networkidle')
                page.evaluate('''() => {
                    const probe = probeComposerInput;
                    const apply = applyCliState;
                    window.__pauseMenuWatch = false;
                    applyCliState = (uid, cli, options = {}) => {
                        if (!__pauseMenuWatch || options.status === false) apply(uid, cli, options);
                    };
                    window.__menuChecks = [];
                    probeComposerInput = async uid => {
                        const result = await probe(uid);
                        __menuChecks.push({uid,result});
                        if (__menuChecks.length > 10) __menuChecks.shift();
                        return result;
                    };
                }''')
                card = page.locator('#composer-question')

                def shown(source, generation):
                    deadline = time.monotonic() + 5
                    shown = screens[source].with_suffix('.shown')
                    while not shown.exists() or shown.read_text() != str(generation):
                        assert time.monotonic() < deadline, (source, generation)
                        page.wait_for_timeout(10)
                    page.wait_for_timeout(100)

                def loaded(source, generation):
                    shown(source, generation)
                    return page.evaluate('async () => await probeComposerInput(composerUid)')

                def clear(source):
                    loaded(source, frame(source, 'Private CLI menu test. No provider configured.'))
                    expect(card).to_be_hidden()

                def pause_background():
                    page.evaluate('''async () => {
                        while (composerInputProbeBusy) await new Promise(resolve => setTimeout(resolve, 10));
                        composerInputProbeBusy = true;
                        __pauseMenuWatch = true;
                    }''')

                for source in args.sources:
                    fixtures = json.loads((REPO / 'tests/fixtures' / f'cli_menus_{source}.json').read_text())
                    if isinstance(fixtures, dict):
                        fixtures = fixtures.get('cases', fixtures.get('fixtures'))
                    if args.cases:
                        fixtures = [fixture for fixture in fixtures if fixture['name'] in args.cases]
                    if not page.locator('#new-session').is_visible():
                        page.locator('#header-more-btn').click()
                    page.locator('#new-session').click()
                    page.locator(f'label:has(input[name="new-source"][value="{source}"])').click()
                    expect(page.locator(f'input[name="new-source"][value="{source}"]')).to_be_checked()
                    page.locator('#new-cwd').fill(str(root / 'work'))
                    with page.expect_response(lambda response: urlsplit(response.url).path == '/api/term/create') as created:
                        page.locator('#new-session-go').click()
                    assert created.value.status == 200, created.value.text()
                    receipt = created.value.json()
                    private_host = json.loads((root / 'host' / (receipt['name'] + '.json')).read_text())
                    assert host_request(private_host, {'op':'resize', 'cols':160, 'rows':100})['ok']
                    expect(page.locator('#composer')).to_be_visible()
                    page.locator('#cinput').fill('Keep this message draft')
                    page.evaluate('async () => await composerDraftWrites')
                    for fixture in fixtures:
                        cases += 1
                        clear(source)
                        result = loaded(source, frame(source, fixture['screen']))
                        expected = fixture['expected']
                        if expected is None:
                            assert not result.get('prompt'), (source, fixture['name'], result)
                            expect(card).to_be_hidden()
                            continue
                        try:
                            subset(result['prompt'], expected)
                        except AssertionError as error:
                            diagnostic = host_request(private_host, {'op':'capture', 'styled':True})
                            failed = REPO / 'target/cli-menu-audit' / f'failed-{source}-{fixture["name"]}.json'
                            failed.parent.mkdir(parents=True, exist_ok=True)
                            failed.write_text(json.dumps(diagnostic, ensure_ascii=False, indent=2))
                            raise AssertionError((source, fixture['name'], error.args)) from error
                        expect(card).to_be_visible()
                        expect(page.locator('#csend')).to_be_disabled()
                        expect(page.locator('#dlive')).to_have_text('?')
                        expect(page.locator('#dlive')).to_have_css('background-color', 'rgb(251, 191, 36)')
                        expect(page.locator('#composer-input-status')).to_be_visible()
                        expect(page.locator('#composer-input-status')).to_contain_text('等待用户回答')
                        assert page.locator('#composer-input-status').evaluate(
                            "node => getComputedStyle(node, '::before').content") == '"?"'
                        assert not page.evaluate('T.openViews.size'), 'Composer answers must not require terminal attach'
                        prompt = result['prompt']
                        if source == 'codex' and prompt.get('kind') == 'screen_menu':
                            labels = [action['label'] for action in prompt.get('actions', [])]
                            if fixture['name'] in ('slow_response', 'model_picker', 'request_options', 'checkbox_experiments'):
                                assert not any(label in labels for label in
                                    ('Previous options', 'Next options', 'Previous page', 'Next page')), labels
                            if fixture['name'] == 'request_hidden_options':
                                assert 'Previous options' in labels and 'Next options' in labels, labels
                            if fixture['name'] == 'keymap_picker':
                                assert 'Next options' in labels and 'Next page' in labels, labels
                                assert 'Previous options' not in labels and 'Previous page' not in labels, labels
                            if fixture['name'] == 'slow_response':
                                expect(card.locator('.question-option')).to_have_count(2)
                                expect(card.locator('.question-header')).to_have_text('Giving this request a little extra thought')
                                expect(card.locator('.question-text')).to_have_text('')
                                # Unread content stays recorded and accessible in the
                                # tooltip, while the waiting marker remains one symbol.
                                page.evaluate('''() => {
                                    const item = document.querySelector('#side .item.sel');
                                    S.unread.set(item.dataset.uid, {count:2});
                                    paintItemStatus(item);
                                }''')
                                marker = page.locator('#side .item.sel > .ico > .item-status')
                                expect(marker).to_have_text('?')
                                expect(marker).to_have_attribute('title', re.compile('2 条新内容'))
                                page.evaluate('''() => {
                                    const item = document.querySelector('#side .item.sel');
                                    S.unread.delete(item.dataset.uid);
                                    paintItemStatus(item);
                                }''')
                        # The history watch contains CLI status but no CHECK
                        # screen projection. It must preserve this live card.
                        page.evaluate('''() => {
                            const draft = composerDraft();
                            const id = draft.inputPrompt.id;
                            applyCliState(composerUid, draft.cli);
                            if (draft.inputPrompt?.id !== id) throw new Error('Watch erased current menu');
                            applyCliState(composerUid, {...draft.cli,
                                observed_at:draft.cli.observed_at - 1,
                                input:{state:'ready',code:'',message:''}});
                            if (draft.inputPrompt?.id !== id) throw new Error('Older watch replaced CHECK');
                        }''')
                        for index, option in enumerate(prompt['questions'][0].get('options', [])):
                            if not option.get('keys'):
                                expect(card.locator(f'[data-question-option="{index}"]')).to_be_disabled()
                        actions = []
                        for index, option in enumerate(prompt['questions'][0].get('options', [])):
                            if option.get('keys'):
                                actions.append((f'[data-question-option="{index}"]', key_bytes(option['keys']), option['label']))
                        for index, action in enumerate(prompt.get('actions', [])):
                            if action.get('keys'):
                                actions.append((f'[data-question-action="{index}"]', key_bytes(action['keys']), action['label']))
                        if prompt.get('cancel_keys'):
                            actions.append(('.question-cancel', key_bytes(prompt['cancel_keys']), 'cancel'))
                        if prompt.get('text'):
                            field = prompt['text']
                            value = 'browser answer'
                            typed = value.encode()
                            if field.get('mode') != 'data':
                                typed = b'\x1b[200~' + typed + b'\x1b[201~'
                            actions.append(('.question-text-form .question-submit',
                                key_bytes(field.get('before_keys', [])) + typed + key_bytes(field.get('after_keys', [])), 'text'))
                        for selector, native, outcome in actions:
                            clear(source)
                            loaded(source, frame(source, fixture['screen'], native, outcome))
                            if outcome == 'text':
                                page.locator('#composer-question .question-text-input').fill('browser answer')
                                page.evaluate('async () => await probeComposerInput(composerUid)')
                                expect(page.locator('#composer-question .question-text-input')).to_have_value('browser answer')
                            count = len(writes)
                            card.locator(selector).click()
                            deadline = time.monotonic() + 7
                            outcome_file = screens[source].with_suffix('.outcome')
                            while outcome_file.read_text() != outcome:
                                found = outcome_file.read_text()
                                assert not found.startswith('WRONG:'), (source, fixture['name'], selector, found, native.hex())
                                assert time.monotonic() < deadline, (source, fixture['name'], selector, found, dialogs,
                                    page.evaluate('({uid:composerUid,prompt:composerDraft()?.inputPrompt,pending:composerDraft()?.inputAnswer,status:composerDraft()?.inputStatus})'),
                                    writes[count:], screens[source].with_suffix('.trace').read_bytes().hex(),
                                    page.evaluate('__menuChecks'))
                                page.wait_for_timeout(10)
                            assert len(writes) > count
                            assert all(write.get('instance_id') and write.get('token') == '' for write in writes[count:])
                            expect(page.locator('#cinput')).to_have_value('Keep this message draft')
                            if outcome in ('Previous options', 'Next options', 'Previous page', 'Next page',
                                           'Previous shortcut group', 'Next shortcut group'):
                                expect(card.locator(selector)).to_be_enabled(timeout=5000)
                                assert len(writes) == count + 1, 'Navigation must never automatically resend'
                            exercised += 1
                        print(f"PASS {source}: {fixture['name']} ({len(actions)} native actions)", flush=True)
                    positive = next((fixture for fixture in fixtures if fixture['expected']
                        and fixture['expected'].get('kind') != 'folder_trust'), None)
                    if positive:
                        clear(source)
                        loaded(source, frame(source, positive['screen']))
                        buttons = card.locator('.question-option:enabled')
                        if buttons.count():
                            page.set_viewport_size({'width':390, 'height':844})
                            page.evaluate('showMobileDetail()')
                            expect(card).to_be_visible()
                            # Hold only background polling while the screen changes;
                            # the clicked control must still perform its own CHECK.
                            pause_background()
                            try:
                                count = len(writes)
                                shown(source, frame(source, 'A different native screen without a menu.'))
                                buttons.first.click()
                                expect(card).to_be_hidden(timeout=5000)
                                assert len(writes) == count, 'A stale card must not write keys'
                                expect(page.locator('#cinput')).to_have_value('Keep this message draft')
                            finally:
                                page.evaluate('composerInputProbeBusy = false; __pauseMenuWatch = false')
                                page.set_viewport_size({'width':1280, 'height':720})
                    scopes = {
                        'claude': ('bash_permission', 'rm -rf /work/demo/generated', 'rm -rf /work/demo/cache'),
                        'codex': ('approval_command', 'echo hello world', 'echo changed world'),
                        'grok': ('approval_bash', 'git status', 'git status --short'),
                        'opencode': ('permission_shell', 'printf inspect-only', 'printf changed-only'),
                    }
                    scope_name, before, after = scopes[source]
                    scope = next((fixture for fixture in fixtures if fixture['name'] == scope_name), None)
                    if scope:
                        clear(source)
                        original = loaded(source, frame(source, scope['screen']))['prompt']['id']
                        pause_background()
                        try:
                            count = len(writes)
                            changed = scope['screen'].replace(before, after, 1)
                            assert changed != scope['screen']
                            shown(source, frame(source, changed))
                            card.locator('.question-option:enabled').first.click()
                            page.wait_for_function('id => composerDraft()?.inputPrompt?.id !== id', arg=original)
                            assert page.evaluate('composerDraft()?.inputPrompt?.id'), 'Changed approval still needs a card'
                            assert len(writes) == count, 'Changed command must invalidate the shown approval'
                        finally:
                            page.evaluate('composerInputProbeBusy = false; __pauseMenuWatch = false')
                    clear(source)
                    assert not errors and not dialogs, (errors, dialogs)
                assert exercised, 'No menu interaction exercised'
            finally:
                browser.close()
    print(f'PASS cli_menus_browser: {cases} source-derived screens, {exercised} native actions, all drafts retained')


if __name__ == '__main__':
    if '--fake' in sys.argv:
        fake()
    else:
        main()
