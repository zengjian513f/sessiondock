#!/usr/bin/env python3
"""Synthetic Codex names: isolated HTTP, optional adapter parity and Chromium.

Never imports Python index/state or discovers CLI homes. Every native path is
generated in a temporary directory and the recorded messages execute nothing.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from urllib.error import URLError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, build_opener

from history_parity import BINARY, REPO, Corpus, codex_row, codex_message, encoded, get_json, cursor_query
from provider_parity import NoRedirects, adapter_module, api, load_adapters, normalized


def write_index(path, rows):
    path.write_bytes(b"".join(encoded(row) for row in rows))


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    for sid, parent in (("standalone", None), ("root", None), ("middle", "root"),
                        ("leaf", "middle"), ("invalid-date", None), ("unicode", None), ("helper", "root")):
        meta = {"id": sid, "cwd": "/synthetic/names", "timestamp": "2026-09-11T10:00:00Z"}
        if parent:
            meta["forked_from_id"] = parent
            if sid != "helper":
                meta["history_base"] = {"thread_id": parent, "end_byte_offset": 0}
        if sid == "helper":
            meta.update(thread_source="subagent", parent_thread_id=parent,
                        source={"subagent": {"thread_spawn": {"parent_thread_id": parent,
                                "agent_path": "/root/helper", "agent_role": "reviewer"}}})
        text = sid + " synthetic searchable message"
        corpus.put(sid, "codex", [codex_row("session_meta", meta), codex_message("user", text)],
                   [text], parent="root" if sid == "helper" else None)
    rows = [
        {"id": "standalone", "thread_name": "Initial standalone name", "updated_at": "2026-09-11T18:00:00+08:00"},
        {"id": "root", "thread_name": "Timestamp newer but not final", "updated_at": "2026-09-12T10:00:00Z"},
        {"id": "root", "thread_name": "Root renamed title", "updated_at": "2026-09-11T10:00:00Z"},
        {"id": "root", "thread_name": ""},
        {"id": "middle", "thread_name": "Middle renamed title", "updated_at": 1789120800000},
        {"id": "invalid-date", "thread_name": "Valid name without date", "updated_at": "broken"},
        {"id": "unicode", "thread_name": " \n " + "名称" * 60 + "\t", "updated_at": "2026-09-11 10:00:00"},
        {"id": "helper", "thread_name": "Must not overwrite native agent label"},
    ]
    index = root / "session_index.jsonl"
    write_index(index, rows)
    return corpus, index, rows


@contextmanager
def server(corpus, binary, index):
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    environment.update(SESSIONDOCK_BIND=f"127.0.0.1:{port}", SESSIONDOCK_WEB_DIR=str(REPO / "legacy-web"))
    for source in ("claude", "codex", "grok"):
        environment["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(corpus.root / source)
    if index:
        assert index.resolve().is_relative_to(corpus.root.resolve())
        environment["SESSIONDOCK_CODEX_INDEX"] = str(index)
    base = f"http://127.0.0.1:{port}"
    opener = build_opener(ProxyHandler({}), NoRedirects())
    with tempfile.TemporaryFile(mode="w+b") as log:
        process = subprocess.Popen([str(binary.resolve(strict=True))], cwd=REPO, env=environment, stdout=log, stderr=log)
        try:
            for _ in range(150):
                assert process.poll() is None, "isolated server exited early"
                try:
                    get_json(opener, base, "/api/health")
                    break
                except (OSError, URLError):
                    time.sleep(0.05)
            else:
                raise AssertionError("isolated server failed to become ready")
            yield base, opener
        except BaseException:
            log.flush()
            log.seek(0)
            output = log.read().decode("utf-8", "replace")
            if output:
                print(output, file=sys.stderr, end="")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()  # Only this test's exact child process.
                    process.wait(timeout=5)
                    raise AssertionError("isolated server shutdown exceeded five seconds")


def parity(corpus, base, opener, index, rows, python_source):
    listed = get_json(opener, base, "/api/sessions")
    actual = {row["sid"]: row for row in listed["sessions"]}
    assert "helper" not in actual
    assert actual["root"]["title"] == "Root renamed title"
    assert actual["leaf"]["title"] == "Middle renamed title"
    assert actual["leaf"]["renamed_to"] is None
    assert actual["invalid-date"]["renamed_at"] is None
    assert actual["invalid-date"]["title"] == "Valid name without date"
    assert actual["unicode"]["title"] == "名称" * 55 + "…"
    if python_source:
        adapter = load_adapters(python_source, fixture_root=corpus.root, codex_paths=corpus.paths)["codex"]
        module = adapter_module({"claude": adapter})
        module.CODEX_INDEX = index
        # Restore ONLY the name reader, after every native root and inherited
        # path lookup was sandboxed by the shared adapter-only helper.
        adapter._thread_names = type(adapter)._thread_names.__get__(adapter)
        raw = [adapter.session_meta(path) for path in corpus.paths.values()]
        expected = {row["sid"]: row for row in adapter.finalize_sessions(raw)}
        assert expected.keys() == actual.keys()
        for sid, row in expected.items():
            for field in ("title", "renamed_to", "renamed_at", "fork_depth", "root_sid"):
                old, new = row.get(field), actual[sid].get(field)
                if field == "renamed_at":
                    old, new = normalized({"ts": old}).get("ts"), normalized({"ts": new}).get("ts")
                assert old == new, (sid, field, old, new)
            assert [(a["id"], a["title"]) for a in row.get("agent_items", [])] == [
                (a["id"], a["title"]) for a in actual[sid].get("agent_items", [])]
    original = api(opener, base, corpus.uid("standalone"))
    rows.append({"id": "standalone", "thread_name": "Updated standalone title", "updated_at": "2026-09-11T10:30:00Z"})
    write_index(index, rows)
    updated = api(opener, base, corpus.uid("standalone"), cursor_query(original, append=1))
    assert updated["reset"] is False and updated["messages"] == []
    assert updated["end"] == original["end"] and updated["anchor"] == original["anchor"]
    assert updated["version"]["head"] == original["version"]["head"]
    assert updated["meta"]["title"] == "Updated standalone title"
    search = get_json(opener, base, "/api/search?" + urlencode({"q": "standalone synthetic"}))
    assert next(row for row in search["results"] if row["sid"] == "standalone")["title"] == "Updated standalone title"
    agent = api(opener, base, corpus.uid("root"), urlencode({"agent": "helper"}))
    assert agent["meta"]["parent_title"] == "Root renamed title"
    assert agent["meta"]["title"] == actual["root"]["agent_items"][0]["title"]
    for raw in (b"{broken}\n", b'{"id":"standalone","thread_name":'):
        index.write_bytes(raw)
        listed = get_json(opener, base, "/api/sessions")
        fallback = next(row for row in listed["sessions"] if row["sid"] == "standalone")
        assert fallback["title"] == "standalone synthetic searchable message"
        viewed = api(opener, base, corpus.uid("standalone"))
        assert viewed["meta"]["title"] == fallback["title"]
        search = get_json(opener, base, "/api/search?q=synthetic")
        assert next(row for row in search["results"] if row["sid"] == "standalone")["title"] == fallback["title"]
    index.unlink()  # This tool created this temporary fixture file.
    listed = get_json(opener, base, "/api/sessions")
    assert next(row for row in listed["sessions"] if row["sid"] == "standalone")["title"] == "standalone synthetic searchable message"
    write_index(index, rows)
    assert api(opener, base, corpus.uid("standalone"))["meta"]["title"] == "Updated standalone title"


def browser_check(corpus, base, index, rows):
    from playwright.sync_api import expect, sync_playwright
    with sync_playwright() as playwright:
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            context.add_init_script("""(() => { window.__namePackets = []; const Native = window.EventSource;
              window.EventSource = class extends Native { constructor(url, options) { super(url, options);
                this.addEventListener('message', e => { try { window.__namePackets.push(JSON.parse(e.data)); } catch {} });
              }}; })();""")
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(base, wait_until="networkidle")
            item = page.locator(f'#side .item[data-uid="{corpus.uid("standalone")}"]')
            item.click()
            expect(page.locator(".dtitle h2")).to_contain_text("Updated standalone title")
            expect(page.locator("#msgs")).to_contain_text("standalone synthetic searchable message")
            page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
            page.evaluate("window.__namePackets = []")
            rows.append({"id": "standalone", "thread_name": "SSE renamed title", "updated_at": "2026-09-11T11:00:00Z"})
            write_index(index, rows)
            expect(page.locator(".dtitle h2")).to_contain_text("SSE renamed title", timeout=10000)
            expect(item.locator(".t")).to_have_text("SSE renamed title")
            page.wait_for_function("window.__namePackets.some(p => p.meta?.title === 'SSE renamed title' && p.reset === false && p.messages.length === 0)")
            expect(page.locator("#msgs")).to_contain_text("standalone synthetic searchable message")
            index.write_bytes(b"{broken}\n")
            expect(page.locator(".dtitle h2")).to_contain_text("standalone synthetic searchable message", timeout=10000)
            expect(page.locator("#migration-read-error")).to_have_count(0)
            expect(page.locator("#msgs")).to_contain_text("standalone synthetic searchable message")
            expect(page.locator("#a-term")).to_be_visible()
            expect(page.locator("#a-term")).to_be_enabled()
            write_index(index, rows)
            expect(page.locator(".dtitle h2")).to_contain_text("SSE renamed title", timeout=10000)
            page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
            page.set_viewport_size({"width": 390, "height": 844})
            if not page.locator("#a-term").is_visible():
                item.click()
            expect(page.locator(".dtitle h2")).to_contain_text("SSE renamed title")
            expect(page.locator("#a-term")).to_be_visible()
            assert not errors, errors
            context.close()
        finally:
            browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--python-source", type=Path)
    parser.add_argument("--browser", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-names-") as temporary:
        corpus, index, rows = build(Path(temporary))
        before = {path: path.read_bytes() for path in corpus.paths.values()}
        with server(corpus, args.binary, None) as (base, opener):
            assert api(opener, base, corpus.uid("standalone"))["meta"]["title"] == "standalone synthetic searchable message"
        with server(corpus, args.binary, index) as (base, opener):
            parity(corpus, base, opener, index, rows, args.python_source)
            if args.browser:
                browser_check(corpus, base, index, rows)
        assert all(path.read_bytes() == old for path, old in before.items()), "name reads changed native transcripts"
    print("PASS synthetic Codex names: explicit-only, file-order/time/unicode, fork/agent titles, metadata-only cursor, search, corruption/recovery, native unchanged"
          + (", Python adapter parity" if args.python_source else "") + (", desktop/mobile Chromium and SSE" if args.browser else ""))


if __name__ == "__main__":
    main()
