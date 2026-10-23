#!/usr/bin/env python3
"""Synthetic Codex names: isolated HTTP and desktop/mobile Chromium checks.

Never imports Python index/state or discovers CLI homes. Every native path is
generated in a temporary directory and the recorded messages execute nothing.
"""
from __future__ import annotations


import argparse
from contextlib import contextmanager
import os
from pathlib import Path
from frontend_paths import frontend_dir
import socket
import subprocess
import sys
import tempfile
import time
from urllib.error import URLError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, build_opener

from history_fixtures import BINARY, REPO, Corpus, codex_row, codex_message, encoded, get_json, cursor_query
from http_fixtures import NoRedirects, api


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
    environment.update(SESSIONDOCK_BIND=f"127.0.0.1:{port}", SESSIONDOCK_WEB_DIR=str(frontend_dir()))
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


def parity(corpus, base, opener, index, rows):
    listed = get_json(opener, base, "/api/sessions")
    actual = {row["sid"]: row for row in listed["sessions"]}
    assert "helper" not in actual
    assert actual["root"]["title"] == "Root renamed title"
    assert actual["leaf"]["title"] == "Middle renamed title"
    assert actual["leaf"]["renamed_to"] is None
    assert actual["invalid-date"]["renamed_at"] is None
    assert actual["invalid-date"]["title"] == "Valid name without date"
    assert actual["unicode"]["title"] == "名称" * 55 + "…"
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
            page.locator("#a-chat").click()
            expect(page.locator(".dtitle h2")).to_contain_text("Updated standalone title")
            expect(page.locator("#msgs")).to_contain_text("standalone synthetic searchable message")
            page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
            page.evaluate("window.__namePackets = []")
            rows.append({"id": "standalone", "thread_name": "SSE renamed title", "updated_at": "2026-09-11T11:00:00Z"})
            write_index(index, rows)
            expect(page.locator(".dtitle h2")).to_contain_text("SSE renamed title", timeout=10000)
            expect(item.locator(".t")).to_have_text("SSE renamed title")
            page.wait_for_function("window.__namePackets.some(p => p.meta?.title === 'SSE renamed title' && p.reset === false && p.messages.length === 0)")
            page.reload(wait_until="networkidle")
            item.click()
            expect(page.locator(".dtitle h2")).to_contain_text("SSE renamed title")
            expect(page.locator("#msgs")).to_contain_text("standalone synthetic searchable message")
            index.write_bytes(b"{broken}\n")
            expect(page.locator(".dtitle h2")).to_contain_text("standalone synthetic searchable message", timeout=10000)
            expect(page.locator("#migration-read-error")).to_have_count(0)
            expect(page.locator("#msgs")).to_contain_text("standalone synthetic searchable message")
            expect(page.locator("#a-term")).to_be_visible()
            expect(page.locator("#a-term")).to_have_attribute("aria-disabled", "true")
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


def first_input_check(binary):
    """Large preambles, late first input, append/cold/rewrite and report priority."""
    from playwright.sync_api import expect, sync_playwright
    with tempfile.TemporaryDirectory(prefix="sessiondock-first-name-") as temporary:
        corpus, index, names = build(Path(temporary))
        prefix = [codex_row("session_meta", {"id": "late", "cwd": "/synthetic/names"}),
                  codex_message("developer", "Synthetic tools and rules " + "x" * 135000),
                  *[codex_row("token_usage_record", {"total_tokens": n}) for n in range(125)],
                  codex_message("user", "# AGENTS.md instructions for /synthetic/names\n<INSTRUCTIONS>Rules</INSTRUCTIONS>"),
                  codex_message("user", "<environment_context>synthetic environment</environment_context>")]
        path = corpus.put("late", "codex", prefix, [])
        initial = path.read_bytes()
        title = "执行旧实验补足与重评，检查首条消息后的自动标题"
        user = encoded(codex_message("user", title))
        shots = REPO / "target/validation/codex-first-name"
        shots.mkdir(parents=True, exist_ok=True)
        with sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            with server(corpus, binary, None) as (base, opener):
                page = browser.new_page(viewport={"width": 1280, "height": 900})
                page.goto(base, wait_until="networkidle")
                item = page.locator(f'#side .item[data-uid="{corpus.uid("late")}"]')
                item.click()
                page.locator("#a-chat").click()
                expect(page.locator(".dtitle h2")).to_contain_text("(无标题)")
                page.screenshot(path=str(shots / "before.png"))
                # A torn native record is not a submitted input yet.
                with path.open("ab") as out:
                    out.write(user[:-1])
                page.reload(wait_until="networkidle")
                expect(page.locator(".dtitle h2")).to_contain_text("(无标题)")
                with path.open("ab") as out:
                    out.write(b"\n")
                expect(page.locator(".dtitle h2")).to_have_text(title, timeout=10000)
                expect(item.locator(".t")).to_have_text(title)
                expect(page.locator("#msgs")).to_contain_text(title)
                page.screenshot(path=str(shots / "desktop.png"))
                with path.open("ab") as out:
                    out.write(encoded(codex_row("token_usage_record", {"padding": "y" * 600000})))
                    out.write(encoded(codex_message("user", "Later input must not rename")))
                page.reload(wait_until="networkidle")
                expect(page.locator(".dtitle h2")).to_have_text(title)
                search = get_json(opener, base, "/api/search?" + urlencode({"q": "旧实验补足"}))
                assert next(r for r in search["results"] if r["sid"] == "late")["title"] == title
                # Reports and manual names always keep native explicit precedence.
                for explicit_title in ("BUG: 保留缺陷报告专用名称", "功能需求: 保留功能报告专用名称", "Manual title"):
                    write_index(index, names + [{"id": "late", "thread_name": explicit_title}])
                    expect(page.locator(".dtitle h2")).to_have_text(explicit_title, timeout=10000)
                    expect(item.locator(".t")).to_have_text(explicit_title)
                write_index(index, names)
                expect(page.locator(".dtitle h2")).to_have_text(title, timeout=10000)
                page.set_viewport_size({"width": 390, "height": 844})
                page.emulate_media(color_scheme="dark")
                item.click()
                expect(page.locator(".dtitle h2")).to_have_text(title)
                page.screenshot(path=str(shots / "mobile-dark.png"))
                page.close()
            # Cold scan must find input outside both the head and tail windows.
            with server(corpus, binary, None) as (base, opener):
                page = browser.new_page(viewport={"width": 1280, "height": 900})
                page.goto(base, wait_until="networkidle")
                page.locator(f'#side .item[data-uid="{corpus.uid("late")}"]').click()
                expect(page.locator(".dtitle h2")).to_have_text(title)
                # Truncation and replacement discard the old first-input cache.
                path.write_bytes(initial)
                page.reload(wait_until="networkidle")
                expect(page.locator(".dtitle h2")).to_contain_text("(无标题)")
                replacement = path.with_suffix(".replacement")
                replacement.write_bytes(initial + encoded(codex_message("user", "Replacement input")))
                replacement.replace(path)
                page.reload(wait_until="networkidle")
                expect(page.locator(".dtitle h2")).to_have_text("Replacement input")
                path.write_bytes(initial + encoded(codex_message("user", "Same-length input")))
                page.reload(wait_until="networkidle")
                expect(page.locator(".dtitle h2")).to_have_text("Same-length input")
                page.close()
            browser.close()
    print("PASS first-input title beyond head/tail: append, partial line, cold read, rewrite, report/manual priority", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    first_input_check(args.binary)
    for explicit in (False, True):
        with tempfile.TemporaryDirectory(prefix="sessiondock-names-") as temporary:
            corpus, index, rows = build(Path(temporary))
            if explicit:
                override = index.with_name("override.jsonl")
                index.rename(override)
                write_index(index, [{"id": "root", "thread_name": "Must not override explicit names"}])
                index = override
            before = {path: path.read_bytes() for path in corpus.paths.values()}
            with server(corpus, args.binary, index if explicit else None) as (base, opener):
                parity(corpus, base, opener, index, rows)
                browser_check(corpus, base, index, rows)
            assert all(path.read_bytes() == old for path, old in before.items()), "name reads changed native transcripts"
    print("PASS synthetic Codex names: default sibling/explicit override, file-order/time/unicode, fork/agent titles, metadata-only cursor, search, corruption/recovery, native unchanged"
          + ", desktop/mobile Chromium and SSE")


if __name__ == "__main__":
    main()
