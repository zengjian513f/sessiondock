#!/usr/bin/env python3
"""SEND must distinguish a dim Claude suggestion from consumed paste text."""
import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY, REPO, Corpus, isolated_server
from private_hosts import private_hosts
from send_browser import create_claude, initialize, wait_history


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    binary = parser.parse_args().binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-suggestion-') as temporary, private_hosts(Path(temporary)):
        root = Path(temporary)
        for name in ('host', 'work/claude-area', 'ledger', 'state', 'home', 'claude', 'codex', 'grok'):
            (root / name).mkdir(parents=True, mode=0o700)
        suggestion = root / 'suggestion'
        delay = root / 'paste-delay'
        suggestion.write_text('继续处理这个问题')
        cli = root / 'cli.py'
        cli.write_text('''import sys
from pathlib import Path
sys.path.insert(0, ''' + repr(str(REPO / 'tests')) + ''')
from fake_claude_cli import Fake, RULE, parse

class SuggestedFake(Fake):
    def render(self, footer=''):
        if self.buffer:
            return super().render(footer)
        # The cursor stays before dim ghost text, just like Claude's suggestion.
        text = Path(''' + repr(str(suggestion)) + ''').read_text()
        self.write('\\x1b[2J\\x1b[H' + RULE + '\\r\\n❯ \\x1b7\\x1b[2m'
                   + text + '\\x1b[0m\\r\\n' + RULE + '\\x1b8')

SuggestedFake(parse(sys.argv[1:])).run()
''')
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'claude-cli-v1', 'source': 'claude',
                'executable': str(Path(sys.executable).resolve()), 'args': [str(cli), '--reply'],
                'new_args': ['--session-id', '{session_id}'], 'resume_args': ['--resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TERM': 'xterm-256color',
                    'LANG': 'C.UTF-8', 'SESSIONDOCK_TEST_CLAUDE_ROOT': str(root / 'claude'),
                    'SESSIONDOCK_TEST_PASTE_DELAY': str(delay)}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger', binary=binary)
        with isolated_server(Corpus(root), binary, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, state_dir=root / 'state',
                file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(service_workers='block')
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            try:
                page.goto(base, wait_until='networkidle')
                receipt = create_claude(page, base, root / 'work', open_terminal=False)
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'")

                def send(text, minimum=0):
                    page.locator('#cinput').fill(text)
                    page.evaluate('async () => await composerDraftWrites')
                    expect(page.locator('#csend')).to_be_enabled()
                    started = time.monotonic()
                    with page.expect_response(lambda r: urlsplit(r.url).path.endswith('/conversation/send')) as sent:
                        page.locator('#csend').click()
                    elapsed = time.monotonic() - started
                    assert sent.value.status == 200, sent.value.text()
                    print(f'SEND suggestion: {elapsed:.3f}s, minimum={minimum}', flush=True)
                    assert minimum <= elapsed < 2.5, elapsed
                    expect(page.locator('#cinput')).to_have_value('')
                    expect(page.locator('#csend')).to_have_attribute('aria-busy', 'false')
                    wait_history(page, text)

                # Identical visible text changes from ghost suggestion to real
                # input; it must settle after 200ms, not the three-second limit.
                send(suggestion.read_text())

                # Conversely, unchanged dim text that happens to end with the
                # new prompt must not count as proof the paste was consumed.
                context.request.post(base + '/api/term/kill', data={
                    'record_id': receipt['record_id'], 'instance_id': receipt['instance_id']})
                suggestion.write_text('建议：等重绘后再发送')
                delay.write_text('1200')
                receipt = create_claude(page, base, root / 'work', open_terminal=False)
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'")
                send('等重绘后再发送', minimum=1.35)
                assert not errors, errors
                context.request.post(base + '/api/term/kill', data={
                    'record_id': receipt['record_id'], 'instance_id': receipt['instance_id']})
            finally:
                browser.close()
    print('PASS send_suggestion_browser: matching dim suggestion settles promptly; stale suggestion cannot confirm paste')


if __name__ == '__main__':
    main()
