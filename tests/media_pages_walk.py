#!/usr/bin/env python3
"""Scale walk of per-message media continuation over real HTTP pages.

Private Codex (250 + 16 embedded images) and Claude tool_result (100 images)
fixtures on a loopback Rust server. No CLI, native home, remote fetch, or model.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode

from history_parity import (
    BINARY, Corpus, claude_row, codex_message, codex_row, encoded, get_json, isolated_server,
)
from media_browser import GREEN, PNG, TOKEN, image, uid

HEX32 = re.compile(r"[0-9a-f]{32}\Z")
PNG_SHA = hashlib.sha256(base64.b64decode(PNG)).hexdigest()
GREEN_SHA = hashlib.sha256(base64.b64decode(GREEN)).hexdigest()


def data_at(index):
    return PNG if index % 2 == 0 else GREEN


def digest_at(index):
    return PNG_SHA if index % 2 == 0 else GREEN_SHA


def blocks(source, count):
    return [image(source, data_at(index)) for index in range(count)]


def codex_images(text, count):
    return codex_row("response_item", {"type": "message", "role": "user", "content": [
        {"type": "input_text", "text": text}, *blocks("codex", count)]})


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    corpus.put("codex-walk", "codex", [
        codex_row("session_meta", {"id": "codex-walk", "cwd": "/synthetic/pages-walk"}),
        codex_images("WALK 250", 250), codex_images("WALK 16", 16),
        codex_message("assistant", "WALK RECEIVED")], [])
    name = "claude-walk"
    # Claude user blocks split into messages; one tool_result is a single message.
    corpus.put(name, "claude", [
        claude_row(name, "user", "u0", None, "CLAUDE WALK"),
        claude_row(name, "assistant", "call", "u0",
                   [{"type": "tool_use", "id": "tool-walk", "name": "synthetic_image", "input": {}}]),
        claude_row(name, "user", "result", "call", [{"type": "tool_result", "tool_use_id": "tool-walk",
            "content": [{"type": "text", "text": "CLAUDE 100 IMAGES"}, *blocks("claude", 100)]}]),
        claude_row(name, "assistant", "a0", "result", "CLAUDE WALK RECEIVED")], [])
    corpus.put("codex-over", "codex", [
        codex_row("session_meta", {"id": "codex-over", "cwd": "/synthetic/pages-over"}),
        codex_images("WALK 257", 257)], [])
    return corpus


def media_route(corpus, name, cursor):
    return "/api/messages/" + uid(corpus, name) + "/media-page?" + urlencode({"cursor": cursor})


def expect_http(opener, base, route, code):
    try:
        with opener.open(base + route, timeout=10) as response:
            response.read(4096)
    except HTTPError as error:
        body = error.read(8192).decode()
        assert error.code == code, (route, error.code, code, body)
        return body
    raise AssertionError(f"expected HTTP {code} for {route}")


def fetch_page(opener, base, route):
    with opener.open(base + route, timeout=10) as response:
        assert response.status == 200
        return json.loads(response.read(1024 * 1024 + 1))


def find(window, marker):
    found = [message for message in window["messages"] if marker in message["text"] and message.get("media")]
    assert len(found) == 1, [message["text"] for message in window["messages"]]
    return found[0]


def srcs(items):
    tokens = []
    for item in items:
        assert TOKEN.fullmatch(item["src"]) and item.get("lazy") is True and "error" not in item, item
        tokens.append(item["src"])
    return tokens


def check_bytes(opener, base, tokens, start):
    for offset, source in enumerate(tokens):
        with opener.open(base + source, timeout=10) as reply:
            assert reply.status == 200
            assert hashlib.sha256(reply.read(65536)).hexdigest() == digest_at(start + offset)


def walk(opener, base, corpus, name, marker, total):
    window = get_json(opener, base, "/api/messages/" + uid(corpus, name) + "?window=1")
    dumped = json.dumps(window)
    assert PNG not in dumped and GREEN not in dumped and "data:image" not in dumped
    message = find(window, marker)
    assert len(message["media"]) == 16, len(message["media"])
    more = message["media_more"]
    assert more["remaining"] == total - 16 and more["total"] == total, more
    assert HEX32.fullmatch(more["cursor"]), more
    tokens = srcs(message["media"])
    check_bytes(opener, base, tokens, 0)
    cursor, start = more["cursor"], 16
    while True:
        route = media_route(corpus, name, cursor)
        reply = fetch_page(opener, base, route)
        assert fetch_page(opener, base, route)["page"]["next"] == reply["page"]["next"]
        page, media = reply["page"], reply["media"]
        end = min(start + 16, total)
        assert page["cursor"] == cursor and page["start"] == start and page["end"] == end, page
        assert page["total"] == total and page["remaining"] == total - end, page
        assert 0 < len(media) <= 16 and len(media) == end - start
        batch = srcs(media)
        tokens += batch
        check_bytes(opener, base, batch, start)
        start = end
        if page["remaining"] == 0:
            assert page["next"] is None, page
            break
        assert HEX32.fullmatch(page["next"]) and page["next"] != cursor, page
        cursor = page["next"]
    assert start == total and len(tokens) == total and len(set(tokens)) == total
    return window, more["cursor"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-media-pages-") as temporary:
        corpus = build(Path(temporary))
        with isolated_server(corpus, args.binary) as (base, opener):
            window, cursor = walk(opener, base, corpus, "codex-walk", "WALK 250", 250)
            sixteen = find(window, "WALK 16")
            assert len(sixteen["media"]) == 16 and "media_more" not in sixteen, sixteen
            print("PASS codex window: 250-image 16 inline + media_more remaining=234 total=250; 16-image has no media_more")
            print("PASS walk 250: start/end/total/remaining, ≤16/page, unique src, stable next, sha256 PNG/GREEN")
            walk(opener, base, corpus, "claude-walk", "CLAUDE 100 IMAGES", 100)
            print("PASS claude tool_result 100-image continuation pages")
            walk(opener, base, corpus, "codex-over", "WALK 257", 257)
            print("PASS 257 images are all available through continuation")
            expect_http(opener, base, media_route(corpus, "codex-walk", "xyz"), 400)
            expect_http(opener, base, media_route(corpus, "claude-walk", cursor), 403)
            print("PASS malformed media cursor 400; cursor on the other session 403")
            with corpus.paths["codex-walk"].open("ab") as stream:
                stream.write(encoded(codex_message("assistant", "APPENDED AFTER GRANT")))
            appended = get_json(opener, base, "/api/messages/" + uid(corpus, "codex-walk") + "?window=1")
            assert appended["messages"][-1]["text"] == "APPENDED AFTER GRANT"
            after = fetch_page(opener, base, media_route(corpus, "codex-walk", cursor))
            assert after["page"]["start"] == 16 and after["page"]["end"] == 32 and after["page"]["total"] == 250
            check_bytes(opener, base, srcs(after["media"]), 16)
            print("PASS append keeps the old media cursor valid")


if __name__ == "__main__":
    main()
