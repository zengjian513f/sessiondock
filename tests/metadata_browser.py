#!/usr/bin/env python3
"""Synthetic preferences through the real legacy UI; no original state or CLI."""
from browser_runtime import js, scoped_frontend
import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, batch35_meta, build_corpus, codex_message, isolated_server


def refresh_catalog(page):
    # Fetch and accept the real list, including metadata enrichment. Do not
    # inject rows or call the header renderer directly.
    assert page.evaluate(js('loadSessions(true)', 'runtime.core.list.loadSessions(true)'))
    page.evaluate('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))')


def assert_same_focused_header(page, header, button, selector):
    if scoped_frontend():
        assert page.evaluate('''({header, button, selector}) =>
            header === document.querySelector('#detail > .dhead') && header.isConnected
            && button === document.querySelector(selector) && button.isConnected
            && document.activeElement === button''',
            {'header': header, 'button': button, 'selector': selector}), selector


def check_header_metadata_refresh(page, corpus, base):
    """Open menus survive accepted metadata, while UID/agent changes replace them."""
    uid, agent = corpus.uid('claude-branch'), 'claude-agent-one'
    sidecar = corpus.paths[agent].with_suffix('.meta.json')
    original = sidecar.read_bytes()
    child_selector = f'#session-view-menu button[data-agent="{agent}"]'
    main_selector = '#session-view-menu button[data-agent=""]'
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
    try:
        # First refresh an open main-view menu with its child button focused.
        # Only the native sidecar description changes; transcript bytes stay intact.
        page.locator('#a-view-switch').click()
        child = page.locator(child_selector)
        expect(child).to_be_visible()
        child.focus()
        header, button = page.locator('#detail > .dhead').element_handle(), child.element_handle()
        sidecar.write_text(json.dumps({'description': 'Refreshed Claude child one', 'agentType': 'reviewer'}))
        refresh_catalog(page)
        expect(child.locator('b')).to_have_text('Refreshed Claude child one')
        assert_same_focused_header(page, header, button, child_selector)
        if scoped_frontend():
            expect(child).to_be_visible()
            expect(page.locator('#a-view-switch')).to_have_attribute('aria-expanded', 'true')
        elif not child.is_visible():
            page.locator('#a-view-switch').click()  # legacy intentionally remounts
        child.click()
        expect(page.locator('#msgs')).to_contain_text('Claude agent answer')
        expect(page.locator('#a-view-switch')).to_contain_text('Refreshed Claude child one')
        expect(page.locator('#session-view-menu')).to_be_hidden()
        if scoped_frontend():
            assert not header.evaluate('e => e.isConnected'), 'changing agent must replace the header'
        button.dispose()
        header.dispose()

        # The selected agent title and its menu row both derive from the new
        # description, even though the focused main-view button is unchanged.
        page.locator('#a-view-switch').click()
        main = page.locator(main_selector)
        expect(main).to_be_visible()
        main.focus()
        header, button = page.locator('#detail > .dhead').element_handle(), main.element_handle()
        sidecar.write_text(json.dumps({'description': 'Refreshed Claude child two', 'agentType': 'reviewer'}))
        refresh_catalog(page)
        expect(page.locator('#a-view-switch')).to_contain_text('Refreshed Claude child two')
        expect(page.locator(child_selector + ' b')).to_have_text('Refreshed Claude child two')
        assert_same_focused_header(page, header, button, main_selector)
        if scoped_frontend():
            expect(main).to_be_visible()
            expect(page.locator('#a-view-switch')).to_have_attribute('aria-expanded', 'true')
        elif not main.is_visible():
            page.locator('#a-view-switch').click()
        main.click()
        expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
        expect(page.locator('#session-view-menu')).to_be_hidden()
        if scoped_frontend():
            assert not header.evaluate('e => e.isConnected'), 'returning to main must replace the header'
        button.dispose()
        header.dispose()

        page.locator('#a-view-switch').click()
        header = page.locator('#detail > .dhead').element_handle()
        page.locator(f'#side .item[data-uid="{corpus.uid("claude-compact")}"]').click()
        expect(page.locator('#msgs')).to_contain_text('Claude post compact answer')
        expect(page.locator('#session-view-menu')).to_have_count(0)
        if scoped_frontend():
            assert not header.evaluate('e => e.isConnected'), 'changing UID must replace the header'
        header.dispose()
    finally:
        sidecar.write_bytes(original)
        refresh_catalog(page)

    # Keep a fork menu open while the HTTP writer changes both the selected
    # row and an ancestor. The latter must update the already visible chain.
    fork_uid, parent_uid = corpus.uid('codex-grandchild'), corpus.uid('codex-parent')
    page.locator(f'#side .item[data-uid="{fork_uid}"]').click()
    expect(page.locator('#msgs')).to_contain_text('Codex nested fork answer')
    page.locator('#a-fork-chain').click()
    toggle_selector = f'#fork-chain-menu .chain-row[data-uid="{parent_uid}"] .chain-toggle'
    toggle = page.locator(toggle_selector)
    expect(toggle).to_have_text('显示')
    toggle.focus()
    header, button = page.locator('#detail > .dhead').element_handle(), toggle.element_handle()
    response = page.request.post(base + '/api/sessions/fork-visibility', data={'uids': [parent_uid], 'visible': True})
    assert response.ok, response.text()
    response = page.request.post(base + '/api/session/star', data={'uid': fork_uid, 'starred': True})
    assert response.ok, response.text()
    refresh_catalog(page)
    expect(page.locator('#a-star')).to_have_attribute('aria-pressed', 'true')
    if scoped_frontend():
        expect(toggle).to_have_text('隐藏')
        expect(toggle).to_be_visible()
        expect(page.locator('#a-fork-chain')).to_have_attribute('aria-expanded', 'true')
        assert_same_focused_header(page, header, button, toggle_selector)
    elif not toggle.is_visible():
        page.locator('#a-fork-chain').click()
    expect(toggle).to_have_text('隐藏')
    with page.expect_response(lambda r: r.url.endswith('/api/sessions/fork-visibility') and r.request.method == 'POST') as saved:
        toggle.click()
    assert saved.value.ok, saved.value.text()
    expect(page.locator(f'#side .item[data-uid="{parent_uid}"]')).to_have_count(0)
    expect(toggle).to_have_text('显示')
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
    expect(page.locator('#fork-chain-menu')).to_have_count(0)
    expect(page.locator('#session-view-menu')).to_be_hidden()
    if scoped_frontend():
        assert not header.evaluate('e => e.isConnected'), 'fork menu must not cross session identities'
    button.dispose()
    header.dispose()
    response = page.request.post(base + '/api/session/star', data={'uid': fork_uid, 'starred': False})
    assert response.ok, response.text()
    refresh_catalog(page)


def check_shown_fork_chain(page, corpus):
    """BUG-20260929-162715-6739df: five saved-visible, same-title generations."""
    selectors = [f'#side .item[data-uid="{corpus.uid(f"shown-fork-{i}")}"]' for i in range(5)]
    labels = ['父会话（原始）'] + [f'父会话（分叉 {i}）' for i in range(1, 4)] + ['分叉 4']
    for i, (selector, label) in enumerate(zip(selectors, labels)):
        expect(page.locator(selector + ' .t')).to_have_text('Shared fork title')
        expect(page.locator(selector + ' .m')).to_contain_text(label)
        page.locator(selector).click()
        expect(page.locator('#msgs')).to_contain_text(f'Generation {i} answer')
    page.locator('#a-fork-chain').click()
    for i in range(4):
        page.locator(f'#fork-chain-menu .chain-row[data-uid="{corpus.uid(f"shown-fork-{i}")}"] .chain-toggle').click()
        expect(page.locator(selectors[i])).to_have_count(0)
    expect(page.locator(selectors[4])).to_be_visible()
    # Hidden ancestors remain navigable, and showing them restores the labels.
    page.locator(f'#fork-chain-menu .chain-row[data-uid="{corpus.uid("shown-fork-0")}"] .chain-open').click()
    expect(page.locator('#msgs')).to_contain_text('Generation 0 answer')
    page.locator(selectors[4]).click()
    page.locator('#a-fork-chain').click()
    for i in range(4):
        page.locator(f'#fork-chain-menu .chain-row[data-uid="{corpus.uid(f"shown-fork-{i}")}"] .chain-toggle').click()
        expect(page.locator(selectors[i] + ' .m')).to_contain_text(labels[i])
    page.locator('#a-fork-chain').click()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-metadata-browser-") as temporary:
        corpus = build_corpus(Path(temporary))
        inherited = [codex_message('user', 'Shared fork title')]
        for i in range(5):
            sid = f'shown-fork-{i}'
            inherited = [batch35_meta(sid, forked_from_id=f'shown-fork-{i-1}' if i else None),
                         *inherited, codex_message('assistant', f'Generation {i} answer')]
            corpus.put(sid, 'codex', inherited, [])
        native_before = {path: path.read_bytes() for path in corpus.paths.values()}
        state = corpus.root / "state"
        state.mkdir(mode=0o700)
        (state / 'session-metadata.json').write_text(json.dumps({
            'schema_version': 1, 'revision': 1,
            'sessions': {corpus.uid(f'shown-fork-{i}'): {'fork_parent_visible': True} for i in range(4)},
        }))
        with sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                with isolated_server(corpus, args.binary, state_dir=state) as (base, _):
                    context = browser.new_context(viewport={"width":1280,"height":900}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                    pages = [context.new_page(), context.new_page()]
                    errors = []
                    for page in pages:
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        page.goto(base, wait_until="networkidle")
                        expect(page.locator("#backend-notice")).to_be_hidden()  # no standing banner
                        page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]').click()
                        expect(page.locator("#msgs")).to_contain_text("Claude selected answer")
                        page.wait_for_function(js("_es && _es.readyState === EventSource.OPEN", 'runtime.core.sync.watching && runtime.core.sync.watching.readyState === EventSource.OPEN'))
                    first, other = pages
                    check_header_metadata_refresh(first, corpus, base)
                    check_shown_fork_chain(first, corpus)
                    button = f'#side .star-toggle[data-star-uid="{corpus.uid("claude-branch")}"]'
                    first.locator(button).click()
                    expect(first.locator(button)).to_have_attribute("aria-pressed", "true")
                    expect(other.locator(button)).to_have_attribute("aria-pressed", "true")
                    first.locator(button).click()
                    expect(other.locator(button)).to_have_attribute("aria-pressed", "false")
                    first.locator(button).click()
                    expect(other.locator(button)).to_have_attribute("aria-pressed", "true")
                    first.locator(f'#side .item[data-uid="{corpus.uid("codex-grandchild")}"]').click()
                    expect(first.locator("#msgs")).to_contain_text("Codex nested fork answer")
                    parent = f'#side .item[data-uid="{corpus.uid("codex-parent")}"]'
                    expect(first.locator(parent)).to_have_count(0)
                    # Opening the hidden parent from the chain leaves it out of the
                    # sidebar; its own menu is the way back to the branch.
                    first.locator("#a-fork-chain").click()
                    first.locator(f'#fork-chain-menu .chain-row[data-uid="{corpus.uid("codex-parent")}"] .chain-open').click()
                    expect(first.locator("#msgs")).to_contain_text("Codex discarded parent tail")
                    expect(first.locator(parent)).to_have_count(0)
                    expect(first.locator("#a-fork-chain")).to_have_attribute("title", "子会话")
                    first.locator("#a-fork-chain").click()
                    first.locator(f'#fork-chain-menu .chain-row[data-uid="{corpus.uid("codex-fork")}"] .chain-open').click()
                    expect(first.locator("#msgs")).to_contain_text("Codex fork answer")
                    first.locator(f'#side .item[data-uid="{corpus.uid("codex-grandchild")}"]').click()
                    expect(first.locator("#msgs")).to_contain_text("Codex nested fork answer")
                    first.locator("#a-fork-chain").click()
                    toggle = first.locator(f'#fork-chain-menu .chain-row[data-uid="{corpus.uid("codex-parent")}"] .chain-toggle')
                    toggle.click()
                    expect(first.locator(parent)).to_be_visible()
                    expect(toggle).to_have_text("隐藏")
                    first.set_viewport_size({"width":390,"height":844})
                    # Narrow layout may return to the sidebar; follow normal navigation.
                    if not first.locator("#a-term").is_visible():
                        first.locator(f'#side .item[data-uid="{corpus.uid("codex-grandchild")}"]').click()
                    expect(first.locator("#a-term")).to_be_visible()
                    expect(first.locator("#a-term")).to_be_enabled()
                    expect(first.locator(f'#side .item[data-uid="{corpus.uid("shown-fork-3")}"] .m')).to_contain_text('父会话（分叉 3）')
                    assert not errors, errors
                    context.close()  # Release SSE before shutting down the writer.
                with isolated_server(corpus, args.binary, state_dir=state) as (base, _):
                    context = browser.new_context(service_workers="block")
                    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                    page = context.new_page()
                    page.goto(base, wait_until="networkidle")
                    expect(page.locator(button)).to_have_attribute("aria-pressed", "true")
                    expect(page.locator(parent)).to_be_visible()
                    for i in range(5):
                        expect(page.locator(f'#side .item[data-uid="{corpus.uid(f"shown-fork-{i}")}"]')).to_be_visible()
                    context.close()
                assert all(path.read_bytes() == before for path, before in native_before.items())
                print("PASS preferences browser: main/agent and fork menus across metadata refresh (Vue DOM/focus identity), session switches, five same-title fork generations, open/hide/restore, cross-tab SSE star/unstar, parent visibility, mobile console, writer restart, native files unchanged")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
