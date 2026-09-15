#!/usr/bin/env python3
"""Synthetic-only legacy Chromium search acceptance; no native homes or CLIs.

Build the server first, then run this script with Python Playwright/Chromium.
Every session is generated in a temporary directory and the server is bound to
an isolated loopback port. Tests drive the existing search box and flag buttons.
"""

import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, Corpus, claude_row, codex_message, codex_row, isolated_server


def corpus(root):
    data = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    rows = [claude_row("search-main", "user", "u0", None, "Synthetic search title"),
            claude_row("search-main", "assistant", "a0", "u0", "Needle Cat cat caterpillar cat. 猫 猫猫 a.b A.B"),
            claude_row("search-main", "user", "old-u", "a0", "DISCARDED_SEARCH_ONLY"),
            claude_row("search-main", "assistant", "old-a", "old-u", "DISCARDED_SEARCH_ANSWER"),
            {"type": "last-prompt", "leafUuid": "a0"}]
    data.put("search-main", "claude", rows, [])
    data.put("search-broken", "claude", [
        claude_row("search-broken", "user", "u0", None, "Unsupported synthetic history"),
        # Scalar content is unreadable (unknown kinds are skipped).
        {"type": "user", "message": {"role": "user", "content": 42}}], [])
    data.put("codex-search", "codex", [
        codex_row("session_meta", {"id": "codex-search", "cwd": "/synthetic/search"}),
        codex_message("user", "Codex browser fixture"),
        codex_message("assistant", "Needle from another provider")], [])
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-search-browser-") as directory:
        data = corpus(Path(directory))
        with isolated_server(data, args.binary) as (base, _opener), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                context.route("**/*", lambda route: route.continue_()
                              if route.request.url.startswith(base + "/") else route.abort())
                page = context.new_page()
                errors, requests, searches = [], [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("request", lambda request: requests.append(request.url))
                page.on("response", lambda response: searches.append((response.url, response.status,
                    response.headers.get("content-type", ""))) if "/api/search?" in response.url else None)
                page.goto(base, wait_until="networkidle")
                assert page.evaluate("SessionDockCapabilities.allows('search')") is True
                expect(page.locator("#backend-notice")).to_be_hidden()  # no standing banner

                def single_row():
                    layout = page.evaluate("""() => {
                        const box = document.querySelector('.qbox').getBoundingClientRect();
                        return {left: box.left, right: box.right,
                            controls: [...document.querySelectorAll('.qbox input, .qbox button')]
                                .map(el => { const r = el.getBoundingClientRect();
                                    return {left: r.left, right: r.right, center: r.top + r.height / 2}; })};
                    }""")
                    centers = [control["center"] for control in layout["controls"]]
                    assert len(centers) == 5 and max(centers) - min(centers) < 1, layout
                    assert all(control["left"] >= layout["left"] and control["right"] <= layout["right"]
                               for control in layout["controls"]), layout

                single_row()
                original_width = page.locator('#left').evaluate("el => el.style.width")
                page.locator('#left').evaluate("el => el.style.width = '200px'")
                single_row()
                page.locator('#left').evaluate("(el, width) => el.style.width = width", original_width)
                page.locator(f'#side .item[data-uid="{data.uid("search-main")}"]').click()
                expect(page.locator("#msgs")).to_contain_text("Needle Cat cat")

                def wait_search(old):
                    page.wait_for_function("old => String(document.querySelector('#stat').dataset.seq || '') !== old", arg=old)
                    expect(page.locator("#search-progress")).not_to_be_visible()

                def search(query):
                    old = page.locator("#stat").get_attribute("data-seq") or ""
                    page.locator("#q").fill(query)
                    page.locator("#q").press("Enter")
                    wait_search(old)

                def flag(name):
                    old = page.locator("#stat").get_attribute("data-seq") or ""
                    page.locator(f'#opts button[data-o="{name}"]').click()
                    wait_search(old)

                def mode(name):
                    old = page.locator("#stat").get_attribute("data-seq") or ""
                    toggle = page.locator('#search-mode-toggle')
                    assert toggle.inner_text() != ("OR" if name == "any" else "AND")
                    toggle.click()
                    wait_search(old)

                def hits(expected):
                    # Read the actual rendered result row, not a private state
                    # override; the highlighted snippet and hit badge coexist.
                    item = page.locator(f'#side .item[data-uid="{data.uid("search-main")}"]')
                    expect(item).to_be_visible()
                    expect(item.locator(".m")).to_contain_text(f"命中 {expected}")

                search("needle")
                expect(page.locator("#side .item[data-uid]")).to_have_count(2)
                expect(page.locator("#stat")).to_contain_text("结果不完整")
                expect(page.locator("#stat")).to_contain_text("Unsupported synthetic history")
                expect(page.locator("#msgs")).to_contain_text("Needle Cat cat")
                page.locator(f'#side .item[data-uid="{data.uid("codex-search")}"]').click()
                expect(page.locator("#msgs")).to_contain_text("Needle from another provider")
                expect(page.locator("#a-term")).to_be_visible()
                expect(page.locator("#a-term")).to_be_enabled()

                # Session-level AND spans messages; both terms are highlighted.
                search("Needle Synthetic")
                expect(page.locator("#side .item[data-uid]")).to_have_count(1)
                main = page.locator(f'#side .item[data-uid="{data.uid("search-main")}"]')
                expect(main.locator(".snip")).to_contain_text("Needle")
                expect(main.locator(".snip")).to_contain_text("Synthetic")
                main.click()
                expect(page.locator("#msgs mark").filter(has_text="Needle")).to_have_count(1)
                expect(page.locator("#msgs mark").filter(has_text="Synthetic")).to_have_count(1)
                mode("any")
                expect(page.locator("#side .item[data-uid]")).to_have_count(2)
                assert "mode=any" in searches[-1][0]
                page.reload(wait_until="networkidle")
                expect(page.locator('#search-mode-toggle')).to_have_text("OR")
                expect(page.locator('#search-mode-toggle')).to_have_attribute("aria-pressed", "true")
                search("Needle Synthetic")
                expect(page.locator("#side .item[data-uid]")).to_have_count(2)
                mode("all")
                search('"Needle Cat" Synthetic')
                expect(page.locator("#side .item[data-uid]")).to_have_count(1)
                search('"Needle Synthetic"')
                expect(page.locator("#side .item[data-uid]")).to_have_count(0)

                search("cat")
                hits(4)
                flag("word")
                hits(3)
                flag("case")
                hits(2)
                search("c.t")
                expect(page.locator("#side .item[data-uid]")).to_have_count(0)
                flag("regex")
                expect(page.locator("#search-mode-toggle")).to_be_disabled()
                single_row()
                hits(2)
                assert any("word=1" in url and "case=1" in url and "regex=1" in url for url, _, _ in searches)
                page.locator(f'#side .item[data-uid="{data.uid("search-main")}"]').click()
                expect(page.locator("#msgs")).to_contain_text("Needle Cat cat")
                expect(page.locator("#msgs")).not_to_contain_text("DISCARDED_SEARCH_ONLY")

                # Lookahead is accepted by the server.
                search("(?=cat)")
                expect(page.locator("#stat")).to_contain_text("全文命中")
                expect(page.locator("#stat")).not_to_have_class("err")
                assert searches[-1][1] == 200
                expect(page.locator("#a-term")).to_be_visible()
                search("cat")
                hits(2)
                expect(page.locator("#stat")).not_to_have_class("err")

                # Normal reload clears the search using the legacy control;
                # the next search still uses the independently namespaced flags.
                page.locator("#reload").click()
                expect(page.locator("#q")).to_have_value("")
                expect(page.locator("#side .item[data-uid]")).to_have_count(3)
                assert not errors, errors
                assert all(url.startswith(base + "/") for url in requests)
                assert any(status == 200 and "application/x-ndjson" in mime for _, status, mime in searches)
                assert all("progress=1" in url for url, _, _ in searches)
                print("PASS legacy search browser: AND/OR across messages, phrases, per-term highlighting, stored mode, inline regex, NDJSON results/progress and flags")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
