#!/usr/bin/env python3
"""Open a native session while the later terminal script is still loading."""
import argparse
import json
from pathlib import Path
import tempfile
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, build_corpus, isolated_server
from browser_evidence_fixtures import run_with_evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('target/startup-actions'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-startup-actions-') as tmp, sync_playwright() as pw:
        corpus = build_corpus(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, _):
            browser = pw.chromium.launch(headless=True)
            try:
                page = browser.new_page()
                held, errors = [], []
                page.on('pageerror', lambda error:errors.append(str(error)))
                page.route('**/term.js?*', lambda route:held.append(route))
                page.goto(base, wait_until='commit')
                page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
                assert held and not errors, errors
                expect(page.locator('#a-term')).to_be_visible()
                page.screenshot(path=str(args.output/'before-terminal-script.png'))
                for route in held: route.continue_()
                page.wait_for_load_state('domcontentloaded')
                page.locator(f'#side .item[data-uid="{corpus.uid("codex-fork")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('Codex fork answer')
                assert not errors, errors
                page.screenshot(path=str(args.output/'loaded.png'))
                (args.output/'result.json').write_text(json.dumps({'passed':[
                    'real session click and fixture answer while terminal script held',
                    'no undefined action helper', 'second session renders after script completes']},indent=2)+'\n')
                print('PASS native session opens before deferred terminal script; actions initialize afterwards',flush=True)
            finally: browser.close()


if __name__ == '__main__':
    run_with_evidence(main, 'target/startup-actions')
