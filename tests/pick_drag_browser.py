#!/usr/bin/env python3
"""Multi-select: drag with the left mouse button to pick or unpick a run of rows.

In 多选 mode, pressing on a row and dragging over others applies one state to the
whole run: the pressed row unpicked → the run is picked, the pressed row picked →
the run is unpicked. Dragging back restores rows that left the run, the release
does not toggle the pressed row again, no text gets selected, and holding at the
list's bottom edge scrolls it. A plain click still toggles one row, and entering or
leaving 多选 patches the existing rows instead of rebuilding the list.
Real mouse input against an isolated server; synthetic corpus.
"""
from browser_runtime import js
import argparse
import os
import re
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, build_corpus, codex_row, isolated_server

PICKED = re.compile(r"\bpicked\b")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-pick-drag-") as temporary, sync_playwright() as pw:
        corpus = build_corpus(Path(temporary) / "corpus")
        for index in range(30):
            sid = f"pick-drag-{index:02d}"
            corpus.put(sid, "codex", [
                codex_row("session_meta", {"id": sid, "cwd": "/tmp", "timestamp": f"2026-09-11T10:{index:02d}:00Z"}),
                codex_row("response_item", {"type": "message", "role": "user", "content": f"drag row {index}"})], [])
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = pw.chromium.launch(**launch)
        with isolated_server(corpus, args.binary) as (base, _):
            context = browser.new_context(viewport={"width": 1280, "height": 700}, service_workers="block")
            context.route("**/*", lambda route: route.continue_()
                          if route.request.url.startswith(base + "/") else route.abort())
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(base, wait_until="networkidle")
            page.wait_for_function(js("S.sessions.length >= 30", 'runtime.core.state.catalog.sessions.length >= 30'))
            # Entering and leaving 多选 patches the rows in place: same nodes,
            # the checkboxes come and go, no rebuild of the whole list.
            page.evaluate("() => document.querySelectorAll('#side .item[data-uid]').forEach(n => { n.__kept = true; })")
            for on in (True, False):
                page.evaluate(js("on => setPicking(on)", 'on => runtime.bulk.setPicking(on)'), on)
                state = page.evaluate("""() => {
                    const rows = [...document.querySelectorAll('#side .item[data-uid]')];
                    return {rows: rows.length, kept: rows.filter(n => n.__kept).length,
                            boxes: document.querySelectorAll('#side .item-pick').length,
                            heads: document.querySelectorAll('#side .ghead-pick').length,
                            groups: document.querySelectorAll('#side > .group').length};
                }""")
                assert state["kept"] == state["rows"], state
                if on:
                    assert state["boxes"] >= 30 and state["heads"] == state["groups"], state
                else:
                    assert state["boxes"] == 0 and state["heads"] == 0, state
            first = page.locator("#side .item[data-uid]").first
            first.click(button="right")
            page.locator('#item-menu [data-act="pick"]').click()
            expect(page.locator("#side-picked")).to_have_text("已选 1 项")

            def rows():
                return page.evaluate("""() => [...document.querySelectorAll('#side .item[data-uid]')]
                    .filter(r => r.querySelector('.item-pick') && r.getClientRects().length).map(r => r.dataset.uid)""")

            uids = rows()
            assert len(uids) >= 30, len(uids)

            def row(i):
                return page.locator(f'#side .item[data-uid="{uids[i]}"]')

            def center(i):
                row(i).scroll_into_view_if_needed()
                box = row(i).bounding_box()
                return box["x"] + box["width"] / 2, box["y"] + box["height"] / 2

            def drag(*path):
                page.mouse.move(*center(path[0]))
                page.mouse.down()
                for i in path[1:]:
                    page.mouse.move(*center(i), steps=6)
                page.mouse.up()

            def picked():
                return {i for i, uid in enumerate(uids) if page.evaluate(js("uid => pickedSessions.has(uid)", 'uid => runtime.bulk.state.picked.has(uid)'), uid)}

            def expect_picked(indices):
                page.wait_for_function(js("n => pickedSessions.size === n", 'n => runtime.bulk.state.picked.size === n'), arg=len(indices))
                assert picked() == set(indices), (picked(), indices)
                for i in range(8):
                    (expect(row(i)).to_have_class if i in indices else expect(row(i)).not_to_have_class)(
                        PICKED)
                expect(page.locator("#side-picked")).to_have_text(f"已选 {len(indices)} 项")

            expect_picked({0})
            # Pressing an unpicked row picks the whole run.
            drag(1, 3)
            expect_picked({0, 1, 2, 3})
            assert page.evaluate("getSelection().toString()") == "", "drag selected text"
            row(9).locator(".t").dblclick()
            assert page.evaluate("getSelection().toString()") == "", "double click selected text"
            expect_picked({0, 1, 2, 3})       # a double click toggles twice
            # A plain click still toggles one row.
            row(5).click()
            expect_picked({0, 1, 2, 3, 5})
            row(5).click()
            expect_picked({0, 1, 2, 3})
            row(4).click()
            expect_picked({0, 1, 2, 3, 4})
            # Pressing a picked row unpicks the run; dragging back restores rows that left it.
            drag(2, 4, 1)
            expect_picked({0, 3, 4})
            # Out and back to the pressed row: the release does not toggle it again.
            drag(0, 1, 0)
            expect_picked({3, 4})
            # Holding at the bottom edge scrolls the list and extends the run.
            side = page.locator("#side")
            page.evaluate("document.querySelector('#side').scrollTop = 0")
            box = side.bounding_box()
            page.mouse.move(*center(6))
            page.mouse.down()
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] - 4, steps=8)
            page.wait_for_function(js("uid => pickedSessions.has(uid)", 'uid => runtime.bulk.state.picked.has(uid)'), arg=uids[16])
            assert page.evaluate("document.querySelector('#side').scrollTop") > 0
            page.mouse.up()
            beyond = picked()
            assert {3, 4, *range(6, 17)}.issubset(beyond), beyond
            assert 5 not in beyond and not ({0, 1, 2} & beyond), beyond
            assert not errors, errors
            context.close()
        browser.close()
    print("PASS pick drag: drag from an unpicked row picks the run, from a picked row unpicks it; "
          "dragging back restores, the release does not re-toggle, no text selection, edge auto-scroll; "
          "a plain click still toggles one row; entering/leaving 多选 keeps the row nodes")


if __name__ == "__main__":
    main()
