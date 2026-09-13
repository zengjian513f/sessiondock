#!/usr/bin/env python3
"""Real Chromium decoding of fixed synthetic GIF/WebP/AVIF/BMP native media.

Fixtures were encoded once with Pillow. Runtime needs no image encoder/decoder
CLI, native homes, model invocation or external network access.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import tempfile
from urllib.error import HTTPError

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, claude_row, codex_row, encoded, get_json, isolated_server
from media_browser import TOKEN, image, native_bytes, route, uid

# All six genuinely encoded fixtures have a 3x2 canvas; animated ones have two
# distinct solid-color frames. Chromium independently verifies actual decoding.
FORMATS = (
    ("gif", "image/gif", "R0lGODdhAwACAIEAAOYeCgAAAAAAAAAAACwAAAAAAwACAAAIBgABCBwYEAA7"),
    ("gif-animated", "image/gif", "R0lGODlhAwACAIEAAOYeCgAAAAAAAAAAACH/C05FVFNDQVBFMi4wAwEAAAAh+QQACAAAACwAAAAAAwACAAAIBgABCBwYEAAh+QQBDAABACwAAAAAAwACAIEKHuYAAAAAAAAAAAAIBgABCBwYEAA7"),
    ("webp", "image/webp", "UklGRh4AAABXRUJQVlA4TBEAAAAvAkAAAAdQj5pXof+BiOh/AAA="),
    ("webp-animated", "image/webp", "UklGRogAAABXRUJQVlA4WAoAAAACAAAAAgAAAQAAQU5JTQYAAAAAAAAAAABBTk1GKgAAAAAAAAAAAAIAAAEAAFAAAAJWUDhMEQAAAC8CQAAAB1CPmleh/4GI6H8AAEFOTUYqAAAAAAAAAAAAAgAAAQAAeAAAAFZQOEwRAAAALwJAAAAHUI8q1Lz/gYjofwAA"),
    ("avif", "image/avif", "AAAAIGZ0eXBhdmlmAAAAAGF2aWZtaWYxbWlhZk1BMUIAAADrbWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAcGljdAAAAAAAAAAAAAAAAAAAAAAOcGl0bQAAAAAAAQAAAB5pbG9jAAAAAEQAAAEAAQAAAAEAAAETAAAAKAAAAChpaW5mAAAAAAABAAAAGmluZmUCAAAAAAEAAGF2MDFDb2xvcgAAAABqaXBycAAAAEtpcGNvAAAAFGlzcGUAAAAAAAAAAwAAAAIAAAAQcGl4aQAAAAADCAgIAAAADGF2MUOBAAwAAAAAE2NvbHJuY2x4AAEADQAGgAAAABdpcG1hAAAAAAAAAAEAAQQBAoMEAAAAMG1kYXQSAAoIGAQrRAQ0GhAyGhTHh4ZlAgggnkAAAJBLsrmsYuXxFjb2VyIt"),
    ("bmp", "image/bmp", "Qk1OAAAAAAAAADYAAAAoAAAAAwAAAAIAAAABABgAAAAAABgAAADEDgAAxA4AAAAAAAAAAAAACh7mCh7mCh7mAAAACh7mCh7mCh7mAAAA"),
)


def excessive_gif(frames, canvas=3):
    data = base64.b64decode(FORMATS[0][2])
    header = bytearray(data[:25])
    header[6:10] = canvas.to_bytes(2, "little") * 2
    return bytes(header) + data[25:-1] * frames + b";"


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
        name = source + "-formats"
        blocks = [image(source, data, mime) for _, mime, data in FORMATS]
        marker = source.title() + " formats complete"
        if source == "claude":
            rows = [claude_row(name, "user", "u", None, blocks[:3]),
                    claude_row(name, "assistant", "call", "u", [{"type": "tool_use", "id": "formats", "name": "synthetic_formats", "input": {}}]),
                    claude_row(name, "user", "result", "call", [{"type": "tool_result", "tool_use_id": "formats", "content": [{"type": "text", "text": "Mixed formats"}, *blocks[3:]]}]),
                    claude_row(name, "assistant", "final", "result", marker)]
            corpus.put(name, source, rows, [])
        elif source == "codex":
            corpus.put(name, source, [codex_row("session_meta", {"id": name, "session_id": name, "cwd": "/synthetic/formats"}),
                codex_row("response_item", {"type": "message", "role": "user", "content": blocks[:3]}),
                codex_row("response_item", {"type": "function_call", "name": "synthetic_formats", "call_id": "formats", "arguments": "{}"}),
                codex_row("response_item", {"type": "function_call_output", "call_id": "formats", "output": [{"type": "text", "text": "Mixed formats"}, *blocks[3:]]}),
                codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "content": [{"type": "output_text", "text": marker}]})], [])
        else:
            path = root / "grok/project-formats" / name
            path.mkdir(parents=True)
            (path / "summary.json").write_text(json.dumps({"info": {"id": name, "cwd": "/synthetic/formats"}}))
            (path / "chat_history.jsonl").write_bytes(b"".join(encoded(row) for row in [
                {"type": "user", "prompt_index": 1, "content": blocks[:3]},
                {"type": "assistant", "content": "", "tool_calls": [{"id": "formats", "name": "synthetic_formats", "arguments": "{}"}]},
                {"type": "tool_result", "tool_call_id": "formats", "content": [{"type": "text", "text": "Mixed formats"}, *blocks[3:]]},
                {"type": "assistant", "content": marker}]))
            corpus.paths[name] = path
    for suffix, data in (("bad-signature", b"not a GIF container"), ("frame-limit", excessive_gif(129)), ("pixel-limit", excessive_gif(65, 1024))):
        name = "claude-" + suffix
        corpus.put(name, "claude", [claude_row(name, "user", "u", None, [
            {"type": "text", "text": "Readable text beside " + suffix},
            image("claude", base64.b64encode(data).decode(), "image/gif")])], [])
    return corpus


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-formats-browser-") as temporary:
        corpus = build(Path(temporary))
        original = native_bytes(corpus.root)
        with isolated_server(corpus, args.binary) as (base, opener), sync_playwright() as playwright:
            caps = get_json(opener, base, "/api/meta")["capabilities"]
            assert caps["media"] is True and caps["media_remote"] is False
            assert caps["media_lazy"] is True
            for source in ("claude", "codex", "grok"):
                response = get_json(opener, base, route(corpus, source + "-formats"))
                serialized = json.dumps(response)
                assert "base64" not in serialized and "data:image" not in serialized
                media = [item for row in response["messages"] for item in row.get("media", [])]
                assert len(media) == len(FORMATS), (source, response)
                for item, (name, mime, data) in zip(media, FORMATS):
                    assert data not in serialized
                    assert TOKEN.fullmatch(item["src"]) and item.get("lazy") is True, (name, item)
                    assert not {"mime", "width", "height"}.intersection(item)
                    with opener.open(base + item["src"], timeout=5) as reply:
                        assert reply.status == 200 and reply.headers["Content-Type"] == mime
                        assert reply.headers["X-Content-Type-Options"] == "nosniff"
                        assert "private" in reply.headers["Cache-Control"] and "no-store" in reply.headers["Cache-Control"]
                        assert reply.read() == base64.b64decode(data, validate=True)
            for suffix, status in (("bad-signature", 422), ("frame-limit", 413), ("pixel-limit", 413)):
                history = get_json(opener, base, route(corpus, "claude-" + suffix))
                assert "Readable text beside " + suffix in json.dumps(history)
                images = [item for row in history["messages"] for item in row.get("media", [])]
                assert len(images) == 1 and images[0].get("lazy") is True
                assert not {"mime", "width", "height", "error"}.intersection(images[0])
                try:
                    opener.open(base + images[0]["src"], timeout=5)
                except HTTPError as error:
                    assert error.code == status, (suffix, error.code)
                    body = json.loads(error.read())
                    assert body.get("error") and "base64" not in json.dumps(body)
                else:
                    raise AssertionError("invalid image accepted: " + suffix)
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                context.route("**/*", lambda request: request.continue_() if request.request.url.startswith(base + "/") else request.abort())
                page = context.new_page()
                errors, requests = [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("request", lambda request: requests.append(request.url))
                page.goto(base, wait_until="networkidle")

                def verify_images():
                    for toggle in page.locator("#msgs .turn-process.folded .fold-toggle").all():
                        toggle.click()
                    images = page.locator("#msgs img")
                    expect(images).to_have_count(6)
                    for item in images.all():
                        item.scroll_into_view_if_needed()
                    page.wait_for_function("Array.from(document.querySelectorAll('#msgs img')).every(i=>i.complete&&i.naturalWidth===3&&i.naturalHeight===2)")
                    for src in images.evaluate_all("items=>items.map(i=>i.src)"):
                        assert src.startswith(base) and TOKEN.fullmatch(src[len(base):]), src
                    expect(page.locator("#migration-read-error")).to_have_count(0)

                for viewport in ({"width": 1280, "height": 900}, {"width": 390, "height": 844}):
                    page.set_viewport_size(viewport)
                    for source in ("claude", "codex", "grok"):
                        if page.locator(".mobile-back").is_visible():
                            page.locator(".mobile-back").click()
                        page.locator(f'#side .item[data-uid="{uid(corpus, source + "-formats")}"]').click()
                        expect(page.locator("#msgs")).to_contain_text(source.title() + " formats complete")
                        verify_images()
                        if source == "claude" and viewport["width"] > 700:
                            # Observe actual rendered frame changes, not just a
                            # positive naturalWidth for an animation's poster.
                            for index in (1, 3):
                                frames = set()
                                for delay in (30, 55, 85, 35, 60, 90):
                                    page.wait_for_timeout(delay)
                                    frames.add(page.locator("#msgs img").nth(index).screenshot())
                                assert len(frames) >= 2, "animation stayed on one frame: " + FORMATS[index][0]
                        page.reload(wait_until="networkidle")
                        expect(page.locator("#msgs")).to_contain_text(source.title() + " formats complete")
                        verify_images()
                        assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
                for suffix in ("bad-signature", "frame-limit", "pixel-limit"):
                    if page.locator(".mobile-back").is_visible():
                        page.locator(".mobile-back").click()
                    page.locator(f'#side .item[data-uid="{uid(corpus, "claude-" + suffix)}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Readable text beside " + suffix)
                    expect(page.locator("#migration-read-error")).to_have_count(0)
                    expect(page.locator("#msgs .media-load-error")).to_be_visible()
                    expected_status = 422 if suffix == "bad-signature" else 413
                    expect(page.locator("#msgs .media-load-error")).to_contain_text(str(expected_status))
                    expect(page.locator("#msgs img")).to_have_count(1)
                    expect(page.locator("#msgs img")).to_be_hidden()
                    expect(page.locator("#msgs .media-load-retry")).to_be_visible()
                    expect(page.locator(".mobile-back")).to_be_visible()
                    button = page.locator("#a-term")
                    expect(button).to_be_visible()
                    expect(button).to_have_attribute("data-unavailable", "true")
                    assert button.get_attribute("disabled") is None
                    reason = page.evaluate("consoleUnavailableReason(S.sel, S.agent, false)")
                    assert reason and "尚未加载" not in reason and "正在读取" not in reason, reason
                    dialogs = []
                    def accept_dialog(dialog):
                        dialogs.append(dialog.message)
                        dialog.accept()
                    page.once("dialog", accept_dialog)
                    button.click()
                    assert dialogs == ["控制台不可用：\n" + reason]
                assert not errors, errors
                assert all(url.startswith(base + "/") for url in requests), requests
                assert native_bytes(corpus.root) == original, "native fixture bytes changed"
                print("PASS media formats: GIF/WebP static+animated, AVIF/BMP, three providers and tool results, real Chromium 3x2 decoding, reload/mobile, visible invalid/limit errors, native bytes unchanged")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
