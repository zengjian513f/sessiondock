#!/usr/bin/env python3
"""Synthetic embedded PNG/JPEG HTTP and actual Chromium acceptance.

Only a private loopback development server and generated native fixtures are
used. No CLI, native home, external image fetch or model invocation is allowed.
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
from urllib.parse import urlencode

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, claude_row, codex_row, encoded, get_json, isolated_server

# Generated with Pillow from solid RGB pixels; Chromium below independently
# checks actual decoding and exact dimensions, not just signature/header bytes.
PNG = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAE0lEQVR4nGP8z8DAwMDAxIBMAQAUQAEF3SN5DgAAAABJRU5ErkJggg=="
JPEG = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAACAAMDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDxyiiiv3E8w//Z"
GREEN = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAIAAAB2XpiaAAAADUlEQVR4nGNk+M8ABwALDwEB+wO/9wAAAABJRU5ErkJggg=="
TOKEN = re.compile(r"/api/media/[0-9a-f]{32}\Z")
REMOTE = "https://media.example.invalid/forbidden.png"


def image(source, data=PNG, mime="image/png"):
    if source == "claude":
        return {"type": "image", "source": {"type": "base64", "media_type": mime, "data": data}}
    if source == "codex":
        return {"type": "input_image", "image_url": f"data:{mime};base64,{data}"}
    return {"type": "image_url", "image_url": {"url": f"data:{mime};base64,{data}"}}


def uid(corpus, name):
    path = corpus.paths[name]
    source = name.split("-", 1)[0]
    return source + ":" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    name = "claude-media"
    rows = [claude_row(name, "user", "u0", None, [image("claude")]),
            claude_row(name, "assistant", "call", "u0", [{"type": "tool_use", "id": "tool-image", "name": "synthetic_image", "input": {}}]),
            claude_row(name, "user", "result", "call", [{"type": "tool_result", "tool_use_id": "tool-image",
                "content": [{"type": "text", "text": "Claude mixed media"}, image("claude", JPEG, "image/jpeg")]}]),
            claude_row(name, "assistant", "final", "result", "Claude media complete"),
            claude_row(name, "user", "discarded-u", "final", [image("claude", GREEN)]),
            claude_row(name, "assistant", "discarded-a", "discarded-u", "Discarded image branch"),
            {"type": "last-prompt", "leafUuid": "final"}]
    corpus.put(name, "claude", rows, [])
    agent = "claude-media-agent"
    path = corpus.paths[name].with_suffix("") / "subagents" / f"agent-{agent}.jsonl"
    path.parent.mkdir(parents=True)
    path.write_bytes(b"".join(encoded(row) for row in [
        claude_row(name, "user", "agent-u", None, [image("claude", GREEN)], isSidechain=True, agentId=agent),
        claude_row(name, "assistant", "agent-a", "agent-u", "Claude agent image only", isSidechain=True, agentId=agent)]))
    corpus.paths[agent] = path

    def meta(name, **extra):
        return codex_row("session_meta", {"id": name, "session_id": name, "cwd": "/synthetic/media", **extra})
    name = "codex-media"
    prefix = [meta(name), codex_row("response_item", {"type": "message", "role": "user", "content": [image("codex")]})]
    rows = prefix + [codex_row("response_item", {"type": "function_call", "name": "synthetic_image", "call_id": "tool-image", "arguments": "{}"}),
        codex_row("response_item", {"type": "function_call_output", "call_id": "tool-image", "output": [
            {"type": "text", "text": "Codex mixed media"}, {"type": "image", "mime_type": "image/jpeg", "data": JPEG}]}),
        codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer",
            "content": [{"type": "output_text", "text": "Codex media complete"}]})]
    corpus.put(name, "codex", rows, [])
    corpus.put("codex-media-fork", "codex", [meta("codex-media-fork", forked_from_id=name,
        history_base={"thread_id": name, "end_byte_offset": sum(map(lambda row: len(encoded(row)), prefix))}),
        codex_row("response_item", {"type": "message", "role": "assistant", "content": [
            {"type": "output_text", "text": "Codex fork green"}, image("codex", GREEN)]})], [])
    corpus.put("codex-media-agent", "codex", [meta("codex-media-agent", session_id=name,
        thread_source="subagent", parent_thread_id=name, forked_from_id=name),
        codex_row("response_item", {"type": "message", "role": "assistant", "content": [
            {"type": "output_text", "text": "Codex agent green"}, image("codex", GREEN)]})], [])

    name = "grok-media"
    path = root / "grok/project-media" / name
    path.mkdir(parents=True)
    (path / "summary.json").write_text(json.dumps({"info": {"id": name, "cwd": "/synthetic/media"}}))
    (path / "chat_history.jsonl").write_bytes(b"".join(encoded(row) for row in [
        {"type": "user", "content": [image("grok")], "prompt_index": 1},
        {"type": "assistant", "content": "", "tool_calls": [{"id": "tool-image", "name": "synthetic_image", "arguments": "{}"}]},
        {"type": "tool_result", "tool_call_id": "tool-image", "content": {"isError": False, "content": [
            {"type": "text", "text": "Grok mixed media"}, image("grok", JPEG, "image/jpeg")]}},
        {"type": "assistant", "content": "Grok media complete"}]))
    corpus.paths[name] = path
    for name, block in [
        ("claude-invalid", image("claude", "not-base64")),
        ("claude-remote", {"type": "image", "source": {"type": "url", "url": REMOTE}}),
        ("claude-path", {"type": "image", "source": {"type": "file", "path": str(root / "private-never-read.png")}}),
    ]:
        corpus.put(name, "claude", [claude_row(name, "user", "bad-u", None, [block])], [])
    (root / "private-never-read.png").write_bytes(base64.b64decode(PNG))
    return corpus


def native_bytes(root):
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for source in ("claude", "codex", "grok") for path in (root / source).rglob("*") if path.is_file()}


def route(corpus, name, agent=""):
    return "/api/messages/" + uid(corpus, name) + ("?" + urlencode({"agent": agent}) if agent else "")


def verify_api(corpus, base, opener):
    capabilities = get_json(opener, base, "/api/meta")["capabilities"]
    assert capabilities["media"] is True and capabilities["media_remote"] is True
    assert capabilities["media_lazy"] is True
    for source in ("claude", "codex", "grok"):
        response = get_json(opener, base, route(corpus, source + "-media"))
        serialized = json.dumps(response)
        assert PNG not in serialized and JPEG not in serialized and "data:image" not in serialized
        messages = response["messages"]
        images = [media for message in messages for media in message.get("media", [])]
        assert len(images) == 2, (source, images)
        assert any(message["text"] == "[图片]" and message.get("media") for message in messages), source
        assert any("mixed media" in message["text"] and message.get("media") for message in messages), source
        for media, expected, mime in zip(images, (PNG, JPEG), ("image/png", "image/jpeg")):
            assert TOKEN.fullmatch(media["src"]), media
            assert media.get("alt") and media.get("lazy") is True
            assert not {"mime", "width", "height"}.intersection(media)
            with opener.open(base + media["src"], timeout=5) as reply:
                assert reply.status == 200
                assert reply.headers["Content-Type"] == mime
                assert reply.headers["X-Content-Type-Options"] == "nosniff"
                assert "private" in reply.headers["Cache-Control"] and "no-store" in reply.headers["Cache-Control"]
                assert reply.read(2 * 1024 * 1024) == base64.b64decode(expected, validate=True)
    available = get_json(opener, base, route(corpus, "claude-path"))
    items = [item for message in available["messages"] for item in message.get("media", [])]
    assert len(items) == 1 and TOKEN.fullmatch(items[0]["src"]), items
    with opener.open(base + items[0]["src"], timeout=5) as reply:
        assert reply.status == 200
        assert reply.read() == (corpus.root / "private-never-read.png").read_bytes()
    invalid = get_json(opener, base, route(corpus, "claude-invalid"))
    invalid_items = [item for message in invalid["messages"] for item in message.get("media", [])]
    assert invalid_items == [], invalid_items

    remote = get_json(opener, base, route(corpus, "claude-remote"))
    images = [item for message in remote["messages"] for item in message.get("media", [])]
    assert len(images) == 1 and images[0]["external"] is True and images[0]["src"] == REMOTE


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-media-browser-") as temporary:
        corpus = build(Path(temporary))
        expected_native = native_bytes(corpus.root)
        with isolated_server(corpus, args.binary) as (base, opener), sync_playwright() as playwright:
            verify_api(corpus, base, opener)
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                context.add_init_script("""(() => {window.__mediaPackets=[]; const Native=window.EventSource;
                  window.EventSource=class extends Native {constructor(url,options){super(url,options);
                    this.addEventListener('message',event=>{try{window.__mediaPackets.push(JSON.parse(event.data));
                      if(window.__mediaPackets.length>100)window.__mediaPackets.shift();}catch{}});}};})();""")
                page = context.new_page()
                errors, requests = [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("request", lambda request: requests.append(request.url))
                assert page.goto(base, wait_until="networkidle").status == 200

                def open_source(name, marker):
                    if page.viewport_size["width"] < 700 and page.locator(".mobile-back").is_visible():
                        page.locator(".mobile-back").click()
                    page.locator(f'#side .item[data-uid="{uid(corpus, name)}"]').click()
                    expect(page.locator("#msgs")).to_contain_text(marker)

                def images(dimensions):
                    for toggle in page.locator("#msgs .turn-process.folded .fold-toggle").all():
                        toggle.click()
                    found = page.locator("#msgs img")
                    try:
                        expect(found).to_have_count(len(dimensions))
                    except AssertionError:
                        print("MEDIA DOM", page.locator("#msgs img").evaluate_all(
                            "items=>items.map(i=>({src:i.getAttribute('src'),width:i.naturalWidth,height:i.naturalHeight}))"))
                        print("MEDIA TEXT", page.locator("#msgs").inner_text()[:600])
                        raise
                    for item in found.all():
                        item.scroll_into_view_if_needed()
                        expect(item).to_be_visible()
                    page.wait_for_function("Array.from(document.querySelectorAll('#msgs img')).every(i=>i.complete&&i.naturalWidth>0)")
                    actual = found.evaluate_all("items=>items.map(i=>[i.naturalWidth,i.naturalHeight])")
                    assert sorted(actual) == sorted(map(list, dimensions)), actual
                    for src in found.evaluate_all("items=>items.map(i=>i.src)"):
                        assert src.startswith(base) and TOKEN.fullmatch(src[len(base):]), src
                    expect(page.locator("#migration-read-error")).to_have_count(0)

                for source in ("claude", "codex", "grok"):
                    open_source(source + "-media", source.title() + " media complete")
                    images([(2, 3), (3, 2)])
                for source, marker in (("claude", "Claude agent image only"), ("codex", "Codex agent green")):
                    open_source(source + "-media", source.title() + " media complete")
                    page.locator("#a-view-switch").click()
                    page.locator(f'#session-view-menu button[data-agent="{source}-media-agent"]').click()
                    expect(page.locator("#msgs")).to_contain_text(marker)
                    images([(4, 1)])
                    page.locator("#a-view-switch").click()
                    page.locator('#session-view-menu button[data-agent=""]').click()
                    expect(page.locator("#msgs")).to_contain_text(source.title() + " media complete")
                    images([(2, 3), (3, 2)])
                open_source("codex-media-fork", "Codex fork green")
                images([(2, 3), (4, 1)])
                expect(page.locator("#msgs")).not_to_contain_text("Codex mixed media")
                open_source("claude-media", "Claude media complete")
                expect(page.locator("#msgs")).not_to_contain_text("Discarded image branch")
                images([(2, 3), (3, 2)])
                page.reload(wait_until="networkidle")
                expect(page.locator("#msgs")).to_contain_text("Claude media complete")
                images([(2, 3), (3, 2)])
                page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                page.evaluate("window.__mediaPackets=[]")
                path = corpus.paths["claude-media"]
                with path.open("ab") as stream:
                    stream.write(encoded(claude_row("claude-media", "user", "sse-u", "final", [image("claude", GREEN)])))
                    stream.write(encoded(claude_row("claude-media", "assistant", "sse-a", "sse-u", "Media SSE complete")))
                expected_native[str(path.relative_to(corpus.root))] = hashlib.sha256(path.read_bytes()).hexdigest()
                expect(page.locator("#msgs")).to_contain_text("Media SSE complete", timeout=10000)
                images([(2, 3), (3, 2), (4, 1)])
                page.wait_for_function("window.__mediaPackets.some(p=>p.messages?.some(m=>m.media?.length))")
                assert all("data:image" not in json.dumps(packet) for packet in page.evaluate("window.__mediaPackets"))
                page.set_viewport_size({"width": 390, "height": 844})
                for source in ("claude", "codex", "grok"):
                    open_source(source + "-media", "Media SSE complete" if source == "claude" else source.title() + " media complete")
                    images([(2, 3), (3, 2), (4, 1)] if source == "claude" else [(2, 3), (3, 2)])
                    assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
                page.locator(".mobile-back").click()
                page.locator(f'#side .item[data-uid="{uid(corpus, "claude-invalid")}"]').click()
                # Python ignores malformed media and keeps the history readable.
                expect(page.locator("#detail")).not_to_contain_text("读取失败")
                expect(page.locator("#msgs img")).to_have_count(0)
                expect(page.locator("#migration-read-error")).to_have_count(0)
                assert not errors, errors
                assert not any(url.startswith(REMOTE) or not url.startswith(base + "/") for url in requests), requests
                assert native_bytes(corpus.root) == expected_native, "server modified synthetic native bytes"
                print("PASS media browser: three providers, real PNG/JPEG decoding, tool images, isolated agents/fork/branch, reload/SSE/mobile, malformed media ignored, no remote fetch, native bytes unchanged")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
