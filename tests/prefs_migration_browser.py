#!/usr/bin/env python3
"""SessionDock preference namespace and persistence browser contract.

Seeded through an init script before the page's own scripts run, the served
page must:

1. apply `sessiondock.*` values on first load (theme, font, sidebar width, nesting,
   timeline view, compact turns, chip filter, cache limit, tool icons, unread
   badges, last selection);
2. write changes back only under that namespace and retain them after reload;
3. key `files.html` preferences as `sessiondock.files-<key>`;
4. leave a fresh browser at the defaults and use only SessionDock keys.

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

                # Files preferences use the same SessionDock namespace.
                manager = context.new_page()
                manager.on("pageerror", lambda error: errors.append(str(error)))
                manager.goto(f"{base}/files.html?{urlencode({'uid': corpus.uid(sid), 'ref': str(files) + '/'})}", wait_until="networkidle")
                expect(manager.locator("#entries")).to_contain_text("other.txt")
                assert manager.evaluate("document.querySelector('#view').value") == "grid"
                assert manager.evaluate("document.querySelector('#sort').value") == "size"
                expect(manager.locator("#order")).to_contain_text("降序")
                dump = manager.evaluate(LS_DUMP)
                assert json.loads(dump["sessiondock.files-view"])["view"] == "grid", dump
                assert json.loads(dump["sessiondock.files-clipboard"])["action"] == "copy", dump
                assert "文件管理 · SessionDock" in manager.title()
                manager.locator("#view").select_option("list")
                manager.wait_for_function("JSON.parse(localStorage.getItem('sessiondock.files-view')).view === 'list'")
                dump = manager.evaluate(LS_DUMP)
                assert json.loads(dump["sessiondock.files-view"])["view"] == "list", dump
                assert all(k == "__prefs_seeded" or k.startswith("sessiondock.") for k in dump), dump
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
                fresh.close()
                assert not errors, errors
            finally:
                browser.close()
        assert all(path.read_bytes() == before for path, before in native_before.items())
        print("PASS preferences browser: sessiondock.* values apply and persist, files.html uses the same "
              "namespace, a fresh browser keeps the defaults, and PWA identity is SessionDock")


if __name__ == "__main__":
    main()
