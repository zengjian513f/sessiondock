#!/usr/bin/env python3
"""Search starts expanded and supports folds isolated from saved list folds."""

import argparse
import json
import os
from pathlib import Path
import tempfile
from urllib.request import Request

from browser_runtime import choose_toolbar_option
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, batch35_agent_meta, claude_row, codex_message, codex_row, encoded, isolated_server
from search_browser import corpus


def lazy_search(browser, base, data, width, screenshots=None):
    """Fulltext children are independent of the compact catalog's expansions."""
    context = browser.new_context(viewport={'width': width, 'height': 900}, service_workers='block')
    page = context.new_page()
    errors, lists = [], []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.on('request', lambda request: lists.append(request.url) if '/api/sessions' in request.url else None)
    try:
        page.goto(base, wait_until='networkidle')
        page.locator('#nest-hidden').click()
        page.wait_for_function('!sessionLoadActive && S.childMode === "hidden"')
        parent_uid, child_uid = data.uid('search-main'), data.uid('codex-search')
        parent = page.locator(f'#side .item[data-uid="{parent_uid}"]')
        child = page.locator(f'#side .item[data-uid="{child_uid}"]')
        only = page.locator('#side .item.agent[data-agent="only"]')
        worker = page.locator('#side .item.agent[data-agent="worker"]')
        grand = page.locator(f'#side .item[data-uid="{data.uid("grandchild")}"]')
        expect(parent).to_be_visible()
        expect(parent.locator('.nest-caret')).to_have_attribute('aria-expanded', 'false')
        expect(child).to_have_count(0)
        expect(only).to_have_count(0)
        saved = page.evaluate('JSON.stringify([[...S.lazyOpen], [...S.nestClosed], localStorage.getItem("sessiondock.childMode")])')

        def search(query):
            old = page.locator('#stat').get_attribute('data-seq') or ''
            page.locator('#q').fill(query)
            page.locator('#q').press('Enter')
            page.wait_for_function("old => (document.querySelector('#stat').dataset.seq || '') !== old", arg=old)

        search('慢特征')
        expect(parent).to_be_visible()
        expect(parent.locator('.snip')).to_have_count(0)
        expect(only).to_be_visible()
        expect(only.locator('.snip mark')).to_have_text('慢特征')
        expect(page.locator('#side-search-count')).to_have_text('1 条')
        expect(parent.locator('.nest-caret')).to_have_attribute('aria-expanded', 'true')
        if screenshots:
            screenshots.mkdir(parents=True, exist_ok=True)
            for theme in ('light', 'dark'):
                page.locator('#header-more-btn').click()
                page.locator('#settings').click()
                page.locator('#setting-theme').select_option(theme)
                page.locator('#settings-dialog .modal-close').click()
                expect(only).to_be_visible()
                page.screenshot(path=str(screenshots / f'lazy-search-{width}-{theme}.png'))
        before = len(lists)
        parent.locator('.nest-caret').click()
        expect(only).to_have_count(0)
        page.evaluate('async () => await runSessionPoll()')
        expect(only).to_have_count(0)
        parent.locator('.nest-caret').click()
        expect(only).to_be_visible()
        # Only the explicit reconciliation above may read the compact catalog.
        assert len(lists) == before + 1, lists[before:]
        only.click()
        expect(page.locator('#msgs')).to_contain_text('SidecarOnly body 慢特征')
        if width == 390:
            page.locator('.mobile-back').click()
        search('Needle')
        expect(child).to_be_visible()
        expect(worker).to_be_visible()
        expect(only).to_have_count(0)
        expect(page.locator('#side-search-count')).to_have_text('3 条')
        search('CodexWorkerOnly')
        expect(child).to_be_visible()
        expect(page.locator('#side .item.agent[data-agent="codex-worker"]')).to_be_visible()
        search('IndependentOnly')
        expect(grand).to_be_visible()
        grand.click()
        expect(page.locator('#msgs')).to_contain_text('IndependentOnly child')
        if width == 390:
            page.locator('.mobile-back').click()
        page.locator('#side-search-exit').click()
        expect(child).to_have_count(0)
        expect(only).to_have_count(0)
        expect(grand).to_have_count(0)
        expect(parent.locator('.nest-caret')).to_have_attribute('aria-expanded', 'false')
        assert page.evaluate('JSON.stringify([[...S.lazyOpen], [...S.nestClosed], localStorage.getItem("sessiondock.childMode")])') == saved
        # A branch opened before searching must remain open afterwards too.
        parent.locator('.nest-caret').click()
        expect(child).to_be_visible()
        page.wait_for_function('!sessionLoadActive && S.lazyPending.size === 0')
        opened = page.evaluate('JSON.stringify([...S.lazyOpen])')
        search('慢特征')
        expect(only).to_be_visible()
        parent.locator('.nest-caret').click()
        expect(only).to_have_count(0)
        search('慢特征')
        expect(only).to_be_visible()
        page.locator('#side-search-exit').click()
        expect(child).to_be_visible()
        expect(only).to_be_visible()
        assert page.evaluate('JSON.stringify([...S.lazyOpen])') == opened
        assert not errors, errors
        print(f'PASS lazy search {width}px: sidecar-only Chinese hit, nested sessions, folds/poll, click history and restored expansions', flush=True)
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots-dir', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-search-no-fold-') as directory:
        data = corpus(Path(directory))
        (data.root / 'state').mkdir()
        agent_dir = data.paths['search-main'].with_suffix('') / 'subagents'
        agent_dir.mkdir(parents=True)
        for agent, title, body in [('worker', 'Synthetic worker', 'Needle worker body'),
                                   ('other', 'Other worker', 'unrelated worker body'),
                                   ('only', 'SidecarOnly worker', 'SidecarOnly body 慢特征')]:
            (agent_dir / f'agent-{agent}.meta.json').write_text(json.dumps({'description': title, 'agentType': 'general-purpose'}))
            (agent_dir / f'agent-{agent}.jsonl').write_bytes(b''.join(encoded(row) for row in [
                claude_row('search-main', 'user', f'{agent}-u', None, title, isSidechain=True, agentId=agent),
                claude_row('search-main', 'assistant', f'{agent}-a', f'{agent}-u', body,
                           isSidechain=True, agentId=agent)]))
        with (agent_dir / 'agent-worker.jsonl').open('ab') as transcript:
            transcript.write(encoded({'type': 'attachment', 'attachment': {'type': 'synthetic_unknown'}}))
        for sid, body in [('missed-child', 'unrelated independent child'),
                          ('grandchild', 'IndependentOnly child')]:
            data.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/search'}),
                codex_message('user', body)], [])
        data.put('codex-worker', 'codex', [batch35_agent_meta('codex-worker', 'codex-search'),
            codex_message('user', 'CodexWorkerOnly body')], [], parent='codex-search')
        with isolated_server(data, args.binary, state_dir=data.root / 'state',
                             extra_env={'SESSIONDOCK_SEARCH_CACHE_DIR': str(data.root / 'search-cache')}) as (base, opener), sync_playwright() as pw:
            parent_uid, child_uid = data.uid('search-main'), data.uid('codex-search')
            missed_uid, grand_uid = data.uid('missed-child'), data.uid('grandchild')
            for child, parent in [(child_uid, parent_uid), (missed_uid, parent_uid), (grand_uid, child_uid)]:
                with opener.open(Request(base + '/api/session/nest',
                    data=json.dumps({'uid': child, 'parent_uid': parent}).encode(),
                    headers={'Content-Type': 'application/json'})) as response:
                    assert response.status == 200
            sessions = json.load(opener.open(base + '/api/sessions'))
            result = json.load(opener.open(base + '/api/search?q=Needle'))
            matched = next(row for row in result['results'] if row['uid'] == parent_uid)
            assert [item['id'] for item in matched['agent_items']] == ['worker'], matched
            assert 'migration_warnings' not in matched['agent_items'][0], matched

            def enrich(rows):
                for row in rows:
                    row['updated'] = '2026-09-25T06:00:00Z'
                    row['cwd'] = '/synthetic/search'
                return rows

            enrich(sessions['sessions'])
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**launch)
            try:
                for width in (1280, 390):
                    lazy_search(browser, base, data, width, args.screenshots_dir)
                for view, width in [('date', 1280), ('tree', 1280), ('date', 390), ('tree', 390)]:
                    context = browser.new_context(viewport={'width': width, 'height': 900}, service_workers='block')
                    context.route('**/api/sessions?**', lambda route: route.fulfill(json=sessions))
                    context.route('**/api/sessions', lambda route: route.fulfill(json=sessions))
                    def messages(route):
                        response = route.fetch()
                        payload = response.json()
                        if payload.get('meta'):
                            enrich([payload['meta']])
                        route.fulfill(response=response, json=payload)
                    context.route('**/api/messages/**', messages)
                    context.route('**/api/watch?**', lambda route: route.abort())
                    page = context.new_page()
                    errors = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    page.goto(base, wait_until='networkidle')
                    choose_toolbar_option(page, "view", view)
                    if not page.locator('#nest-toggle').get_attribute('aria-pressed') == 'true':
                        page.locator("#nest-flat" if page.evaluate("S.nest") else "#nest-toggle").click()
                    parent = page.locator(f'#side .item[data-uid="{parent_uid}"]')
                    child = page.locator(f'#side .item[data-uid="{child_uid}"]')
                    worker = page.locator('#side .item.agent[data-agent="worker"]')
                    other = page.locator('#side .item.agent[data-agent="other"]')
                    only = page.locator('#side .item.agent[data-agent="only"]')
                    missed = page.locator(f'#side .item[data-uid="{missed_uid}"]')
                    grand = page.locator(f'#side .item[data-uid="{grand_uid}"]')
                    expect(child).to_be_visible()
                    expect(worker).to_be_visible()
                    expect(other).to_be_visible()
                    expect(only).to_be_visible()
                    expect(missed).to_be_visible()
                    expect(grand).to_be_visible()
                    parent.locator('.nest-caret').click()
                    expect(child).to_have_count(0)
                    group = page.locator('#side > .group').first

                    def check_red_themes(caret):
                        colors = []
                        for theme in ('light', 'dark'):
                            if not page.locator('#settings').is_visible():
                                page.locator('#header-more-btn').click()
                            page.locator('#settings').click()
                            page.locator('#setting-theme').select_option(theme)
                            page.locator('#settings-dialog .modal-close').click()
                            expect(page.locator('html')).to_have_attribute('data-theme', theme)
                            caret.hover()
                            if caret.evaluate("node => node.tagName === 'BUTTON'"):
                                caret.focus()
                            paint = caret.evaluate('''node => {
                                let background = node;
                                while (background && getComputedStyle(background).backgroundColor === 'rgba(0, 0, 0, 0)')
                                    background = background.parentElement;
                                return {color: getComputedStyle(node).color,
                                    glyph: getComputedStyle(node, '::before').color,
                                    background: getComputedStyle(background).backgroundColor};
                            }''')

                            def channels(color):
                                return [int(value.strip()) for value in color[4:-1].split(',')]

                            def luminance(rgb):
                                linear = [v / 255 / 12.92 if v / 255 <= .04045
                                          else ((v / 255 + .055) / 1.055) ** 2.4 for v in rgb]
                                return sum(v * weight for v, weight in zip(linear, [.2126, .7152, .0722]))

                            red, green, blue = channels(paint['color'])
                            assert red > green * 1.4 and red > blue * 1.4, (theme, paint)
                            assert paint['glyph'] == paint['color'], (theme, paint)
                            fg, bg = luminance([red, green, blue]), luminance(channels(paint['background']))
                            contrast = (max(fg, bg) + .05) / (min(fg, bg) + .05)
                            assert contrast >= 4.5, (theme, paint, contrast)
                            colors.append(paint['color'])
                        assert colors[0] != colors[1], colors
                        return colors[-1]

                    group.locator('.ghead').click()
                    expect(parent).to_have_count(0)
                    saved = page.evaluate('JSON.stringify([[...S.closed], [...S.nestClosed]])')
                    stored = page.evaluate('JSON.stringify([localStorage.getItem("sessiondock.closed"), localStorage.getItem("sessiondock.nestClosed")])')

                    # Flat mode also folds matching sidecars under a matched owner.
                    page.locator("#nest-flat" if page.evaluate("S.nest") else "#nest-toggle").click()
                    page.locator('#q').fill('Synthetic')
                    expect(parent).to_be_visible()
                    expect(worker).to_be_visible()
                    parent.locator('.nest-caret').click()
                    expect(worker).to_have_count(0)
                    parent.locator('.nest-caret').click()
                    expect(worker).to_be_visible()
                    page.locator("#nest-flat" if page.evaluate("S.nest") else "#nest-toggle").click()

                    # Typing filters titles and must already expose saved folds.
                    page.locator('#q').fill('"Synthetic worker"')
                    expect(parent).to_be_visible()
                    expect(worker).to_be_visible()
                    expect(other).to_have_count(0)
                    expect(only).to_have_count(0)
                    expect(child).to_have_count(0)
                    expect(page.locator('#side .group.closed')).to_have_count(0)
                    expect(parent.locator('.nest-caret')).to_have_attribute('aria-expanded', 'true')
                    parent.locator('.nest-caret').click()
                    expect(worker).to_have_count(0)
                    parent.locator('.nest-caret').click()
                    expect(worker).to_be_visible()
                    group.locator('.ghead').click()
                    expect(parent).to_have_count(0)
                    group.locator('.ghead').click()
                    expect(parent).to_be_visible()

                    # Enter searches real fixture bodies; the matched child is
                    # visible even when its parent was previously folded.
                    old = page.locator('#stat').get_attribute('data-seq') or ''
                    page.locator('#q').fill('Needle')
                    page.locator('#q').press('Enter')
                    page.wait_for_function("old => (document.querySelector('#stat').dataset.seq || '') !== old", arg=old)
                    expect(parent).to_be_visible()
                    expect(child).to_be_visible()
                    expect(worker).to_be_visible()
                    expect(worker.locator('.snip mark')).to_have_text('Needle')
                    expect(worker.locator('.m')).to_contain_text('命中 1')
                    expect(other).to_have_count(0)
                    expect(only).to_have_count(0)
                    expect(missed).to_have_count(0)
                    expect(grand).to_have_count(0)
                    expect(parent.locator('.nest-caret')).to_have_attribute('aria-expanded', 'true')
                    expect(group.locator('.caret')).to_be_visible()
                    parent.locator('.nest-caret').click()
                    expect(child).to_have_count(0)
                    expect(worker).to_have_count(0)
                    # Polling and rerendering preserve this search's folds.
                    sessions['sig'] += '-fold-refresh'
                    page.evaluate('async () => await runSessionPoll()')
                    expect(child).to_have_count(0)
                    expect(parent.locator('.nest-caret')).to_have_attribute('aria-expanded', 'false')
                    folded_color = check_red_themes(parent.locator('.nest-caret'))
                    group.locator('.ghead').click()
                    expect(parent).to_have_count(0)
                    check_red_themes(group.locator('.caret'))
                    page.evaluate('renderSide()')
                    expect(parent).to_have_count(0)
                    group.locator('.ghead').click()
                    expect(parent).to_be_visible()
                    expect(child).to_have_count(0)
                    parent.locator('.nest-caret').click()
                    expect(child).to_be_visible()
                    assert parent.locator('.nest-caret').evaluate('node => getComputedStyle(node).color') != folded_color
                    assert group.locator('.caret').evaluate('node => getComputedStyle(node).color') != folded_color
                    child.click()
                    expect(page.locator('#msgs')).to_contain_text('Needle from another provider')
                    if width == 390:
                        page.locator('.mobile-back').click()
                    worker.click()
                    expect(page.locator('#msgs')).to_contain_text('Needle worker body')
                    if width == 390:
                        page.locator('.mobile-back').click()
                    # Browser history reveals a target hidden by search folds.
                    parent.locator('.nest-caret').click()
                    group.locator('.ghead').click()
                    page.go_back()
                    expect(page.locator('#msgs')).to_contain_text('Needle from another provider')
                    if width == 390:
                        page.locator('.mobile-back').click()
                    expect(child).to_be_visible()
                    # A changed sessions response must not replace the matched
                    # sidecar subset with the full metadata list.
                    sessions['sig'] += '-refresh'
                    page.evaluate('runSessionPoll()')
                    expect(worker).to_be_visible()
                    expect(worker.locator('.snip mark')).to_have_text('Needle')
                    expect(other).to_have_count(0)
                    expect(only).to_have_count(0)
                    assert page.evaluate('JSON.stringify([[...S.closed], [...S.nestClosed]])') == saved

                    def search(query):
                        old = page.locator('#stat').get_attribute('data-seq') or ''
                        page.locator('#q').fill(query)
                        page.locator('#q').press('Enter')
                        page.wait_for_function("old => (document.querySelector('#stat').dataset.seq || '') !== old", arg=old)

                    # A new Enter run of the same query also starts expanded.
                    parent.locator('.nest-caret').click()
                    group.locator('.ghead').click()
                    old = page.locator('#stat').get_attribute('data-seq') or ''
                    page.locator('#q').press('Enter')
                    page.wait_for_function("old => (document.querySelector('#stat').dataset.seq || '') !== old", arg=old)
                    expect(child).to_be_visible()
                    expect(worker).to_be_visible()
                    parent.locator('.nest-caret').click()
                    group.locator('.ghead').click()
                    search('SidecarOnly')
                    expect(parent).to_be_visible()
                    expect(parent.locator('.snip')).to_have_count(0)
                    expect(parent.locator('.m')).not_to_contain_text('命中')
                    expect(only).to_be_visible()
                    expect(only.locator('.snip mark')).to_have_text(['SidecarOnly', 'SidecarOnly'])
                    expect(page.locator('#side .item')).to_have_count(2)
                    expect(page.locator('#side-search-count')).to_have_text('1 条')
                    sessions['sig'] += '-sidecar-refresh'
                    page.evaluate('runSessionPoll()')
                    expect(parent).to_be_visible()
                    expect(parent.locator('.snip')).to_have_count(0)
                    expect(only.locator('.snip mark')).to_have_text(['SidecarOnly', 'SidecarOnly'])
                    page.locator("#nest-flat" if page.evaluate("S.nest") else "#nest-toggle").click()
                    expect(parent).to_have_count(0)
                    expect(only).to_be_visible()
                    expect(page.locator('#side .item')).to_have_count(1)
                    page.locator("#nest-flat" if page.evaluate("S.nest") else "#nest-toggle").click()
                    expect(parent).to_be_visible()
                    expect(parent.locator('.snip')).to_have_count(0)
                    expect(worker).to_have_count(0)
                    expect(child).to_have_count(0)
                    only.click()
                    expect(page.locator('#msgs')).to_contain_text('SidecarOnly body')
                    if width == 390:
                        page.locator('.mobile-back').click()
                    search('Needle SidecarOnly')
                    expect(page.locator('#side .item')).to_have_count(0)
                    search('CodexWorkerOnly')
                    expect(parent).to_be_visible()
                    expect(child).to_be_visible()
                    expect(parent.locator('.snip')).to_have_count(0)
                    expect(child.locator('.snip')).to_have_count(0)
                    codex_worker = page.locator('#side .item.agent[data-agent="codex-worker"]')
                    expect(codex_worker).to_be_visible()
                    expect(codex_worker.locator('.snip mark')).to_have_text('CodexWorkerOnly')
                    expect(page.locator('#side .item')).to_have_count(3)
                    codex_worker.click()
                    expect(page.locator('#msgs')).to_contain_text('CodexWorkerOnly body')
                    if width == 390:
                        page.locator('.mobile-back').click()
                    search('IndependentOnly')
                    expect(grand).to_be_visible()
                    expect(parent).to_be_visible()
                    expect(child).to_be_visible()
                    expect(parent.locator('.snip')).to_have_count(0)
                    expect(child.locator('.snip')).to_have_count(0)
                    expect(missed).to_have_count(0)
                    expect(page.locator('#side .item.agent')).to_have_count(0)
                    grand.click()
                    expect(page.locator('#msgs')).to_contain_text('IndependentOnly child')
                    if width == 390:
                        page.locator('.mobile-back').click()
                    # Flat mode keeps only independently matched sessions too.
                    page.locator("#nest-flat" if page.evaluate("S.nest") else "#nest-toggle").click()
                    expect(grand).to_be_visible()
                    expect(page.locator('#side .item')).to_have_count(1)
                    page.locator("#nest-flat" if page.evaluate("S.nest") else "#nest-toggle").click()

                    # Reusing a persistent search cache must still observe an
                    # append to a previously unmatched sidecar on the next search.
                    with (agent_dir / 'agent-other.jsonl').open('ab') as transcript:
                        transcript.write(encoded(claude_row('search-main', 'assistant', 'other-new', 'other-a',
                            'FreshSidecar body', isSidechain=True, agentId='other')))
                    search('FreshSidecar')
                    expect(parent).to_be_visible()
                    expect(parent.locator('.snip')).to_have_count(0)
                    expect(other).to_be_visible()
                    expect(other.locator('.snip mark')).to_have_text('FreshSidecar')
                    expect(worker).to_have_count(0)
                    expect(only).to_have_count(0)
                    other.click()
                    expect(page.locator('#msgs')).to_contain_text('FreshSidecar body')
                    if width == 390:
                        page.locator('.mobile-back').click()
                    # A sidecar read failure is reported, never silently treated
                    # as a complete search with no matches.
                    (agent_dir / 'agent-other.jsonl').write_bytes(encoded({
                        'type': 'user', 'message': {'role': 'user', 'content': 42}}))
                    search('FreshSidecar')
                    expect(other).to_have_count(0)
                    expect(page.locator('#stat')).to_contain_text('结果不完整')
                    (agent_dir / 'agent-other.jsonl').write_bytes(encoded(claude_row('search-main', 'user',
                        'restored', None, 'Other worker', isSidechain=True, agentId='other')))
                    assert page.evaluate('JSON.stringify([[...S.closed], [...S.nestClosed]])') == saved
                    assert page.evaluate('JSON.stringify([localStorage.getItem("sessiondock.closed"), localStorage.getItem("sessiondock.nestClosed")])') == stored
                    page.locator('#side-search-exit').click()
                    expect(group).to_have_class('group closed')
                    assert group.locator('.caret').evaluate('node => getComputedStyle(node).color') != folded_color
                    expect(parent).to_have_count(0)
                    group.locator('.ghead').click()
                    expect(parent).to_be_visible()
                    expect(child).to_have_count(0)
                    expect(worker).to_have_count(0)
                    assert parent.locator('.nest-caret').evaluate('node => getComputedStyle(node).color') != folded_color
                    parent.locator('.nest-caret').click()
                    expect(child).to_be_visible()
                    expect(worker).to_be_visible()
                    expect(other).to_be_visible()
                    expect(only).to_be_visible()
                    expect(missed).to_be_visible()
                    expect(grand).to_be_visible()
                    assert not errors, errors
                    context.close()
                    print(f'PASS isolated search folds: {view} at {width}px; only matched sidecars/independent children, AND isolation, refresh, clicks, flat mode, exit restores folds', flush=True)
            finally:
                browser.close()


if __name__ == '__main__':
    main()
