#!/usr/bin/env python3
"""Sidebar pointer interactions; isolated Rust server, no CLI.

A phone held sideways (or a tablet) is wider than the 720px mobile breakpoint, so it gets the
desktop split layout with the ``#drag`` divider between the session list and the detail pane. A
finger on that divider produces pointer/touch events only: the browser never synthesises
``mousemove`` for a touch drag, and without ``touch-action: none`` it turns the gesture into a
scroll and fires ``pointercancel``. The divider must move under a real touch drag, keep working
with a mouse, and never leave the page stuck in the dragging state. The stored width lives under
the Rust storage namespace (``sessiondock.width``).
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import sync_playwright

from history_parity import BINARY, build_corpus, isolated_server

PHONE_LANDSCAPE = {"width": 814, "height": 380}   # iPhone 15 Pro Max, CSS px
DESKTOP = {"width": 1280, "height": 900}
PROBE = """() => {
  const left = $('#left').getBoundingClientRect();
  const drag = $('#drag').getBoundingClientRect();
  return {left: left.width, drag: {x: drag.x, y: drag.y, w: drag.width, h: drag.height},
          dragging: document.body.classList.contains('dragging'),
          sideVar: getComputedStyle(document.documentElement).getPropertyValue('--side-width').trim(),
          prefix: STORAGE_PREFIX,
          stored: localStorage.getItem(STORAGE_PREFIX + 'width'),
          touchAction: getComputedStyle($('#drag')).touchAction,
          coarse: matchMedia('(pointer: coarse)').matches};
}"""


def touch_drag(page, x0, y0, x1, y1, steps=8, end=True):
    """Real touch input through CDP so the browser's own gesture handling runs."""
    cdp = page.context.new_cdp_session(page)
    point = lambda x, y: {"x": x, "y": y, "radiusX": 8, "radiusY": 8, "force": 1}  # noqa: E731
    cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [point(x0, y0)]})
    for i in range(1, steps + 1):
        cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [
            point(x0 + (x1 - x0) * i / steps, y0 + (y1 - y0) * i / steps)]})
        page.wait_for_timeout(20)
    if end:
        cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
    else:
        cdp.send("Input.dispatchTouchEvent", {"type": "touchCancel", "touchPoints": []})
    cdp.detach()


def open_first_session(page, base, uid):
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(base, wait_until="networkidle")
    page.wait_for_function("S.sessions.length && T.listLoaded")
    page.evaluate("uid => openSession(uid)", uid)
    page.wait_for_selector(".dhead h2")
    page.evaluate("setSideWidth(340, true)")
    page.wait_for_timeout(100)
    return errors


def check_touch(browser, base, uid):
    context = browser.new_context(viewport=PHONE_LANDSCAPE, has_touch=True, is_mobile=True,
                                  device_scale_factor=3, service_workers="block")
    page = context.new_page()
    errors = open_first_session(page, base, uid)
    before = page.evaluate(PROBE)
    assert before["prefix"] == "sessiondock.", before
    assert before["drag"]["w"] > 0 and before["left"] == 340, before
    cx = before["drag"]["x"] + before["drag"]["w"] / 2
    cy = before["drag"]["y"] + before["drag"]["h"] / 2

    # A finger dragging the divider 120px to the right widens the list by 120px.
    touch_drag(page, cx, cy, cx + 120, cy)
    page.wait_for_timeout(150)
    after = page.evaluate(PROBE)
    assert abs(after["left"] - 460) < 3, (before, after)
    assert after["sideVar"] == f"{round(after['left'])}px" and after["stored"] == str(round(after["left"])), after
    assert not after["dragging"], after
    assert after["touchAction"] == "none", after

    # Fingers are wider than 5px: on a coarse pointer the hit zone extends a few px to either side
    # without changing the visible divider.
    if before["coarse"]:
        hit = page.evaluate("([x, y]) => [x - 4, x + 4].map(px => document.elementFromPoint(px, y)?.id)",
                            [cx + 120, cy])
        assert hit == ["drag", "drag"], hit
        assert page.evaluate(PROBE)["drag"]["w"] == before["drag"]["w"]
        touch_drag(page, cx + 120 + 4, cy, cx + 40, cy)
        page.wait_for_timeout(150)
        assert abs(page.evaluate(PROBE)["left"] - 380) < 3, page.evaluate(PROBE)
        page.evaluate("setSideWidth(460, true)")

    # A cancelled touch (the OS taking over the gesture) must not leave the page stuck in the
    # dragging state with the divider following nothing.
    touch_drag(page, cx + 120, cy, cx + 60, cy, end=False)
    page.wait_for_timeout(150)
    cancelled = page.evaluate(PROBE)
    assert not cancelled["dragging"], cancelled
    assert abs(cancelled["left"] - 400) < 3, cancelled
    touch_drag(page, cx + 200, cy + 20, cx + 200, cy - 20)
    page.wait_for_timeout(100)
    assert abs(page.evaluate(PROBE)["left"] - 400) < 3, page.evaluate(PROBE)

    # Dragging must not scroll the session list underneath.
    page.evaluate('$("#side").scrollTop = 0')
    touch_drag(page, cx + 60, cy + 60, cx + 60, cy - 60)
    page.wait_for_timeout(100)
    assert page.evaluate('$("#side").scrollTop') == 0
    assert not errors, errors
    context.close()


def check_mouse(browser, base, uid):
    context = browser.new_context(viewport=DESKTOP, service_workers="block")
    page = context.new_page()
    errors = open_first_session(page, base, uid)
    before = page.evaluate(PROBE)
    cx = before["drag"]["x"] + before["drag"]["w"] / 2
    cy = before["drag"]["y"] + 200
    page.mouse.move(cx, cy)
    page.mouse.down()
    page.mouse.move(cx + 130, cy, steps=6)
    mid = page.evaluate(PROBE)
    assert mid["dragging"] and abs(mid["left"] - 470) < 3, mid
    page.mouse.up()
    after = page.evaluate(PROBE)
    assert not after["dragging"] and abs(after["left"] - 470) < 3, after
    assert after["stored"] == str(round(after["left"])), after
    assert page.evaluate("getSelection().toString()") == ""
    page.dblclick("#drag")
    page.wait_for_timeout(100)
    assert page.evaluate(PROBE)["left"] == 340
    # Coarse-pointer hit zone must not exist for a mouse: 5px is the whole target.
    fine = page.evaluate("([x, y]) => [x - 6, x + 6].map(px => document.elementFromPoint(px, y)?.id)", [cx, cy])
    assert "drag" not in fine, fine

    # Drag-selecting sidebar text produces a browser click after mouseup. It must neither open
    # that row nor lose the selection; automatic metadata/list rendering waits until copying is
    # done, then catches up from the current in-memory session list.
    row = page.locator(f'#side .item[data-uid]:not([data-uid="{uid}"])').first
    title = row.locator(".t")
    other_uid = row.get_attribute("data-uid")
    old_title = title.text_content()
    box = title.bounding_box()
    assert box and old_title, (box, old_title)
    page.evaluate("""() => {
      document.querySelector('#side').addEventListener('pointerdown', () => {
        window.__sidebarSelectionRenderRace = setInterval(() => renderSide(), 1);
      }, {once: true});
    }""")
    page.mouse.move(box["x"] + 3, box["y"] + box["height"] / 2)
    page.mouse.down()
    page.mouse.move(box["x"] + min(box["width"] - 3, 90), box["y"] + box["height"] / 2, steps=8)
    page.mouse.up()
    page.evaluate("clearInterval(window.__sidebarSelectionRenderRace)")
    selected = page.evaluate("getSelection().toString()")
    assert selected.strip(), selected
    assert page.evaluate("S.sel") == uid, (uid, other_uid, page.evaluate("S.sel"))
    assert row.get_attribute("data-uid") == other_uid

    updated_title = old_title + "（后台更新）"
    page.evaluate("""([otherUid, updatedTitle]) => {
      S.sessions = S.sessions.map(row => row.uid === otherUid ? {...row, title: updatedTitle} : row);
      renderSide();
    }""", [other_uid, updated_title])
    assert page.evaluate("getSelection().toString()") == selected
    assert title.text_content() == old_title
    page.evaluate("getSelection().removeAllRanges()")
    page.wait_for_function("([otherUid, updatedTitle]) => document.querySelector(`#side .item[data-uid=\"${CSS.escape(otherUid)}\"] .t`)?.textContent === updatedTitle",
                           arg=[other_uid, updated_title])
    assert page.evaluate("getSelection().toString()") == ""
    assert not errors, errors
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-side-drag-") as directory:
        data = build_corpus(Path(directory))
        with isolated_server(data, args.binary) as (base, _opener), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                uid = data.uid("claude-branch")
                check_touch(browser, base, uid)
                check_mouse(browser, base, uid)
            finally:
                browser.close()
    print("PASS sidebar pointer browser: touch drag/cancel/scroll isolation at 814x380, mouse drag/dblclick, "
          "persistent desktop text selection with deferred refresh, width stored under sessiondock.", flush=True)


if __name__ == "__main__":
    main()
