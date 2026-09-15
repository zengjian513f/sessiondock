#!/usr/bin/env python3
"""Synthetic Python adapter/media versus isolated Rust HTTP differential.

Requires an explicitly selected Python checkout. Never starts its service,
discovers real histories, reads native homes, runs a CLI or fetches a remote URL.
Differences are named assertions, not transformations that hide missing media.
"""
from __future__ import annotations

import argparse
import base64
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
import hashlib
import importlib.util
import json
import mimetypes
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import types
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import unquote, urlsplit

sys.dont_write_bytecode = True

from history_parity import BINARY, Corpus, claude_row, codex_row, encoded, isolated_server
from media_browser import PNG, JPEG, GREEN, TOKEN, image, native_bytes, route
from media_formats_browser import FORMATS, excessive_gif
from provider_parity import normalized
from python_oracle import package_dir as oracle_package_dir


def forbidden(*args, **kwargs):
    raise AssertionError("Python differential attempted discovery, network or process execution")


@contextmanager
def python_only():
    # These guards surround Python adapter/media calls, not the separately
    # authorized Rust loopback server or the harness's HTTP requests.
    with ExitStack() as stack:
        for owner, name in ((socket, "create_connection"), (socket.socket, "connect"),
                            (socket.socket, "connect_ex"), (subprocess, "Popen"), (os, "system")):
            stack.enter_context(patch.object(owner, name, forbidden))
        yield


def load_python(source, root):
    source = source.resolve(strict=True)
    package_dir = oracle_package_dir(source, ("adapters.py", "media.py"))
    namespace = "_sessiondock_synthetic_media_parity"
    package = types.ModuleType(namespace)
    package.__path__ = [str(package_dir)]
    sys.modules[namespace] = package
    modules = {}
    with python_only(), patch.object(Path, "home", return_value=root / "unused-home"):
        for name in ("media", "adapters"):
            spec = importlib.util.spec_from_file_location(namespace + "." + name, package_dir / (name + ".py"))
            module = importlib.util.module_from_spec(spec)
            sys.modules[spec.name] = module
            spec.loader.exec_module(module)
            setattr(package, name, module)
            modules[name] = module
    adapter_module, media = modules["adapters"], modules["media"]
    adapter_module.CLAUDE_ROOT = root / "claude"
    adapter_module.CODEX_ROOT = root / "codex"
    adapter_module.GROK_ROOT = root / "grok"
    adapter_module.CODEX_INDEX = root / "unused-session-index"
    instances = {name: getattr(adapter_module, name.title() + "Adapter")() for name in ("claude", "codex", "grok")}
    for adapter in instances.values():
        adapter.list_sessions = forbidden
        adapter.scan_sessions = forbidden
    instances["codex"]._find_session_path = forbidden
    instances["codex"]._name_event = lambda sid: None
    instances["codex"]._thread_names = lambda: {}
    original_register = media.register_path

    def fixture_path(value, cwd=None, name="图片"):
        # Keep original Python path resolution, symlink behavior, MIME and token
        # semantics, but only after proving both spelling and target are synthetic.
        raw = unquote(value.strip().strip("<>"))
        if raw.startswith("file://"):
            raw = urlsplit(raw).path
        if raw.startswith("~"):
            raise AssertionError("HOME expansion is outside synthetic parity")
        candidate = Path(raw)
        if not candidate.is_absolute():
            if not cwd:
                return original_register(value, cwd, name)
            candidate = Path(cwd) / candidate
        if not candidate.is_relative_to(root) or not candidate.resolve().is_relative_to(root):
            raise AssertionError("Python media attempted a non-fixture path")
        return original_register(value, cwd, name)

    media.register_path = fixture_path
    # MIME guessing is code-under-test, but host-specific MIME configuration is
    # unnecessary. Use the standard-library builtin mappings without file reads.
    with patch.object(mimetypes, "knownfiles", []):
        mimetypes.init()
    loaded = {name for name in sys.modules if name.startswith(namespace + ".")}
    required = {namespace + ".adapters", namespace + ".media"}
    # Newer comparison checkouts share a pure JSON decoder with the adapters.
    # Keep the import boundary narrow and verify every helper's source.
    assert required <= loaded <= required | {namespace + ".fastjson"}, sorted(loaded)
    for name in loaded:
        expected = package_dir / (name.rsplit(".", 1)[1] + ".py")
        assert Path(sys.modules[name].__file__).resolve() == expected.resolve(), name
    return instances, media


@dataclass
class Case:
    name: str
    source: str
    kind: str
    content: object
    delta: str | None = None


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    files = root / "files"
    files.mkdir()
    formats = (("png", "image/png", PNG), ("jpeg", "image/jpeg", JPEG), *FORMATS)
    for name, _, data in formats:
        extension = "gif" if name.startswith("gif") else "webp" if name.startswith("webp") else name
        (files / (name + "." + extension)).write_bytes(base64.b64decode(data))
    (files / "unknown.payload").write_bytes(base64.b64decode(PNG))
    (root / "outside.png").write_bytes(base64.b64decode(GREEN))
    if os.name != "nt":
        (files / "linked.png").symlink_to(files / "png.png")
    cases = []
    for source in ("claude", "codex", "grok"):
        blocks = [image(source, data, mime) for _, mime, data in formats]
        cases.extend([
            Case("native-formats", source, "user", blocks),
            Case("mixed-native", source, "user", [{"type": "text", "text": "Native mixed text"}, image(source)]),
            Case("tool-formats", source, "tool", [{"type": "text", "text": "Tool mixed text"}, *blocks], "codex-tool-media" if source == "codex" else None),
            Case("mcp-native", source, "user", [{"type": "image", "mimeType": "image/png", "data": PNG}]),
            Case("mcp-tool", source, "tool", [{"type": "text", "text": "MCP mixed text"}, {"type": "image", "mimeType": "image/png", "data": PNG}], "codex-tool-media" if source == "codex" else None),
            Case("mcp-envelope", source, "tool", {"isError": False, "content": [{"type": "text", "text": "MCP envelope text"}, {"type": "image", "mimeType": "image/png", "data": PNG}]}, "mcp-envelope"),
            Case("absolute-url", source, "user", [{"type": "image_url", "image_url": str(files / "png.png")}]),
            Case("file-uri", source, "user", [{"type": "image_url", "image_url": {"url": (files / "png.png").as_uri()}}]),
            Case("structured-path", source, "user", [{"type": "image", "source": {"type": "file", "path": str(files / "png.png")}}], "path-field"),
            Case("unknown-extension", source, "user", [{"type": "image_url", "image_url": str(files / "unknown.payload")}], "unknown-extension"),
            Case("text-paths", source, "user", [{"type": "text", "text": "Markdown ![image alt](./png.png) and raw ./jpeg.jpeg"}], "raw-gallery-ref"),
            Case("fenced-path", source, "user", [{"type": "text", "text": "Literal code\n```\n./png.png\n```\nNo image requested"}]),
            Case("tool-raw-path", source, "tool", [{"type": "text", "text": "Synthetic tool stdout ./png.png"}]),
            Case("tool-markdown", source, "tool", [{"type": "text", "text": "Explicit ![tool image](./png.png)"}]),
        ])
    cases.extend([
        Case("remote", "claude", "user", [{"type": "image_url", "image_url": "https://media.example.invalid/never.png"}]),
        Case("outside-root", "claude", "user", [{"type": "image_url", "image_url": str(root / "outside.png")}]),
        Case("invalid-container", "claude", "user", [image("claude", "AAAA")]),
        Case("many-frame-gif", "claude", "user", [image("claude", base64.b64encode(excessive_gif(129)).decode(), "image/gif")]),
    ])
    if os.name != "nt":
        cases.append(Case("symlink", "claude", "user", [{"type": "image_url", "image_url": str(files / "linked.png")}]))
    for case in cases:
        sid = case.source + "-" + case.name
        if case.source == "claude":
            content = case.content if case.kind == "user" else [{"type": "tool_result", "tool_use_id": "synthetic", "content": case.content}]
            corpus.put(sid, "claude", [claude_row(sid, "user", "u", None, content, cwd=str(files))], [])
        elif case.source == "codex":
            payload = {"type": "message", "role": "user", "content": case.content} if case.kind == "user" else {"type": "function_call_output", "call_id": "synthetic", "output": case.content}
            corpus.put(sid, "codex", [codex_row("session_meta", {"id": sid, "cwd": str(files)}), codex_row("response_item", payload)], [])
        else:
            path = root / "grok/project" / sid
            path.mkdir(parents=True)
            (path / "summary.json").write_text(json.dumps({"info": {"id": sid, "cwd": str(files)}}))
            row = {"type": "user", "prompt_index": 1, "content": case.content} if case.kind == "user" else {"type": "tool_result", "tool_call_id": "synthetic", "content": case.content}
            (path / "chat_history.jsonl").write_bytes(encoded(row))
            corpus.paths[sid] = path
    return corpus, files, cases


def rust_response(opener, base, path):
    try:
        with opener.open(base + path, timeout=10) as response:
            body = response.read(8 * 1024 * 1024 + 1)
            assert len(body) <= 8 * 1024 * 1024
            return response.status, json.loads(body)
    except HTTPError as error:
        return error.code, json.loads(error.read(8192))


def digest(data):
    return hashlib.sha256(data).hexdigest()


def media_signature(item, fetch):
    if "error" in item:
        assert "src" not in item
        return {"error": item["error"]["status"]}
    src = item["src"]
    if item.get("external"):
        assert src == "https://media.example.invalid/never.png"
        return {"external": src}
    assert TOKEN.fullmatch(src), src
    if item.get("lazy") is True:
        assert not {"mime", "width", "height"}.intersection(item)
    try:
        data, mime = fetch(src)
    except HTTPError as error:
        assert item.get("lazy") is True
        detail = json.loads(error.read(8192))
        assert detail.get("error") and "base64" not in json.dumps(detail)
        return {"error": error.code}
    if item.get("lazy") is not True:
        assert item["mime"] == mime
    # Ignore token identity, display alt and deferred dimensions/MIME metadata.
    # Preserve byte identity, MIME, count, ordering and inline/gallery semantics.
    result = {"sha256": digest(data), "mime": mime}
    if "ref" in item:
        result["ref"] = item["ref"]
    if item.get("gallery"):
        result["gallery"] = True
    return result


def signatures(messages, fetch):
    return [{"message": normalized(row), "media": [media_signature(item, fetch) for item in row.get("media", [])]}
            for row in messages if row.get("role") != "status"]


def flatten(rows):
    return [item for row in rows for item in row["media"]]


def check_delta(case, python, rust, status):
    left, right = flatten(python), flatten(rust)
    delta = case.delta
    assert status == 200
    if delta == "raw-gallery-ref":
        assert len(python) == len(rust) == 1
        assert python[0]["message"] == rust[0]["message"]
        assert len(left) == len(right) == 2 and left[0] == right[0]
        assert "ref" not in left[1] and right[1] == {**left[1], "ref": "./jpeg.jpeg"}
    elif delta in ("path-field", "unknown-extension", "mcp-envelope", "codex-tool-media"):
        assert left == [], (delta, left)
        encodings = (("png","image/png",PNG),("jpeg","image/jpeg",JPEG),*FORMATS) if case.name == "tool-formats" else (("png","image/png",PNG),)
        assert right == [{"sha256":digest(base64.b64decode(data)),"mime":mime} for _,mime,data in encodings], (delta,right)
        assert len(rust) == 1
        if delta in ("path-field", "unknown-extension"):
            assert rust[0]["message"]["role"] == "user" and rust[0]["message"]["text"] == "[图片]"
        else:
            assert rust[0]["message"]["role"] == "tool_result"
            assert rust[0]["message"]["call_id"] == "synthetic"
            assert rust[0]["message"]["text"] == {"tool-formats":"Tool mixed text","mcp-tool":"MCP mixed text","mcp-envelope":"MCP envelope text"}[case.name]
    else:
        raise AssertionError("unclassified delta: " + str(delta))
    assert python != rust or status != 200, "documented difference unexpectedly disappeared"


def browser_check(corpus, base):
    from playwright.sync_api import expect, sync_playwright
    from media_browser import uid
    with sync_playwright() as playwright:
        options = {"headless":True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**options)
        try:
            page = browser.new_page(viewport={"width":1280,"height":900})
            page.route("**/*",lambda request:request.continue_() if request.request.url.startswith(base+"/") else request.abort())
            errors = []
            page.on("pageerror",lambda error:errors.append(str(error)))
            page.goto(base,wait_until="networkidle")
            for name, count, images in (("claude-mixed-native",2,1),("claude-native-formats",8,8),
                                        ("codex-native-formats",1,8),("grok-tool-formats",1,8)):
                page.locator(f'#side .item[data-uid="{uid(corpus,name)}"]').click()
                expect(page.locator("#mcount-total")).to_have_text(f"{count} 条消息")
                for item in page.locator("#msgs .turn-process.folded .fold-toggle").all():
                    item.click()
                expect(page.locator("#msgs img")).to_have_count(images)
                if name.startswith("claude"):
                    expect(page.locator('#msgs .msg[data-role="user"]')).to_have_count(count)
                if name == "claude-mixed-native":
                    expect(page.locator('#msgs .msg[data-role="user"]').first).to_have_text("Native mixed text")
                if name == "codex-native-formats":
                    assert page.locator("#msgs").inner_text().count("[图片]") == 1
                if name == "grok-tool-formats":
                    expect(page.locator("#msgs .tool-out")).to_have_text("Tool mixed text")
                    expect(page.locator("#msgs .more")).to_have_count(0)
                print(f"PASS Chromium {name}: {count} counted native messages, {images} images, no invented placeholder lines")
            assert not errors,errors
        finally:
            browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python-source", required=True, type=Path)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--browser", action="store_true", help="also assert real Rust legacy message grouping in Chromium")
    args = parser.parse_args()
    failures = []
    with tempfile.TemporaryDirectory(prefix="sessiondock-media-parity-") as temporary:
        corpus, files, cases = build(Path(temporary))
        before = native_bytes(corpus.root)
        adapters, python_media = load_python(args.python_source, corpus.root)
        with isolated_server(corpus, args.binary, file_roots=(files,)) as (base, opener):
            def rust_fetch(src):
                with opener.open(base + src, timeout=5) as response:
                    assert response.headers["X-Content-Type-Options"] == "nosniff"
                    return response.read(2 * 1024 * 1024), response.headers["Content-Type"]
            def python_fetch(src):
                with python_only():
                    result = python_media.get(src.rsplit("/", 1)[-1])
                assert result is not None
                return result[0], result[1]
            for case in cases:
                sid = case.source + "-" + case.name
                try:
                    with python_only():
                        native, end = adapters[case.source].read(str(corpus.paths[sid]))
                        for row in native:
                            python_media.enrich_message(row, str(files))
                        expected = signatures(native, python_fetch)
                    status, response = rust_response(opener, base, route(corpus, sid))
                    if status == 200:
                        for row in response["messages"]:
                            for item in row.get("media", []):
                                if "src" in item and not item.get("external"):
                                    assert item.get("lazy") is True
                                    assert not {"mime", "width", "height"}.intersection(item)
                    actual = signatures(response["messages"], rust_fetch) if status == 200 else []
                    if status == 200:
                        assert response["end"] == end, "EOF cursor differs"
                        assert response["message_total"] == sum(row.get("counted") is not False for row in response["messages"])
                        assert all(data not in json.dumps(response) for data in (PNG,JPEG,GREEN))
                    if case.delta:
                        check_delta(case, expected, actual, status)
                        print(f"DELTA {sid}: {case.delta}")
                    else:
                        assert status == 200, (status, response)
                        assert response["message_total"] == sum(row.get("counted") is not False for row in native if row.get("role") != "status")
                        assert len(expected) == len(actual), ("message count",len(expected),len(actual))
                        for index, (left, right) in enumerate(zip(expected, actual)):
                            assert left == right, (index,left,right)
                        print(f"PASS {sid}: {len(actual)} messages, {len(flatten(actual))} exact-byte images")
                except (AssertionError, HTTPError) as error:
                    failures.append(sid)
                    print("FAIL", sid, str(error)[:1800])
            if args.browser:
                browser_check(corpus,base)
            # Python's cached file token identifies an old stat but get() reads
            # the current path; Rust binds the grant to the observed version.
            sid = "claude-absolute-url"
            with python_only():
                native, _ = adapters["claude"].read(str(corpus.paths[sid]))
            python_token = native[0]["media"][0]["src"]
            status, initial = rust_response(opener, base, route(corpus, sid))
            assert status == 200
            rust_token = initial["messages"][0]["media"][0]["src"]
            replacement = files / "replacement.tmp"
            replacement.write_bytes(base64.b64decode(GREEN))
            replacement.replace(files / "png.png")
            assert python_fetch(python_token)[0] == base64.b64decode(GREEN)
            assert rust_response(opener, base, rust_token)[0] == 409
            status, latest = rust_response(opener, base, route(corpus, sid))
            assert status == 200
            fresh_token = latest["messages"][0]["media"][0]["src"]
            assert fresh_token != rust_token
            assert rust_fetch(fresh_token)[0] == base64.b64decode(GREEN)
            print("DELTA file-replacement: Python old token serves replacement; Rust old token409/new token exact replacement bytes")
        # Local reads stay available when no explicit file roots are configured.
        with isolated_server(corpus, args.binary) as (base, opener):
            with python_only():
                native, _ = adapters["claude"].read(str(corpus.paths["claude-absolute-url"]))
                python_bytes = python_media.get(native[0]["media"][0]["src"].rsplit("/",1)[-1])[0]
            assert python_bytes == base64.b64decode(GREEN)
            status, response = rust_response(opener, base, route(corpus,"claude-absolute-url"))
            assert status == 200 and response["messages"][0]["text"] == "[图片]"
            item = response["messages"][0]["media"][0]
            assert item.get("lazy") is True and TOKEN.fullmatch(item["src"])
            with opener.open(base + item["src"], timeout=5) as reply:
                assert reply.status == 200
                assert reply.read() == python_bytes
            print("PASS no-file-roots: ambient local image bytes match Python")
        assert native_bytes(corpus.root) == before, "native fixture modified"
    if failures:
        raise SystemExit("Unexpected media differences: " + ", ".join(failures))
    same = sum(case.delta is None for case in cases)
    print(f"PASS {len(cases)+2} synthetic differential cases: {same+1} exact semantic matches, {len(cases)-same+1} explicitly asserted deltas; no Python service/home/CLI/network")


if __name__ == "__main__":
    main()
