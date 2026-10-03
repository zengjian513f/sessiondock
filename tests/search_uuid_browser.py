#!/usr/bin/env python3
"""Quick metadata filtering plus UUID/UID Enter search and actual navigation."""
import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, claude_row, codex_row, codex_message, encoded, isolated_server

CLAUDE_ID = '6cfb25b1-75dc-482f-a03f-94ab752210ca'
CODEX_ID = '019ad384-b752-7200-861e-f0abea312456'
AGENT_ID = 'a8e91d52-9fb4-4cd6-aaf7-dc31620b7741'
BROKEN_ID = 'fe59d50d-a31f-4e7e-ba64-863980cee2ef'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-search-uuid-') as temporary:
        data = Corpus(Path(temporary))
        for source in ('claude', 'codex', 'grok'):
            (data.root / source).mkdir()
        data.put(CLAUDE_ID, 'claude', [
            claude_row(CLAUDE_ID, 'user', 'u0', None, 'Parent fixture title'),
            claude_row(CLAUDE_ID, 'assistant', 'a0', 'u0', 'Parent answer')], [])
        data.put(CODEX_ID, 'codex', [
            codex_row('session_meta', {'id': CODEX_ID, 'cwd': '/synthetic/uuid'}),
            codex_row('turn_context', {'model': 'synthetic-search-model', 'cwd': '/synthetic/uuid'}),
            codex_message('user', 'Second fixture title'),
            codex_message('assistant', 'Codex answer')], [])
        data.put(BROKEN_ID, 'claude', [
            claude_row(BROKEN_ID, 'user', 'u0', None, 'Broken fixture title'),
            {'type': 'user', 'message': {'role': 'user', 'content': 42}}], [])
        agents = data.paths[CLAUDE_ID].with_suffix('') / 'subagents'
        agents.mkdir(parents=True)
        (agents / f'agent-{AGENT_ID}.meta.json').write_text(json.dumps({
            'description': 'Worker fixture title', 'agentType': 'reviewer'}))
        (agents / f'agent-{AGENT_ID}.jsonl').write_bytes(b''.join(encoded(row) for row in [
            claude_row(CLAUDE_ID, 'user', 'au0', None, 'Worker fixture title', isSidechain=True, agentId=AGENT_ID),
            claude_row(CLAUDE_ID, 'assistant', 'aa0', 'au0', 'Worker answer', isSidechain=True, agentId=AGENT_ID)]))
        cache_dir = data.root / 'search-cache'
        with isolated_server(data, args.binary, extra_env={
                'SESSIONDOCK_SEARCH_CACHE_DIR': str(cache_dir)}) as (base, _), sync_playwright() as pw:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**launch)
            try:
                for width in (1280, 390):
                    context = browser.new_context(viewport={'width': width, 'height': 800}, service_workers='block')
                    context.route('**/*', lambda route: route.continue_()
                                  if route.request.url.startswith(base + '/') else route.abort())
                    page = context.new_page()
                    errors = []
                    searches = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    page.on('request', lambda request: searches.append(request.url)
                            if '/api/search?' in request.url else None)
                    page.goto(base, wait_until='networkidle')
                    if width == 1280:
                        # The page lists sessions but has not requested full-text search.
                        # Wait beyond the former two-second startup warmup delay.
                        page.wait_for_timeout(3500)
                        assert not list(cache_dir.iterdir()), list(cache_dir.iterdir())
                        assert not searches, searches
                    # Flat mode makes the actual matches distinguishable from parent navigation rows.
                    if page.locator('#nest-toggle').get_attribute('aria-pressed') == 'true':
                        page.locator('#nest-toggle').click()

                    def clear():
                        if width < 600 and page.locator('.mobile-back').is_visible():
                            page.locator('.mobile-back').click()
                        page.locator('#q').fill('')
                        expect(page.locator('#side-search-state')).to_be_hidden()

                    def lookup(query, sid, answer, agent=False):
                        clear()
                        page.locator('#q').fill(query)
                        expect(page.locator('#side-search-count')).to_have_text('1 条')
                        row = page.locator(f'#side .item[data-agent="{sid}"]' if agent else
                                           f'#side .item[data-uid="{data.uid(sid)}"]')
                        expect(row).to_be_visible()
                        row.click()
                        if answer:
                            expect(page.locator('#msgs')).to_contain_text(answer)
                        if width < 600 and page.locator('.mobile-back').is_visible():
                            page.locator('.mobile-back').click()
                        for _ in range(2):  # Cold and warm persistent-cache paths.
                            old = page.locator('#stat').get_attribute('data-seq') or ''
                            page.locator('#q').press('Enter')
                            page.wait_for_function("old => (document.querySelector('#stat').dataset.seq || '') !== old", arg=old)
                            expect(page.locator('#side-search-count')).to_have_text('1 条')
                            expect(row).to_be_visible()
                            expect(row.locator('.snip')).to_contain_text(query)
                        row.click()
                        if answer:
                            expect(page.locator('#msgs')).to_contain_text(answer)

                    def quick(query, sid, answer, agent=False):
                        clear()
                        before = len(searches)
                        page.locator('#q').fill(query)
                        expect(page.locator('#side-search-label')).to_have_text('筛选结果')
                        expect(page.locator('#side-search-count')).to_have_text('1 条')
                        row = page.locator(f'#side .item[data-agent="{sid}"]' if agent else
                                           f'#side .item[data-uid="{data.uid(sid)}"]')
                        expect(row).to_be_visible()
                        row.click()
                        expect(page.locator('#msgs')).to_contain_text(answer)
                        assert len(searches) == before, searches

                    for query in ('synthetic-search-model', 'search-model', '/synthetic/uuid',
                                  'Codex', 'Codex uuid synthetic-search-model'):
                        quick(query, CODEX_ID, 'Codex answer')
                    quick('reviewer', AGENT_ID, 'Worker answer', agent=True)
                    quick('Claude reviewer', AGENT_ID, 'Worker answer', agent=True)
                    clear()
                    page.locator('#opts button[data-o="case"]').click()
                    quick('Codex', CODEX_ID, 'Codex answer')
                    clear()
                    page.locator('#opts button[data-o="case"]').click()
                    page.locator('#opts button[data-o="regex"]').click()
                    quick('synthetic-search-m.del', CODEX_ID, 'Codex answer')
                    clear()
                    page.locator('#opts button[data-o="regex"]').click()
                    page.locator('#search-mode-toggle').click()
                    quick('absent-model synthetic-search-model', CODEX_ID, 'Codex answer')
                    clear()
                    page.locator('#search-mode-toggle').click()

                    for sid, answer in ((CLAUDE_ID, 'Parent answer'), (CODEX_ID, 'Codex answer')):
                        for query in (sid, sid[:8], sid[9:18], sid[-12:], data.uid(sid), data.uid(sid)[-8:]):
                            lookup(query, sid, answer)
                        expect(page.locator('#side .item.agent')).to_have_count(0)
                    for query in (AGENT_ID, AGENT_ID[9:18]):
                        lookup(query, AGENT_ID, 'Worker answer', agent=True)
                    lookup(BROKEN_ID[:8], BROKEN_ID, None)
                    # Both a parent and its sidecar may grow while the cache is hot.
                    # Submit through the page so the next request must see the new body.
                    def body_search(query, selector):
                        clear()
                        old = page.locator('#stat').get_attribute('data-seq') or ''
                        page.locator('#q').fill(query)
                        page.locator('#q').press('Enter')
                        page.wait_for_function("old => (document.querySelector('#stat').dataset.seq || '') !== old", arg=old)
                        expect(page.locator('#search-progress')).not_to_be_visible()
                        expect(page.locator('#side-search-count')).to_have_text('1 条')
                        expect(page.locator(selector + ' .snip')).to_contain_text(query)

                    for path, row, token, selector in (
                        (data.paths[CLAUDE_ID], claude_row(CLAUDE_ID, 'user', f'append-{width}', 'a0',
                             f'parentfresh{width}'), f'parentfresh{width}',
                             f'#side .item[data-uid="{data.uid(CLAUDE_ID)}"]'),
                        (agents / f'agent-{AGENT_ID}.jsonl', claude_row(CLAUDE_ID, 'user', f'agent-append-{width}',
                             'aa0', f'agentfresh{width}', isSidechain=True, agentId=AGENT_ID),
                             f'agentfresh{width}', f'#side .item[data-agent="{AGENT_ID}"]'),
                    ):
                        before = {p.name: p.stat().st_mtime_ns for p in cache_dir.iterdir()}
                        assert before, 'search must populate the persistent cache'
                        with path.open('ab') as stream:
                            stream.write(encoded(row))
                        # No idle refresh of stale entries, either.
                        page.wait_for_timeout(1200)
                        assert before == {p.name: p.stat().st_mtime_ns for p in cache_dir.iterdir()}
                        body_search(token, selector)
                        hot = {p.name: p.stat().st_mtime_ns for p in cache_dir.iterdir()}
                        assert hot != before, 'append must refresh the cached version'
                        body_search(token, selector)
                        assert hot == {p.name: p.stat().st_mtime_ns for p in cache_dir.iterdir()}
                    clear()
                    page.locator('#q').fill('uuid-that-does-not-exist')
                    expect(page.locator('#side-search-count')).to_have_text('0 条')
                    assert not errors, errors
                    print(f'PASS quick metadata/UUID browser width={width}: model, directory, CLI name, agent type, AND/OR/case/regex, no search request; UUID/UID, sidecar, cold/hot search and navigation', flush=True)
                    context.close()
            finally:
                browser.close()


if __name__ == '__main__':
    main()
