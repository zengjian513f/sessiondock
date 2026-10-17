#!/usr/bin/env python3
"""Claude's explicit native rewind retracts interrupted branches (free Chromium)."""

import argparse
import os
from pathlib import Path
import tempfile
from urllib.parse import quote

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY, Corpus, claude_row, encoded, get_json, isolated_server


EMPTY = "00000000-0000-0000-0000-000000000000"
OLD = "RewindDiscarded 审查旧输入：逐一复核已经完成的实验并保留工具过程。"
NEW = "RewindCurrent 重新描述任务：查看上一轮结论，再进行独立复核。"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-explicit-rewind-") as temporary:
        root = Path(temporary)
        for source in ("claude", "codex", "grok"):
            (root / source).mkdir()
        corpus = Corpus(root)
        for sid, prefix in (("rewind-first", False), ("rewind-later", True)):
            rows = [claude_row(sid, "attachment", "hook", None,
                               attachment={"type": "hook_success"})]
            parent = "hook"
            if prefix:
                rows += [claude_row(sid, "user", "base-u", parent, "保留的问题"),
                         claude_row(sid, "assistant", "base-a", "base-u", "保留的回答")]
                parent = "base-a"
            rows += [claude_row(sid, "user", "old-u", parent, OLD),
                     claude_row(sid, "assistant", "old-a", "old-u", "RewindDiscarded 旧的工具进展"),
                     claude_row(sid, "user", "old-int", "old-a", "[Request interrupted by user]")]
            corpus.put(sid, "claude", rows, [])
        with isolated_server(corpus, args.binary) as (base, opener), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                context.route("**/*", lambda route: route.continue_()
                              if route.request.url.startswith(base + "/") else route.abort())
                page = context.new_page()
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base, wait_until="networkidle")

                def append(sid, *rows):
                    with corpus.paths[sid].open("ab") as stream:
                        stream.write(b"".join(encoded(row) for row in rows))

                def select(sid):
                    page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                    expect(page.locator("#msgs")).to_be_visible()
                    page.wait_for_function("_es && _es.readyState === EventSource.OPEN")

                def screenshot(name):
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / (name + ".png")), full_page=True)

                for sid, tip in (("rewind-first", EMPTY), ("rewind-later", "base-a")):
                    select(sid)
                    expect(page.locator("#msgs")).to_contain_text(OLD)
                    parent = "hook" if tip == EMPTY else tip
                    # Normal Esc + sibling input preserves interrupted history.
                    # A routine last-prompt must not imply an explicit rewind.
                    append(sid, claude_row(sid, "user", "new-u", parent, NEW),
                           {"type": "last-prompt", "leafUuid": "new-u", "sessionId": sid})
                    expect(page.locator("#msgs")).to_contain_text(NEW, timeout=10000)
                    expect(page.locator("#msgs")).to_contain_text(OLD)
                    screenshot(sid + "-before")
                    # BUG-20261017-072946-4cbf80: the explicit marker can be
                    # flushed AFTER the replacement input/attachments, before
                    # a routine last-prompt declaring the new active leaf.
                    append(sid, {"type": "last-prompt", "leafUuid": tip, "explicit": True, "sessionId": sid},
                           {"type": "last-prompt", "leafUuid": "new-u", "sessionId": sid})
                    expect(page.locator("#msgs")).not_to_contain_text("RewindDiscarded", timeout=10000)
                    expect(page.locator("#msgs")).to_contain_text(NEW)
                    if tip != EMPTY:
                        expect(page.locator("#msgs")).to_contain_text("保留的回答")
                    data = get_json(opener, base, "/api/messages/" + quote(corpus.uid(sid), safe=""))
                    assert all("RewindDiscarded" not in m.get("text", "") for m in data["messages"]), data
                    hits = get_json(opener, base, "/api/search?q=RewindDiscarded")["results"]
                    assert all(hit["uid"] != corpus.uid(sid) for hit in hits), hits
                    page.reload(wait_until="networkidle")
                    select(sid)
                    expect(page.locator("#msgs")).to_contain_text(NEW)
                    expect(page.locator("#msgs")).not_to_contain_text("RewindDiscarded")
                    for width, theme in ((1280, "light"), (1280, "dark"), (390, "light"), (390, "dark")):
                        page.set_viewport_size({"width": width, "height": 900})
                        page.evaluate("theme => { localStorage.setItem('sessiondock.theme', theme); document.documentElement.dataset.theme = theme; }", theme)
                        if width < 600 and page.locator("#side").is_visible():
                            page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                        expect(page.locator("#msgs")).to_be_visible()
                        expect(page.locator("#msgs")).to_contain_text(NEW)
                        screenshot(f"{sid}-{width}-{theme}")
                    page.set_viewport_size({"width": 1280, "height": 900})

                    # New interrupted siblings AFTER the boundary still render.
                    append(sid, claude_row(sid, "user", "later-u", "new-u", "回滚之后的普通中断输入"),
                           claude_row(sid, "user", "later-int", "later-u", "[Request interrupted by user]"),
                           claude_row(sid, "user", "next-u", "new-u", "回滚之后的新输入"))
                    expect(page.locator("#msgs")).to_contain_text("回滚之后的新输入", timeout=10000)
                    expect(page.locator("#msgs")).to_contain_text("回滚之后的普通中断输入")
                    expect(page.locator("#msgs")).not_to_contain_text("RewindDiscarded")

                # A final explicit empty leaf has no missing-ancestor warning;
                # an explicit nonempty leaf retains its prefix without siblings.
                for sid, tip in (("rewind-first", EMPTY), ("rewind-later", "base-a")):
                    select(sid)
                    append(sid, {"type": "last-prompt", "leafUuid": tip, "explicit": True, "sessionId": sid})
                    expect(page.locator("#msgs")).not_to_contain_text(NEW, timeout=10000)
                    data = get_json(opener, base, "/api/messages/" + quote(corpus.uid(sid), safe=""))
                    assert not any("叶子" in w for w in data["meta"].get("migration_warnings", [])), data
                    if tip == EMPTY:
                        expect(page.locator("#msgs .msg")).to_have_count(0)
                    else:
                        expect(page.locator("#msgs")).to_contain_text("保留的回答")
                    screenshot(sid + "-final")
                assert not errors, errors
                print("PASS claude_explicit_rewind_browser: live reset, reload, search, empty/nonempty leaf, delayed marker and later Esc siblings")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
