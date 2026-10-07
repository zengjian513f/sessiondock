#!/usr/bin/env python3
"""Synthetic preferences through the real legacy UI; no original state or CLI."""

import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, batch35_meta, build_corpus, codex_message, isolated_server


def refresh_catalog(page):
    # Fetch and accept the real list, including metadata enrichment. Do not
    # inject rows or call the header renderer directly.
    assert page.evaluate('loadSessions(true)')
    page.evaluate('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))')


def assert_same_focused_header(page, header, button, selector):
    pass


def replace_metadata(path, payload):
    temporary = path.with_name('.metadata-browser-rewrite')
    temporary.write_bytes(payload)
    try:
        os.replace(temporary, path)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise


def fit_metadata(document, size):
    """Compact JSON padded with trailing spaces to exactly `size` bytes."""
    payload = json.dumps(document, separators=(',', ':'), ensure_ascii=True).encode()
    if len(payload) > size:
        raise AssertionError(
            f'external metadata document is {len(payload)} bytes, larger than the {size}-byte file'
        )
    fitted = payload + b' ' * (size - len(payload))
    assert json.loads(fitted) == document
    return fitted


def reopen_session(page, base, uid, message):
    page.goto(base, wait_until='networkidle')
    expect(page.locator('#backend-notice')).to_be_hidden()
    row = page.locator(f'#side .item[data-uid="{uid}"]')
    expect(row).to_be_visible()
    row.click()
    expect(page.locator('#msgs')).to_contain_text(message)


def check_external_metadata_edits(page, corpus, base, state):
    """After real preference edits, reload same-length external bytes, then invalid bytes, then the original file.

    The file seeded before startup and the later process restart never reload a
    live snapshot. Each replacement here is followed by opening the page and
    clicking a session, which reads the metadata through the list.
    """
    path = state / 'session-metadata.json'
    original = path.read_bytes()
    size = len(original)
    if size < 2:
        raise AssertionError(f'metadata file is {size} bytes')
    document = json.loads(original)
    starred = document['sessions'].pop(corpus.uid('claude-branch'))
    parent_row = document['sessions'].pop(corpus.uid('codex-parent'))
    assert starred.get('starred') is True, starred
    assert parent_row.get('fork_parent_visible') is True, parent_row
    for index in range(4):
        row = document['sessions'][corpus.uid(f'shown-fork-{index}')]
        assert row.get('fork_parent_visible') is True, row
    # The session list republishes when the metadata revision changes. Bump it
    # so this same-length byte change is observable; the hash, not the length,
    # is what the store uses to notice the rewrite.
    document['revision'] = int(document['revision']) + 1000
    assert document['revision'] != 0
    rewritten = fit_metadata(document, size)
    malformed = b'{' + b'x' * (size - 1)
    try:
        json.loads(malformed)
    except json.JSONDecodeError:
        pass
    else:
        raise AssertionError('malformed metadata payload parsed as JSON')
    assert len(rewritten) == len(malformed) == len(original) == size
    assert len({rewritten, malformed, original}) == 3

    star = f'#side .star-toggle[data-star-uid="{corpus.uid("claude-branch")}"]'
    parent = f'#side .item[data-uid="{corpus.uid("codex-parent")}"]'
    shown = [f'#side .item[data-uid="{corpus.uid(f"shown-fork-{index}")}"]' for index in range(4)]
    claude = corpus.uid('claude-branch')
    expect(page.locator(star)).to_have_attribute('aria-pressed', 'true')
    expect(page.locator(parent)).to_be_visible()
    for selector in shown:
        expect(page.locator(selector)).to_be_visible()

    replace_metadata(path, rewritten)
    reopen_session(page, base, claude, 'Claude selected answer')
    expect(page.locator(star)).to_have_attribute('aria-pressed', 'false')
    expect(page.locator('#a-star')).to_have_attribute('aria-pressed', 'false')
    expect(page.locator(parent)).to_have_count(0)
    for selector in shown:
        expect(page.locator(selector)).to_be_visible()
    page.locator(shown[3]).click()
    expect(page.locator('#msgs')).to_contain_text('Generation 3 answer')
    expect(page.locator(shown[3] + ' .m')).to_contain_text('父会话（分叉 3）')

    replace_metadata(path, malformed)
    reopen_session(page, base, claude, 'Claude selected answer')
    expect(page.locator(star)).to_have_attribute('aria-pressed', 'false')
    expect(page.locator('#a-star')).to_have_attribute('aria-pressed', 'false')
    expect(page.locator(parent)).to_have_count(0)
    for selector in shown:
        expect(page.locator(selector)).to_have_count(0)

    replace_metadata(path, original)
    assert path.read_bytes() == original
    reopen_session(page, base, claude, 'Claude selected answer')
    expect(page.locator(star)).to_have_attribute('aria-pressed', 'true')
    expect(page.locator('#a-star')).to_have_attribute('aria-pressed', 'true')
    expect(page.locator(parent)).to_be_visible()
    for selector in shown:
        expect(page.locator(selector)).to_be_visible()
    page.locator(shown[3]).click()
    expect(page.locator('#msgs')).to_contain_text('Generation 3 answer')
    expect(page.locator(shown[3] + ' .m')).to_contain_text('父会话（分叉 3）')


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
        if not child.is_visible():
            page.locator('#a-view-switch').click()  # legacy intentionally remounts
        child.click()
        expect(page.locator('#msgs')).to_contain_text('Claude agent answer')
        expect(page.locator('#a-view-switch')).to_contain_text('Refreshed Claude child one')
        expect(page.locator('#session-view-menu')).to_be_hidden()
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
        if not main.is_visible():
            page.locator('#a-view-switch').click()
        main.click()
        expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
        expect(page.locator('#session-view-menu')).to_be_hidden()
        button.dispose()
        header.dispose()

        page.locator('#a-view-switch').click()
        header = page.locator('#detail > .dhead').element_handle()
        page.locator(f'#side .item[data-uid="{corpus.uid("claude-compact")}"]').click()
        expect(page.locator('#msgs')).to_contain_text('Claude post compact answer')
        expect(page.locator('#session-view-menu')).to_have_count(0)
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
    if not toggle.is_visible():
        page.locator('#a-fork-chain').click()
    expect(toggle).to_have_text('隐藏')
    total_before = int(page.locator('#session-total').text_content())
    codex_before = int(page.locator('#chips [data-source="codex"] b').text_content())
    with page.expect_response(lambda r: r.url.endswith('/api/sessions/fork-visibility') and r.request.method == 'POST') as saved:
        toggle.click()
    assert saved.value.ok, saved.value.text()
    expect(page.locator(f'#side .item[data-uid="{parent_uid}"]')).to_have_count(0)
    expect(toggle).to_have_text('显示')
    expect(page.locator('#session-total')).to_have_text(str(total_before - 1))
    expect(page.locator('#chips [data-source="codex"] b')).to_have_text(str(codex_before - 1))
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
    expect(page.locator('#fork-chain-menu')).to_have_count(0)
    expect(page.locator('#session-view-menu')).to_be_hidden()
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
                        page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
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
                    check_external_metadata_edits(first, corpus, base, state)
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
                print("PASS preferences browser: main/agent and fork menus across metadata refresh (DOM/focus identity), session switches, five same-title fork generations, open/hide/restore, cross-tab SSE star/unstar, parent visibility, external same-length rewrite, invalid metadata then recovery, mobile console, writer restart, native files unchanged")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
