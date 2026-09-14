#!/usr/bin/env python3
"""Subagent menu (Python agent_menu_e2e port): start/end times, end-time ordering, running dots.

Isolated Rust server, synthetic Claude session with three subagents, desktop and 390 px.
``agent_items[].active`` is what the backend reads off the sidecar transcripts (an open last
turn with no stop notice in the owner's file): the worker's turn is open, the others ended. Only the
owner's liveness is page state (``S.live``; a synthetic corpus has no CLI process). The worker
finishing and resuming are edits to its transcript that reach the menu through a list refresh.
"""
from __future__ import annotations

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import sync_playwright

from history_parity import BINARY, Corpus, claude_row, encoded, get_json, isolated_server

SID = "menu-owner"
# Served out of order on purpose; early ends first, late ends last, worker is still running (its last
# turn is open) but its last record is earlier than late's.
AGENTS = [("early", "Early finished", "Explore", (9, 0), (9, 30), "end_turn"),
          ("worker", "Still working", "general-purpose", (9, 20), (9, 40), "tool_use"),
          ("late", "Late finished", "general-purpose", (9, 10), (10, 0), "end_turn")]


def today(hour, minute):
    return datetime.now().astimezone().replace(hour=hour, minute=minute, second=0, microsecond=0).isoformat()


def agent_records(agent, title, start, end, stop, finished=None):
    """A sidecar transcript: question, answer (closed only by end_turn), optionally a closing answer."""
    rows = [claude_row(SID, "user", f"{agent}-u", None, f"{title} question", isSidechain=True, agentId=agent,
                       timestamp=today(*start)),
            claude_row(SID, "assistant", f"{agent}-a", f"{agent}-u", f"{title} answer", isSidechain=True,
                       agentId=agent, timestamp=today(*end))]
    rows[-1]["message"]["stop_reason"] = stop
    if finished:
        rows.append(claude_row(SID, "assistant", f"{agent}-done", f"{agent}-a", f"{title} done", isSidechain=True,
                               agentId=agent, timestamp=today(*finished)))
    return rows


def corpus(root: Path) -> Corpus:
    data = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    data.put(SID, "claude", [
        claude_row(SID, "user", "u0", None, "Menu owner question", timestamp=today(8, 0)),
        claude_row(SID, "assistant", "a0", "u0", "Menu owner answer", timestamp=today(8, 1))],
        ["Menu owner question", "Menu owner answer"])
    agents = data.paths[SID].with_suffix("") / "subagents"
    agents.mkdir(parents=True)
    for agent, title, kind, start, end, stop in AGENTS:
        path = agents / f"agent-{agent}.jsonl"
        path.write_bytes(b"".join(encoded(row) for row in agent_records(agent, title, start, end, stop)))
        path.with_suffix(".meta.json").write_text(json.dumps({"description": title, "agentType": kind}))
    data.paths["agent-worker"] = agents / "agent-worker.jsonl"
    return data


def worker(data, finished=None):
    """Rewrite the worker's transcript: still running, or closed by an end_turn answer at `finished`."""
    _agent, title, _kind, start, end, stop = AGENTS[1]
    data.paths["agent-worker"].write_bytes(b"".join(encoded(row) for row in
                                                   agent_records("worker", title, start, end, stop, finished)))


ROWS_JS = """items => items.map(b => ({
  agent: b.dataset.agent, on: b.classList.contains('on'), running: b.classList.contains('running'),
  dot: b.querySelectorAll('.view-live').length, kind: b.querySelector('.view-kind')?.textContent.trim(),
  span: b.querySelector('.view-span')?.textContent.trim() ?? '', title: b.querySelector('b').textContent}))"""

STICKY_TOOLBAR = """() => {
  // 展开中的回合工具条 sticky 在消息区顶部；真实会话滚到一个回合中间时就是这样。
  const turn = document.createElement('div');
  turn.className = 'msg turn-process';
  turn.innerHTML = '<div class="turn-toolbar"><button class="turn-preview" type="button">🔧 5 · 1 分 21 秒</button></div>'
    + '<div style="height:1200px"></div>';
  const msgs = document.querySelector('#msgs');
  msgs.prepend(turn);
  msgs.scrollTop = 40;
}"""

GEOMETRY_JS = """items => items.map(b => {
  const r = b.getBoundingClientRect(), menu = document.querySelector('#session-view-menu').getBoundingClientRect();
  const inside = el => { if (!el) return true; const e = el.getBoundingClientRect();
    return e.left >= r.left - .5 && e.right <= r.right + .5 && e.top >= r.top - .5 && e.bottom <= r.bottom + .5; };
  const hitAt = (el, left) => { if (!el) return true; const e = el.getBoundingClientRect();
    return b.contains(document.elementFromPoint(left ? e.x + 6 : e.x + e.width / 2, e.y + e.height / 2)); };
  const span = b.querySelector('.view-span'), dot = b.querySelector('.view-live');
  return {agent: b.dataset.agent, span: inside(span), dot: inside(dot),
          fits: r.right <= menu.right + .5 && r.left >= menu.left - .5,
          hit: hitAt(b), kindHit: hitAt(b.querySelector('.view-kind'), true),
          titleHit: hitAt(b.querySelector('b'), true), spanHit: hitAt(span),
          dotVisible: !dot || (dot.offsetWidth > 0 && dot.offsetHeight > 0)};
})"""


def open_menu(page):
    switch = page.locator("#a-view-switch")
    if switch.get_attribute("aria-expanded") == "true":
        switch.click()
        page.wait_for_function('document.querySelector("#session-view-menu").hidden')
    switch.click()
    page.wait_for_function('!document.querySelector("#session-view-menu").hidden')
    return page.locator("#session-view-menu button").evaluate_all(ROWS_JS)


def assert_geometry(page):
    """Spans and dots sit inside their own row, rows fit the menu at 390 px too, and every part is
    actually clickable (not covered by the sticky turn toolbar in the message area)."""
    boxes = page.locator("#session-view-menu button[data-agent]").evaluate_all(GEOMETRY_JS)
    assert all(all(v for k, v in b.items() if k != "agent") for b in boxes), boxes


def reload_rows(page, predicate):
    page.evaluate("loadSessions(true)")   # ?force=1: the index rescans the corpus before answering
    page.wait_for_function(predicate)


def check_page(page, uid, data):
    page.evaluate("uid => openSession(uid)", uid)
    page.locator("#a-view-switch").wait_for()
    page.wait_for_function('document.querySelector("#msgs .msg")')
    page.evaluate(STICKY_TOOLBAR)

    # The worker's turn is open but the owner is not live: no dot and pure end-time order.
    menu = open_menu(page)
    assert [r["agent"] for r in menu] == ["", "late", "worker", "early"], menu
    assert [r["dot"] for r in menu] == [0, 0, 0, 0] and not any(r["running"] for r in menu), menu
    assert menu[0]["on"] and menu[0]["kind"] == "主会话", menu
    assert [r["span"] for r in menu[1:]] == ["今天 09:10 → 10:00", "今天 09:20 → 09:40", "今天 09:00 → 09:30"], menu
    assert [r["kind"] for r in menu[1:]] == ["子代理 · general-purpose", "子代理 · general-purpose",
                                            "子代理 · Explore"], menu
    assert_geometry(page)

    # Owner live and worker active: the running one leads with a dot and no end time; the head is
    # not rebuilt for that.
    probe = page.evaluate('document.querySelector(".dhead").__probe = Math.random()')
    page.evaluate("uid => { S.live.add(uid); paintLive(); }", uid)
    menu = open_menu(page)
    assert page.evaluate('document.querySelector(".dhead").__probe') == probe
    assert [r["agent"] for r in menu] == ["", "worker", "late", "early"], menu
    assert [r["dot"] for r in menu] == [0, 1, 0, 0], menu
    assert [r["running"] for r in menu] == [False, True, False, False], menu
    assert menu[1]["span"] == "今天 09:20 → 运行中", menu
    assert page.locator("#session-view-menu .view-live").get_attribute("title") == "运行中"
    dot_color, side_color = page.evaluate("""() => [
      getComputedStyle(document.querySelector('#session-view-menu .view-live')).backgroundColor,
      getComputedStyle(document.querySelector('#dlive')).backgroundColor]""")
    assert dot_color == side_color, (dot_color, side_color)
    assert page.locator("#dlive").evaluate("e => e.classList.contains('visible')")
    assert_geometry(page)

    # A list refresh reaches the next open: the worker's transcript closes with an end_turn at 10:05.
    worker(data, finished=(10, 5))
    reload_rows(page, "S.sessions[0].agent_items.every(a => !a.active)")
    menu = open_menu(page)
    assert [r["agent"] for r in menu] == ["", "worker", "late", "early"], menu
    assert [r["dot"] for r in menu] == [0, 0, 0, 0], menu
    assert menu[1]["span"] == "今天 09:20 → 10:05", menu
    worker(data)
    reload_rows(page, "S.sessions[0].agent_items.some(a => a.active)")

    # Switching into a subagent view moves the mark; the order stays.
    page.locator('#session-view-menu button[data-agent="late"]').click()
    page.wait_for_function('document.querySelector(".dhead h2")?.textContent.includes("Late finished")')
    page.wait_for_function("uid => _es && _esUid === uid && S.agent === 'late'", arg=uid)
    menu = open_menu(page)
    assert [r["agent"] for r in menu] == ["", "worker", "late", "early"], menu
    assert [r["on"] for r in menu] == [False, False, True, False], menu
    page.locator('#session-view-menu button[data-agent=""]').click()
    page.wait_for_function('!document.querySelector(".dhead h2")?.textContent.includes("Late finished")')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-agent-menu-") as directory:
        data = corpus(Path(directory))
        with isolated_server(data, args.binary) as (base, opener), sync_playwright() as playwright:
            row = next(r for r in get_json(opener, base, "/api/sessions")["sessions"] if r["sid"] == SID)
            # The backend reads `active` off the sidecars: only the worker's last turn is open.
            assert {a["id"]: a["active"] for a in row["agent_items"]} == {
                "early": False, "worker": True, "late": False}, row["agent_items"]
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                for width, height in ((1400, 900), (390, 760)):
                    context = browser.new_context(viewport={"width": width, "height": height},
                                                  service_workers="block",
                                                  color_scheme="dark" if width < 720 else "light")
                    page = context.new_page()
                    errors, failed = [], []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    # A view switch aborts its own in-flight incremental read
                    # (net::ERR_ABORTED is the page's doing, not a server failure).
                    page.on("requestfailed", lambda request: failed.append((request.url, request.failure))
                            if "/api/watch?" not in request.url and "ERR_ABORTED" not in str(request.failure) else None)
                    page.goto(base, wait_until="networkidle")
                    page.wait_for_function("S.sessions.length > 0")
                    check_page(page, row["uid"], data)
                    assert not errors, errors
                    assert not failed, failed
                    context.close()
            finally:
                browser.close()
    print("PASS agent menu browser: end-time order while the owner is not live, running dot/open span from the "
          "backend's `active`, finish/resume via transcript edits, geometry at desktop + 390px", flush=True)


if __name__ == "__main__":
    main()
