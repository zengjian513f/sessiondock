#!/usr/bin/env python3
"""Synthetic-only legacy Chromium search acceptance; no native homes or CLIs.

Build the server first, then run this script with Python Playwright/Chromium.
Every session is generated in a temporary directory and the server is bound to
an isolated loopback port. Tests drive the existing search box and flag buttons.
"""

import argparse
import json
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
            claude_row("search-main", "assistant", "a0", "u0", "Needle Cat cat caterpillar cat. 猫 猫猫 a.b A.B 无法识别的tag？ xtag"),
            claude_row("search-main", "assistant", "context-a", "a0",
                       "持久，跨板块扩散周期多变。但单纯扫窗口是纯参数网格搜索，缺乏机制突破，应附随先验分组进行差异化评估。"),
            claude_row("search-main", "user", "old-u", "a0", "DISCARDED_SEARCH_ONLY"),
            claude_row("search-main", "assistant", "old-a", "old-u", "DISCARDED_SEARCH_ANSWER"),
            {"type": "last-prompt", "leafUuid": "context-a"}]
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
                expect(page.locator('#side-search-state')).to_be_hidden()
                normal_background = page.locator('#side').evaluate('el => getComputedStyle(el).backgroundColor')

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
                expect(page.locator('#search-mode-toggle')).to_have_text("AND")
                expect(page.locator('#search-mode-toggle')).to_have_class("on")
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

                # The server supplies 40 characters before the hit. At the
                # minimum sidebar width that used to put the actual keyword
                # below the two visible snippet lines (BUG-20261001-075448).
                page.locator('#left').evaluate("el => el.style.width = '200px'")
                search('分组')
                snippet = page.locator(f'#side .item[data-uid="{data.uid("search-main")}"] .snip')

                def visible_snippet_hit():
                    expect(snippet.locator('mark')).to_have_text('分组')
                    geometry = snippet.evaluate('''el => {
                        const box = el.getBoundingClientRect(), hit = el.querySelector('mark').getBoundingClientRect();
                        return {top: box.top, bottom: box.bottom, hitTop: hit.top, hitBottom: hit.bottom};
                    }''')
                    assert geometry['hitTop'] >= geometry['top'] and geometry['hitBottom'] <= geometry['bottom'], geometry

                visible_snippet_hit()
                flag('case')  # Reuses the existing row through patchSidebarRow.
                visible_snippet_hit()
                flag('case')
                flag('regex')
                visible_snippet_hit()
                flag('regex')
                page.locator('#left').evaluate("(el, width) => el.style.width = width", original_width)

                search("needle")
                expect(page.locator('#side-search-state')).to_be_visible()
                expect(page.locator('#side-search-query')).to_have_text('needle')
                expect(page.locator('#side-search-count')).to_have_text('2 条')
                assert page.locator('#side').evaluate('el => getComputedStyle(el).backgroundColor') != normal_background
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
                expect(page.locator('#search-mode-toggle')).to_have_class("on")
                search("Needle Synthetic")
                expect(page.locator("#side .item[data-uid]")).to_have_count(2)
                mode("all")
                search('"Needle Cat" Synthetic')
                expect(page.locator("#side .item[data-uid]")).to_have_count(1)
                search('"Needle Synthetic"')
                expect(page.locator("#side .item[data-uid]")).to_have_count(0)

                search("tag")
                hits(2)
                flag("word")
                hits(1)
                main = page.locator(f'#side .item[data-uid="{data.uid("search-main")}"]')
                expect(main.locator(".snip")).to_contain_text("无法识别的tag")
                main.click()
                expect(page.locator("#msgs mark")).to_have_count(1)
                expect(page.locator("#msgs mark")).to_have_text("tag")
                assert page.evaluate("""() => {
                    const mark = document.querySelector('#msgs mark');
                    return mark.previousSibling.textContent.endsWith('的');
                }""")
                flag("word")

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
                expect(page.locator("#search-mode-toggle")).to_have_class("on")
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

                # Proxy-buffered NDJSON must not rebuild the entire sidebar
                # for every match record (BUG-20260927-070141-5ebf14).
                result = page.request.get(base + "/api/search?q=Needle").json()
                burst = [
                    {"type": "matches", "results": result["results"]},
                    {"type": "progress", "done": 1, "total": 2},
                ] * 1000
                payload = "\n".join(json.dumps(event) for event in [
                    *burst, {"type": "result", "data": result},
                ]) + "\n"
                page.route("**/api/search?**", lambda route: route.fulfill(
                    status=200, content_type="application/x-ndjson", body=payload))
                page.evaluate("""() => {
                    window.searchPaints = 0;
                    const original = showSearchMatches;
                    showSearchMatches = rows => { window.searchPaints++; original(rows); };
                }""")
                search("Needle")
                expect(page.locator("#side .item[data-uid]")).to_have_count(2)
                paints = page.evaluate("window.searchPaints")
                assert 1 <= paints < 20, paints
                page.unroute("**/api/search?**")

                # Keep a synthetic stream open while the user hits Backspace.
                # Its late result must not restore the canceled search.
                page.evaluate(r"""result => {
                    const original = window.fetch;
                    window.fetch = (url, opts) => {
                        if (!String(url).includes('api/search?')) return original(url, opts);
                        const encoder = new TextEncoder();
                        window.lateSearchResult = null;
                        return Promise.resolve(new Response(new ReadableStream({
                            start(controller) {
                                controller.enqueue(encoder.encode(JSON.stringify({
                                    type: 'matches', results: result.results
                                }) + '\n'));
                                window.lateSearchResult = () => {
                                    controller.enqueue(encoder.encode(JSON.stringify({
                                        type: 'result', data: result
                                    }) + '\n'));
                                    controller.close();
                                };
                            }
                        }), {headers: {'Content-Type': 'application/x-ndjson'}}));
                    };
                    window.restoreSearchFetch = () => { window.fetch = original; };
                }""", result)
                page.locator("#q").fill("Needle")
                page.locator("#q").press("Enter")
                expect(page.locator("#side-search-count")).to_have_text("2 条")
                page.locator("#q").press("Backspace")
                expect(page.locator("#q")).to_have_value("Needl")
                expect(page.locator("#search-progress")).not_to_be_visible()
                expect(page.locator("#side-search-label")).to_have_text("筛选结果")
                page.evaluate("window.lateSearchResult(); window.restoreSearchFetch()")
                page.wait_for_timeout(100)
                expect(page.locator("#side-search-label")).to_have_text("筛选结果")
                expect(page.locator("#side .item[data-uid]")).to_have_count(0)
                print(f"PASS buffered search: 2000 NDJSON records, {paints} result paints; Backspace cancels late results")

                # Reloading the page clears the search;
                # the next search still uses the independently namespaced flags.
                page.reload(wait_until="networkidle")
                expect(page.locator("#q")).to_have_value("")
                expect(page.locator("#side .item[data-uid]")).to_have_count(3)
                expect(page.locator('#side-search-state')).to_be_hidden()
                # The search mode remains obvious inside the list area at both
                # widths; its heading stays put when the session list scrolls.
                for width in (1280, 390):
                    page.set_viewport_size({'width': width, 'height': 700})
                    page.reload(wait_until='networkidle')
                    if width < 600 and page.locator('.mobile-back').is_visible():
                        page.locator('.mobile-back').click()
                    search('Needle')
                    banner = page.locator('#side-search-state')
                    expect(banner).to_be_visible()
                    before = banner.bounding_box()
                    assert before['height'] <= 32, before
                    page.locator('#side').evaluate("el => { el.style.height = '60px'; el.style.flex = 'none'; el.scrollTop = el.scrollHeight; }")
                    assert banner.bounding_box()['y'] == before['y']
                    expect(page.locator('#side-search-exit')).to_be_in_viewport()
                    assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                    page.locator('#side-search-exit').click()
                    expect(banner).to_be_hidden()
                    expect(page.locator('#side .item[data-uid]')).to_have_count(3)
                    assert page.locator('#side').evaluate('el => getComputedStyle(el).backgroundColor') == normal_background
                    page.locator('#side').evaluate("el => { el.style.height = ''; el.style.flex = ''; }")
                    page.locator('#q').fill('很长的搜索条件' * 20)
                    expect(banner).to_be_visible()
                    assert banner.bounding_box()['height'] <= 32
                    expect(page.locator('#side-search-exit')).to_be_in_viewport()
                    assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                    search('no-such-session')
                    expect(page.locator('#side')).to_contain_text('当前搜索无匹配会话')
                    page.get_by_role('button', name='返回全部会话', exact=True).click()
                    expect(banner).to_be_hidden()
                    expect(page.locator('#q')).to_have_value('')
                    expect(page.locator('#side .item[data-uid]')).to_have_count(3)
                assert not errors, errors
                assert all(url.startswith(base + "/") for url in requests)
                assert any(status == 200 and "application/x-ndjson" in mime for _, status, mime in searches)
                assert all("progress=1" in url for url, _, _ in searches)
                print("PASS legacy search browser: AND/OR across messages, phrases, per-term highlighting, stored mode, inline regex, NDJSON results/progress and flags")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
