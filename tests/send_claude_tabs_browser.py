#!/usr/bin/env python3
"""Claude's exact tab expansion settles SEND without accepting other whitespace.

Synthetic native CLI; browser SEND, repeated inputs, negative evidence, and
reopening an old receipt whose native echo precedes the history window.
"""
import argparse
from contextlib import ExitStack
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import sys
import tempfile
import uuid

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY, REPO, Corpus, isolated_server
from private_hosts import private_hosts
from send_browser import SETTINGS, create_claude, initialize, wait_history


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-claude-tabs-') as temporary, private_hosts(Path(temporary)):
        root = Path(temporary)
        print('ROOT', root, flush=True)
        for name in ('host', 'work/claude-area', 'ledger', 'state', 'home', 'claude', 'codex', 'grok'):
            (root / name).mkdir(parents=True, mode=0o700)
        hold = root / 'hold'
        fake = root / 'fake.py'
        fake.write_text('''import sys
from pathlib import Path
sys.path.insert(0, %r)
from fake_claude_cli import Fake, parse
class TabsFake(Fake):
    def record(self, text, before=()):
        if not Path(%r).exists():
            super().record(text.replace('\\t', '    '), before)
TabsFake(parse(sys.argv[1:])).run()
''' % (str(REPO / 'tests'), str(hold)))
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
            'host_dir': str(root / 'host'), 'adapters': [], 'profiles': [{
                'id': 'claude-cli-v1', 'source': 'claude', 'executable': sys.executable,
                'args': [str(fake), '--settings', SETTINGS, '--reply'],
                'new_args': ['--session-id', '{session_id}'], 'resume_args': ['--resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TERM': 'xterm-256color',
                        'LANG': 'C.UTF-8', 'SESSIONDOCK_TEST_CLAUDE_ROOT': str(root / 'claude')}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger', args.binary)

        def server():
            return isolated_server(Corpus(root), args.binary, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, state_dir=root / 'state', file_roots=(root / 'work',),
                file_write_roots=(root / 'work',))

        with ExitStack() as servers, sync_playwright() as pw:
            base, _ = servers.enter_context(server())
            browser = pw.chromium.launch(headless=True)
            context = browser.new_context(service_workers='block', viewport={'width': 1280, 'height': 900})
            context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
            page = context.new_page()
            errors, sends = [], []
            page.on('pageerror', lambda e: errors.append(str(e)))
            page.on('request', lambda r: sends.append(r.post_data_json) if r.url.endswith('/conversation/send') else None)
            try:
                page.goto(base, wait_until='domcontentloaded')
                receipt = create_claude(page, base, root / 'work', open_terminal=False)
                page.wait_for_function('composerUid && !composerDraft().loading && takenOver(composerUid)')
                native = root / 'claude/project-history' / (receipt['declared_sid'] + '.jsonl')

                def send(text):
                    if not page.locator('#composer').is_visible():
                        page.locator('#a-chat').click()
                    page.locator('#cinput').fill(text)
                    expect(page.locator('#csend')).to_be_enabled()
                    with page.expect_response(lambda r: r.url.endswith('/conversation/send')) as response:
                        page.locator('#csend').click()
                    assert response.value.ok, response.value.text()
                    return response.value.json()['request_id']

                def bubble(request):
                    return page.locator(f'.queued-send[data-request-id="{request}"]')

                def append(text, role='user', age=0, path=None, sid=None):
                    path = path or native
                    rows = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
                    parent = next((r['uuid'] for r in reversed(rows) if r.get('uuid')), None)
                    row = {'type': role, 'uuid': str(uuid.uuid4()), 'parentUuid': parent,
                           'sessionId': sid or receipt['declared_sid'], 'cwd': str(root / 'work/claude-area'),
                           'timestamp': (datetime.now(timezone.utc) - timedelta(seconds=age)).isoformat(),
                           'message': {'role': role, 'content': text}}
                    with path.open('a') as stream:
                        stream.write(json.dumps(row) + '\n')
                    return row['uuid']

                send('Bootstrap tab echo observation')
                wait_history(page, 'OK: Bootstrap tab echo observation')
                # Full-view recovery must not reuse an already consumed exact
                # echo either; preserve ordinary SEND behavior alongside tabs.
                hold.touch()
                plain = send('Bootstrap tab echo observation')
                page.wait_for_timeout(1800)
                expect(bubble(plain)).to_have_count(1)
                append('Bootstrap tab echo observation')
                expect(bubble(plain)).to_have_count(0, timeout=6000)
                hold.unlink()
                text = 'TabEcho\talpha\tbeta\nsecond\trow'
                expanded = text.replace('\t', '    ')
                request = send(text)
                wait_history(page, 'OK: TabEcho')
                rows = [json.loads(line) for line in native.read_text().splitlines()]
                assert any(r.get('type') == 'user' and r['message']['content'] == expanded for r in rows)
                expect(bubble(request)).to_have_count(0, timeout=6000)
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / 'tabs.png'), full_page=True)
                print('PASS exact four-space native expansion settles tab SEND', flush=True)

                # Keep the next input unconsumed: rereading the previous echo
                # must not settle a second, identical SEND inside five seconds.
                hold.touch()
                repeated = send(text)
                page.wait_for_timeout(1800)
                expect(bubble(repeated)).to_have_count(1)
                append(expanded)
                expect(bubble(repeated)).to_have_count(0, timeout=6000)
                print('PASS one native echo cannot settle repeated tab inputs twice', flush=True)

                negative = 'Held\talpha  beta\nsecond\trow'
                request = send(negative)
                original = json.loads((root / 'state/conversations/conversation-ledger.json').read_text())
                queued = next(r for q in original['queued'].values() for r in q if r['request_id'] == request)
                assert queued['text'] == negative
                append(negative.replace('\t', '  '))
                append(negative.replace('\t', '    ').replace('alpha  beta', 'alpha beta'))
                append(negative.replace('\t', '    ').replace('\n', ' '))
                append(negative.replace('\t', '    '), role='assistant')
                append(negative.replace('\t', '    '), age=60)
                other_sid = str(uuid.uuid4())
                append(negative.replace('\t', '    '), path=native.with_name(other_sid + '.jsonl'), sid=other_sid)
                page.wait_for_timeout(1800)
                page.reload(wait_until='domcontentloaded')
                expect(bubble(request)).to_have_count(1)
                after = json.loads((root / 'state/conversations/conversation-ledger.json').read_text())
                unchanged = next(r for q in after['queued'].values() for r in q if r['request_id'] == request)
                assert (unchanged['text'], unchanged['echo_hash']) == (queued['text'], queued['echo_hash'])
                append(negative.replace('\t', '    '))
                expect(bubble(request)).to_have_count(0, timeout=6000)
                print('PASS different spaces/newlines, assistant, old time and other session do not settle SEND', flush=True)

                old_text = 'Held old receipt\tbefore rewind\tand later history'
                old_request = send(old_text)
                expect(bubble(old_request)).to_have_count(1)
                uid = page.evaluate('S.sel')
                page.goto('about:blank')
                servers.close()
                append(old_text.replace('\t', '    '))
                for index in range(700):
                    leaf = append('Later history item %d' % index, role='assistant' if index % 2 else 'user')
                append('Discarded after explicit rewind')
                append('Discarded assistant after explicit rewind', role='assistant')
                with native.open('a') as stream:
                    stream.write(json.dumps({'type': 'last-prompt', 'leafUuid': leaf, 'explicit': True,
                                             'sessionId': receipt['declared_sid']}) + '\n')
                base, _ = servers.enter_context(server())
                page.goto(base, wait_until='domcontentloaded')
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                expect(bubble(old_request)).to_have_count(0, timeout=10000)
                wire = context.request.get(base + '/api/messages/' + uid + '?window=1').json()
                assert wire.get('partial'), 'must exercise a paginated opening window'
                assert not any(m.get('text') == old_text.replace('\t', '    ') for m in wire['messages'])
                assert not any('Discarded' in m.get('text', '') for m in wire['messages'])
                assert len(sends) == 6, sends
                assert not errors, errors
                print('PASS restart reconciles old tab receipt outside history window without resend', flush=True)
            finally:
                context.close()
                browser.close()


if __name__ == '__main__':
    main()
