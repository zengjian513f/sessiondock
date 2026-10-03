#!/usr/bin/env python3
"""Synthetic preferences through the real legacy UI; no original state or CLI."""
from browser_runtime import js
import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, batch35_meta, build_corpus, codex_message, isolated_server


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
                print("PASS preferences browser: five same-title fork generations, open/hide/restore, cross-tab SSE star/unstar, parent visibility, mobile console, writer restart, native files unchanged")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
