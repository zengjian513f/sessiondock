#!/usr/bin/env python3
"""SessionDock preference namespace and persistence browser contract.

Seeded through an init script before the page's own scripts run, the served
page must:

1. apply `sessiondock.*` values on first load (theme, font, sidebar width, nesting,
   timeline view, compact turns, chip filter, cache limit, tool icons, unread
   badges, last selection);
2. write changes back only under that namespace and retain them after reload;
3. leave a fresh browser at the defaults and use only SessionDock keys.

Synthetic corpus only; the Playwright context blocks service workers and
every request outside the isolated server.
"""
import argparse
import json
import os
import tempfile
from pathlib import Path
from urllib.parse import urlencode

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, build_corpus, claude_row, isolated_server

LS_DUMP = "() => Object.fromEntries(Object.keys(localStorage).sort().map(k => [k, localStorage.getItem(k)]))"


def seed_script(values):
    """Seed once per storage area (the init script runs on every navigation)."""
    body = "".join(f"localStorage.setItem({json.dumps(k)}, {json.dumps(v)});" for k, v in values.items())
    return ("(() => { try { if (localStorage.getItem('__prefs_seeded')) return; "
            f"{body} localStorage.setItem('__prefs_seeded', '1'); }} catch {{}} }})()")


def set_scale(page, value):
    slider = page.locator("#setting-scale")
    slider.focus()
    slider.press("Home")
    for _ in range(int(value) - 30):
        slider.press("ArrowRight")
    expect(slider).to_have_value(str(value))
    expect(page.locator("#setting-scale-value")).to_have_text(f"{value}%")


def pinch(page, cdp, start=35, end=49, target="#side", cancel=False):
    box = page.locator(target).bounding_box()
    x, y = box["x"] + box["width"] / 2, box["y"] + min(200, box["height"] / 2)
    def points(radius):
        return [{"x": x-radius, "y": y, "id": 1}, {"x": x+radius, "y": y, "id": 2}]
    cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": points(start)})
    for step in range(1, 13):
        cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": points(start+(end-start)*step/12)})
        page.wait_for_timeout(20)
    cdp.send("Input.dispatchTouchEvent", {"type": "touchCancel" if cancel else "touchEnd", "touchPoints": []})
    page.wait_for_timeout(100)


def check_pinch(browser, base):
    for width in (390, 820, 1280):
        context = browser.new_context(viewport={"width": width, "height": 900}, is_mobile=True,
                                      has_touch=True, service_workers="block")
        page = context.new_page()
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.goto(base, wait_until="networkidle")
        cdp = context.new_cdp_session(page)
        page.evaluate("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))")
        if not page.locator("#settings").is_visible():
            page.locator("#header-more-btn").tap()
        page.locator("#settings").tap()
        slider_box = page.locator("#setting-scale").bounding_box()
        x, y = slider_box["x"] + slider_box["width"] / 2, slider_box["y"] + slider_box["height"] / 2
        cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x, "y": y, "id": 1}]})
        for step in range(1, 6):
            cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": x+slider_box["width"]*.04*step, "y": y, "id": 1}]})
        cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
        assert 105 < page.evaluate("interfaceScale()") < 125
        after = page.locator("#setting-scale").bounding_box()
        # Reciprocal fractional zoom can round the centered dialog by a pixel.
        assert all(abs(after[key] - slider_box[key]) < 2 for key in ("x", "y", "width", "height")), (slider_box, after)
        page.locator("#setting-scale-reset").tap()
        page.locator("#settings-dialog .modal-close").tap()
        pinch(page, cdp)
        assert page.evaluate("interfaceScale()") == 140
        page.wait_for_timeout(650)
        expect(page.locator("#item-menu")).to_be_hidden()
        assert abs(page.evaluate("visualViewport.scale") - 1) < .01
        assert page.evaluate("document.querySelector('#app').getBoundingClientRect().height") >= 899
        pinch(page, cdp, start=49, end=35)
        assert page.evaluate("interfaceScale()") == 100
        # Explicit control ownership: both touches reach the child, no page zoom.
        page.evaluate("""() => {
          const widget = document.createElement('div');
          widget.id = 'pinch-widget'; widget.dataset.pinchOwner = '';
          widget.style.cssText = 'position:fixed;inset:100px 20px;z-index:1000;touch-action:none';
          widget.addEventListener('touchmove', e => { e.preventDefault(); widget.dataset.moved='yes'; }, {passive:false});
          document.querySelector('#app').append(widget);
        }""")
        pinch(page, cdp, target="#pinch-widget")
        assert page.locator("#pinch-widget").get_attribute("data-moved") == "yes"
        assert page.evaluate("interfaceScale()") == 100
        page.locator("#pinch-widget").evaluate("e => e.remove()")
        pinch(page, cdp, end=42, cancel=True)
        assert page.evaluate("interfaceScale()") == 120
        pinch(page, cdp, start=90, end=15)
        assert page.evaluate("interfaceScale()") == 30
        page.reload(wait_until="networkidle")
        assert page.evaluate("interfaceScale()") == 30
        assert abs(page.evaluate("visualViewport.scale") - 1) < .01
        assert not errors, errors
        context.close()
        print(f"PASS Chromium touch emulation at {width}px: pinch, reverse, widget ownership, cancel, persistence", flush=True)


def check_compact_scale(page):
    def settings():
        page.evaluate("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))")
        if not page.locator("#settings").is_visible():
            page.locator("#header-more-btn").click()
        page.locator("#settings").click()

    for width in (320, 390, 820, 1100, 1440):
        page.set_viewport_size({"width": width, "height": 900})
        for value in ("30", "31", "75", "100", "137", "150"):
            print(f"scale browser: {width}px {value}%", flush=True)
            settings()
            set_scale(page, value)
            page.wait_for_function("v => Math.abs(parseFloat(getComputedStyle(document.documentElement).zoom) - v / 100) < .001", arg=int(value))
            box = page.locator("#settings-dialog").bounding_box()
            assert box and box["x"] >= -1 and box["x"] + box["width"] <= width + 1, (width, value, box)
            page.locator('#settings-dialog .modal-close').click()
            box = page.locator("#app").bounding_box()
            assert abs(box["height"] - 900) <= 2 and abs(box["width"] - width) <= 2, (width, value, box)
            search = page.locator("#q")
            search.fill("unlikely-scale-search")
            expect(search).to_have_value("unlikely-scale-search")
            search.fill("")
            page.locator("#side .item").first.click()
            expect(page.locator("#msgs")).to_be_visible()
            if width <= 720:
                page.locator("#detail .mobile-back").click()
            page.reload(wait_until="networkidle")
            settings()
            expect(page.locator("#setting-scale")).to_have_value(value)
            assert page.evaluate("localStorage.getItem('sessiondock.interfaceScale')") == value
            page.locator('#settings-dialog .modal-close').click()
    page.set_viewport_size({"width": 1280, "height": 900})
    page.wait_for_function("getComputedStyle(document.documentElement).zoom === '1.5'")
    settings()
    page.locator("#setting-scale-reset").click()
    expect(page.locator("#setting-scale")).to_have_value("100")
    page.locator('#settings-dialog .modal-close').click()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-prefs-migration-") as temporary:
        corpus = build_corpus(Path(temporary))
        files = corpus.root / "files"
        files.mkdir()
        (files / "notes.md").write_text("# Synthetic document\n\nscoped file content\n")
        (files / "other.txt").write_text("another entry\n")
        sid = "claude-prefs-files"
        corpus.put(sid, "claude", [claude_row(sid, "user", "u0", None, f"Open `{files}/`", cwd=str(files))], [])
        native_before = {path: path.read_bytes() for path in corpus.paths.values()}
        target = corpus.uid("claude-branch")
        unread = corpus.uid("claude-compact")  # opening `target` clears its own badge, so seed another row
        seed = {
            "sessiondock.theme": '"dark"', "sessiondock.font": '"cascadia"', "sessiondock.width": "420",
            "sessiondock.sideCollapsed": "false", "sessiondock.nest": "true", "sessiondock.view": '"date"',
            "sessiondock.compactTurns": "false", "sessiondock.off": '["grok"]', "sessiondock.cacheMb": "64",
            "sessiondock.toolIcons": '"boss"', "sessiondock.unread": json.dumps([[unread, {"count": 3}]]),
            "sessiondock.sel": json.dumps(target), "sessiondock.nodesOff": '["stale-node"]',
            "sessiondock.settingsTab": '"appearance"',
            "sessiondock.mobilePage": '"list"',
            "sessiondock.files-view": json.dumps({"view": "grid", "order": "desc", "sort": "size", "hidden": True}),
            "sessiondock.files-clipboard": json.dumps({"action": "copy", "node": None, "paths": [str(files / "notes.md")]}),
        }
        with isolated_server(corpus, args.binary, file_roots=(files,)) as (base, _), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                errors = []
                # --- seeded browser: SessionDock preferences apply and persist ---
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block",
                                              color_scheme="light")
                context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                context.add_init_script(seed_script(seed))
                page = context.new_page()
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base, wait_until="networkidle")
                page.wait_for_function("typeof S !== 'undefined' && Array.isArray(S.sessions) && S.sessions.length > 0")
                expect(page.locator("#backend-notice")).to_be_hidden()
                # 1: applied.
                assert page.evaluate("document.documentElement.dataset.theme") == "dark"
                assert "Cascadia" in page.evaluate("getComputedStyle(document.documentElement).getPropertyValue('--terminal-font')")
                assert page.evaluate("document.documentElement.dataset.toolIcons") == "boss"
                state = page.evaluate("({nest: S.nest, view: S.view, compact: S.compactTurns, off: [...S.off], "
                                      "unread: [...S.unread], sel: S.sel, cache: cacheLimitMb, "
                                      "width: parseInt(document.querySelector('#left').style.width, 10), "
                                      "collapsed: document.body.classList.contains('side-collapsed'), "
                                      "prefix: STORAGE_PREFIX, nodesOff: [...Nodes.off]})")
                assert state["prefix"] == "sessiondock.", state
                assert state["nest"] is True and state["view"] == "date" and state["compact"] is False, state
                assert state["off"] == ["grok"] and state["cache"] == 64 and state["width"] == 420, state
                assert state["sel"] == target and state["unread"] == [[unread, {"count": 3}]], state
                assert state["nodesOff"] == ["stale-node"], state
                page.wait_for_function("uid => S.sel === uid && document.querySelectorAll('#msgs .msg').length > 0", arg=target)
                # The current namespace is the only preference source.
                assert page.evaluate("store.get('mobilePage', null)") == "list"
                # Every seeded preference remains under the SessionDock prefix.
                dump = page.evaluate(LS_DUMP)
                for key in ("theme", "font", "width", "nest", "view", "compactTurns", "off", "cacheMb",
                            "toolIcons", "unread", "sel", "nodesOff"):
                    assert dump.get("sessiondock." + key) is not None, (key, sorted(dump))
                    assert dump["sessiondock." + key] == seed["sessiondock." + key], key
                assert dump["sessiondock.mobilePage"] == '"list"'
                # Changes update the same namespace.
                page.locator("#nest-toggle").click()
                page.wait_for_function("S.nest === false")
                after = page.evaluate(LS_DUMP)
                assert after["sessiondock.nest"] == "false", after
                page.locator("#settings").click()
                page.locator("#setting-theme").select_option("light")
                page.wait_for_function("document.documentElement.dataset.theme === 'light'")
                after = page.evaluate(LS_DUMP)
                assert after["sessiondock.theme"] == '"light"', after
                page.keyboard.press("Escape")
                assert all(k == "__prefs_seeded" or k.startswith("sessiondock.") for k in after), after
                # A reload keeps the updated values.
                page.reload(wait_until="networkidle")
                page.wait_for_function("typeof S !== 'undefined' && Array.isArray(S.sessions) && S.sessions.length > 0")
                assert page.evaluate("document.documentElement.dataset.theme") == "light"
                assert page.evaluate("S.nest") is False

                context.close()

                # A fresh browser stays at the defaults.
                fresh = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block",
                                            color_scheme="light")
                fresh.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                page = fresh.new_page()
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base, wait_until="networkidle")
                page.wait_for_function("typeof S !== 'undefined' && Array.isArray(S.sessions) && S.sessions.length > 0")
                assert page.evaluate("document.documentElement.dataset.theme") == "light"
                assert "Ubuntu Sans Mono" in page.evaluate("getComputedStyle(document.documentElement).getPropertyValue('--terminal-font')")
                assert page.evaluate("({nest: S.nest, view: S.view, off: [...S.off], cache: cacheLimitMb})") == {"nest": False, "view": "tree", "off": [], "cache": 256}
                dump = page.evaluate(LS_DUMP)
                assert all(k.startswith("sessiondock.") for k in dump), dump
                # PWA identity: the shell is SessionDock and the manifest is served as such.
                assert page.evaluate("document.querySelector('meta[name=apple-mobile-web-app-title]').content") == "SessionDock"
                manifest = page.evaluate("fetch('manifest.webmanifest').then(r => r.json())")
                assert manifest["name"] == "SessionDock" and manifest["short_name"] == "SessionDock", manifest
                assert page.evaluate("navigator.serviceWorker.getRegistrations().then(list => list.length)") == 0
                check_compact_scale(page)
                fresh.close()
                check_pinch(browser, base)
                assert not errors, errors
            finally:
                browser.close()
        assert all(path.read_bytes() == before for path, before in native_before.items())
        print("PASS preferences browser: sessiondock.* values apply and persist, "
              "a fresh browser keeps the defaults, and PWA identity is SessionDock")


if __name__ == "__main__":
    main()
