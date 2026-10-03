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

from browser_runtime import js
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
    page.wait_for_function(js('S.sessions.length === %d && T.listLoaded' % COUNT, 'runtime.core.state.catalog.sessions.length === %d && runtime.terminal.state.listLoaded' % COUNT))
    page.evaluate(js(r"""() => {
      S.view = 'date';
      S.nest = true;
      store.set('view', 'date');
      store.set('nest', true);
      renderView();
      renderSide();
    }""", r"""() => {
      runtime.core.state.sidebar.view = 'date';
      runtime.core.state.sidebar.nest = true;
      runtime.core.preferences.set('view', 'date');
      runtime.core.preferences.set('nest', true);
      runtime.sidebarView.renderView();
      runtime.sidebarView.renderSide();
    }"""))
    page.wait_for_function("document.querySelectorAll('#side .item[data-uid]').length === %d" % COUNT)
    uids = page.evaluate("() => [...document.querySelectorAll('#side .item[data-uid]')].map(n => n.dataset.uid)")
    first, last = uids[0], uids[-1]
    assert first != last, uids
    page.evaluate(js('uid => openSession(uid)', 'uid => runtime.core.open.openSession(uid)'), first)
    page.wait_for_function(js('uid => S.sel === uid', 'uid => runtime.core.state.selection.sel === uid'), arg=first)
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
    page.evaluate(js(r"""() => {
      window.__sidebarRenders = 0;
      const inner = renderSide;
      renderSide = function() {
        window.__sidebarRenders += 1;
        return inner.apply(this, arguments);
      };
    }""", r"""() => {
      window.__sidebarRenders = 0;
      const inner = runtime.sidebarView.renderSide;
      runtime.sidebarView.renderSide = function() {
        window.__sidebarRenders += 1;
        return inner.apply(this, arguments);
      };
    }"""))
    before = last_row_state(page)
    assert before["uid"] == last, before
    assert before["scrollTop"] > 0, before
    assert before["visible"], before
    count = page.evaluate(js('sidebarSessions().length', 'runtime.core.index.sidebarSessions().length'))
    title = before["title"]
    page.locator(f'#side .item[data-uid="{last}"] .t').click()
    page.wait_for_function(js('uid => S.sel === uid', 'uid => runtime.core.state.selection.sel === uid'), arg=last)
    page.wait_for_function("title => document.querySelector('.dhead h2')?.textContent.includes(title)",
                           arg=title)
    after = last_row_state(page)
    assert after["uid"] == last, after
    assert after["sel"] is True, after
    assert after["count"] == before["count"] == COUNT, (before, after)
    assert page.evaluate(js('sidebarSessions().length', 'runtime.core.index.sidebarSessions().length')) == count
    assert page.evaluate("window.__sidebarRenders") == 0, page.evaluate("window.__sidebarRenders")
    assert abs(after["scrollTop"] - before["scrollTop"]) <= 1, (before, after)
    assert abs(after["y"] - before["y"]) <= 2, (before, after)
    assert after["visible"], after



def check_reconciliation(page):
    # The selected row was changed by the preceding click. Establish its new
    # presentation, then an unchanged refresh must leave every row in place.
    page.evaluate(js(r"""() => {
      renderSide();
      window.__sideRows = [...document.querySelectorAll('#side .item')];
      const side = document.querySelector('#side');
      const observer = new MutationObserver(() => {});
      observer.observe(side, {childList: true, subtree: true});
      renderSide();
      window.__sideMutations = observer.takeRecords().length;
      observer.disconnect();
    }""", r"""async () => {
      runtime.sidebarView.renderSide();
      await Promise.resolve();
      window.__sideRows = [...document.querySelectorAll('#side .item')];
      const side = document.querySelector('#side');
      window.__sideMutations = 0;
      const observer = new MutationObserver(records => { window.__sideMutations += records.length; });
      observer.observe(side, {childList: true, subtree: true});
      runtime.sidebarView.renderSide();
      await Promise.resolve();
      window.__sideMutations += observer.takeRecords().length;
      observer.disconnect();
    }"""))
    assert page.evaluate("__sideMutations") == 0
    assert page.evaluate("__sideRows.every(row => row.isConnected)")
    page.evaluate(js(r"""() => {
      const base = S.sessions[0];
      S.sessions = [...S.sessions, {...base, uid: 'synthetic-extra', sid: 'synthetic-extra', title: 'Added row'}];
      renderSide();
    }""", r"""async () => {
      const base = runtime.core.state.catalog.sessions[0];
      runtime.core.state.catalog.sessions = [...runtime.core.state.catalog.sessions, {...base, uid: 'synthetic-extra', sid: 'synthetic-extra', title: 'Added row'}];
      runtime.sidebarView.renderSide();
      await Promise.resolve();
    }"""))
    assert page.locator('#side .item[data-uid="synthetic-extra"]').count() == 1
    assert page.evaluate("__sideRows.every(row => row.isConnected)")
    page.evaluate(js(r"""() => {
      S.sessions = S.sessions.filter(row => row.uid !== 'synthetic-extra');
      renderSide();
    }""", r"""async () => {
      runtime.core.state.catalog.sessions = runtime.core.state.catalog.sessions.filter(row => row.uid !== 'synthetic-extra');
      runtime.sidebarView.renderSide();
      await Promise.resolve();
    }"""))
    assert page.evaluate("__sideRows.every(row => row.isConnected)")
    group = page.locator('#side > .group').first
    group.locator('.ghead').click()
    assert group.locator('.item').count() == 0
    assert 'closed' in group.get_attribute('class')
    group.locator('.ghead').click()
    assert group.locator('.item').count() > 0
    assert 'closed' not in group.get_attribute('class')

    # A stalled list read is shared; a subsequent explicit refresh wins even
    # if that old transport ignores cancellation and eventually returns data.
    page.evaluate(js(r"""() => {
      window.__nativeFetch = fetch;
      window.__pollCalls = 0;
      window.fetch = (url, options) => {
        if (String(url).includes('api/sessions?sig=')) {
          __pollCalls++;
          return new Promise(resolve => { window.__finishPoll = resolve; });
        }
        return __nativeFetch(url, options);
      };
      window.__pollPromise = pollSessions();
      pollSessions(); pollSessions();
    }""", r"""() => {
      window.__nativeFetch = runtime.core.network.fetch;
      window.__pollCalls = 0;
      runtime.core.network.fetch = (url, options) => {
        if (String(url).includes('api/sessions?sig=')) {
          __pollCalls++;
          return new Promise(resolve => { window.__finishPoll = resolve; });
        }
        return __nativeFetch(url, options);
      };
      window.__pollPromise = runtime.core.list.pollSessions();
      runtime.core.list.pollSessions(); runtime.core.list.pollSessions();
    }"""))
    assert page.evaluate('__pollCalls') == 1
    page.evaluate(js('() => loadSessions(false)', '() => runtime.core.list.loadSessions(false)'))
    page.evaluate(js(r"""async () => {
      __finishPoll(new Response(JSON.stringify({sig: 'stale', sessions: []}),
        {headers: {'Content-Type': 'application/json'}}));
      await __pollPromise;
      window.fetch = __nativeFetch;
    }""", r"""async () => {
      __finishPoll(new Response(JSON.stringify({sig: 'stale', sessions: []}),
        {headers: {'Content-Type': 'application/json'}}));
      await __pollPromise;
      runtime.core.network.fetch = __nativeFetch;
    }"""))
    assert page.evaluate(js('S.sessions.length', 'runtime.core.state.catalog.sessions.length')) == COUNT
    assert page.evaluate(js('S.sig', 'runtime.core.state.catalog.sig')) != 'stale'


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
                check_reconciliation(page)
                context.close()
            finally:
                browser.close()
    print("PASS sidebar select scroll browser: last date/nest row click keeps scroll, membership and selection",
          flush=True)


if __name__ == "__main__":
    main()
