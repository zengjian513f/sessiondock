#!/usr/bin/env python3
"""Synthetic Grok metadata, optional Python adapter parity and browser SSE.

Uses only generated temporary histories and an isolated loopback Rust server.
No CLI, production state, native home discovery or recorded commands are run.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
from urllib.error import HTTPError

from history_parity import BINARY, Corpus, cursor_query, encoded, get_json, isolated_server
from provider_parity import api, load_adapters, normalized


def uid(path):
    return "grok:" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


INFLIGHT_ROWS = [
    {"type": "user", "content": [{"type": "text", "text":
        "The user sent a message while you were working:\n<user_query>\n不要显示任何续写状况。显示最新那个会话就行。\n</user_query>\n"
        "Make sure to complete any unfinished tasks from previous turns."}], "prompt_index": 7},
    {"type": "user", "content": [{"type": "text", "text":
        "The user interrupted the previous turn:\n<user_query>\n如果有多个提交，amend合并。\n</user_query>\n"
        "Make sure to complete any unfinished tasks from previous turns."}], "prompt_index": 8},
    {"type": "user", "content": [{"type": "text", "text": "请看这段 <user_query>示例</user_query> 标签怎么渲染"}],
     "prompt_index": 9},
    {"type": "user", "content": "<image_files>\n1. /synthetic/shot.png\n</image_files>\nThe user interrupted the previous turn:\n"
                                "<user_query>\n[Image #1] 看图\n</user_query>\nMake sure to complete any unfinished tasks from previous turns.",
     "prompt_index": 10},
    {"type": "user", "content": "The user was away:\n<user_query>\n保留原样\n</user_query>", "prompt_index": 11},
]
INFLIGHT_TEXTS = [
    "不要显示任何续写状况。显示最新那个会话就行。",
    "如果有多个提交，amend合并。",
    "请看这段 <user_query>示例</user_query> 标签怎么渲染",
    "[Image #1] 看图",
    "The user was away:\n<user_query>\n保留原样\n</user_query>",
]


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    cases = {
        "summary-only": ({"session_summary": "Summary only visible"}, None),
        "empty-chat": ({"generated_title": "Empty chat visible"}, b""),
        "explicit": ({"info": {"id": "native-summary-id", "cwd": "/synthetic/explicit"},
                      "generated_title": "  Summary\n title\tchosen ", "session_summary": "Unused",
                      "created_at": "2026-09-11T18:00:00.123+08:00",
                      "last_active_at": "2026-09-11T10:30:00.456Z",
                      "updated_at": "2026-09-12T10:00:00Z",
                      "current_model_id": "synthetic-grok-model", "agent_name": "synthetic-agent"},
                     encoded({"type": "user", "content": "Synthetic Grok body", "prompt_index": 7,
                              "timestamp": "2000-01-01T00:00:00Z"})),
        "fallback": ({"created_at": "invalid", "last_active_at": "invalid",
                      "updated_at": "2026-09-11T10:00:00Z"}, None),
        "numeric": ({"created_at": 1789120800.125, "last_active_at": 1789120800456}, b""),
        "unicode": ({"generated_title": " \t" + "名" * 111 + "\n"}, None),
        "whitespace": ({"generated_title": "\t \n", "session_summary": "Unused"}, None),
        "updated": ({"info": None, "last_active_at": "", "updated_at": "2026-09-11 10:00:00"}, None),
        "browser": ({"generated_title": "Grok browser initial", "created_at": "2026-09-11T10:00:00Z",
                     "updated_at": "2026-09-11T10:00:00Z"}, None),
        # Python 16cc89c: in-flight `user_query` envelopes (message sent while working /
        # after an interrupt) carry a protocol prefix and suffix outside the tags; a tag
        # inside ordinary text is not an envelope.
        "inflight": ({"generated_title": "In-flight envelopes"},
                     b"".join(encoded(row) for row in INFLIGHT_ROWS)),
    }
    for name, (summary, chat) in cases.items():
        path = root / "grok/%2Fsynthetic%2F%E4%B8%AD%E6%96%87+project" / name
        path.mkdir(parents=True)
        (path / "summary.json").write_text(json.dumps(summary), encoding="utf-8")
        os.utime(path / "summary.json", ns=(1700000000456000000,) * 2)
        if chat is not None:
            (path / "chat_history.jsonl").write_bytes(chat)
            os.utime(path / "chat_history.jsonl", ns=(1700000100789000000,) * 2)
        corpus.paths[name] = path
    # These invalid summaries are outside the exact two-level schema.
    (root / "grok/summary.json").write_text("{invalid", encoding="utf-8")
    (corpus.paths["explicit"] / "attachments/deeper").mkdir(parents=True)
    (corpus.paths["explicit"] / "attachments/deeper/summary.json").write_text("{invalid", encoding="utf-8")
    return corpus, cases


def expect_error(opener, base, route, status):
    try:
        get_json(opener, base, route)
    except HTTPError as error:
        assert error.code == status, (route, error.code, status)
        assert json.loads(error.read(1024 * 1024)).get("error")
    else:
        raise AssertionError("invalid Grok input returned successful empty history")


def parity(corpus, cases, base, opener, python_source):
    adapter = load_adapters(python_source, fixture_root=corpus.root)["grok"] if python_source else None
    listed = get_json(opener, base, "/api/sessions")["sessions"]
    assert {row["uid"] for row in listed} == {uid(path) for path in corpus.paths.values()}
    for name, path in corpus.paths.items():
        batch = api(opener, base, uid(path))
        meta = batch["meta"]
        chat = path / "chat_history.jsonl"
        assert meta["path"] == str(path) and meta["chat_exists"] == chat.exists()
        assert batch["version"]["exists"] == chat.exists()
        assert (batch["version"]["mtime"] is None) == (not chat.exists())
        # Batch 44 WP-C: Python `_dir_size`, the whole session directory.
        size = sum(p.stat().st_size for p in path.rglob("*") if p.is_file())
        assert meta["size"] == size and "size_scope" not in meta, (meta["size"], size)
        if not chat.exists() or not chat.stat().st_size:
            assert batch["messages"] == [] and batch["end"] == batch["start"] == 0
        if adapter:
            expected = adapter.session_meta(path)
            for field in ("uid", "source", "sid", "title", "cwd", "created", "updated", "path", "model", "branch"):
                old, new = expected.get(field), meta.get(field)
                if field in ("created", "updated"):
                    old, new = normalized({"ts": old}).get("ts"), normalized({"ts": new}).get("ts")
                assert old == new, (name, field, old, new)
            expected_messages, end = adapter.read(str(path))
            assert end == batch["end"]
            def body(message):
                fields = normalized(message)
                # Existing parser difference: Python Grok omits record ts,
                # Rust preserves it. Metadata timestamps above are compared.
                fields.pop("ts", None)
                return fields
            assert [body(message) for message in expected_messages] == [body(message) for message in batch["messages"]]
            assert expected["size"] == size, (name, expected["size"], size)
        if name == "inflight":
            texts = [message["text"] for message in batch["messages"] if message.get("role") == "user"]
            assert texts == INFLIGHT_TEXTS, texts
    path = corpus.paths["summary-only"]
    old = api(opener, base, uid(path))
    chat = path / "chat_history.jsonl"
    chat.write_bytes(b"")
    empty = api(opener, base, uid(path), cursor_query(old, append=1))
    assert empty["reset"] is False and empty["messages"] == [] and empty["meta"]["chat_exists"] is True
    assert empty["anchor"] == old["anchor"] and empty["end"] == old["end"] == 0
    row = encoded({"type": "user", "content": "Grok incremental fixture", "prompt_index": 1})
    chat.write_bytes(row[:-2])
    partial = api(opener, base, uid(path), cursor_query(empty, append=1))
    assert partial["end"] == 0 and partial["messages"] == []
    with chat.open("ab") as output:
        output.write(row[-2:])
    committed = api(opener, base, uid(path), cursor_query(partial, append=1))
    assert committed["reset"] is False and committed["messages"][0]["text"] == "Grok incremental fixture"
    chat.unlink()  # Exact temporary fixture created above.
    deleted = api(opener, base, uid(path), cursor_query(committed, append=1))
    assert deleted["reset"] is True and deleted["end"] == 0 and deleted["version"]["exists"] is False
    summary = path / "summary.json"
    original = summary.read_bytes()
    for invalid in (b"{broken", b"null", b'{"info":42}'):
        summary.write_bytes(invalid)
        # Batch 34: an invalid summary fails only its own session. The list
        # still publishes (one file never fails the list) with the row marked
        # unsupported and the reason in migration_warnings; opening it is 503.
        expect_error(opener, base, "/api/messages/" + uid(path), 503)
        listed = get_json(opener, base, "/api/sessions?force=1")
        row = next(r for r in listed["sessions"] if r["uid"] == uid(path))
        assert row.get("supported") is False and row.get("migration_warnings"), row
    with summary.open("wb") as output:
        output.truncate(16 * 1024 * 1024 + 1)  # budgets::GROK_SUMMARY_BYTES + 1
    expect_error(opener, base, "/api/messages/" + uid(path), 413)
    summary.write_bytes(original)
    assert api(opener, base, uid(path))["meta"]["chat_exists"] is False


def browser_check(corpus, cases, base):
    from playwright.sync_api import expect, sync_playwright
    path = corpus.paths["browser"]
    summary = path / "summary.json"
    chat = path / "chat_history.jsonl"
    with sync_playwright() as playwright:
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            context.add_init_script("""(() => { window.__grokPackets = []; const Native = window.EventSource;
              window.EventSource = class extends Native { constructor(url, options) { super(url, options);
                this.addEventListener('message', e => { try { window.__grokPackets.push(JSON.parse(e.data)); } catch {} });
              }}; })();""")
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(base, wait_until="networkidle")
            item = page.locator(f'#side .item[data-uid="{uid(path)}"]')
            item.click()
            expect(page.locator(".dtitle h2")).to_contain_text("Grok browser initial")
            expect(page.locator("#a-term")).to_be_visible()
            expect(page.locator("#a-term")).to_be_enabled()
            page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
            page.evaluate("window.__grokPackets = []")
            chat.write_bytes(b"")
            # Same byte cursor and fixed summary timestamps: existence alone
            # must still cause a real browser EventSource metadata packet.
            page.wait_for_function("window.__grokPackets.some(p => p.meta?.chat_exists === true && p.version?.exists === true && p.reset === false && p.end === 0 && p.messages.length === 0)")
            page.evaluate("window.__grokPackets = []")
            updated = dict(cases["browser"][0], generated_title="Grok SSE summary changed")
            summary.write_text(json.dumps(updated), encoding="utf-8")
            expect(page.locator(".dtitle h2")).to_contain_text("Grok SSE summary changed", timeout=10000)
            expect(item.locator(".t")).to_have_text("Grok SSE summary changed")
            page.wait_for_function("window.__grokPackets.some(p => p.meta?.title === 'Grok SSE summary changed' && p.reset === false && p.messages.length === 0)")
            chat.write_bytes(encoded({"type": "user", "content": "Browser Grok appended body", "prompt_index": 1}))
            expect(page.locator("#msgs")).to_contain_text("Browser Grok appended body", timeout=10000)
            summary.write_bytes(b"{broken")
            expect(page.locator("#migration-read-error")).to_be_visible(timeout=10000)
            expect(page.locator("#msgs")).to_contain_text("Browser Grok appended body")
            expect(page.locator("#a-term")).to_be_visible()
            summary.write_text(json.dumps(updated), encoding="utf-8")
            page.locator("#migration-read-error button").click()
            expect(page.locator("#migration-read-error")).to_have_count(0)
            page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
            page.evaluate("window.__grokPackets = []")
            chat.unlink()  # Exact temporary fixture created above.
            page.wait_for_function("window.__grokPackets.some(p => p.meta?.chat_exists === false && p.version?.mtime === null && p.end === 0)")
            expect(page.locator("#msgs")).not_to_contain_text("Browser Grok appended body", timeout=10000)
            page.set_viewport_size({"width": 390, "height": 844})
            if not page.locator("#a-term").is_visible():
                item.click()
            expect(page.locator(".dtitle h2")).to_contain_text("Grok SSE summary changed")
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
    with tempfile.TemporaryDirectory(prefix="sessiondock-grok-") as temporary:
        corpus, cases = build(Path(temporary))
        unchanged = {path: path.read_bytes() for name, directory in corpus.paths.items()
                     if name not in ("summary-only", "browser") for path in directory.rglob("*") if path.is_file()}
        with isolated_server(corpus, args.binary) as (base, opener):
            parity(corpus, cases, base, opener, args.python_source)
            if args.browser:
                browser_check(corpus, cases, base)
        assert all(path.read_bytes() == old for path, old in unchanged.items()), "reads modified synthetic native history"
    print("PASS Grok synthetic metadata: summary-only/empty, UID, timestamps/title/cwd/model, bounded size, incremental cursor, corruption/recovery, in-flight user_query envelopes"
          + (", Python adapter parity" if args.python_source else "")
          + (", desktop/mobile Chromium and existence/title/body SSE" if args.browser else ""))


if __name__ == "__main__":
    main()
