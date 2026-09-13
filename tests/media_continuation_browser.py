#!/usr/bin/env python3
"""Per-message media continuation: real Rust pages over HTTP and in Chromium.

A single synthetic message carries more typed images than one projection may
embed. Only private generated Codex/Claude records and a loopback Rust server
are used; no CLI, native home, remote image or model call is involved. Owned
fixture rewrites keep size and mtime so only content verification can notice.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlsplit

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, claude_row, codex_message, codex_row, encoded, get_json, isolated_server
from media_browser import GREEN, PNG, TOKEN, image, native_bytes, uid

# PNG decodes to 2x3, GREEN to 4x1: alternating data proves order and decoding.
HEX32 = re.compile(r"[0-9a-f]{32}\Z")


def data_at(index):
    return PNG if index % 2 == 0 else GREEN


def codex_images(text, count):
    return codex_row("response_item", {"type": "message", "role": "user", "content": [
        {"type": "input_text", "text": text}, *[image("codex", data_at(index)) for index in range(count)]]})


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    corpus.put("codex-many", "codex", [codex_row("session_meta", {"id": "codex-many", "cwd": "/synthetic/many"}),
        codex_message("user", "BEFORE MANY IMAGES"), codex_images("MANY IMAGES", 40),
        codex_message("assistant", "MANY IMAGES RECEIVED")], [])
    corpus.put("codex-rewrite", "codex", [codex_row("session_meta", {"id": "codex-rewrite", "cwd": "/synthetic/rewrite"}),
        codex_images("REWRITE IMAGES", 40), codex_message("assistant", "REWRITE RECEIVED")], [])
    # Claude user content blocks project as separate messages (adapter parity);
    # one tool result with many images is the Claude shape of a single message.
    name = "claude-many"
    corpus.put(name, "claude", [
        claude_row(name, "user", "u0", None, "CLAUDE MANY"),
        claude_row(name, "assistant", "call", "u0", [{"type": "tool_use", "id": "tool-many", "name": "synthetic_image", "input": {}}]),
        claude_row(name, "user", "result", "call", [{"type": "tool_result", "tool_use_id": "tool-many", "content": [
            {"type": "text", "text": "CLAUDE MANY IMAGES"}, *[image("claude", data_at(index)) for index in range(20)]]}]),
        claude_row(name, "assistant", "a0", "result", "CLAUDE MANY RECEIVED")], [])
    return corpus


def media_route(corpus, name, cursor, agent=None):
    query = {"cursor": cursor}
    if agent is not None:
        query["agent"] = agent
    return "/api/messages/" + uid(corpus, name) + "/media-page?" + urlencode(query)


def expect_status(opener, base, route, expected):
    try:
        with opener.open(base + route, timeout=10) as response:
            response.read(4096)
    except HTTPError as error:
        assert error.code == expected, (route, error.code, expected)
        body = json.loads(error.read(8192))
        assert body.get("error") and "media" not in body, body
        return body
    raise AssertionError(f"expected explicit {expected} for {route}")


def fetch_page(opener, base, route):
    with opener.open(base + route, timeout=10) as response:
        assert response.status == 200
        assert response.headers["Content-Type"] == "application/json; charset=utf-8", response.headers["Content-Type"]
        assert response.headers["X-Content-Type-Options"] == "nosniff"
        cache = response.headers["Cache-Control"]
        assert "private" in cache and "no-store" in cache, cache
        raw = response.read(1024 * 1024 + 1)
        assert len(raw) <= 1024 * 1024, "media page exceeds the browser read budget"
        return json.loads(raw)


def many_message(window, marker):
    found = [message for message in window["messages"] if marker in message["text"] and message.get("media")]
    assert len(found) == 1, [message["text"] for message in window["messages"]]
    return found[0]


def check_descriptors(items):
    for item in items:
        assert TOKEN.fullmatch(item["src"]), item
        assert item.get("alt") and item.get("lazy") is True and "error" not in item, item
        assert not {"mime", "width", "height"}.intersection(item), item
    return [item["src"] for item in items]


def check_bytes(opener, base, sources, start):
    for offset, source in enumerate(sources):
        with opener.open(base + source, timeout=10) as reply:
            assert reply.status == 200 and reply.headers["Content-Type"] == "image/png"
            assert reply.read(65536) == base64.b64decode(data_at(start + offset), validate=True), start + offset


def rewrite(path, old, new, expected_native, root):
    raw = path.read_bytes()
    changed = raw.replace(old, new, 1)
    assert changed != raw and len(changed) == len(raw)
    stamp = path.stat()
    path.write_bytes(changed)
    os.utime(path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    assert path.stat().st_mtime_ns == stamp.st_mtime_ns and path.stat().st_size == stamp.st_size
    expected_native[str(path.relative_to(root))] = hashlib.sha256(changed).hexdigest()


def append(path, record, expected_native, root):
    with path.open("ab") as stream:
        stream.write(encoded(record))
    expected_native[str(path.relative_to(root))] = hashlib.sha256(path.read_bytes()).hexdigest()


def verify_http(corpus, base, opener, expected_native):
    capabilities = get_json(opener, base, "/api/meta")["capabilities"]
    assert capabilities["media_continuation"] is True, capabilities
    assert capabilities["media_lazy"] is True and capabilities["history_pages"] is True

    def walk(name, marker, total, agent=None):
        window = get_json(opener, base, "/api/messages/" + uid(corpus, name) + "?window=1")
        serialized = json.dumps(window)
        assert PNG not in serialized and GREEN not in serialized and "data:image" not in serialized
        message = many_message(window, marker)
        assert len(message["media"]) == 16, len(message["media"])
        more = message["media_more"]
        assert more["remaining"] == total - 16 and more["total"] == total, more
        assert HEX32.fullmatch(more["cursor"]), more
        assert not any("media_more" in other for other in window["messages"] if other is not message)
        sources = check_descriptors(message["media"])
        check_bytes(opener, base, sources, 0)
        cursors = [more["cursor"]]
        start = 16
        while True:
            reply = fetch_page(opener, base, media_route(corpus, name, cursors[-1], agent))
            page = reply["page"]
            expected_end = min(start + 16, total)
            assert page["cursor"] == cursors[-1] and page["start"] == start and page["end"] == expected_end, page
            assert page["total"] == total and page["remaining"] == total - expected_end, page
            assert len(reply["media"]) == expected_end - start
            sources += check_descriptors(reply["media"])
            check_bytes(opener, base, sources[start:], start)
            start = expected_end
            if page["remaining"] == 0:
                assert page["next"] is None, page
                break
            assert HEX32.fullmatch(page["next"]) and page["next"] != page["cursor"], page
            assert page["next"] not in cursors
            cursors.append(page["next"])
        assert start == total and len(sources) == total and len(set(sources)) == total
        return window, cursors

    window, cursors = walk("codex-many", "MANY IMAGES", 40)
    assert len(cursors) == 2, cursors
    walk("claude-many", "CLAUDE MANY IMAGES", 20)
    first, second = cursors
    # Wrong agent / wrong owner are refused; malformed and unknown tokens are
    # explicit; reads never consume a grant.
    expect_status(opener, base, media_route(corpus, "codex-many", first, "codex-other-agent"), 403)
    expect_status(opener, base, media_route(corpus, "claude-many", first), 403)
    for malformed in ("", "xyz", first[:-1], first.upper(), first + "0"):
        expect_status(opener, base, media_route(corpus, "codex-many", malformed), 400)
    expect_status(opener, base, "/api/messages/" + uid(corpus, "codex-many") + "/media-page", 400)
    expect_status(opener, base, media_route(corpus, "codex-many", "0" * 32), 404)
    # Every read mints a fresh random `next`; earlier tokens stay valid.
    again = fetch_page(opener, base, media_route(corpus, "codex-many", first))
    assert again["page"]["start"] == 16 and again["page"]["end"] == 32 and len(again["media"]) == 16
    assert HEX32.fullmatch(again["page"]["next"]) and again["page"]["next"] != first
    for token in (second, again["page"]["next"]):
        assert fetch_page(opener, base, media_route(corpus, "codex-many", token))["page"]["next"] is None

    # An ordinary append keeps every old grant valid; the message is unchanged.
    path = corpus.paths["codex-many"]
    append(path, codex_message("assistant", "APPENDED AFTER GRANT"), expected_native, corpus.root)
    appended = get_json(opener, base, "/api/messages/" + uid(corpus, "codex-many") + "?window=1")
    assert appended["messages"][-1]["text"] == "APPENDED AFTER GRANT"
    assert many_message(appended, "MANY IMAGES")["media_more"]["remaining"] == 24
    after = fetch_page(opener, base, media_route(corpus, "codex-many", first))
    assert after["page"]["start"] == 16 and after["page"]["end"] == 32 and HEX32.fullmatch(after["page"]["next"])
    check_bytes(opener, base, [item["src"] for item in after["media"]], 16)
    for token in (second, after["page"]["next"]):
        last = fetch_page(opener, base, media_route(corpus, "codex-many", token))
        assert last["page"]["remaining"] == 0 and last["page"]["next"] is None and len(last["media"]) == 8
        check_bytes(opener, base, [item["src"] for item in last["media"]], 32)

    # A same-size restored-mtime rewrite of the image message line changes the
    # timeline: old grants answer 409, a fresh window issues a new cursor.
    _, stale = walk("codex-rewrite", "REWRITE IMAGES", 40)
    path = corpus.paths["codex-rewrite"]
    rewrite(path, b"REWRITE IMAGES", b"REWRITE IMAGEZ", expected_native, corpus.root)
    for cursor in stale:
        expect_status(opener, base, media_route(corpus, "codex-rewrite", cursor), 409)
    fresh = get_json(opener, base, "/api/messages/" + uid(corpus, "codex-rewrite") + "?window=1")
    replacement = many_message(fresh, "REWRITE IMAGEZ")["media_more"]["cursor"]
    assert replacement not in stale
    assert fetch_page(opener, base, media_route(corpus, "codex-rewrite", replacement))["page"]["start"] == 16
    for cursor in stale:
        expect_status(opener, base, media_route(corpus, "codex-rewrite", cursor), 409)

    searched = get_json(opener, base, "/api/search?" + urlencode({"q": "MANY IMAGES"}))
    assert any(row.get("uid") == uid(corpus, "codex-many") for row in searched["results"]), searched
    assert searched.get("incomplete") is False
    assert "data:image" not in json.dumps(searched) and PNG not in json.dumps(searched)
    print("PASS media continuation HTTP: 16-image window plus media_more, Codex 16/32/40 and Claude 16/20 pages with decoded alternating bytes, 403/400/404, read-only grants across append, 409 after same-size restored-mtime rewrite, search unaffected")


def browser(corpus, base, expected_native):
    with sync_playwright() as playwright:
        options = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        chromium = playwright.chromium.launch(**options)
        try:
            context = chromium.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            page = context.new_page()
            errors, requests, held = [], [], []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.on("request", lambda request: requests.append(request.url))
            hold_watch = {"uid": None}

            def watch_route(route):
                query = parse_qs(urlsplit(route.request.url).query)
                if hold_watch["uid"] and query.get("uid") == [hold_watch["uid"]]:
                    held.append(route)  # Never answered: keeps the stale cursor visible.
                else:
                    route.continue_()
            page.route("**/api/watch?*", watch_route)
            assert page.goto(base, wait_until="networkidle").status == 200
            assert page.evaluate("AgentHubCapabilities.config.media_continuation===true"), "real backend must declare media_continuation"

            def select(name, marker):
                if page.viewport_size["width"] < 700 and page.locator(".mobile-back").is_visible():
                    page.locator(".mobile-back").click()
                page.locator(f'#side .item[data-uid="{uid(corpus, name)}"]').click()
                expect(page.locator("#msgs")).to_contain_text(marker)
                expect(page.locator("#a-term")).to_be_visible()
                expect(page.locator("#a-term")).to_be_enabled()

            def unfold():
                for toggle in page.locator("#msgs .turn-process.folded .fold-toggle").all():
                    toggle.click()

            def decoded(count):
                unfold()
                images = page.locator("#msgs img")
                expect(images).to_have_count(count)
                images.last.scroll_into_view_if_needed()
                page.wait_for_function("""count => {const items=[...document.querySelectorAll('#msgs img')];
                  return items.length===count && items.every((img,index)=>img.complete
                    && img.naturalWidth===(index%2?4:2) && img.naturalHeight===(index%2?1:3));}""", arg=count)
                for src in images.evaluate_all("items=>items.map(i=>i.src)"):
                    assert src.startswith(base) and TOKEN.fullmatch(src[len(base):]), src
                expect(page.locator("#migration-read-error")).to_have_count(0)

            def state():
                return page.evaluate("""(() => {const key=viewKey(S.sel,S.agent),e=cache.get(key);
                  const many=e.msgs.find(m=>m.media_more||m.media?.length>=16);
                  return {text:e.msgs.map(m=>m.text),end:e.end,anchor:e.anchor,version:e.version,
                    cursor:S.cursors.get(key),media:many?many.media.length:0,more:many?.media_more||null};})()""")

            def load_more(expected_text, count, remaining):
                unfold()
                button = page.locator("#msgs .media-more")
                expect(button).to_have_count(1)
                expect(button).to_have_text(expected_text)
                expect(button).to_be_enabled()
                button.scroll_into_view_if_needed()
                button.click()
                page.wait_for_function("mediaPageRequests.size===0")
                decoded(count)
                expect(page.locator(".media-page-error")).to_have_count(0)
                if remaining:
                    expect(page.locator("#msgs .media-more")).to_have_text(f"还有 {remaining} 张图片，加载下一批")
                else:
                    expect(page.locator("#msgs .media-more")).to_have_count(0)

            def continuation(name, marker, total):
                select(name, marker)
                decoded(16)
                before = state()
                assert before["media"] == 16 and before["more"]["remaining"] == total - 16 and before["more"]["total"] == total
                loaded = 16
                while loaded < total:
                    pending = total - loaded
                    loaded = min(loaded + 16, total)
                    load_more(f"还有 {pending} 张图片，加载下一批", loaded, total - loaded)
                after = state()
                assert after["media"] == total and after["more"] is None, after
                assert after["text"] == before["text"], "message order changed"
                assert (after["end"], after["anchor"], after["version"], after["cursor"]) == (
                    before["end"], before["anchor"], before["version"], before["cursor"]), "media page touched the live cursor"
                assert page.evaluate("_es && _es.readyState===EventSource.OPEN")

            continuation("codex-many", "APPENDED AFTER GRANT", 40)
            start = len(requests)
            append(corpus.paths["codex-many"], codex_message("assistant", "AFTER MANY IMAGES"), expected_native, corpus.root)
            expect(page.locator("#msgs")).to_contain_text("AFTER MANY IMAGES", timeout=10000)
            page.wait_for_function("cache.get(viewKey(S.sel,S.agent)).msgs.at(-1).text==='AFTER MANY IMAGES'")
            decoded(40)
            assert state()["media"] == 40 and state()["more"] is None
            assert not any("window=1" in url for url in requests[start:] if "/api/messages/" in url), "ordinary append reset the window"
            continuation("claude-many", "CLAUDE MANY RECEIVED", 20)
            page_requests = [url for url in requests if "/media-page?" in url]
            assert len(page_requests) == 3, page_requests
            assert all(parse_qs(urlsplit(url).query).get("agent") is None for url in page_requests)

            page.set_viewport_size({"width": 390, "height": 844})
            page.reload(wait_until="networkidle")
            continuation("codex-many", "AFTER MANY IMAGES", 40)
            assert page.evaluate("document.documentElement.scrollWidth<=innerWidth")
            expect(page.locator("#a-term")).to_be_visible()

            # Failure path: the live stream for this view is withheld, so the
            # rendered cursor stays stale after an owned same-size rewrite. The
            # real server answers 409; the snapshot and a retry remain.
            page.set_viewport_size({"width": 1280, "height": 900})
            hold_watch["uid"] = uid(corpus, "codex-rewrite")
            select("codex-rewrite", "REWRITE RECEIVED")
            decoded(16)
            for _ in range(50):
                if held:
                    break
                page.wait_for_timeout(100)
            assert len(held) == 1, "watch for the rewrite view was not held"
            before = state()
            page.evaluate("S.lastSync=Date.now()")  # Keep the periodic fallback sync out of this short window.
            rewrite(corpus.paths["codex-rewrite"], b"REWRITE IMAGEZ", b"REWRITE IMAGEY", expected_native, corpus.root)
            unfold()
            button = page.locator("#msgs .media-more")
            expect(button).to_have_text("还有 24 张图片，加载下一批")
            button.click()
            page.wait_for_function("mediaPageRequests.size===0")
            notice = page.locator("#msgs .media-page-error")
            expect(notice).to_be_visible()
            expect(notice).to_have_attribute("role", "alert")
            text = notice.inner_text()
            assert "409" in text or "已加载的图片保持不变" in text, text
            expect(button).to_have_text("重试加载图片")
            expect(button).to_be_enabled()
            expect(page.locator("#msgs img")).to_have_count(16)
            assert state() == before, "failed page changed the snapshot"
            button.click()
            page.wait_for_function("mediaPageRequests.size===0")
            expect(page.locator("#msgs .media-page-error")).to_have_count(1)
            expect(page.locator("#msgs img")).to_have_count(16)
            expect(button).to_be_enabled()
            expect(page.locator("#a-term")).to_be_visible()
            expect(page.locator("#a-term")).to_be_enabled()
            assert state() == before
            assert not errors, errors
            assert all(url.startswith(base + "/") for url in requests), requests
            assert all("window=1" in url or "start=" in url or "/media-page?" in url or "/page?" in url
                       for url in requests if "/api/messages/" in url), "unbounded history fetch occurred"
            print("PASS media continuation browser: desktop/mobile Codex 16->32->40 and Claude 16->20 decoded in order, live cursor untouched, SSE append after pages, console visible, real 409 after rewrite with visible alert/retry and intact snapshot, no page errors")
        finally:
            for route in held:  # Release the withheld stream before teardown.
                try:
                    route.abort()
                except Exception:
                    pass
            chromium.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-media-continuation-") as temporary:
        corpus = build(Path(temporary))
        expected_native = native_bytes(corpus.root)
        with isolated_server(corpus, args.binary) as (base, opener):
            verify_http(corpus, base, opener, expected_native)
            assert native_bytes(corpus.root) == expected_native, "server modified synthetic native bytes"
            browser(corpus, base, expected_native)
            assert native_bytes(corpus.root) == expected_native, "server modified synthetic native bytes"


if __name__ == "__main__":
    main()
