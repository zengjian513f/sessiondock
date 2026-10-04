#!/usr/bin/env python3
"""Tool group folding through real Chromium clicks and native Codex appends.

Only temporary synthetic rollout files and an isolated Rust server are used.
The same user paths cover the legacy and Vue entries; runtime queries only
observe accepted checkpoints/activity, never create renderer state.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import tempfile

from playwright.sync_api import expect, sync_playwright

from browser_runtime import js
from history_parity import BINARY, Corpus, codex_row, encoded, isolated_server
from media_browser import PNG

GROUP = "#msgs > .grp"
FOLDED = re.compile(r"(?:^|\s)folded(?:\s|$)")
ACCEPTED = js(
    """([uid, end, activity]) => {
      const entry = cache.get(viewKey(uid, null));
      return S.sel === uid && !S.agent && entry?.end === end
        && (!activity || entry.activity?.state === activity);
    }""",
    """([uid, end, activity]) => {
      const entry = runtime.core.cache.cache.get(runtime.viewKey(uid, null));
      return runtime.core.state.selection.sel === uid
        && !runtime.core.state.selection.agent && entry?.end === end
        && (!activity || entry.activity?.state === activity);
    }""",
)
WATCHING = js(
    "_es && _es.readyState === EventSource.OPEN",
    "runtime.core.sync.watching && runtime.core.sync.watching.readyState === EventSource.OPEN",
)


def record(sid, kind, payload):
    row = codex_row(kind, {**payload, "turn_id": sid})
    row["timestamp"] = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    return row


def activity(sid, state):
    return record(sid, "event_msg", {"type": "task_" + state})


def message(sid, role, text, phase=None):
    payload = {"type": "message", "role": role, "content": [{
        "type": "input_text" if role == "user" else "output_text", "text": text,
    }]}
    if phase:
        payload["phase"] = phase
    return record(sid, "response_item", payload)


def tool(sid, number):
    return record(sid, "response_item", {
        "type": "function_call", "name": "exec_command", "call_id": f"{sid}-{number}",
        "arguments": json.dumps({"cmd": f"echo TOOL {number}"}),
    })


def result(sid, number, text, *, images=0):
    output = text if not images else [
        {"type": "text", "text": text},
        *[{"type": "image", "mime_type": "image/png", "data": PNG} for _ in range(images)],
    ]
    return record(sid, "response_item", {
        "type": "function_call_output", "call_id": f"{sid}-{number}", "output": output,
    })


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    # One visual tool group needs no outer completed-turn process wrapper.
    # task_started/task_complete drive real activity instead of an openTail hook.
    for sid, numbers, complete in (
        ("codex-stream", (1,), False),
        ("codex-final", (21, 22), False),
        ("codex-idle", (4, 5), False),
        ("codex-manual", (6, 7), True),
        ("codex-media", (11, 12), True),
    ):
        # Keep the native 4 KiB checkpoint prefix stable: these assertions
        # exercise hot suffix appends, not a cold short-file reset/re-render.
        rows = [codex_row("session_meta", {"id": sid, "cwd": "/synthetic/tool-fold",
                                         "fixture_padding": "x" * 5000}),
                message(sid, "user", f"Synthetic tool fold {sid}"), activity(sid, "started"),
                *[tool(sid, number) for number in numbers]]
        if complete:
            rows.append(activity(sid, "complete"))
        corpus.put(sid, "codex", rows, [])
    return corpus


def append_native(corpus, expected, sid, *rows):
    path = corpus.paths[sid]
    assert path.resolve().is_relative_to(corpus.root.resolve()), path
    assert path.read_bytes() == expected[sid], f"unexpected native fixture write: {sid}"
    suffix = b"".join(encoded(row) for row in rows)
    with path.open("ab") as stream:
        stream.write(suffix)
    expected[sid] += suffix
    return len(expected[sid])


def wait_accepted(page, corpus, expected, sid, state):
    page.wait_for_function(ACCEPTED, arg=[corpus.uid(sid), len(expected[sid]), state], timeout=15000)


def select(page, corpus, expected, sid, state):
    page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]:not(.agent)').click()
    expect(page.locator('#msgs [data-role="user"]')).to_contain_text(f"Synthetic tool fold {sid}")
    wait_accepted(page, corpus, expected, sid, state)
    page.wait_for_function(WATCHING, timeout=15000)


def group_state(page, count, *, folded):
    group = page.locator(GROUP)
    expect(group).to_have_count(1)
    expect(group.locator(".group-count")).to_contain_text(f"×{count}")
    if folded:
        expect(group).to_have_class(FOLDED)
    else:
        expect(group).not_to_have_class(FOLDED)
        expect(group.locator(":scope > .tool-entry")).to_have_count(count)
    expect(group.locator(".fold-toggle")).to_have_attribute("aria-expanded", str(not folded).lower())
    return group


def same_group(node):
    # Retaining a Playwright handle does not write any product/DOM business state.
    assert node.evaluate("node => node.isConnected && document.querySelector('#msgs > .grp') === node"), \
        "native append replaced the user-opened tool group"


def verify(page, corpus, expected, requests):
    sid = "codex-stream"
    select(page, corpus, expected, sid, "working")
    expect(page.locator(GROUP)).to_have_count(0)
    expect(page.locator('#msgs > .tool-msg[data-role="tool"]')).to_have_count(1)
    expect(page.locator('#msgs > .tool-msg .tool-head')).to_contain_text("TOOL 1")
    for number in (2, 3):
        append_native(corpus, expected, sid, tool(sid, number))
        wait_accepted(page, corpus, expected, sid, "working")
        group_state(page, number, folded=False)
        expect(page.locator('#msgs > .tool-msg[data-role="tool"]')).to_have_count(0)
    append_native(corpus, expected, sid,
                  message(sid, "assistant", "NON TOOL PROGRESS", "commentary"))
    wait_accepted(page, corpus, expected, sid, "working")
    group = group_state(page, 3, folded=True)
    expect(page.locator('#msgs > [data-role="assistant"]')).to_contain_text("NON TOOL PROGRESS")
    assert group.evaluate("node => node.nextElementSibling?.dataset.role === 'assistant'"), \
        "non-tool progress did not follow/seal the streamed group"

    sid = "codex-final"
    select(page, corpus, expected, sid, "working")
    group_state(page, 2, folded=False)
    # A native final_answer seals even before the separate idle marker arrives.
    append_native(corpus, expected, sid,
                  message(sid, "assistant", "FINAL TOOL CONCLUSION", "final_answer"))
    wait_accepted(page, corpus, expected, sid, "working")
    group_state(page, 2, folded=True)
    expect(page.locator('#msgs > [data-role="assistant"]')).to_contain_text("FINAL TOOL CONCLUSION")
    append_native(corpus, expected, sid, activity(sid, "complete"))
    wait_accepted(page, corpus, expected, sid, "idle")
    group_state(page, 2, folded=True)

    sid = "codex-idle"
    select(page, corpus, expected, sid, "working")
    group_state(page, 2, folded=False)
    # Activity-only native completion exercises sealing without any new message.
    append_native(corpus, expected, sid, activity(sid, "complete"))
    wait_accepted(page, corpus, expected, sid, "idle")
    group_state(page, 2, folded=True)
    expect(page.locator('#msgs [data-role="assistant"]')).to_have_count(0)

    sid = "codex-manual"
    select(page, corpus, expected, sid, "idle")
    group = group_state(page, 2, folded=True)
    expect(group.locator(":scope > .tool-entry")).to_have_count(0)
    node = group.element_handle()
    try:
        group.locator(".fold-toggle").click()
        group_state(page, 2, folded=False)
        same_group(node)
        append_native(corpus, expected, sid, tool(sid, 8))
        wait_accepted(page, corpus, expected, sid, "idle")
        group_state(page, 3, folded=False)
        same_group(node)
        append_native(corpus, expected, sid, result(sid, 7, "PAIRED RESULT SEVEN"))
        wait_accepted(page, corpus, expected, sid, "idle")
        group_state(page, 3, folded=False)
        same_group(node)
        entries = group.locator(":scope > .tool-entry")
        expect(entries.nth(1).locator(".tool-status")).to_have_count(1)
        expect(group.locator(".tool-status")).to_have_count(1)
        expect(group.locator(".tool-out").filter(has_text="PAIRED RESULT SEVEN")).to_have_count(1)
        expect(group.locator('[data-role="tool_result"]')).to_have_count(0)
        # Activity-only working -> idle must respect the real user's expansion.
        append_native(corpus, expected, sid, activity(sid, "started"))
        wait_accepted(page, corpus, expected, sid, "working")
        group_state(page, 3, folded=False)
        append_native(corpus, expected, sid, activity(sid, "complete"))
        wait_accepted(page, corpus, expected, sid, "idle")
        group_state(page, 3, folded=False)
        same_group(node)
        group.locator(":scope > .disclosure").click()
        group_state(page, 3, folded=True)
        same_group(node)
        # A later paired result stays folded; reopening reveals each result once.
        append_native(corpus, expected, sid, result(sid, 8, "PAIRED RESULT EIGHT"))
        wait_accepted(page, corpus, expected, sid, "idle")
        group_state(page, 3, folded=True)
        same_group(node)
        group.locator(".fold-toggle").click()
        group_state(page, 3, folded=False)
        same_group(node)
        expect(group.locator(".tool-status")).to_have_count(2)
        expect(entries.nth(1).locator(".tool-status")).to_have_count(1)
        expect(entries.nth(2).locator(".tool-status")).to_have_count(1)
        for text in ("PAIRED RESULT SEVEN", "PAIRED RESULT EIGHT"):
            expect(group.locator(".tool-out").filter(has_text=text)).to_have_count(1)
        expect(group.locator('[data-role="tool_result"]')).to_have_count(0)
    finally:
        node.dispose()

    sid = "codex-media"
    select(page, corpus, expected, sid, "idle")
    group = group_state(page, 2, folded=True)
    before_media = len([url for url in requests if "/api/media/" in url])
    append_native(corpus, expected, sid, result(sid, 12, "FOLDED IMAGE RESULT", images=20))
    wait_accepted(page, corpus, expected, sid, "idle")
    group_state(page, 2, folded=True)
    expect(group.locator(".tool-entry, .media-gallery, img, .media-more")).to_have_count(0)
    assert len([url for url in requests if "/api/media/" in url]) == before_media, \
        "folded tool result fetched media before a user click"
    continuation = page.evaluate(js(
        "SessionDockCapabilities.config.media_continuation === true",
        "runtime.capabilities.config.media_continuation === true",
    ))
    group.locator(".fold-toggle").click()
    group_state(page, 2, folded=False)
    expect(group.locator(".tool-status")).to_have_count(1)
    media_entry = group.locator(":scope > .tool-entry").nth(1)
    expect(media_entry.locator(".tool-out")).to_contain_text("FOLDED IMAGE RESULT")
    expect(media_entry.locator(".media-gallery img")).to_have_count(16)
    expect(group.locator(":scope > .tool-entry").nth(0).locator(".media-gallery")).to_have_count(0)
    more = media_entry.locator(".media-more[data-media-cursor]")
    expect(more).to_have_count(1 if continuation else 0)
    if continuation:
        expect(more).to_be_visible()
        expect(more).to_have_text("还有 4 张图片，加载下一批")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-tool-group-fold-") as directory:
        corpus = build(Path(directory))
        expected = {sid: path.read_bytes() for sid, path in corpus.paths.items()}
        state = corpus.root / "state"
        state.mkdir(mode=0o700)
        with isolated_server(corpus, args.binary, state_dir=state) as (base, _opener), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                context.route("**/*", lambda route: route.continue_()
                              if route.request.url.startswith(base + "/") else route.abort())
                page = context.new_page()
                page.set_default_timeout(15000)
                errors, requests = [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("request", lambda request: requests.append(request.url))
                page.goto(base, wait_until="networkidle")
                verify(page, corpus, expected, requests)
                assert not errors, errors
                context.close()
            finally:
                browser.close()
        assert {sid: path.read_bytes() for sid, path in corpus.paths.items()} == expected, \
            "browser/server modified native fixtures beyond the explicit test appends"
    print("PASS tool group fold browser: native single->group across batches; streamed tail open; "
          "non-tool/final/idle sealing; real manual open preserves group DOM and expansion across "
          "tools/results/idle; explicit refold; results pair exactly once; folded media stays "
          "unmaterialized/unrequested until click; native media_more capability", flush=True)


if __name__ == "__main__":
    main()
