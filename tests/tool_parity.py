#!/usr/bin/env python3
"""Synthetic rich-tool contract, optional Python differential and Chromium check.

Build the server first. All JSONL and summary files are generated in one new
temporary directory. No native home discovery, actual tool command execution,
media registration, production HTTP, or paid model invocation is permitted.
The --python-source option imports only the explicitly selected adapters.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import tempfile

from history_parity import BINARY, Corpus, claude_row, codex_row, encoded, get_json, isolated_server
from provider_parity import api, load_adapters, normalized


def tool_cases():
    patch = ("*** Begin Patch\n*** Add File: new.rs\n+new file\n*** Update File: old.rs\n"
             "*** Move to: moved.rs\n@@\n-old value\n+new value\n*** Delete File: gone.rs\n-old\n*** End Patch")
    cases = [
        ("Bash", {"command": "/usr/bin/zsh -lc 'printf synthetic'"}),
        ("functions.exec_command", {"cmd": ["bash", "-lc", "rg synthetic src"]}),
        ("functions.exec", 'text(await tools.exec_command({cmd: "one"})); text(await tools.exec_command({"cmd": "two"}));'),
        ("Read", {"file_path": "src/read.rs", "offset": 12, "limit": 20}),
        ("Grep", {"pattern": "needle", "glob": "*.rs"}),
        ("Glob", {"pattern": "src/**/*.rs"}),
        ("WebFetch", {"url": "https://example.test/synthetic"}),
        ("WebSearch", {"query": "synthetic tool fixture"}),
        ("Agent", {"description": "Pure fixture review", "subagent_type": "reviewer"}),
        ("TodoWrite", {"todos": [{"subject": "Inspect"}, {"content": "Test"}, {"subject": "Report"}]}),
        ("functions.update_plan", {"plan": [{"step": "read"}, {"step": "test"}]}),
        ("functions.exec", 'tools.update_plan({plan:[{step:"read"},{step:"test"}]});'),
        ("functions.exec", 'tools.one({}); tools.two({}); tools.one({});'),
        ("unknown_named_tool", {"z": "first", "a": False, "r": 4, "b": "last", "extra": "omitted"}),
        ("Write", {"file_path": "created.rs", "content": "one\r\ntwo\u2028"}),
        ("Edit", {"file_path": "edited.rs", "old_string": "old value\nkeep", "new_string": "new value\nkeep"}),
        ("MultiEdit", {"path": "multi.rs", "edits": [{"old_string": "a", "new_string": "b"}, {}, None]}),
        ("functions.apply_patch", patch),
        ("functions.exec", "text(await tools.apply_patch(" + json.dumps(patch) + "));"),
        ("AskUserQuestion", {"questions": {"header": "Select", "question": "Which fixture?", "multiple": True,
                                         "options": ["A", {"label": "B", "description": "second"}, None]}}),
        ("AskUserQuestion", {"questions": []}),  # Invalid questions fall back to a normal tool.
    ]
    # Deterministic line-diff corpus checks tie breaks, grouping, EOF and popular
    # line autojunk against difflib, not merely superficial + / - counts.
    rng = random.Random(711)
    alphabet = ["a", "b", "c", "", "中文", "++++", "---"]
    for index in range(64):
        before = [rng.choice(alphabet) for _ in range(rng.randrange(0, 55))]
        after = before.copy()
        for _ in range(rng.randrange(1, 8)):
            position = rng.randrange(len(after) + 1)
            after[position:position + rng.randrange(0, 5)] = [rng.choice(alphabet) for _ in range(rng.randrange(0, 5))]
        cases.append(("Edit", {"path": f"random-{index}.txt", "old_string": "\n".join(before), "new_string": "\n".join(after)}))
    popular = ["same"] * 220
    changed = popular.copy()
    changed[110] = "changed"
    cases.append(("Edit", {"path": "autojunk.txt", "old_string": "\n".join(popular), "new_string": "\n".join(changed)}))
    return cases


def build_corpus(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    cases = tool_cases()
    sid = "claude-tools"
    rows = [claude_row(sid, "user", "u", None, "Synthetic rich tools")]
    parent = "u"
    for index, (name, arguments) in enumerate(cases):
        uid = f"a{index}"
        rows.append(claude_row(sid, "assistant", uid, parent,
            [{"type": "tool_use", "id": f"c{index}", "name": name, "input": arguments}]))
        parent = uid
        if name == "AskUserQuestion" and arguments["questions"]:
            uid = f"r{index}"
            rows.append(claude_row(sid, "user", uid, parent, [{"type": "tool_result", "tool_use_id": f"c{index}",
                "is_error": True, "content": "The user doesn't want to proceed with this tool use. fixture"}]))
            parent = uid
    rows.append(claude_row(sid, "assistant", "final", parent, "Synthetic tools complete"))
    corpus.put(sid, "claude", rows, [])

    sid = "codex-tools"
    rows = [codex_row("session_meta", {"id": sid, "cwd": "/synthetic/tools", "timestamp": "2026-09-11T10:00:00Z"}),
            codex_row("response_item", {"type": "message", "role": "user", "turn_id": "turn-tools",
                                       "content": [{"type": "input_text", "text": "Synthetic rich tools"}]})]
    for index, (name, arguments) in enumerate(cases):
        rows.append(codex_row("response_item", {"type": "function_call", "name": name,
                    "arguments": json.dumps(arguments, ensure_ascii=False), "call_id": f"c{index}", "turn_id": "turn-tools"}))
        if name == "AskUserQuestion" and arguments["questions"]:
            rows.append(codex_row("response_item", {"type": "function_call_output", "call_id": f"c{index}", "turn_id": "turn-tools",
                        "output": {"answers": {"first": {"answers": ["A", " ", "B"]}, "empty": {"answers": []}, "third": " C "}}}))
    for index, result in enumerate([
        [{"type": "text", "text": "Script completed\nOutput:\n" + json.dumps({"output": "actual command failed", "exit_code": 2, "wall_time_seconds": .25})},
         {"type": "text", "text": "extra wrapper tail"}],
        {"output": "business JSON, not command envelope"},
        {"text": "plain object text"},
        "Output:\n" + json.dumps({"output": "still running", "session_id": 42, "wall_time_seconds": 1}),
    ]):
        rows.append(codex_row("response_item", {"type": "function_call_output", "call_id": f"out{index}", "turn_id": "turn-tools", "output": result}))
    rows.append(codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "turn-tools",
                "content": [{"type": "output_text", "text": "Synthetic tools complete"}]}))
    corpus.put(sid, "codex", rows, [])

    sid = "grok-tools"
    path = root / "grok/project-tools" / sid
    path.mkdir(parents=True)
    rows = [{"type": "user", "content": "Synthetic rich tools", "prompt_index": 1}]
    for index, (name, arguments) in enumerate(cases):
        rows.append({"type": "assistant", "content": "", "tool_calls": [{"id": f"c{index}", "name": name, "arguments": json.dumps(arguments, ensure_ascii=False)}]})
    rows.append({"type": "tool_result", "tool_call_id": "c0", "content": "synthetic shell output"})
    rows.append({"type": "assistant", "content": "Synthetic tools complete"})
    (path / "chat_history.jsonl").write_bytes(b"".join(encoded(row) for row in rows))
    (path / "summary.json").write_text(json.dumps({"info": {"id": sid, "name": "Synthetic rich tools", "cwd": "/synthetic/tools"}}))
    corpus.paths[sid] = path
    return corpus


def uid(corpus, source):
    return source + ":" + hashlib.sha1(str(corpus.paths[source + "-tools"]).encode()).hexdigest()[:16]


def rich(message):
    result = normalized(message)
    for field in ("summary", "changes"):
        if field in message:
            result[field] = message[field]
    return result


def verify(corpus, base, opener, python_source):
    listed = get_json(opener, base, "/api/sessions?force=1")
    assert len(listed["sessions"]) == 3
    adapters = load_adapters(python_source, fixture_root=corpus.root,
                            codex_paths={"codex-tools": corpus.paths["codex-tools"]}) if python_source else None
    for source in ("claude", "codex", "grok"):
        response = api(opener, base, uid(corpus, source))
        messages = response["messages"]
        assert response["meta"].get("supported", True) is not False
        tools = [row for row in messages if row["role"] == "tool"]
        assert any(row.get("summary") == "$ printf synthetic" for row in tools)
        assert any(row.get("summary") == "z=first a=False r=4 b=last" for row in tools)
        files = [change for row in tools for change in row.get("changes") or []]
        assert any(change.get("new_path") == "moved.rs" for change in files)
        assert len(files) >= 74
        assert all(not row.get("changes_unavailable_reason") for row in tools)
        if source == "codex":
            assert any(row["text"] == "actual command failed" and row.get("error") and row.get("duration_s") == .25 for row in messages)
            assert any(row["role"] == "answer" and row["text"] == "A、B\nC" for row in messages)
        if adapters:
            path = corpus.paths[source + "-tools"]
            native, native_end = adapters[source].read(str(path))
            assert native_end == response["end"], (source, "cursor")
            # The API exposes activity separately from its countable body.
            expected = [rich(row) for row in native if row["role"] != "status"]
            actual = [rich(row) for row in messages]
            assert len(expected) == len(actual), (source, "message count", len(expected), len(actual))
            for index, (left, right) in enumerate(zip(expected, actual)):
                assert left == right, (source, index, left, right)
        print(f"PASS {source}: {len(messages)} messages, {len(files)} rich file changes" + ("; Python parity" if adapters else ""))


def browser_check(corpus, base):
    from playwright.sync_api import expect, sync_playwright

    with sync_playwright() as playwright:
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(base, wait_until="networkidle")
            for source in ("claude", "codex", "grok"):
                page.locator(f'#side .item[data-uid="{uid(corpus, source)}"]').click()
                expect(page.locator("#msgs")).to_contain_text("Synthetic tools complete")
                for toggle in page.locator('#msgs .turn-process.folded .fold-toggle').all():
                    toggle.click()
                expect(page.locator("#msgs")).to_contain_text("z=first a=False r=4 b=last")
                expect(page.locator("#msgs .file-change-card").first).to_be_visible()
                expect(page.locator("#msgs")).to_contain_text("old.rs → moved.rs")
                card = page.locator("#msgs .file-change-card").filter(has_text="edited.rs").first
                expect(card).to_contain_text("片段")
                expect(card).to_contain_text("new value")
                card.locator('[data-diff-view="split"]').click()
                expect(card.locator('[data-diff-view="split"]')).to_have_attribute("aria-pressed", "true")
                expect(page.locator("#a-term")).to_be_visible()
                expect(page.locator("#a-term")).to_be_enabled()
                expect(page.locator("#migration-read-error")).to_have_count(0)
                print(f"PASS Chromium {source}: existing legacy diff cards and split view")
            page.set_viewport_size({"width": 390, "height": 844})
            # Crossing the breakpoint intentionally opens the mobile list;
            # enter detail using its normal visible session navigation.
            page.locator(f'#side .item[data-uid="{uid(corpus, "grok")}"]').click()
            for toggle in page.locator('#msgs .turn-process.folded .fold-toggle').all():
                toggle.click()
            expect(page.locator("#msgs .file-change-card").first).to_be_visible()
            expect(page.locator("#a-term")).to_be_visible()
            assert not errors, errors
            context.close()
        finally:
            browser.close()


def oversize_browser(root, binary):
    """A separate fixture checks an intentional bounded-display delta from Python."""
    from playwright.sync_api import expect, sync_playwright

    root.mkdir()
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    corpus = Corpus(root)
    content = "OVERSIZE START " + "x" * (512 * 1024) + " OVERSIZE END"
    corpus.put("codex-tools", "codex", [
        codex_row("session_meta", {"id":"codex-tools", "cwd":"/synthetic/oversize"}),
        codex_row("response_item", {"type":"message", "role":"user", "turn_id":"oversize", "content":"Synthetic oversize"}),
        codex_row("response_item", {"type":"function_call", "name":"Write", "call_id":"oversize", "turn_id":"oversize",
                                    "arguments":{"file_path":"oversize.txt", "content":content}}),
        codex_row("response_item", {"type":"message", "role":"assistant", "phase":"final_answer", "turn_id":"oversize", "content":"Oversize fixture complete"}),
    ], [])
    with isolated_server(corpus, binary) as (base, opener), sync_playwright() as playwright:
        messages = api(opener, base, uid(corpus, "codex"))["messages"]
        tool = next(row for row in messages if row["role"] == "tool")
        assert "512 KiB" in tool["changes_unavailable_reason"]
        assert tool["changes"] is None and json.loads(tool["text"])["content"] == content
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            context = browser.new_context(viewport={"width":1280, "height":900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(base, wait_until="networkidle")
            page.locator(f'#side .item[data-uid="{uid(corpus, "codex")}"]').click()
            expect(page.locator("#msgs")).to_contain_text("Oversize fixture complete")
            for toggle in page.locator('#msgs .turn-process.folded .fold-toggle').all():
                toggle.click()
            # A tool group also has its own ordinary disclosure button.
            for toggle in page.locator('#msgs .grp.folded .fold-toggle').all():
                toggle.click()
            expect(page.locator("#msgs .tool-change-warning")).to_be_visible()
            expect(page.locator("#msgs .tool-change-warning")).to_contain_text("512 KiB")
            entry = page.locator("#msgs .tool-entry").filter(has=page.locator(".tool-change-warning"))
            entry.locator(".tool-toggle").click()
            expect(entry.locator(".tool-args")).to_be_visible()
            assert json.loads(entry.locator(".tool-args").text_content())["content"] == content
            page.set_viewport_size({"width":390, "height":844})
            page.locator(f'#side .item[data-uid="{uid(corpus, "codex")}"]').click()
            for toggle in page.locator('#msgs .turn-process.folded .fold-toggle').all():
                toggle.click()
            for toggle in page.locator('#msgs .grp.folded .fold-toggle').all():
                toggle.click()
            expect(page.locator("#msgs .tool-change-warning")).to_be_visible()
            assert not errors, errors
            context.close()
            print("PASS Chromium oversize: visible budget warning and exact complete raw arguments; desktop and mobile")
        finally:
            browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python-source", type=Path)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--browser", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-tool-parity-") as temporary:
        corpus = build_corpus(Path(temporary))
        with isolated_server(corpus, args.binary) as (base, opener):
            verify(corpus, base, opener, args.python_source)
            if args.browser:
                browser_check(corpus, base)
        if args.browser:
            oversize_browser(Path(temporary) / "oversize-fixture", args.binary)


if __name__ == "__main__":
    main()
