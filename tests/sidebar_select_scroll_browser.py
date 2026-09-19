#!/usr/bin/env python3
"""Clicking the last sidebar row must not jump the list.

Isolated Rust server, synthetic Claude rows, Playwright Chromium. The reported
path is date view with nesting on: scroll to the last conversation, click it,
and the scrollbar used to jump up a few rows even though membership was
unchanged — `openSession` rebuilt `#side` and dropped `scrollTop`.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import sync_playwright

from history_parity import BINARY, Corpus, claude_row, isolated_server

COUNT = 24
DAY = "2026-04"


def corpus(root: Path) -> Corpus:
    data = Corpus(root)
    (root / "claude").mkdir(parents=True)
    (root / "codex").mkdir(parents=True)
    (root / "grok").mkdir(parents=True)
    for index in range(COUNT):
        sid = f"scroll-{index:02d}"
        day = 11 + (index // 3)
        hour = 10 + (index % 3)
        stamp = f"{DAY}-{day:02d}T{hour:02d}:00:00Z"
        title = f"Session {index:02d}"
        rows = [
            claude_row(sid, "user", "u0", None, title, cwd="/proj/scroll", timestamp=stamp),
            claude_row(sid, "assistant", "a0", "u0", f"reply {title}", cwd="/proj/scroll", timestamp=stamp),
        ]
        data.put(sid, "claude", rows, [title, f"reply {title}"])
    return data


def last_row_state(page):
    return page.evaluate("""() => {
      const side = document.querySelector('#side');
      const rows = [...side.querySelectorAll('.item[data-uid]')];
      const last = rows.at(-1);
      const box = last.getBoundingClientRect();
      return {
        count: rows.length,
        uid: last.dataset.uid,
        title: last.querySelector('.t')?.textContent,
        sel: last.classList.contains('sel'),
        y: Math.round(box.y),
        bottom: Math.round(box.bottom),
        visible: box.bottom > 0 && box.top < innerHeight,
        scrollTop: side.scrollTop,
        scrollMax: side.scrollHeight - side.clientHeight,
      };
    }""")


def run(page):
    page.wait_for_function("S.sessions.length === %d && T.listLoaded" % COUNT)
    page.evaluate("""() => {
      S.view = 'date';
      S.nest = true;
      store.set('view', 'date');
      store.set('nest', true);
      renderView();
      renderSide();
    }""")
    page.wait_for_function("document.querySelectorAll('#side .item[data-uid]').length === %d" % COUNT)
    uids = page.evaluate("() => [...document.querySelectorAll('#side .item[data-uid]')].map(n => n.dataset.uid)")
    first, last = uids[0], uids[-1]
    assert first != last, uids
    page.evaluate("uid => openSession(uid)", first)
    page.wait_for_function("uid => S.sel === uid", arg=first)
    page.wait_for_selector(".dhead h2")
    page.evaluate("""() => {
      const side = document.querySelector('#side');
      side.scrollTop = side.scrollHeight;
    }""")
    page.wait_for_function("""uid => {
      const last = [...document.querySelectorAll('#side .item[data-uid]')].at(-1);
      const box = last?.getBoundingClientRect();
      return last?.dataset.uid === uid && box && box.top < innerHeight && box.bottom > 0;
    }""", arg=last)
    page.evaluate("""() => {
      window.__sidebarRenders = 0;
      const inner = renderSide;
      renderSide = function() {
        window.__sidebarRenders += 1;
        return inner.apply(this, arguments);
      };
    }""")
    before = last_row_state(page)
    assert before["uid"] == last, before
    assert before["scrollTop"] > 0, before
    assert before["visible"], before
    count = page.evaluate("sidebarSessions().length")
    title = before["title"]
    page.locator(f'#side .item[data-uid="{last}"] .t').click()
    page.wait_for_function("uid => S.sel === uid", arg=last)
    page.wait_for_function("title => document.querySelector('.dhead h2')?.textContent.includes(title)",
                           arg=title)
    after = last_row_state(page)
    assert after["uid"] == last, after
    assert after["sel"] is True, after
    assert after["count"] == before["count"] == COUNT, (before, after)
    assert page.evaluate("sidebarSessions().length") == count
    assert page.evaluate("window.__sidebarRenders") == 0, page.evaluate("window.__sidebarRenders")
    assert abs(after["scrollTop"] - before["scrollTop"]) <= 1, (before, after)
    assert abs(after["y"] - before["y"]) <= 2, (before, after)
    assert after["visible"], after


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-sidebar-scroll-") as directory:
        data = corpus(Path(directory))
        with isolated_server(data, args.binary) as (base, _opener), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 520},
                                              service_workers="block")
                page = context.new_page()
                page.goto(base, wait_until="networkidle")
                run(page)
                context.close()
            finally:
                browser.close()
    print("PASS sidebar select scroll browser: last date/nest row click keeps scroll, membership and selection",
          flush=True)


if __name__ == "__main__":
    main()
