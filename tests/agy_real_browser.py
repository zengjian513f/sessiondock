#!/usr/bin/env python3
# run_validation: skip
"""Operator-only real AGY + Chromium test; invoke directly with --agy PATH.

Excluded from the default validation sweep. No installation, everyday HOME,
credentials, production sessions, or paid model. Build SessionDock separately.
The real binary uses --model sessiondock-fake, no effort, and a temporary
loopback synthetic gateway. The selected compiled frontend is served. Every input,
send and menu answer uses actual page controls or native PTY keyboard events.
Evidence is retained in the printed private /tmp directory; owned hosts die
on success, failure, or the suite's bounded SIGALRM deadline.
"""

from frontend_paths import frontend_dir
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import threading
import traceback
from unittest.mock import patch
from urllib.parse import urlsplit
import uuid

from playwright.sync_api import expect, sync_playwright
from agy_browser import check_phone, open_picker, wait_screen
from agy_test_gateway import MODELS, THINKING, gateway
from history_parity import BINARY, REPO, Corpus, isolated_server
from send_browser import initialize


@contextmanager
def private_proc(root, agy):
    """Expose only this invocation's real CLI/host processes to fd discovery."""
    done = threading.Event()
    def scan():
        while not done.is_set():
            for entry in Path('/proc').iterdir():
                if not entry.name.isdigit():
                    continue
                try:
                    cwd = Path(os.readlink(entry / 'cwd'))
                    executable = Path(os.readlink(entry / 'exe'))
                    if not cwd.is_relative_to(root):
                        continue
                    if executable != agy and executable.name != 'ptyhost':
                        continue
                    link = root / 'proc' / entry.name
                    if not link.exists():
                        link.symlink_to(entry, target_is_directory=True)
                except OSError:
                    pass
            done.wait(0.15)
    thread = threading.Thread(target=scan, daemon=True)
    thread.start()
    try:
        yield
    finally:
        done.set()
        thread.join(timeout=5)


class Evidence:
    def __init__(self, report, root):
        self.report, self.root = report, root
        self.results = []
        self.write()

    def write(self):
        self.report.write_text('# AGY real Chromium browser run\n\n'
            f'Machine: {os.uname().nodename}\n\nPrivate evidence: `{self.root}`\n\n'
            'Operator-only; actual AGY binary, empty child environment, synthetic '
            'loopback gateway, model sessiondock-fake; no paid model or credentials.\n\n'
            + '\n\n'.join(self.results) + '\n')

    def pass_(self, text):
        print('PASS ' + text, flush=True)
        self.results.append('PASS: ' + text)
        self.write()

    def fail(self, name, error, page=None):
        detail = ''.join(traceback.format_exception(error))
        print('FAIL ' + name + '\n' + detail, flush=True)
        self.results.append('FAIL: ' + name + '\n\n```text\n' + detail + '```')
        self.write()
        if page and not page.is_closed():
            try:
                page.screenshot(path=str(self.root / (name + '.png')), full_page=True, timeout=5000)
                (self.root / (name + '-state.json')).write_text(json.dumps(page.evaluate('''() => ({
                    selected:S.sel, input:composerDraft()?.inputStatus,
                    prompt:composerDraft()?.inputPrompt, draft:document.querySelector('#cinput')?.value,
                    detail:document.querySelector('#detail')?.innerText,
                    screen:Array.from({length:T.term?.buffer?.active?.length||0}, (_,i)=>
                      T.term.buffer.active.getLine(i)?.translateToString(true)).join('\\n')})'''),
                    ensure_ascii=False, indent=2))
            except Exception as diagnostic:
                print('Diagnostic failed:', diagnostic, flush=True)


def terminal(page, text='', key=None):
    if not page.locator('#termpane').is_visible():
        page.locator('#a-term').click()
    page.locator('#termpane .xterm-helper-textarea').focus()
    if text:
        page.keyboard.type(text)
    if key:
        # Let the CLI's ESC disambiguation timer finish: adjacent ESC/other
        # bytes can otherwise be interpreted as an Alt chord by its TUI.
        page.keyboard.press(key, delay=100 if key == 'Escape' else 0)


def clear_terminal(page):
    terminal(page, key='Escape')
    terminal(page, key='Escape')
    terminal(page, key='End')
    # AGY's native input does not clear this draft with Ctrl+U. Send actual
    # Backspace keys, without evaluating a terminal write or a business API.
    for _ in range(80):
        page.keyboard.press('Backspace')


def ready(page):
    if not page.locator('#composer').is_visible():
        page.locator('#a-term').click()
    expect(page.locator('#composer')).to_be_visible()
    page.wait_for_function("() => composerDraft()?.inputStatus?.state === 'ready'", timeout=20000)


def send(page, text):
    ready(page)
    page.locator('#cinput').fill(text)
    expect(page.locator('#csend')).to_be_enabled()
    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send', timeout=25000) as sent:
        page.locator('#csend').click()
    assert sent.value.status < 300, sent.value.text()
    return sent.value.json()


def history(page, text):
    page.wait_for_function('''text => [...document.querySelectorAll('#msgs .msg:not(.queued-send)')]
        .some(n => n.textContent.includes(text))''', arg=text, timeout=25000)


def no_gateway_text(records, text):
    assert not any(text in str(r['request']) for r in records), text


def refuse(page, base, name, text, records):
    """Controls are exercised first; direct HTTP also checks the server guard."""
    if not page.locator('#composer').is_visible():
        page.locator('#a-term').click()
    page.locator('#cinput').fill(text)
    page.wait_for_function("() => composerDraft()?.inputStatus?.state !== 'ready'", timeout=15000)
    expect(page.locator('#csend')).to_be_disabled()
    # Read the actual current lease; never call a business function via evaluate.
    identity = page.evaluate('''name => ({uid:S.sel, page:TERM_PAGE_ID, build:BUILD_ID,
        inputLease:T.views.get(name)?.inputLease})''', name)
    lease = {**identity['inputLease'], 'page': identity['page']} if identity.get('inputLease') else None
    check = page.request.post(base + '/api/session/conversation/check', data={
        'uid': identity['uid'], 'name': name, 'lease': lease, '_build': identity['build']})
    assert check.status == 409, check.text()
    code = check.json().get('code')
    assert code in ('cli_input_pending', 'cli_question', 'cli_not_ready'), check.text()
    refused = page.request.post(base + '/api/session/conversation/send', data={
        'uid': identity['uid'], 'name': name, 'lease': lease,
        'text': text, 'request_id': str(uuid.uuid4()), '_build': identity['build']})
    assert refused.status == 409 and refused.json().get('code') == code, refused.text()
    expect(page.locator('#cinput')).to_have_value(text)
    no_gateway_text(records, text)
    return code


def run(browser, base, root, agy, records, evidence, holds):
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(15000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    def record_check(response):
        if urlsplit(response.url).path != '/api/session/conversation/check':
            return
        try:
            with (root / 'browser-checks.jsonl').open('a') as log:
                log.write(json.dumps({'status': response.status, 'body': response.json()}) + '\n')
        except Exception:
            pass  # A response cancelled during page teardown has no body.
    page.on('response', record_check)
    try:
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded && T.sources.agy === true')
        open_picker(page)
        page.locator('#new-session-form label:has(input[value="agy"])').click()
        catalog = page.request.get(base + '/api/term/models?source=agy')
        assert catalog.status == 200, catalog.text()
        assert [model['id'] for model in catalog.json()['models']] == list(MODELS), catalog.text()
        page.locator('#new-model').click()
        page.locator(f'#new-model-options [data-model-option][title="{MODELS[0]}"]').click()
        page.locator('#new-effort').select_option('')
        page.locator('#new-cwd').fill(str(root / 'work'))
        with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/create') as created:
            page.locator('#new-session-go').click()
        assert created.value.status == 200, created.value.text()
        receipt = created.value.json()
        assert receipt['launch_kind'] == 'new_pending' and not receipt.get('declared_sid'), receipt
        assert created.value.request.post_data_json['model'] == MODELS[0]
        assert not created.value.request.post_data_json.get('effort')
        name = receipt['name']
        page.wait_for_function('uid => S.sel === uid', arg='tmux:' + name)
        terminal(page)
        wait_screen(page, 'Do you trust the contents of this project?')
        # Trust only the freshly created fixture cwd, through the actual PTY.
        terminal(page, key='Enter')
        wait_screen(page, '? for shortcuts')
        evidence.pass_('new Agy, two real model TSV entries, sessiondock-fake select, CLI default effort, temporary folder trust')

        marker = 'SESSIONDOCK_AGY_FIRST_' + uuid.uuid4().hex[:10]
        send(page, marker)
        page.wait_for_function("() => S.sel?.startsWith('agy:')", timeout=30000)
        uid = page.evaluate('S.sel')
        history(page, 'echo: ' + marker)
        rows = page.request.get(base + '/api/sessions?force=1').json()['sessions']
        row = next(r for r in rows if r['uid'] == uid)
        sid = row['sid']
        native = root / 'home/.gemini/antigravity-cli'
        transcript = native / 'brain' / sid / '.system_generated/logs/transcript_full.jsonl'
        data = [json.loads(line) for line in transcript.read_text().splitlines()]
        assert any(r.get('thinking') == THINKING for r in data), data
        assert any(r.get('content') == 'echo: ' + marker for r in data), data
        assert not any(r.get('type') == 'ERROR_MESSAGE' for r in data), data
        assert (native / 'conversations' / (sid + '.db')).is_file()
        fds, launches = [], []
        for entry in (root / 'proc').iterdir():
            try:
                if Path(os.readlink(entry / 'exe')) != agy:
                    continue
                argv = (entry / 'cmdline').read_bytes().decode().split('\0')[:-1]
                launches.append(argv)
                fds += [os.readlink(fd) for fd in (entry / 'fd').iterdir()]
            except OSError:
                pass
        assert any(argv[1:] == ['--model', MODELS[0]] for argv in launches), launches
        assert str(native / 'conversations' / (sid + '.db')) in fds, fds
        (root / 'native-binding-evidence.json').write_text(json.dumps({'uid': uid, 'sid': sid,
            'argv': launches, 'fds': fds}, indent=2))
        # Expand the actual native reasoning disclosure instead of changing DOM.
        if not page.locator('#composer').is_visible():
            page.locator('#a-term').click()
        process = page.locator('#msgs .turn-process').first
        if process.count() and 'folded' in (process.get_attribute('class') or ''):
            process.locator('.fold-toggle').first.click()
        thinking = page.locator('#msgs .msg[data-role="thinking"]').first
        expect(thinking).to_have_count(1)
        if 'folded' in (thinking.get_attribute('class') or ''):
            thinking.locator('.fold-toggle').first.click()
        expect(thinking).to_contain_text(THINKING)
        evidence.pass_('first webpage SEND becomes native row, real db fd identity, native text + thinking, model argv')

        multiline = 'SESSIONDOCK_AGY_MULTI_' + uuid.uuid4().hex[:10] + '\nsecond line\nthird line'
        send(page, multiline)
        history(page, 'echo: ' + multiline.splitlines()[0])
        assert any(r['kind'] == 'main' and r['echo'] == multiline for r in records), records
        evidence.pass_('multiline composer SEND reaches gateway intact and renders native echo')

        slow = 'SESSIONDOCK_AGY_BUSY_' + uuid.uuid4().hex[:10]
        holds[slow] = threading.Event()
        try:
            send(page, slow)
            page.wait_for_function("composerDraft()?.cli?.instance?.busy === true", timeout=10000)
            expect(page.locator('#dlive')).to_have_class(re.compile(r'\bturn-working\b'))
            terminal(page, 'BUSY_DRAFT esc to cancel')
            wait_screen(page, 'BUSY_DRAFT esc to cancel')
            checked = page.request.post(base + '/api/session/conversation/check', data={
                'uid': uid, '_build': page.evaluate('BUILD_ID')}).json()
            assert checked['cli']['instance']['busy'] is True, checked
        finally:
            holds[slow].set()
        history(page, 'echo: ' + slow)
        page.wait_for_function("composerDraft()?.cli?.instance?.busy === false", timeout=10000)
        expect(page.locator('#dlive')).not_to_have_class(re.compile(r'\bturn-working\b'))
        clear_terminal(page)
        ready(page)
        evidence.pass_('real in-flight gateway request is busy; completion returns idle without inferring catalog status')

        # Later independent paths still run when one implementation guard fails.
        for stage in ('pty-draft', 'model-select', 'model-cancel', 'resume-menu', 'unknown-menu'):
            try:
                ready(page)
                if stage == 'pty-draft':
                    terminal(page, 'SESSIONDOCK_NATIVE_UNSENT esc to cancel')
                    wait_screen(page, 'SESSIONDOCK_NATIVE_UNSENT')
                    code = refuse(page, base, name, 'SESSIONDOCK_WEB_DRAFT_KEEP', records)
                    assert code == 'cli_input_pending', code
                    wait_screen(page, 'SESSIONDOCK_NATIVE_UNSENT')
                    checked = page.request.post(base + '/api/session/conversation/check', data={
                        'uid': uid, '_build': page.evaluate('BUILD_ID')}).json()
                    assert checked['cli']['instance']['busy'] is False, checked
                else:
                    command = '/model' if stage.startswith('model') else '/resume' if stage == 'resume-menu' else '/permissions'
                    before_command = transcript.read_bytes()
                    result = send(page, command)
                    assert result['state'] == 'sent', result
                    terminal(page)
                    wait_screen(page, 'Switch Model' if stage.startswith('model') else
                                'Conversations' if stage == 'resume-menu' else 'Permission Config Editor')
                    assert transcript.read_bytes() == before_command, command
                    checked = page.request.post(base + '/api/session/conversation/check', data={
                        'uid': uid, '_build': page.evaluate('BUILD_ID')}).json()
                    assert checked['cli']['queued'] == [], (command, checked)
                    assert checked['cli']['instance']['busy'] is None, checked
                    if stage == 'unknown-menu':
                        # The top-level scope picker has a verified parser.
                        # Its deeper Project editor is intentionally unknown.
                        terminal(page, key='Enter')
                    code = refuse(page, base, name, 'SESSIONDOCK_BLOCKED_' + stage, records)
                    if stage == 'unknown-menu':
                        assert code == 'cli_not_ready', code
                    if stage.startswith('model'):
                        expect(page.locator('#composer-question')).to_contain_text('Switch Model')
                        if stage == 'model-select':
                            page.locator('#composer-question [data-question-option]').filter(has_text=MODELS[0]).click()
                        else:
                            page.locator('#composer-question .question-cancel').click()
                        ready(page)
                        expect(page.locator('#cinput')).to_have_value('SESSIONDOCK_BLOCKED_' + stage)
                    if stage == 'resume-menu':
                        # Re-select this fixture's CURRENT entry through the
                        # real PTY; no other conversation exists in its HOME.
                        terminal(page, key='Enter')
                        ready(page)
                evidence.pass_(stage + ': real native input/menu blocks CHECK and SEND, preserves webpage draft; menu SEND has no invented native echo')
            except Exception as error:
                evidence.fail(stage, error, page)
            finally:
                clear_terminal(page)
                if not page.locator('#composer').is_visible():
                    page.locator('#a-term').click()
                page.locator('#cinput').fill('')

        page.reload(wait_until='networkidle')
        page.locator(f'#side .item[data-uid="{uid}"]').click()
        ready(page)
        assert page.evaluate('S.sel') == uid
        history(page, marker)
        evidence.pass_('refresh reconnects the same live native session')
        if not page.locator('#a-session-toggle').is_visible():
            page.locator('#a-more').click()
        expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label', '停止会话')
        page.locator('#a-session-toggle').click()
        expect(page.locator('dialog.app-popup[open]')).to_be_visible()
        page.locator('dialog.app-popup[open] [data-popup-action="ok"]').click()
        page.wait_for_function("() => document.querySelector('#a-session-toggle')?.getAttribute('aria-label') === '启动会话'", timeout=25000)
        with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/takeover', timeout=25000) as resumed:
            page.locator('#a-term').click()
        assert resumed.value.status == 200, resumed.value.text()
        assert resumed.value.json()['launch_kind'] == 'resume', resumed.value.text()
        assert resumed.value.json()['declared_sid'] == sid, resumed.value.text()
        resumed_name = resumed.value.json()['name']
        page.wait_for_function('''name => T.name === name && T.ws?.readyState === WebSocket.OPEN
            && document.querySelector('#a-term')?.getAttribute('aria-busy') !== 'true' ''',
            arg=resumed_name, timeout=20000)
        wait_screen(page, '? for shortcuts')
        followup = 'SESSIONDOCK_AGY_RESUMED_' + uuid.uuid4().hex[:10]
        send(page, followup)
        history(page, 'echo: ' + followup)
        assert page.evaluate('S.sel') == uid
        argv = [(p / 'cmdline').read_bytes().decode().split('\0') for p in (root / 'proc').iterdir()
                if (p / 'cmdline').exists() and Path(os.readlink(p / 'exe')) == agy]
        assert any('--conversation' in a and sid in a for a in argv), argv
        (root / 'resume-argv.json').write_text(json.dumps(argv, indent=2))
        evidence.pass_('stop and --conversation resume continue the same native session and synthetic model')

        # The report worker must use the same launch and composer readiness path.
        if not page.locator('#report-bug').is_visible():
            page.locator('#header-more-btn').click()
        page.locator('#report-bug').click()
        description = 'AGY_REPORT_' + uuid.uuid4().hex[:10] + '\nSynthetic report; reply with the fixture echo.'
        page.locator('#bug-report-description').fill(description)
        page.locator('input[name="bug-report-source"][value="agy"]').check()
        page.locator('#bug-report-model').click()
        page.locator(f'#bug-report-model-options [data-model-option][title="{MODELS[0]}"]').click()
        page.locator('#bug-report-effort').select_option('')
        with page.expect_response(lambda r: urlsplit(r.url).path == '/api/bug-report') as reported:
            page.locator('#bug-report-go').click()
        assert reported.value.status == 202, reported.value.text()
        report = reported.value.json()
        assert report['worker']['source'] == 'agy', report
        prompt = Path(report['path']).joinpath('worker-prompt.md').read_text().strip()
        page.locator('#bug-report-toast').get_by_role('button', name='打开', exact=True).click()
        history(page, description.splitlines()[0])
        page.wait_for_function("() => S.sel?.startsWith('agy:')", timeout=30000)
        report_uid = page.evaluate('S.sel')
        assert report_uid != uid
        assert any(r['kind'] == 'main' and r['echo'].strip() == prompt for r in records), records
        history(page, 'echo: ')
        evidence.pass_('Agy report dialog launches its worker and submits the exact report prompt through normal SEND')
        # Start the responsive picker check from a settled list page; the
        # report worker's asynchronous session selection is a separate path.
        page.goto(base, wait_until='networkidle')
        check_phone(page)
        evidence.pass_('390px light/dark new-session controls fit viewport')
        assert not errors, errors
        assert records and all(r['request']['model'] == MODELS[0 if r['kind'] == 'main' else 1]
                               for r in records), records
        assert any(r['kind'] == 'title' and r['response'] == 'Synthetic AGY title' for r in records), records
        evidence.pass_('planner requests use sessiondock-fake; separate short-title requests use synthetic sessiondock-second; no browser errors')
    except Exception as error:
        evidence.fail('browser-path', error, page)
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--agy', required=True, type=Path, help='explicit real binary; operator-only')
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--ptyhost', required=True, type=Path)
    parser.add_argument('--report', type=Path, default=Path(tempfile.gettempdir()) / 'sessiondock-agy-real-browser-report.md')
    args = parser.parse_args()
    agy, binary, host = (path.resolve(strict=True) for path in (args.agy, args.binary, args.ptyhost))
    browser_cache = os.environ.get('PLAYWRIGHT_BROWSERS_PATH', str(
        Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache')) / 'ms-playwright'))
    chromium = os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE')
    root = Path(tempfile.mkdtemp(prefix='sessiondock-agy-real-')).resolve()
    print('ROOT', root, flush=True)
    evidence = Evidence(args.report, root)
    for name in ('home', 'host', 'ledger', 'state', 'work', 'claude', 'codex', 'grok',
                 'opencode', 'mirror', 'proc', 'config', 'data', 'cache', 'xdg-state', 'tmp'):
        (root / name).mkdir(mode=0o700)
    onboarding = root / 'home/.gemini/antigravity-cli/cache/onboarding.json'
    onboarding.parent.mkdir(parents=True)
    onboarding.write_text(json.dumps({'consumerOnboardingComplete': True,
        'enterpriseOnboardingComplete': True, 'onboardingComplete': True}))
    signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(TimeoutError('suite deadline: 240 seconds')))
    signal.alarm(240)
    holds = {}
    with gateway(root, holds) as (url, records, gateway_errors):
        env = {'HOME': str(root / 'home'), 'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8',
            'TERM': 'xterm-256color', 'XDG_CONFIG_HOME': str(root / 'config'),
            'XDG_DATA_HOME': str(root / 'data'), 'XDG_CACHE_HOME': str(root / 'cache'),
            'XDG_STATE_HOME': str(root / 'xdg-state'), 'TMPDIR': str(root / 'tmp'),
            'AGY_CLI_DISABLE_AUTO_UPDATE': '1', 'AGY_LLM_GATEWAY_URL': url + '/v1',
            'AGY_LLM_GATEWAY_MODELS': ','.join(MODELS),
            'AGY_LLM_GATEWAY_API_KEY': 'synthetic-local-fixture',
            'AGY_LLM_GATEWAY_WIRE_PROTOCOL': 'openai', 'PYTHONDONTWRITEBYTECODE': '1',
            'HTTP_PROXY': url, 'HTTPS_PROXY': url, 'ALL_PROXY': url,
            'NO_PROXY': 'localhost,127.0.0.1,::1'}
        (root / 'child-env.json').write_text(json.dumps(env, indent=2))
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(host),
            'host_dir': str(root / 'host'), 'adapters': [], 'profiles': [
                {'id': 'agy-cli-v1', 'source': 'agy', 'executable': str(agy),
                 'args': [], 'env': env}]}))
        launcher.chmod(0o600)
        extra = {'SESSIONDOCK_OPENCODE_ROOT': str(root / 'opencode'),
            'SESSIONDOCK_OPENCODE_DB': str(root / 'opencode/empty.db'),
            'SESSIONDOCK_AGY_HOME': str(root / 'home/.gemini/antigravity-cli'),
            'SESSIONDOCK_AGY_ROOT': str(root / 'mirror'),
            'SESSIONDOCK_BUG_REPORT_DIR': str(root / 'reports'),
            'SESSIONDOCK_BUG_REPORT_REPO': str(root / 'work'),
            'SESSIONDOCK_PROC_ROOT': str(root / 'proc')}
        try:
            with patch.dict(os.environ, {**env, 'PLAYWRIGHT_BROWSERS_PATH': browser_cache,
                    'SESSIONDOCK_TEST_WEB_DIR': str(frontend_dir())}, clear=True):
                initialize('--initialize-lifecycle', root / 'ledger', binary)
                with private_proc(root, agy), isolated_server(Corpus(root), binary,
                        host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                        launcher_config=launcher, state_dir=root / 'state',
                        file_roots=(root / 'work',), file_write_roots=(root / 'work',),
                        trash_dir=root / 'trash', audit_dir=root / 'audit', extra_env=extra) as (base, _):
                    with sync_playwright() as pw:
                        options = {'headless': True}
                        if chromium:
                            options['executable_path'] = chromium
                        browser = pw.chromium.launch(**options)
                        try:
                            run(browser, base, root, agy, records, evidence, holds)
                        finally:
                            browser.close()
            assert not gateway_errors, gateway_errors
        except Exception as error:
            evidence.fail('fixture', error)
        finally:
            signal.alarm(0)
            for record in (root / 'host').glob('sessiondock-*.json'):
                captured = subprocess.run([str(host), '--dir', str(root / 'host'), 'capture', record.stem],
                    env=env, cwd=root / 'work', timeout=5, capture_output=True, check=False)
                (root / (record.stem + '-final-capture.txt')).write_bytes(captured.stdout + captured.stderr)
                subprocess.run([str(host), '--dir', str(root / 'host'), 'kill', record.stem, '--force'],
                    env=env, cwd=root / 'work', timeout=5, stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL, check=False)
    print('REPORT', args.report, flush=True)
    if any(result.startswith('FAIL:') for result in evidence.results):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
