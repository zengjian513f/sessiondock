#!/usr/bin/env python3
"""Synthetic scoped disk images through real HTTP and legacy Chromium rendering."""
from __future__ import annotations

import argparse
import base64
import ctypes
import json
import os
from pathlib import Path
import struct
import sys
import tempfile
from urllib.error import HTTPError

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, claude_row, codex_row, encoded, get_json, isolated_server
from media_browser import PNG, JPEG, GREEN, TOKEN, native_bytes, route, uid
from media_formats_browser import FORMATS


def image(source, reference):
    if source == "claude":
        return {"type": "image", "source": {"type": "file", "path": str(reference)}}
    if source == "codex":
        return {"type": "input_image", "image_url": str(reference)}
    return {"type": "image_url", "image_url": {"url": str(reference)}}


def build(root):
    corpus = Corpus(root)
    files = root / "files"
    files.mkdir()
    for name, data in (("structured.png", PNG), ("uri.jpg", JPEG), ("markdown.png", GREEN),
                       ("raw.png", PNG), ("unknown.payload", FORMATS[0][2]), ("window.png", PNG)):
        (files / name).write_bytes(base64.b64decode(data))
    (root / "outside.png").write_bytes(base64.b64decode(GREEN))
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
        name = source + "-file-media"
        content = [image(source, files / "structured.png"), image(source, (files / "uri.jpg").as_uri()),
                   image(source, files / "unknown.payload"), {"type": "text", "text":
                       "![markdown](markdown.png) ./raw.png\n" + source.title() + " file media complete"}]
        if source == "claude":
            corpus.put(name, source, [claude_row(name, "user", "u", None, content, cwd=str(files)),
                claude_row(name, "assistant", "a", "u", "Current file branch"),
                claude_row(name, "user", "discarded", "a", [image(source, root / "outside.png")]),
                {"type": "last-prompt", "leafUuid": "a"}], [])
            agent = corpus.paths[name].with_suffix("") / "subagents/agent-worker.jsonl"
            agent.parent.mkdir(parents=True)
            agent.write_bytes(encoded(claude_row(name, "user", "agent-u", None,
                [image(source, files / "markdown.png"), {"type": "text", "text": "Worker image only"}],
                cwd=str(files), isSidechain=True, agentId="worker")))
        elif source == "codex":
            corpus.put(name, source, [codex_row("session_meta", {"id": name, "cwd": str(files)}),
                codex_row("response_item", {"type": "message", "role": "user", "content": content})], [])
        else:
            path = root / "grok/project" / name
            path.mkdir(parents=True)
            (path / "summary.json").write_text(json.dumps({"info": {"id": name, "cwd": str(files)}}))
            (path / "chat_history.jsonl").write_bytes(encoded({"type": "user", "prompt_index": 1, "content": content}))
            corpus.paths[name] = path
    (root / "claude/private.png").write_bytes(base64.b64decode(GREEN))
    denied = {"outside": root / "outside.png", "native": root / "claude/private.png"}
    if os.name != "nt":
        (files / "linked.png").symlink_to(files / "structured.png")
        denied["symlink"] = files / "linked.png"
    for suffix, path in denied.items():
        name = "claude-denied-" + suffix
        corpus.put(name, "claude", [claude_row(name, "user", "u", None,
            [image("claude", path), {"type": "text", "text": "Denied " + suffix + " text survives"}], cwd=str(files))], [])
    corpus.put("claude-remote-markdown", "claude", [claude_row("claude-remote-markdown", "user", "u", None,
        "Remote remains explicit: ![remote blocked](https://media.example.invalid/never.png)", cwd=str(files))], [])
    rows = [codex_row("session_meta", {"id": "codex-file-window", "cwd": str(files)})]
    for index in range(700):
        content = [image("codex", files / "window.png")] if index == 150 else [{"type": "input_text", "text": f"Synthetic window row {index}"}]
        rows.append(codex_row("response_item", {"type": "message", "role": "user", "content": content}))
    corpus.put("codex-file-window", "codex", rows, [])
    return corpus, files, denied


def media(value):
    return [item for row in value["messages"] for item in row.get("media", [])]


def status(opener, url, expected):
    try:
        with opener.open(url, timeout=5) as response:
            assert response.status == expected
            return response.read()
    except HTTPError as error:
        assert error.code == expected, (error.code, expected)
        return error.read()


def check_window_opens(corpus, files, opener, base):
    """Linux IN_OPEN/IN_ACCESS distinguish version capture from first byte read."""
    if sys.platform != "linux":
        return
    libc = ctypes.CDLL(None, use_errno=True)
    libc.inotify_init1.argtypes = [ctypes.c_int]
    libc.inotify_add_watch.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
    descriptor = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
    assert descriptor >= 0
    try:
        assert libc.inotify_add_watch(descriptor, os.fsencode(files / "window.png"), 0x21) >= 0
        def masks():
            try:
                events = os.read(descriptor, 65536)
            except BlockingIOError:
                return []
            result, offset = [], 0
            while offset < len(events):
                _, mask, _, length = struct.unpack_from("iIII", events, offset)
                result.append(mask)
                offset += 16 + length
            return result
        projected = get_json(opener, base, route(corpus, "codex-file-window") + "?window=1")
        assert projected["partial"] and not media(projected)
        assert not masks(), "window omission still opened the image file"
        projected = get_json(opener, base, route(corpus, "codex-file-window"))
        assert len(media(projected)) == 1
        events = masks()
        assert any(mask & 0x20 for mask in events), "descriptor did not capture an opened file version"
        assert not any(mask & 0x1 for mask in events), "history registration read image bytes"
        item = media(projected)[0]
        assert item.get("lazy") is True and "width" not in item
        assert status(opener, base + item["src"], 200) == base64.b64decode(PNG)
        assert any(mask & 0x1 for mask in masks()), "first GET positive control saw no image read"
    finally:
        os.close(descriptor)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-file-media-browser-") as temporary:
        corpus, files, denied = build(Path(temporary))
        original_native = native_bytes(corpus.root)
        original_files = {path: path.read_bytes() for path in files.iterdir() if path.is_file() and not path.is_symlink()}
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                for authorized in (False, True):
                    with isolated_server(corpus, args.binary, file_roots=(files,) if authorized else ()) as (base, opener):
                        if authorized:
                            check_window_opens(corpus, files, opener, base)
                        context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                        context.route("**/*", lambda request: request.continue_() if request.request.url.startswith(base + "/") else request.abort())
                        errors, requests = [], []
                        page = context.new_page()
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        page.on("request", lambda request: requests.append(request.url))
                        page.goto(base, wait_until="networkidle")

                        def open_session(name, marker):
                            if page.locator(".mobile-back").is_visible():
                                page.locator(".mobile-back").click()
                            page.locator(f'#side .item[data-uid="{uid(corpus, name)}"]').click()
                            expect(page.locator("#msgs")).to_contain_text(marker)

                        def pictures(dimensions):
                            images = page.locator("#msgs img")
                            expect(images).to_have_count(len(dimensions))
                            for item in images.all():
                                item.scroll_into_view_if_needed()
                            page.wait_for_function("Array.from(document.querySelectorAll('#msgs img')).every(i=>i.complete&&i.naturalWidth>0)")
                            actual = images.evaluate_all("items=>items.map(i=>[i.naturalWidth,i.naturalHeight])")
                            assert sorted(actual) == sorted(map(list, dimensions)), actual
                            for src in images.evaluate_all("items=>items.map(i=>i.src)"):
                                assert src.startswith(base) and TOKEN.fullmatch(src[len(base):]), src

                        for source in ("claude", "codex", "grok"):
                            name = source + "-file-media"
                            projected = get_json(opener, base, route(corpus, name))
                            assert PNG not in json.dumps(projected) and "base64" not in json.dumps(projected)
                            items = media(projected)
                            assert len(items) == 5, (source, items)
                            if authorized:
                                for item, (mime, data) in zip(items, (("image/png", PNG), ("image/jpeg", JPEG), ("image/gif", FORMATS[0][2]), ("image/png", GREEN), ("image/png", PNG))):
                                    assert item.get("lazy") is True and TOKEN.fullmatch(item["src"]), item
                                    assert not {"mime", "width", "height"}.intersection(item)
                                    with opener.open(base + item["src"], timeout=5) as response:
                                        assert response.headers["Content-Type"] == mime
                                        assert response.headers["X-Content-Type-Options"] == "nosniff"
                                        assert "private" in response.headers["Cache-Control"] and "no-store" in response.headers["Cache-Control"]
                                        assert response.read() == base64.b64decode(data)
                            else:
                                assert all("src" not in item and item["error"]["status"] == 501 for item in items)
                            open_session(name, source.title() + " file media complete")
                            if authorized:
                                pictures([(2, 3), (3, 2), (3, 2), (4, 1), (2, 3)])
                                expect(page.locator("#msgs .media-error")).to_have_count(0)
                            else:
                                expect(page.locator("#msgs img")).to_have_count(0)
                                expect(page.locator("#msgs .media-error")).to_have_count(5)
                                expect(page.locator("#msgs")).to_contain_text("未配置")
                        if authorized:
                            open_session("claude-file-media", "Claude file media complete")
                            page.locator("#a-view-switch").click()
                            page.locator('#session-view-menu button[data-agent="worker"]').click()
                            expect(page.locator("#msgs")).to_contain_text("Worker image only")
                            pictures([(4, 1)])
                            page.locator("#a-view-switch").click()
                            page.locator('#session-view-menu button[data-agent=""]').click()
                            expect(page.locator("#msgs")).to_contain_text("Claude file media complete")
                            pictures([(2, 3), (3, 2), (3, 2), (4, 1), (2, 3)])
                            before = get_json(opener, base, route(corpus, "claude-file-media"))
                            old_token = media(before)[0]["src"]
                            replacement = files / "replacement.tmp"
                            replacement.write_bytes(base64.b64decode(GREEN))
                            replacement.replace(files / "structured.png")
                            original_files[files / "structured.png"] = base64.b64decode(GREEN)
                            status(opener, base + old_token, 409)
                            page.reload(wait_until="networkidle")
                            expect(page.locator("#msgs")).to_contain_text("Claude file media complete")
                            pictures([(4, 1), (3, 2), (3, 2), (4, 1), (2, 3)])
                            refreshed = get_json(opener, base, route(corpus, "claude-file-media"))
                            assert media(refreshed)[0]["src"] != old_token
                            page.set_viewport_size({"width": 390, "height": 844})
                            for suffix in denied:
                                open_session("claude-denied-" + suffix, "Denied " + suffix + " text survives")
                                expect(page.locator("#msgs img")).to_have_count(0)
                                expect(page.locator("#msgs .media-error")).to_have_count(1)
                                assert page.locator("#msgs .media-error").inner_text().strip()
                            open_session("claude-remote-markdown", "Remote remains explicit")
                            expect(page.locator("#msgs img")).to_have_count(0)
                            expect(page.locator("#msgs")).to_contain_text("remote blocked")
                            assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
                        assert not errors, errors
                        assert all(url.startswith(base + "/") for url in requests), requests
                        assert native_bytes(corpus.root) == original_native, "native fixture bytes changed"
                        assert all(path.read_bytes() == data for path, data in original_files.items()), "file changed outside explicit replacement"
                        context.close()
                print("PASS file media: three providers/path/fileURI/Markdown/raw/sniff, no-root visible errors, true decoding, replacement409+reload, branch/agent/mobile/remote isolation, Linux omitted-window zero IN_OPEN and history zero IN_ACCESS/first GET positive read, native bytes unchanged")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
