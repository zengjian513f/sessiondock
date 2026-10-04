#!/usr/bin/env python3
"""Chromium search-state regression using synthetic sessions and private services.

Build beforehand; --binary selects that build and SESSIONDOCK_TEST_WEB_DIR is
honored by isolated_server. No real CLIs are used. A routed NDJSON response
is held until Escape has canceled its request.
"""
import argparse
import asyncio
import json
import os
from pathlib import Path
import tempfile
from urllib.parse import parse_qs, urlsplit

from playwright.async_api import expect, async_playwright
from history_parity import BINARY, Corpus, claude_row, get_json, isolated_server

VIEWPORTS = ((1280, 900), (390, 844))
MAIN = "search-state-main"
BODY = "search-state-body"


def corpus(root):
    data = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    for sid, title, answer in (
        (MAIN, "LocalFilter anchor", "PaneSentinel\n" + "Unchanged paragraph. " * 260 + "\nPaneTail"),
        (BODY, "Another synthetic session", "BodyNeedle response"),
        ("search-state-quiet", "Quiet synthetic session", "Quiet response"),
    ):
        data.put(sid, "claude", [claude_row(sid, "user", "u", None, title),
                                claude_row(sid, "assistant", "a", "u", answer)], [])
    return data


def is_search(request):
    return urlsplit(request.url).path == "/api/search"


async def exercise(browser, base, data, stale_payload, width, height):
    context = await browser.new_context(viewport={"width": width, "height": height},
                                        service_workers="block")
    await context.route("**/*", lambda route: route.continue_()
                        if route.request.url.startswith(base + "/") else route.abort())
    # Live transport is unrelated to this static fixture and can hold reloads open.
    await context.add_init_script("window.EventSource = undefined;")
    page = await context.new_page()
    errors, requests = [], []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on("request", lambda request: requests.append(request) if is_search(request) else None)
    q = page.locator("#q")
    rows = page.locator("#side .item[data-uid]")
    mode = page.locator("#search-mode-toggle")
    flags = {name: page.locator(f'#opts button[data-o="{name}"]')
             for name in ("case", "word", "regex")}
    pane = None
    release = None

    async def open_main():
        nonlocal pane
        await page.locator(f'#side .item[data-uid="{data.uid(MAIN)}"]').click()
        await expect(page.locator("#msgs")).to_contain_text("PaneSentinel")
        disclosure = page.locator('#msgs .msg[data-role="assistant"] button.disclosure')
        await expect(disclosure).to_contain_text("展开全文")
        await disclosure.click()
        await expect(disclosure).to_have_attribute("aria-expanded", "true")
        await expect(page.locator("#msgs")).to_contain_text("PaneTail")
        if width == 390:
            await page.locator(".mobile-back").click()
            await expect(q).to_be_visible()
        else:
            # Keep references to rendered DOM only, including the disclosure.
            await page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
            pane = await page.locator("#msgs").evaluate_handle("""root => {
                const message = root.querySelector('.msg[data-role="assistant"]');
                const body = message.querySelector('.mb');
                return {root, message, body, text: body.textContent, scroll: root.scrollTop,
                    disclosure: message.querySelector('button.disclosure')};
            }""")

    async def unchanged_pane():
        if pane is not None:
            assert await page.evaluate("""saved =>
                saved.root === document.querySelector('#msgs') && saved.message.isConnected
                && saved.body === saved.message.querySelector('.mb')
                && saved.disclosure === saved.message.querySelector('button.disclosure')
                && saved.disclosure.getAttribute('aria-expanded') === 'true'
                && saved.body.textContent === saved.text
                && Math.abs(saved.root.scrollTop - saved.scroll) <= 1
            """, pane), "Fulltext search replaced or changed the opened conversation"

    async def run(action, count, **options):
        old = await page.locator("#stat").get_attribute("data-seq") or ""
        before = len(requests)
        async with page.expect_request(is_search) as pending:
            await action()
        request = await pending.value
        params = parse_qs(urlsplit(request.url).query)
        expected = {"q": await q.input_value(), "progress": "1", **options}
        for key in ("q", "progress", "case", "word", "regex", "mode"):
            assert params.get(key) == ([expected[key]] if key in expected else None), params
        await page.wait_for_function("old => (document.querySelector('#stat').dataset.seq || '') !== old",
                                     arg=old, timeout=15000)
        await expect(page.locator("#search-progress")).to_be_hidden()
        await expect(page.locator("#side-search-label")).to_have_text("搜索结果")
        await expect(page.locator("#side-search-query")).to_have_text(await q.input_value())
        await expect(rows).to_have_count(count)
        assert len(requests) == before + 1, "One user action issued duplicate searches"
        await unchanged_pane()

    try:
        await page.goto(base, wait_until="domcontentloaded")
        await expect(rows).to_have_count(3)
        await expect(mode).to_have_text("AND")
        for button in flags.values():
            await expect(button).to_have_attribute("aria-pressed", "false")
        await open_main()

        await q.fill("LocalFilter")
        await expect(rows).to_have_count(1)
        await expect(page.locator("#side-search-label")).to_have_text("筛选结果")
        await expect(rows.first).to_have_attribute("data-uid", data.uid(MAIN))
        await q.fill("BodyNeedle")
        await expect(rows).to_have_count(0)
        assert not requests, "Typing a local filter issued a fulltext request"
        await run(lambda: q.press("Enter"), 1, mode="all")
        await expect(rows.first).to_have_attribute("data-uid", data.uid(BODY))

        before = len(requests)
        await q.fill("BodyNeedle MissingTerm")
        await expect(page.locator("#side-search-label")).to_have_text("筛选结果")
        await expect(rows).to_have_count(0)
        assert len(requests) == before, "Editing a completed search should return to local filtering"
        await run(lambda: q.press("Enter"), 0, mode="all")
        await run(mode.click, 1, mode="any")
        await expect(mode).to_have_text("OR")
        await run(flags["case"].click, 1, mode="any", case="1")
        await expect(flags["case"]).to_have_attribute("aria-pressed", "true")
        await run(flags["word"].click, 1, mode="any", case="1", word="1")
        await expect(flags["word"]).to_have_attribute("aria-pressed", "true")
        await run(flags["regex"].click, 0, case="1", word="1", regex="1")
        await expect(flags["regex"]).to_have_attribute("aria-pressed", "true")
        await expect(mode).to_be_disabled()

        if pane is not None:
            await pane.dispose()
            pane = None
        await page.reload(wait_until="domcontentloaded")
        # The sid URL restores detail asynchronously after the shell mounts.
        # Leave that restored view only after it is actually ready.
        await expect(page.locator("#msgs")).to_contain_text("PaneSentinel")
        if width == 390:
            await page.locator(".mobile-back").click()
        await expect(q).to_have_value("")
        await expect(rows).to_have_count(3)
        await expect(page.locator("#side-search-state")).to_be_hidden()
        for button in flags.values():
            await expect(button).to_have_attribute("aria-pressed", "true")
        await expect(mode).to_have_text("OR")
        await expect(mode).to_be_disabled()
        await open_main()
        await q.fill("BodyNeedle")
        await run(lambda: q.press("Enter"), 1, case="1", word="1", regex="1")
        await run(flags["regex"].click, 1, mode="any", case="1", word="1")
        await expect(flags["regex"]).to_have_attribute("aria-pressed", "false")
        await expect(mode).to_be_enabled()
        await expect(mode).to_have_text("OR")

        # Hold the actual transport request; release valid matches/progress/result
        # only after the browser reports that Escape aborted that exact request.
        started, release, finished = asyncio.Event(), asyncio.Event(), asyncio.Event()
        held, route_errors = [], []

        async def delayed(route):
            held.append(route.request)
            started.set()
            await release.wait()
            try:
                await route.fulfill(status=200, content_type="application/x-ndjson", body=stale_payload)
            except Exception as error:
                route_errors.append(error)
            finally:
                finished.set()

        await page.route("**/api/search?**", delayed)
        await q.fill("BodyNeedle")
        await q.press("Enter")
        await asyncio.wait_for(started.wait(), timeout=10)
        await expect(page.locator("#search-progress")).to_be_visible()
        async with page.expect_event("requestfailed", predicate=lambda request: request == held[0]):
            await q.press("Escape")
        await expect(q).to_have_value("")
        await expect(page.locator("#search-progress")).to_be_hidden()
        await expect(page.locator("#side-search-state")).to_be_hidden()
        await expect(rows).to_have_count(3)
        release.set()
        await asyncio.wait_for(finished.wait(), timeout=10)
        assert not route_errors, route_errors
        await page.unroute("**/api/search?**", delayed)
        # Drain browser task/render turns without a timing-based sleep.
        await page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
        await expect(q).to_have_value("")
        await expect(page.locator("#side-search-state")).to_be_hidden()
        await expect(page.locator("#stat")).not_to_contain_text("全文命中")
        await expect(rows).to_have_count(3)
        await unchanged_pane()
        await q.fill("BodyNeedle")
        await run(lambda: q.press("Enter"), 1, mode="any", case="1", word="1")
        assert not errors, errors
        print(f"PASS search state {width}x{height}: local/fulltext, flags, reload, Escape/stale response, pane identity",
              flush=True)
    finally:
        if release is not None:
            release.set()
        await context.close()


async def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-search-state-") as temporary:
        data = corpus(Path(temporary))
        state = data.root / "state"
        state.mkdir()
        with isolated_server(data, args.binary, state_dir=state,
                             extra_env={"SESSIONDOCK_SEARCH_CACHE_DIR": str(data.root / "search-cache")}) as (base, opener):
            result = get_json(opener, base, "/api/search?q=BodyNeedle")
            assert [row["uid"] for row in result["results"]] == [data.uid(BODY)]
            payload = "\n".join(json.dumps(event) for event in (
                {"type": "matches", "results": result["results"]},
                {"type": "progress", "done": 3, "total": 3},
                {"type": "result", "data": result},
            )) + "\n"
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            async with async_playwright() as playwright:
                browser = await playwright.chromium.launch(**launch)
                try:
                    for width, height in VIEWPORTS:
                        await exercise(browser, base, data, payload, width, height)
                finally:
                    await browser.close()


if __name__ == "__main__":
    asyncio.run(main())
