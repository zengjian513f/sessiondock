#!/usr/bin/env python3
"""Sidebar nesting: subagents and spawned sessions indent under their spawner.

Isolated Rust server over a synthetic corpus, Playwright Chromium, desktop and 390 px. Every field the
tree reads is published by this backend from the corpus itself: ``spawned_by`` is seeded into
session-metadata.json (the record the process scan writes once), ``agent_items[].active`` is the
sidecar's open turn, and ``continued_in`` is the tail ``continued-in`` record. Mid-test
list changes are made on disk and reach the page through a forced rescan plus the page's own sig
poll. The synthetic corpus has no matching CLI process, so the real live endpoint stays empty.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import sync_playwright

from history_parity import (BINARY, Corpus, claude_row, codex_message, codex_row, encoded, get_json,
                            isolated_server)

DAY = "2026-09-11T"
SPAWNED = {"nest-child-b": {"source": "claude", "sid": "nest-root-a"},
           "nest-grandchild-c": {"source": "grok", "sid": "nest-child-b"},
           "nest-orphan-d": {"source": "claude", "sid": "sid-gone"},
           # A continued session inherits the old process's environment: the scan records the old
           # session as its spawner, and the tree must not nest the continuation under it.
           "nest-new": {"source": "claude", "sid": "nest-old"}}


def stamp(hhmm):
    return f"{DAY}{hhmm}:00Z"


def claude_lines(data, sid, title, cwd, hhmm, tail=(), created=None):
    rows = [claude_row(sid, "user", "u0", None, title, cwd=cwd, timestamp=stamp(created or hhmm)),
            claude_row(sid, "assistant", "a0", "u0", f"reply {title}", cwd=cwd, timestamp=stamp(hhmm)), *tail]
    data.put(sid, "claude", rows, [title, f"reply {title}"])


def grandchild_rows(title):
    return [codex_row("session_meta", {"id": "nest-grandchild-c", "cwd": "/proj/alpha", "timestamp": stamp("09:00")}),
            codex_message("user", title), codex_message("assistant", f"reply {title}")]


def pin_grandchild(data):
    # A Codex row's `updated` is the file mtime: pin it between D (08:30) and A (10:00).
    when = datetime(2026, 9, 11, 9, 0, tzinfo=timezone.utc).timestamp()
    os.utime(data.paths["nest-grandchild-c"], (when, when))


def grok_uid(directory: Path) -> str:
    return "grok:" + hashlib.sha1(str(directory).encode()).hexdigest()[:16]


def corpus(root: Path) -> Corpus:
    data = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    claude_lines(data, "nest-root-a", "Root A", "/proj/alpha", "10:00", created="08:00")
    claude_lines(data, "nest-orphan-d", "Orphan D", "/proj/alpha", "08:30")
    claude_lines(data, "nest-alone-e", "Standalone E", "/proj/gamma", "07:00")
    agents = data.paths["nest-root-a"].with_suffix("") / "subagents"
    agents.mkdir(parents=True)
    # x's last turn is open (no end_turn) and A's transcript carries no stop notice for it: active.
    for agent, title, kind, start, end, stop in (("x", "Agent X still working", "Explore", "09:00", "09:30", "tool_use"),
                                                 ("y", "Agent Y finished", "general-purpose", "10:00", "10:30", "end_turn")):
        answer = claude_row("nest-root-a", "assistant", f"{agent}-a", f"{agent}-u", f"{title} answer",
                            isSidechain=True, agentId=agent, timestamp=stamp(end))
        answer["message"]["stop_reason"] = stop
        path = agents / f"agent-{agent}.jsonl"
        path.write_bytes(b"".join(encoded(row) for row in [
            claude_row("nest-root-a", "user", f"{agent}-u", None, f"{title} question", isSidechain=True,
                       agentId=agent, timestamp=stamp(start)), answer]))
        path.with_suffix(".meta.json").write_text(json.dumps({"description": title, "agentType": kind}))
    data.put("nest-grandchild-c", "codex", grandchild_rows("Grandchild C"), ["Grandchild C", "reply Grandchild C"])
    pin_grandchild(data)
    grok = root / "grok/project-nest/nest-child-b"
    grok.mkdir(parents=True)
    (grok / "chat_history.jsonl").write_text(
        json.dumps({"type": "user", "prompt_index": 1, "content": [{"type": "text", "text": "Child B"}]}) + "\n"
        + json.dumps({"type": "assistant", "content": [{"type": "text", "text": "reply Child B"}]}) + "\n")
    (grok / "summary.json").write_text(json.dumps({
        "info": {"id": "nest-child-b", "cwd": "/proj/beta"}, "generated_title": "Child B",
        "created_at": stamp("08:45"), "last_active_at": stamp("11:00")}))
    # spawned_by lives in the metadata document the process scan writes (metadata_suite seeds it the
    # same way); uids are path hashes, so the continuation written mid-test is seeded before its file exists.
    data.paths["nest-new"] = root / "claude/project-history/nest-new.jsonl"
    uid = {sid: grok_uid(grok) if sid == "nest-child-b" else data.uid(sid) for sid in SPAWNED}
    state = root / "state"
    state.mkdir(mode=0o700)
    state.chmod(0o700)
    document = state / "session-metadata.json"
    document.write_text(json.dumps({"schema_version": 1, "revision": 1, "sessions": {
        uid[sid]: {"spawned_by": parent} for sid, parent in SPAWNED.items()}}) + "\n")
    document.chmod(0o600)
    return data


ROWS_JS = """() => [...document.querySelectorAll('#side .item')].map(n => ({
  key: n.dataset.key, uid: n.dataset.uid || null, agent: n.dataset.agent || null,
  depth: +n.dataset.depth, group: n.closest('.group').dataset.key,
  sel: n.classList.contains('sel'), closed: n.classList.contains('nest-closed'),
  caret: !!n.querySelector('.nest-caret'),
  pad: n.querySelector(':scope > .ico').getBoundingClientRect().left - n.getBoundingClientRect().left,
  caretX: (c => c ? c.getBoundingClientRect().left + c.getBoundingClientRect().width / 2 : null)(n.querySelector('.nest-caret')),
  gheadCaretX: (c => c.getBoundingClientRect().left + c.getBoundingClientRect().width / 2)(n.closest('.group').querySelector('.ghead .caret')),
  dot: !!n.querySelector('.item-status.visible')}))"""
MARKS_JS = """() => [...document.querySelectorAll('#side .item.agent')].map(n =>
  ({mark: !!n.querySelector(':scope > .ico > .agent-mark svg'), icon: n.querySelector(':scope > .ico > .source-icon')?.dataset.source,
    opacity: getComputedStyle(n.querySelector(':scope > .ico > .source-icon')).opacity}))"""


def to_list(page):
    if page.evaluate('document.body.classList.contains("mobile-detail")'):
        page.locator(".dhead .mobile-back").first.click()
        page.wait_for_function('!document.body.classList.contains("mobile-detail")')


def open_item_menu(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').click(button="right")
    page.locator("#item-menu").wait_for(state="visible")


def session_depth(page, uid):
    return page.evaluate(
        "uid => { const n = document.querySelector(`#side .item[data-uid=\"${uid}\"]`); return n ? +n.dataset.depth : null; }",
        uid)


def opened(page, uid, agent=None):
    page.wait_for_function("([uid, agent]) => S.sel === uid && S.agent === agent && _es && _esUid === uid",
                           arg=[uid, agent])


def poll(page, server, predicate):
    """A disk change reaches the list the way a live one does: the index rescans (forced here instead
    of waiting out its 500 ms check TTL) and the page's own sig poll patches or redraws the sidebar."""
    get_json(*server, "/api/sessions?force=1")
    page.evaluate("pollSessions()")
    page.wait_for_function(predicate)


def check_page(page, uid, data, server, width):
    rows = lambda: page.evaluate(ROWS_JS)  # noqa: E731
    A, B, C, D, E = (uid[k] for k in ("a", "b", "c", "d", "e"))
    toggle = page.locator("#nest-toggle")
    assert toggle.is_visible(), "the nest toggle must be visible in the header"
    assert not page.evaluate("S.nest") and toggle.get_attribute("aria-pressed") == "false"

    # Flat: no subagent rows, every row a root, B in its own directory group.
    flat = rows()
    assert all(r["depth"] == 0 and r["agent"] is None and not r["caret"] for r in flat), flat
    assert [r["uid"] for r in flat] == [B, A, C, D, E], flat
    assert len({r["group"] for r in flat}) == 3, flat
    base_pad = flat[0]["pad"]       # icon offset from the row's left edge when flat

    # Nested: B (with C) leaves the beta group for A's subtree; the two
    # transcript agents retain backend order; D (spawner gone) and E stay roots.
    toggle.click()
    assert page.evaluate("S.nest") and toggle.get_attribute("aria-pressed") == "true"
    assert page.evaluate('JSON.parse(localStorage.getItem("sessiondock.nest"))') is True
    tree = rows()
    expect = [(A, None, 0), (B, None, 1), (C, None, 2), (A, "y", 1), (A, "x", 1), (D, None, 0), (E, None, 0)]
    got = [(r["uid"] or r["key"].split("#")[0], r["agent"], r["depth"]) for r in tree]
    assert got == expect, got
    assert len({r["group"] for r in tree}) == 2 and tree[0]["group"] == tree[5]["group"], tree
    assert [r["group"] for r in tree[:5]] == [tree[0]["group"]] * 5, tree
    assert page.evaluate('key => [...document.querySelectorAll(".group")].find(g => g.dataset.key === key)'
                         '.querySelector(".gcount").textContent', tree[0]["group"]) == "4"
    assert [r["caret"] for r in tree] == [True, True, False, False, False, False, False], tree
    assert not tree[3]["dot"] and not tree[4]["dot"], tree
    marks = page.evaluate(MARKS_JS)
    assert marks and all(m["mark"] and m["icon"] == "claude" and m["opacity"] == "1" for m in marks), marks
    # Indent: the icon moves right with depth and lines up per depth; a root's caret shares the group
    # caret's column; a child's caret sits one indent step further right.
    pads = [r["pad"] for r in tree]
    assert pads[0] > base_pad and pads[1] > pads[0] and pads[2] > pads[1] == pads[3] == pads[4], pads
    assert pads[5] == pads[6] == pads[0], pads
    assert all(r["pad"] < width / 3 for r in tree), pads
    assert abs(tree[0]["caretX"] - tree[0]["gheadCaretX"]) < 1, (tree[0]["caretX"], tree[0]["gheadCaretX"])
    assert abs(tree[1]["caretX"] - (tree[0]["caretX"] + (pads[1] - pads[0]))) < 1, (tree[1]["caretX"], tree[0]["caretX"], pads)
    assert not [r for r in tree if r["sel"]]
    # The backend advertises process discovery and this synthetic corpus has no
    # matching processes, so the active count and filtered list are empty.
    assert page.locator("#session-active").text_content() == "0"
    page.locator("#livecount").click()
    assert page.evaluate("S.activeOnly") is True
    assert rows() == []
    page.locator("#allcount").click()
    assert page.evaluate("S.activeOnly") is False and len(rows()) == 7

    # A subagent row opens its view and is the only lit row.
    page.locator('#side .item.agent[data-agent="y"]').click()
    page.wait_for_function('document.querySelector(".dhead h2")?.textContent.includes("Agent Y finished")')
    assert page.evaluate("[S.sel, S.agent]") == [A, "y"]
    opened(page, A, "y")
    assert [r["key"] for r in rows() if r["sel"]] == [A + "#y"]

    # Grandchild C: the title bar carries no spawner link any more; the sidebar leads back to B.
    to_list(page)
    page.evaluate("uid => openSession(uid)", C)
    page.wait_for_function('document.querySelector(".dhead h2")?.textContent.includes("Grandchild C")')
    opened(page, C)
    assert page.locator(".dhead .meta-spawner").count() == 0
    to_list(page)
    page.locator(f'#side .item[data-uid="{B}"]').click()
    page.wait_for_function('document.querySelector(".dhead h2")?.textContent.includes("Child B")')
    assert page.evaluate("[S.sel, S.agent]") == [B, None]
    opened(page, B)

    # Caret: fold A's whole subtree (4 items), persisted across a reload; click again to unfold.
    to_list(page)
    caret = page.locator(f'#side .item[data-uid="{A}"] .nest-caret')
    assert caret.get_attribute("aria-expanded") == "true" and "4 项" in caret.get_attribute("title")
    caret.click()
    folded = rows()
    assert [r["depth"] for r in folded] == [0, 0, 0] and folded[0]["closed"], folded
    assert caret.get_attribute("aria-expanded") == "false"
    assert page.evaluate("[...S.nestClosed]") == [A]
    page.evaluate("store.set('sel', null)")   # no auto-open (and no watch) after the reload
    page.reload()
    page.wait_for_function("S.sessions.length === 5")
    assert page.evaluate("S.nest") and [r["depth"] for r in rows()] == [0, 0, 0], rows()
    page.locator(f'#side .item[data-uid="{A}"] .nest-caret').click()
    assert [r["depth"] for r in rows()] == [0, 1, 2, 1, 1, 0, 0], rows()

    # A list refresh that keeps the tree shape patches nodes in place.
    page.evaluate(f"document.querySelector('#side .item[data-uid=\"{C}\"]').__mark = 1")
    data.put("nest-grandchild-c", "codex", grandchild_rows("Grandchild C renamed"), [])
    pin_grandchild(data)
    poll(page, server, 'S.sessions.some(s => s.title === "Grandchild C renamed")')
    assert page.evaluate(f"document.querySelector('#side .item[data-uid=\"{C}\"]').__mark") == 1
    assert [(r["key"], r["depth"]) for r in rows()] == [
        (A, 0), (B, 1), (C, 2), (A + "#y", 1), (A + "#x", 1), (D, 0), (E, 0)]
    data.put("nest-grandchild-c", "codex", grandchild_rows("Grandchild C"), [])
    pin_grandchild(data)
    poll(page, server, 'S.sessions.some(s => s.title === "Grandchild C")')

    # compact/continue keeps only the newest session of the chain: the old file's row is hidden
    # (its spawned_by-recorded continuation is not its child), opening the old uid follows to the new one.
    claude_lines(data, "nest-old", "Old continued", "/proj/alpha", "12:00")
    old_uid = data.uid("nest-old")
    poll(page, server, 'S.sessions.length === 6')
    to_list(page)
    open_item_menu(page, E)
    page.locator('#item-menu [data-act="attach"]').click()
    page.wait_for_function("S.nestAttach")
    page.locator(f'#side .item[data-uid="{old_uid}"]').click()
    page.wait_for_function("uid => S.sessions.find(s => s.uid === uid).nest_parent?.sid === 'nest-old'", arg=E)
    claude_lines(data, "nest-old", "Old continued", "/proj/alpha", "12:00",
                 tail=[{"type": "continued-in", "continuedInSessionId": "nest-new", "sessionId": "nest-old"}])
    claude_lines(data, "nest-new", "New continued", "/proj/alpha", "12:30")
    new_uid, old_uid = uid["new"], data.uid("nest-old")
    poll(page, server, 'S.sessions.length === 7 && S.sessions.some(s => s.continued_in)')
    assert page.evaluate("uid => S.sessions.find(s => s.uid === uid).continued_in", old_uid) == new_uid
    assert page.evaluate("uid => S.sessions.find(s => s.uid === uid).spawned_by", new_uid) == SPAWNED["nest-new"]
    continued = rows()
    listed = [r["uid"] for r in continued if not r["agent"]]
    assert new_uid in listed and old_uid not in listed, listed
    assert next(r["depth"] for r in continued if r["uid"] == new_uid) == 0
    assert session_depth(page, E) == 1
    assert page.evaluate("([p, w]) => nestDescendantUids(p).has(w)", [new_uid, E])
    to_list(page)
    page.evaluate("uid => openSession(uid)", old_uid)
    page.wait_for_function("uid => S.sel === uid", arg=new_uid)
    page.wait_for_function('document.querySelector(".dhead h2")?.textContent.includes("New continued")')
    opened(page, new_uid)
    assert [r["uid"] for r in rows() if r["sel"]] == [new_uid]
    to_list(page)
    open_item_menu(page, E)
    page.locator('#item-menu [data-act="detach"]').click()
    page.wait_for_function("uid => S.sessions.find(s => s.uid === uid).nest_independent", arg=E)
    for sid in ("nest-old", "nest-new"):
        data.paths.pop(sid).unlink()
    poll(page, server, "S.sessions.length === 5")
    to_list(page)

    # Right-click: detach B from A, restore spawned_by nesting, attach E under A, cancel attach.
    open_item_menu(page, B)
    page.locator('#item-menu [data-act="detach"]').click()
    page.wait_for_function("uid => { const n = document.querySelector(`#side .item[data-uid=\"${uid}\"]`); return n && +n.dataset.depth === 0; }", arg=B)
    assert session_depth(page, C) == 1
    open_item_menu(page, B)
    page.locator('#item-menu [data-act="reattach"]').click()
    page.wait_for_function("uid => { const n = document.querySelector(`#side .item[data-uid=\"${uid}\"]`); return n && +n.dataset.depth === 1; }", arg=B)
    assert session_depth(page, C) == 2
    open_item_menu(page, E)
    page.locator('#item-menu [data-act="attach"]').click()
    page.wait_for_function("S.nestAttach && !document.querySelector('#side-tools').hidden")
    page.locator("#side-pick-cancel").click()
    page.wait_for_function("!S.nestAttach")
    assert session_depth(page, E) == 0
    open_item_menu(page, E)
    page.locator('#item-menu [data-act="attach"]').click()
    page.wait_for_function("S.nestAttach")
    page.locator(f'#side .item[data-uid="{A}"]').click()
    page.wait_for_function("uid => { const n = document.querySelector(`#side .item[data-uid=\"${uid}\"]`); return n && +n.dataset.depth === 1; }", arg=E)
    open_item_menu(page, E)
    page.locator('#item-menu [data-act="detach"]').click()
    page.wait_for_function("uid => { const n = document.querySelector(`#side .item[data-uid=\"${uid}\"]`); return n && +n.dataset.depth === 0; }", arg=E)

    # Off again: the flat list as before, no subagent rows.
    page.locator("#nest-toggle").click()
    back = rows()
    assert [r["uid"] for r in back] == [r["uid"] for r in flat] and all(r["depth"] == 0 for r in back)
    assert not page.locator("#side .item.agent").count()


def check_hidden_spawner(browser, binary, root, width):
    """BUG-20260922-081423-93ca71: a rewind hides the recorded spawner."""
    data = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)

    def put(sid, parent=None):
        meta = {"id": sid, "cwd": "/proj/nest-rewind", "timestamp": stamp("12:00")}
        if parent:
            meta.update(forked_from_id=parent, history_base={
                "thread_id": parent, "end_byte_offset": data.paths[parent].stat().st_size})
        data.put(sid, "codex", [codex_row("session_meta", meta),
                                codex_message("user", sid), codex_message("assistant", "reply " + sid)], [])
        return data.uid(sid)

    parent, worker = put("spawner"), put("worker")
    state = root / "state"
    state.mkdir(mode=0o700)
    document = state / "session-metadata.json"
    relation = {"source": "codex", "sid": "spawner"}
    document.write_text(json.dumps({"schema_version": 1, "revision": 1,
                                   "sessions": {worker: {"spawned_by": relation}}}))
    document.chmod(0o600)
    with isolated_server(data, binary, state_dir=state) as (base, opener):
        context = browser.new_context(viewport={"width": width, "height": 900}, service_workers="block")
        page = context.new_page()
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.goto(base, wait_until="networkidle")
        page.wait_for_function("S.sessions.length === 2")
        page.locator("#nest-toggle").click()
        assert session_depth(page, worker) == 1
        # Real list refresh after new native files appear; no invented browser row fields.
        fork = put("rewind-one", "spawner")
        poll(page, (opener, base), "S.sessions.length === 3")
        assert session_depth(page, parent) is None
        assert session_depth(page, worker) == 1, "worker escaped when its spawner became hidden"
        leaf = put("rewind-two", "rewind-one")
        poll(page, (opener, base), "S.sessions.length === 4")
        assert session_depth(page, fork) is None
        assert session_depth(page, worker) == 1
        assert page.evaluate("uid => nestDescendantUids(uid).has(" + json.dumps(worker) + ")", leaf)
        caret = page.locator(f'#side .item[data-uid="{leaf}"] .nest-caret')
        caret.click()
        assert session_depth(page, worker) is None
        caret.click()
        page.locator(f'#side .item[data-uid="{worker}"]').click()
        opened(page, worker)
        page.wait_for_function('document.querySelector("#msgs")?.textContent.includes("reply worker")')
        to_list(page)
        open_item_menu(page, worker)
        page.locator('#item-menu [data-act="detach"]').click()
        page.wait_for_function("uid => S.sessions.find(s => s.uid === uid).nest_independent", arg=worker)
        assert session_depth(page, worker) == 0
        open_item_menu(page, worker)
        page.locator('#item-menu [data-act="reattach"]').click()
        page.wait_for_function("uid => !S.sessions.find(s => s.uid === uid).nest_independent", arg=worker)
        assert session_depth(page, worker) == 1
        # Showing the historical parent restores its own subtree, without rewriting spawn metadata.
        page.locator(f'#side .item[data-uid="{leaf}"]').click()
        opened(page, leaf)
        page.locator("#a-fork-chain").click()
        page.locator(f'#fork-chain-menu .chain-row[data-uid="{parent}"] .chain-toggle').click()
        page.wait_for_function("uid => S.sessions.find(s => s.uid === uid).fork_parent_visible", arg=parent)
        to_list(page)
        assert page.evaluate("([p, w]) => nestDescendantUids(p).has(w)", [parent, worker])
        assert not page.evaluate("([p, w]) => nestDescendantUids(p).has(w)", [leaf, worker])
        # Filtering an ordinary visible parent must still leave a root; node/source identity is scoped.
        assert page.evaluate("uid => nestEdges(sidebarSessions().filter(s => s.uid !== uid)).nested.size", parent) == 0
        assert json.loads(document.read_text())["sessions"][worker] == {"spawned_by": relation}
        assert not errors, errors
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-nest-tree-") as directory:
        root = Path(directory)
        data = corpus(root)
        with isolated_server(data, args.binary, state_dir=root / "state") as (base, opener), \
                sync_playwright() as playwright:
            server = (opener, base)
            listed = {row["sid"]: row for row in get_json(opener, base, "/api/sessions")["sessions"]}
            uid = {"a": listed["nest-root-a"]["uid"], "b": listed["nest-child-b"]["uid"],
                   "c": listed["nest-grandchild-c"]["uid"], "d": listed["nest-orphan-d"]["uid"],
                   "e": listed["nest-alone-e"]["uid"], "new": data.uid("nest-new")}
            # The backend publishes the fields the tree is built from.
            assert {sid: row.get("spawned_by") for sid, row in listed.items() if row.get("spawned_by")} == {
                sid: parent for sid, parent in SPAWNED.items() if sid != "nest-new"}, listed
            assert {a["id"]: a["active"] for a in listed["nest-root-a"]["agent_items"]} == {"x": True, "y": False}, listed
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
                    # Watch streams and stale message deltas are aborted by the
                    # page itself when selection or navigation changes.
                    page.on("requestfailed", lambda request: failed.append((request.url, request.failure))
                            if "/api/watch?" not in request.url and request.failure != "net::ERR_ABORTED" else None)
                    page.goto(base, wait_until="networkidle")
                    page.wait_for_function("S.sessions.length === 5")
                    check_page(page, uid, data, server, width)
                    assert not errors, errors
                    assert not failed, failed
                    context.close()
                    check_hidden_spawner(browser, args.binary, root / f"rewind-{width}", width)
            finally:
                browser.close()
    print("PASS nest tree browser: tree/order/carets/marks from the backend's spawned_by, active and continued_in, "
          "hidden continued-in parent, hidden rewind spawner, in-place refresh, detach/restore/attach, desktop + 390px", flush=True)


if __name__ == "__main__":
    main()
