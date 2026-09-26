#!/usr/bin/env python3
"""Header and title-bar folding order over N widths (local mode).

Isolated Rust server, synthetic Claude session with a branch (an API field only), no CLI. Widths go from 1698 down to
320 (608 and both sides of every breakpoint included); each width checks two invariants and the
whole sweep checks the fold order:
  header chrome, in this order as the page gets narrower: Agent/view/nest labels, then the
      hostname title, then machine chips into the picker (seeded here; hub does the same),
      then right-side buttons. Buttons that fold are a suffix of the priority list (settings ->
      report -> trash -> page refresh [-> new when terminal_create]); the menu keeps the inline order;
      the filter bar is never squeezed unless everything folded; once folded, one more button
      would not fit; with nothing folded the ... button takes no room; within a tier narrower
      never unfolds chrome or buttons; all three tiers share one header height (no jump across
      1200px / 720px).
  title bar: inline items are a prefix of "actions (star -> fold turns -> report -> stop/delete)
      -> metadata (count, size, time, dir, source, [model,] sid)"; the rest goes into ...
      from the tail (metadata before buttons); an empty menu hides ...; a narrow title never
      yields; within a tier narrower never shows more.
Dragging the divider only changes the detail width; the title bar folds in the same order.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import sync_playwright

from history_parity import BINARY, Corpus, claude_row, isolated_server

SID = "fold-sweep"
HEADER_PRIORITY = ["new-session", "page-reload", "trash", "report-bug", "settings"]
ACTION_ORDER = ["a-star", "a-turns", "report-bug", "a-session-action"]
# The branch is an API field the title bar no longer shows.
META_PRIORITY = ["mcount-total", "size", "time", "meta-node", "cwd", "meta-source", "model", "session-id"]
SLACK = 24   # app.js HEAD_BRIEF_SLACK: room kept for the message count growing wider

HEADER_FOLD_JS = """() => {
  const header = document.querySelector('header'), filters = header.querySelector('.header-filters');
  const actions = header.querySelector('.header-actions');
  const shown = el => {
    if (!el) return false;
    const s = getComputedStyle(el);
    return s.display !== 'none' && s.visibility !== 'hidden' && el.offsetWidth > 0;
  };
  const ids = nodes => [...nodes].filter(b => !b.hidden && !b.classList.contains('hidden'))
    .map(b => b.id).filter(id => id && id !== 'header-more-btn');
  const picker = header.querySelector('#node-picker');
  const nodesPresent = !!(picker && !picker.hidden);
  return {inline: ids(actions.querySelectorAll(':scope > .btn')),
    menu: ids(document.querySelectorAll('#header-menu > .btn')),
    more: !document.querySelector('#header-more').hidden,
    squeezed: filters.scrollWidth > filters.clientWidth || header.scrollWidth > header.clientWidth,
    free: actions.getBoundingClientRect().left - filters.getBoundingClientRect().right
      - parseFloat(getComputedStyle(header).columnGap),
    unit: document.querySelector('#header-more-btn').getBoundingClientRect().width
      + parseFloat(getComputedStyle(actions).columnGap),
    height: header.getBoundingClientRect().height,
    overflow: document.documentElement.scrollWidth > innerWidth,
    labels: shown(header.querySelector('#chips .chip span'))
      || shown(header.querySelector('#view .mobile-label'))
      || shown(header.querySelector('#nest .mobile-label')),
    brand: shown(header.querySelector('.brand-name')),
    nodes_present: nodesPresent,
    nodes_menu: nodesPresent && shown(header.querySelector('#node-pick')),
    fold_labels: header.classList.contains('header-fold-labels'),
    fold_brand: header.classList.contains('header-fold-brand'),
    fold_nodes: header.classList.contains('header-fold-nodes')};
}"""
META_KEY = """e => e.id === 'mcount-total' ? e.id
  : e.classList.contains('session-id') ? 'session-id'
  : e.classList.contains('meta-node') ? 'meta-node'
  : e.classList.contains('meta-source') ? 'meta-source'
  : e.querySelector('code') ? 'cwd' : e.textContent.includes('→') ? 'time'
  : /^[0-9.]+[BKM]$/.test(e.textContent.trim()) ? 'size' : 'model' """
HEAD_STATE_JS = f"""() => {{
  const id = b => b.id || (b.hasAttribute('data-report-bug') ? 'report-bug' : '');
  const key = {META_KEY};
  const h2 = document.querySelector('.dhead h2'), brief = document.querySelector('.dbrief');
  const actions = document.querySelector('.dhead-actions'), more = document.querySelector('#a-more');
  const last = brief && !brief.hidden ? brief : h2;
  const text = h2.querySelector('.session-view-switch > span, :scope > span');
  return {{
    globals: [...actions.querySelectorAll('[id^="a-global-"]')]
      .sort((a, b) => a.dataset.order - b.dataset.order).map(id),
    inline: [...actions.querySelectorAll('button')].filter(b => !b.hidden && b.offsetWidth).map(id),
    menu_actions: [...document.querySelectorAll('#session-actions-menu [role="menu"] > *')].map(id),
    brief: [...document.querySelectorAll('.dbrief > *')].map(key),
    menu_meta: [...document.querySelectorAll('#session-actions-menu .dmeta > *')].map(key),
    more: !!more && !more.hidden && more.offsetWidth > 0,
    free: actions.getBoundingClientRect().left - last.getBoundingClientRect().right,
    gap: parseFloat(getComputedStyle(document.querySelector('.dtitle')).columnGap),
    brief_gap: brief ? parseFloat(getComputedStyle(brief).columnGap) : 0,
    action_gap: parseFloat(getComputedStyle(actions).columnGap),
    title_clipped: text.scrollWidth > text.clientWidth + 1,
    height: document.querySelector('.dhead').offsetHeight,
    overflow: document.documentElement.scrollWidth > innerWidth,
  }};
}}"""
FIRST_MENU_WIDTH_JS = """() => {
  const menu = document.querySelector('#session-actions-menu');
  const was = menu.hidden; menu.hidden = false;
  const item = document.querySelector('#session-actions-menu [role="menu"] > *')
    || document.querySelector('#session-actions-menu .dmeta > *');
  const width = item ? item.getBoundingClientRect().width : 0;
  menu.hidden = was;
  return width;
}"""


def corpus(root: Path) -> Corpus:
    data = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    data.put(SID, "claude", [
        claude_row(SID, "user", "u0", None, "Fold sweep question", gitBranch="feat/fold-sweep"),
        claude_row(SID, "assistant", "a0", "u0", "reply Sweep", gitBranch="feat/fold-sweep")],
        ["Fold sweep question", "reply Sweep"])
    return data


def settle(page):
    # Headless dispatches media-query changes and ResizeObserver only after a painted frame.
    page.evaluate("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))")
    page.wait_for_timeout(40)


def tier_of(width):
    return "narrow" if width <= 720 else "medium" if width <= 1199 else "wide"


def sweep_widths():
    return sorted(set(range(320, 1501, 10)) | {608, 720, 721, 1199, 1200, 1698}, reverse=True)


def check_header(page, width, tiers, header_actions, chrome_tiers):
    fold = page.evaluate(HEADER_FOLD_JS)
    where = f"header@{width}"
    assert fold["inline"] + fold["menu"] == header_actions, (where, fold)
    assert fold["more"] == bool(fold["menu"]), (where, fold)
    if fold["menu"] != header_actions:
        assert not fold["squeezed"], (where, fold)
    if fold["menu"]:
        assert fold["free"] < fold["unit"], (where, fold)
    else:
        assert not page.locator("#header-more-btn").is_visible(), where
    assert fold["height"] <= 52 and not fold["overflow"], (where, fold)
    if fold["fold_brand"] or fold["fold_nodes"] or fold["menu"]:
        assert fold["fold_labels"] or not fold["labels"], (where, fold)
        assert not fold["labels"], (where, "labels must go before title/machines/buttons", fold)
    if fold["fold_nodes"] or fold["menu"]:
        assert not fold["brand"] or width <= 720, (where, "title must go before machines/buttons", fold)
    if fold["menu"] and fold["nodes_present"]:
        assert fold["nodes_menu"] or width <= 720, (where, "machines must go before buttons", fold)
    if fold["fold_nodes"]:
        assert fold["fold_brand"] or width <= 720, (where, fold)
    if fold["fold_brand"]:
        assert fold["fold_labels"] or width <= 720, (where, fold)
    chrome = (not fold["labels"], not fold["brand"], bool(fold["nodes_menu"]), len(fold["menu"]))
    tier = tier_of(width)
    assert chrome >= chrome_tiers.get(tier, chrome), (where, "chrome unfolded while narrowing", fold, chrome_tiers)
    chrome_tiers[tier] = chrome
    folded = len(fold["menu"])
    assert folded >= tiers.get(tier, 0), (where, "unfolded while narrowing", fold, tiers)
    tiers[tier] = folded
    return fold


def check_head(page, width, tier, tiers, key, meta_order):
    state = page.evaluate(HEAD_STATE_JS)
    where = f"{key}@{width}"
    action_order = state["globals"] + ACTION_ORDER
    priority = action_order + meta_order
    inline_actions = [i for i in state["inline"] if i not in ("a-term", "a-more")]
    placed = inline_actions + state["brief"]
    assert placed == priority[:len(placed)], (where, placed, priority, state)
    assert state["brief"] + state["menu_meta"] == meta_order, (where, state)
    assert inline_actions + state["menu_actions"] == action_order, (where, state)
    remaining = bool(state["menu_meta"] or state["menu_actions"])
    assert state["more"] == remaining, (where, state)
    assert ("a-more" in state["inline"]) == remaining, (where, state)
    assert state["height"] <= 46 and not state["overflow"], (where, state)
    if tier == "narrow":
        assert not state["title_clipped"], (where, "a narrow title never yields to metadata", state)
    if remaining:
        # The first menu item really does not fit: the free room is less than its width plus gaps and slack.
        width_next = page.evaluate(FIRST_MENU_WIDTH_JS)
        gap = state["action_gap"] if state["menu_actions"] else state["brief_gap"]
        assert state["free"] < width_next + gap + state["gap"] * 2 + SLACK, (where, state, width_next)
    n = len(placed)
    assert n <= tiers.get(key + tier, n), (where, "unfolded while narrowing", state, tiers)
    tiers[key + tier] = n
    return state


def fold_events(rows, priority_of):
    """Newly folded items in sweep order (wide -> narrow); folding X means everything after X had folded."""
    events, previous = [], None
    for width, folded, tier in rows:
        if previous is not None and previous[1] == tier:
            events.extend((width, tier, item) for item in folded if item not in previous[0])
        previous = (folded, tier)
    for width, tier, item in events:
        order = priority_of(tier)
        later = [i for i in order if order.index(i) > order.index(item)]
        state = next(f for w, f, t in rows if w == width and t == tier)
        assert all(i in state for i in later), (width, tier, item, state)
    return events


def check_status_badge(page, uid):
    # BUG-20260915-115855-eef60c: the mobile h2 clipped the top 4px
    # of the live badge. Use real title markup and the usual live painter.
    for width in (320, 390, 424, 608, 720, 721, 1200):
        page.set_viewport_size({"width": width, "height": 900})
        if width <= 720:
            page.evaluate("showMobileDetail()")
        settle(page)
        for tmux in (False, True):
            page.evaluate("""({uid, tmux}) => {
                S.live.add(uid);
                if (tmux) S.liveTmux.add(uid); else S.liveTmux.delete(uid);
                paintLive();
            }""", {"uid": uid, "tmux": tmux})
            badge = page.evaluate("""() => {
                const dot = document.querySelector('#dlive');
                const r = dot.getBoundingClientRect();
                let top = r.top, right = r.right, bottom = r.bottom, left = r.left;
                for (let e = dot.parentElement; e; e = e.parentElement) {
                    const s = getComputedStyle(e), b = e.getBoundingClientRect();
                    if (s.overflowX !== 'visible') {
                        left = Math.max(left, b.left); right = Math.min(right, b.right);
                    }
                    if (s.overflowY !== 'visible') {
                        top = Math.max(top, b.top); bottom = Math.min(bottom, b.bottom);
                    }
                }
                return {visible: dot.offsetWidth > 0, tmux: dot.classList.contains('tmux'),
                    clipped: [top - r.top, r.right - right, r.bottom - bottom, left - r.left]};
            }""")
            assert badge["visible"] and badge["tmux"] == tmux, (width, badge)
            assert all(abs(n) < 0.1 for n in badge["clipped"]), (width, badge)
    page.evaluate("uid => { S.live.delete(uid); S.liveTmux.delete(uid); paintLive(); }", uid)


def seed_header_nodes(page):
    # Local mode has no machine picker. Three named chips make the third fold step measurable.
    page.evaluate("""() => {
      const picker = document.querySelector('#node-picker');
      picker.hidden = false;
      const chips = document.querySelector('#node-chips');
      chips.replaceChildren();
      for (const [id, name] of [['n1', 'Lyra'], ['n2', 'Cygnus'], ['n3', 'Pavo']]) {
        const b = document.createElement('button');
        b.type = 'button';
        b.dataset.node = id;
        const count = document.createElement('b');
        count.className = 'node-count';
        count.textContent = '9';
        b.append(name + ' ', count);
        chips.appendChild(b);
      }
      const label = document.querySelector('#node-pick .node-pick-label');
      if (label) {
        label.textContent = '全部机器';
        label.classList.remove('node-none');
      }
      layoutHeader();
    }""")
    settle(page)


def run(page, uid):
    page.evaluate("uid => openSession(uid)", uid)
    page.wait_for_function('document.querySelector("#msgs")?.textContent.includes("reply Sweep")')
    check_status_badge(page, uid)
    page.set_viewport_size({"width": 1698, "height": 900})
    page.evaluate("setSideWidth(340, true)")
    settle(page)
    seed_header_nodes(page)
    first = page.evaluate(HEADER_FOLD_JS)
    header_actions = first["inline"] + first["menu"]
    assert header_actions == [i for i in HEADER_PRIORITY if i in header_actions] and "trash" in header_actions, first
    assert first["labels"] and first["brand"] and first["nodes_present"] and not first["nodes_menu"], first
    head = page.evaluate(HEAD_STATE_JS)
    meta_order = head["brief"] + head["menu_meta"]
    assert meta_order == ["mcount-total", "size", "time", "cwd", "meta-source", "session-id"], head
    mobile_actions = ["a-global-settings"]
    if page.evaluate("appDisplayMode.matches || navigator.standalone === true"):
        mobile_actions.append("a-global-page-reload")
    priority_of = lambda tier: (mobile_actions if tier == "narrow" else []) + ACTION_ORDER + meta_order  # noqa: E731
    header_rows, head_rows, heights, header_tiers, head_tiers, chrome_tiers = [], [], [], {}, {}, {}
    chrome_events = []
    previous_chrome = None
    for width in sweep_widths():
        page.set_viewport_size({"width": width, "height": 900})
        settle(page)
        tier = tier_of(width)
        if width <= 720:
            page.evaluate("showMobileList()")
            settle(page)
        fold = check_header(page, width, header_tiers, header_actions, chrome_tiers)
        header_rows.append((width, fold["menu"], tier))
        heights.append((width, fold["height"], tier))
        chrome = (fold["labels"], fold["brand"], not fold["nodes_menu"], len(fold["menu"]))
        if previous_chrome is not None:
            was_labels, was_brand, was_inline, was_folded = previous_chrome
            if was_labels and not fold["labels"]:
                chrome_events.append((width, "labels"))
            if was_brand and not fold["brand"]:
                chrome_events.append((width, "brand"))
            if was_inline and fold["nodes_menu"]:
                chrome_events.append((width, "nodes"))
            if was_folded == 0 and fold["menu"]:
                chrome_events.append((width, "buttons"))
        previous_chrome = chrome
        if width <= 720:
            page.evaluate("showMobileDetail()")
            settle(page)
        state = check_head(page, width, tier, head_tiers, "viewport", meta_order)
        head_rows.append((width, state["menu_meta"] + state["menu_actions"], tier))
    header_events = fold_events(header_rows, lambda tier: header_actions)
    head_events = fold_events(head_rows, priority_of)
    first_chrome = {}
    for width, name in chrome_events:
        first_chrome.setdefault(name, width)
    assert list(first_chrome) == [k for k in ("labels", "brand", "nodes", "buttons") if k in first_chrome], (
        first_chrome, chrome_events)
    assert {"labels", "brand", "nodes"} <= set(first_chrome), (first_chrome, chrome_events)
    chain = [first_chrome[k] for k in ("labels", "brand", "nodes", "buttons") if k in first_chrome]
    assert chain == sorted(chain, reverse=True), (first_chrome, chrome_events)
    assert next(f for w, f, t in header_rows if w == 608) == [], header_rows      # tablet width: all inline
    assert len({h for w, h, t in heights}) == 1, heights                            # one height across tiers
    assert any(t == "narrow" and f for w, f, t in header_rows), header_rows
    for tier in ("narrow", "medium", "wide"):
        assert any(t == tier and not f for w, f, t in header_rows), (tier, header_rows)
    assert header_events, header_rows
    assert any(f for w, f, t in head_rows) and any(not f for w, f, t in head_rows), head_rows
    assert head_events, head_rows
    drag_rows = []
    for width in (1698, 1100):
        page.set_viewport_size({"width": width, "height": 900})
        settle(page)
        tier, drag_tiers = tier_of(width), {}
        for side in range(200, width - 320, 20):
            page.evaluate("w => setSideWidth(w, true)", side)
            settle(page)
            state = check_head(page, side, tier, drag_tiers, f"drag{width}", meta_order)
            drag_rows.append((width - side, state["menu_meta"] + state["menu_actions"], tier))
        page.evaluate("setSideWidth(340, true)")
    drag_events = fold_events(drag_rows, priority_of)
    assert drag_events, drag_rows
    print(f"{len(header_rows)} widths, header chrome " + ", ".join(f"{n}≤{w}" for w, n in chrome_events)
          + "; buttons " + ", ".join(f"{i}≤{w}" for w, t, i in header_events), flush=True)
    print("title bar folds by viewport " + ", ".join(f"{i}≤{w}" for w, t, i in head_events), flush=True)
    print("title bar folds by drag " + ", ".join(f"{i}≤{w}" for w, t, i in drag_events), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-header-fold-") as directory:
        data = corpus(Path(directory))
        with isolated_server(data, args.binary) as (base, _opener), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 1698, "height": 900}, service_workers="block")
                page = context.new_page()
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base, wait_until="networkidle")
                page.wait_for_function("S.sessions.length && T.listLoaded")
                assert page.locator("#reload").count() == 0
                assert not page.locator("#page-reload").is_visible()
                run(page, data.uid(SID))
                context.close()
                for mode in ("standalone", "ios"):
                    context = browser.new_context(viewport={"width": 390, "height": 844})
                    if mode == "ios":
                        context.add_init_script("Object.defineProperty(navigator, 'standalone', {value: true})")
                    else:
                        context.add_init_script("""const original = window.matchMedia.bind(window);
                            window.matchMedia = query => query === '(display-mode: standalone)'
                                ? Object.defineProperty(original(query), 'matches', {value: true}) : original(query);""")
                    page = context.new_page()
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(base, wait_until="networkidle")
                    page.wait_for_function("S.sessions.length && T.listLoaded")
                    if not page.locator("#page-reload").is_visible():
                        page.locator("#header-more-btn").click()
                    assert page.locator("#page-reload").is_visible(), mode
                    page.evaluate("window.beforeRefresh = true")
                    with page.expect_navigation(wait_until="networkidle"):
                        page.get_by_role("button", name="刷新页面", exact=True).or_(
                            page.get_by_role("menuitem", name="刷新页面", exact=True)).click()
                    page.wait_for_function("S.sessions.length && T.listLoaded")
                    assert page.evaluate("window.beforeRefresh === undefined"), mode
                    assert page.evaluate("performance.getEntriesByType('navigation')[0].type") == "reload"
                    assert page.locator(f'#side .item[data-uid="{data.uid(SID)}"]').count() == 1
                    if mode == "standalone":
                        run(page, data.uid(SID))
                    context.close()
                assert not errors, errors
            finally:
                browser.close()
    print("PASS header fold browser: fold order over N widths for the header and the title bar, plus divider drag", flush=True)


if __name__ == "__main__":
    main()
