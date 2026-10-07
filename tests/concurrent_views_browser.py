#!/usr/bin/env python3
"""Concurrent real Chromium opens, paging, rewrites and deletion/recreation.

Only private synthetic regular files and loopback HTTP are used. Browser route
barriers release actual UI requests together; they do not pause backend parsing
or establish a latency improvement. Cache introspection below is read-only.
"""
from __future__ import annotations

import argparse
import asyncio
import os
from pathlib import Path
import tempfile
from urllib.parse import parse_qs, urlsplit

from playwright.async_api import async_playwright, expect
from history_fixtures import BINARY, Corpus, codex_message, codex_row, encoded, isolated_server


SIZES = {"large": 2400, "medium": 850, "small": 12}


def records(sid, count, generation="OLD", padding=0):
    yield codex_row("session_meta", {"id": sid, "cwd": "/synthetic/concurrent-views"})
    for index in range(count):
        row = codex_message("user", f"{sid.upper()} {generation} ROW {index:04d}", index + 1)
        row["payload"]["turn_id"] = f"turn-{index}"
        # Increases native read/AST work without flooding the rendered DOM.
        row["payload"]["synthetic_padding"] = "x" * padding
        yield row


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    for sid, count in SIZES.items():
        corpus.put(sid, "codex", list(records(sid, count, padding=8192 if sid == "large" else 0)), [])
    parent = corpus.put("prefix-parent", "codex", list(records("prefix-parent", 120, padding=1024)), [])
    cut = parent.stat().st_size
    corpus.put("fork", "codex", [
        codex_row("session_meta", {"id": "fork", "cwd": "/synthetic/concurrent-views",
                                  "forked_from_id": "prefix-parent", "history_base": {
                                      "thread_id": "prefix-parent", "end_byte_offset": cut}}),
        codex_message("user", "FORK OWN MESSAGE")], [])
    return corpus


READ = """() => {
  const e = cache.get(viewKey(S.sel, S.agent));
  return e ? {uid:S.sel, text:e.msgs.map(m=>m.text), partial:e.partial,
              end:e.end, anchor:e.anchor, version:e.version,
              cursor:S.cursors.get(viewKey(S.sel,S.agent))} : null;
}"""


class Barrier:
    def __init__(self):
        self.remaining = 0
        self.release = asyncio.Event()
        self.release.set()

    def arm(self, count):
        assert self.remaining == 0
        self.remaining = count
        self.release = asyncio.Event()

    async def arrive(self):
        if self.remaining:
            self.remaining -= 1
            release = self.release
            if self.remaining == 0:
                release.set()
            await asyncio.wait_for(release.wait(), timeout=30)


async def browser_check(corpus, base):
    async with async_playwright() as playwright:
        options = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = await playwright.chromium.launch(**options)
        try:
            # Independent clients have independent HTTP/1 connection pools.
            # Five tabs sharing one context exhaust Chromium's six connections
            # with their live SSE streams before history pages can be fetched.
            contexts = [await browser.new_context(viewport={"width": 1440, "height": 900},
                                                  service_workers="block") for _ in range(5)]
            barrier = Barrier()
            errors, wire, captures = [], [], set()

            async def route_network(route):
                url = route.request.url
                if not url.startswith(base + "/"):
                    await route.abort()
                    return
                path = urlsplit(url).path
                if path.startswith("/api/messages/") and path.count("/") == 3:
                    await barrier.arrive()
                await route.continue_()

            for context in contexts:
                await context.route("**/*", route_network)

            async def capture(response):
                path = urlsplit(response.url).path
                if not path.startswith("/api/messages/"):
                    return
                if response.status != 200:
                    print("HISTORY RESPONSE", response.status, path, (await response.text())[:400], flush=True)
                    return
                try:
                    body = await response.json()
                except Exception:
                    # A page navigation may cancel an old request body.
                    return
                wire.append((response.url, body))

            def record_response(response):
                task = asyncio.create_task(capture(response))
                captures.add(task)
                task.add_done_callback(captures.discard)

            pages = [await context.new_page() for context in contexts]
            for page in pages:
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("response", record_response)
                page.set_default_timeout(60000)
            await asyncio.gather(*(page.goto(base + "/", wait_until="domcontentloaded") for page in pages))
            assignments = ["large", "small", "medium", "large", "fork"]

            async def opened(page, sid, generation="OLD"):
                marker = "FORK OWN MESSAGE" if sid == "fork" else f"{sid.upper()} {generation} ROW {SIZES[sid]-1:04d}"
                await expect(page.locator("#msgs")).to_contain_text(marker)
                await page.wait_for_function("uid => S.sel === uid && !!cache.get(viewKey(S.sel,S.agent))", arg=corpus.uid(sid))
                return await page.evaluate(READ)

            async def select(page, sid):
                if sid == "fork":
                    # The public SID deep link opens a hidden fork through the
                    # ordinary page flow, with the same backend as a row click.
                    await page.goto(base + "/?sid=fork", wait_until="domcontentloaded")
                else:
                    await page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                return await opened(page, sid)

            async def refresh(page, sid, generation="NEW"):
                await page.goto(base + f"/?sid={sid}", wait_until="domcontentloaded")
                return await opened(page, sid, generation if sid == "large" else "OLD")

            barrier.arm(len(pages))
            first = await asyncio.wait_for(asyncio.gather(*(select(page, sid) for page, sid in zip(pages, assignments))), timeout=90)
            assert first[0]["end"] == first[3]["end"] and first[0]["anchor"] == first[3]["anchor"]
            assert first[1]["text"] == [f"SMALL OLD ROW {index:04d}" for index in range(SIZES["small"])]
            assert first[4]["text"] == [f"PREFIX-PARENT OLD ROW {index:04d}" for index in range(120)] + ["FORK OWN MESSAGE"]
            print("PASS concurrent opens: differently sized owners, duplicate owner and inherited prefix", flush=True)

            async def complete_history(page, sid, before):
                if not before["partial"]:
                    return
                await page.locator(".history-gap-load").scroll_into_view_if_needed()
                await page.locator(".history-gap-load").click()
                # The actual gap button may chain multiple finite pages.
                try:
                    await page.wait_for_function("() => { const e=cache.get(viewKey(S.sel,S.agent)); return e && !e.partial && historyPageRequests.size===0; }")
                except Exception:
                    print("PAGING STATE", sid, await page.evaluate("""() => {
                      const e=cache.get(viewKey(S.sel,S.agent));
                      return {uid:S.sel, count:e?.msgs.length, partial:e?.partial,
                        requests:historyPageRequests.size, error:document.querySelector('.history-page-error')?.textContent};
                    }"""), flush=True)
                    raise
                after = await page.evaluate(READ)
                assert after["text"] == [f"{sid.upper()} OLD ROW {index:04d}" for index in range(SIZES[sid])]
                for field in ("end", "anchor", "version", "cursor"):
                    assert after[field] == before[field], (sid, field)

            await asyncio.gather(*(complete_history(page, sid, before) for page, sid, before in zip(pages, assignments, first)))
            if captures:
                await asyncio.gather(*tuple(captures))
            history_pages = [(url, body) for url, body in wire if "/page?" in url]
            assert history_pages, "gap clicks did not fetch real history pages"
            for url, body in history_pages:
                descriptor = body["page"]
                assert descriptor["cursor"] == parse_qs(urlsplit(url).query)["cursor"][0]
                assert 0 <= descriptor["start"] < descriptor["end"] <= descriptor["stop"]
                assert descriptor["remaining"] == descriptor["stop"] - descriptor["end"]
                assert len(body["messages"]) == descriptor["end"] - descriptor["start"]
            print("PASS concurrent gap clicks: exact ordered histories, page ranges and stable live cursors", flush=True)

            with corpus.paths["small"].open("ab") as stream:
                stream.write(encoded(codex_message("user", "SMALL APPEND DURING CONCURRENT REFRESH", 100)))
            barrier.arm(len(pages))
            appended = await asyncio.wait_for(asyncio.gather(*(refresh(page, sid, "OLD") for page, sid in zip(pages, assignments))), timeout=90)
            assert appended[1]["text"] == first[1]["text"] + ["SMALL APPEND DURING CONCURRENT REFRESH"]
            assert appended[1]["end"] > first[1]["end"] and appended[1]["anchor"] != first[1]["anchor"]
            for index in (0, 2, 3, 4):
                assert appended[index]["end"] == first[index]["end"] and appended[index]["anchor"] == first[index]["anchor"]
            first[1] = appended[1]
            print("PASS append with simultaneous refreshes: complete suffix and independent checkpoints", flush=True)

            # Same-length rewrite keeps mtime; old AST/projection reuse must
            # still validate the actual bytes. Replacing the inode also checks
            # stamped-source authority across simultaneous refreshes.
            original = corpus.paths["large"]
            parent = corpus.paths["prefix-parent"]
            for path, sid, count, padding in ((original, "large", SIZES["large"], 8192),
                                               (parent, "prefix-parent", 120, 1024)):
                previous_stat = path.stat()
                replacement = path.with_suffix(".replacement")
                replacement.write_bytes(b"".join(encoded(row) for row in records(sid, count, "NEW", padding)))
                assert replacement.stat().st_size == previous_stat.st_size
                os.utime(replacement, ns=(previous_stat.st_atime_ns, previous_stat.st_mtime_ns))
                replacement.replace(path)
            barrier.arm(len(pages))
            rewritten = await asyncio.wait_for(asyncio.gather(*(refresh(page, sid) for page, sid in zip(pages, assignments))), timeout=90)
            for index in (0, 3):
                assert rewritten[index]["anchor"] != first[index]["anchor"]
                assert all("LARGE OLD ROW" not in text for text in rewritten[index]["text"])
            assert rewritten[4]["text"] == [f"PREFIX-PARENT NEW ROW {index:04d}" for index in range(120)] + ["FORK OWN MESSAGE"]
            assert rewritten[1]["anchor"] == first[1]["anchor"]
            assert rewritten[2]["anchor"] == first[2]["anchor"]
            print("PASS concurrent refresh: rewritten owner and inherited bytes replace old results; other owners stable", flush=True)

            # Remove our native fixture, then use ordinary page navigation to
            # force fresh lists/opens. No old cached transcript may return.
            original.unlink()
            parent.unlink()
            await asyncio.gather(*(page.goto(base + f"/?sid={sid}", wait_until="domcontentloaded") for page, sid in zip(pages, assignments)))
            for index in (0, 3):
                await expect(pages[index].locator(f'#side .item[data-uid="{corpus.uid("large")}"]')).to_have_count(0)
                await expect(pages[index].locator("#detail")).not_to_contain_text("LARGE NEW ROW")
            await opened(pages[1], "small")
            await opened(pages[2], "medium")
            await expect(pages[4].locator("#detail")).not_to_contain_text("PREFIX-PARENT NEW ROW")

            # The same UID returns with entirely different native contents.
            original.write_bytes(b"".join(encoded(row) for row in records("large", SIZES["large"], "NOW", 8192)))
            parent.write_bytes(b"".join(encoded(row) for row in records("prefix-parent", 120, "NOW", 1024)))
            barrier.arm(len(pages))
            restored = await asyncio.wait_for(asyncio.gather(*(refresh(page, sid, "NOW") for page, sid in zip(pages, assignments))), timeout=90)
            for index in (0, 3):
                assert all("LARGE OLD ROW" not in text and "LARGE NEW ROW" not in text for text in restored[index]["text"])
                assert restored[index]["anchor"] != rewritten[index]["anchor"]
            assert restored[4]["text"] == [f"PREFIX-PARENT NOW ROW {index:04d}" for index in range(120)] + ["FORK OWN MESSAGE"]
            assert not errors, errors
            print("PASS deletion/recreation: old owner and inherited results never flow into reopened pages", flush=True)
            await asyncio.gather(*(context.close() for context in contexts))
        finally:
            await browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-concurrent-views-") as temporary:
        corpus = build(Path(temporary))
        # Deliberately churn the shared cache, including the inherited-prefix
        # budget. Valid histories remain readable when not retained.
        with isolated_server(corpus, args.binary, extra_env={
            "SESSIONDOCK_CACHE_ENTRIES": "2", "SESSIONDOCK_VIEW_CACHE_MB": "1",
            "SESSIONDOCK_AST_CACHE_MB": "1", "SESSIONDOCK_HISTORY_PAGE_EVENTS": "200",
        }) as (base, _opener):
            asyncio.run(browser_check(corpus, base))


if __name__ == "__main__":
    main()
