#!/usr/bin/env python3
"""One-time preference migration from the Python `sessiondock.*` keys (batch 44 WP-F).

A same-origin deployment that replaced the Python page still holds every
preference under `sessiondock.<key>` (files pages: bare `sessiondock-files-<key>`).
Seeded through an init script before the page's own scripts run, the real
legacy page must:

1. apply the Python values on first load (theme, font, sidebar width, nesting,
   timeline view, compact turns, chip filter, cache limit, tool icons, unread
   badges, last selection) — the `watcher` monkey's F8 scenario;
2. copy each read value to `sessiondock.<key>` exactly once and never write the
   `sessiondock.*` keys again (a later change lands only under the new prefix);
3. prefer an existing `sessiondock.<key>` over the Python value;
4. on `files.html`, key preferences as `sessiondock.files-<key>` and migrate
   both older spellings (`sessiondock.sessiondock-files-<key>`, then the bare
   Python `sessiondock-files-<key>`);
5. leave a fresh browser (nothing to migrate) at the defaults.

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
            # 3: an existing Rust key wins over the Python one.
            "sessiondock.mobilePage": '"detail"', "sessiondock.mobilePage": '"list"',
            # 4: files pages — the pre-rename Rust spelling beats the bare Python key;
            #    the clipboard only exists under the Python key.
            "sessiondock-files-view": json.dumps({"view": "list", "order": "desc", "sort": "size", "hidden": True}),
            "sessiondock.sessiondock-files-view": json.dumps({"view": "grid", "order": "desc", "sort": "size", "hidden": True}),
            "sessiondock-files-clipboard": json.dumps({"action": "copy", "node": None, "paths": [str(files / "notes.md")]}),
        }
        with isolated_server(corpus, args.binary, file_roots=(files,)) as (base, _), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                errors = []
                # --- seeded browser: the Python preferences apply and migrate once ---
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
                # `mobilePage` existed under both prefixes: the Rust key must win.
                assert page.evaluate("store.get('mobilePage', null)") == "list"
                # 2: copied forward once, under the new prefix only.
                dump = page.evaluate(LS_DUMP)
                for key in ("theme", "font", "width", "nest", "view", "compactTurns", "off", "cacheMb",
                            "toolIcons", "unread", "sel", "nodesOff"):
                    assert dump.get("sessiondock." + key) is not None, (key, sorted(dump))
                    assert dump["sessiondock." + key] == seed["sessiondock." + key], key
                for key in ("theme", "font", "width", "nest", "view", "compactTurns", "off", "cacheMb", "toolIcons", "nodesOff"):
                    assert dump["sessiondock." + key] == seed["sessiondock." + key], (key, dump["sessiondock." + key])
                assert dump["sessiondock.mobilePage"] == '"list"' and dump["sessiondock.mobilePage"] == '"detail"'
                # A change made on the Rust page lands under the new prefix only.
                page.locator("#nest-toggle").click()
                page.wait_for_function("S.nest === false")
                after = page.evaluate(LS_DUMP)
                assert after["sessiondock.nest"] == "false" and after["sessiondock.nest"] == "true", after
                page.locator("#settings").click()
                page.locator("#setting-theme").select_option("light")
                page.wait_for_function("document.documentElement.dataset.theme === 'light'")
                after = page.evaluate(LS_DUMP)
                assert after["sessiondock.theme"] == '"light"' and after["sessiondock.theme"] == '"dark"', after
                page.keyboard.press("Escape")
                # The Python keys are the same set as seeded: nothing was added or removed there.
                assert {k: v for k, v in after.items() if k.startswith("sessiondock.")} == {k: v for k, v in seed.items() if k.startswith("sessiondock.")}
                # A reload keeps the Rust values (light theme chosen above, nest off) — no re-migration.
                page.reload(wait_until="networkidle")
                page.wait_for_function("typeof S !== 'undefined' && Array.isArray(S.sessions) && S.sessions.length > 0")
                assert page.evaluate("document.documentElement.dataset.theme") == "light"
                assert page.evaluate("S.nest") is False

                # 4: files.html keys.
                manager = context.new_page()
                manager.on("pageerror", lambda error: errors.append(str(error)))
                manager.goto(f"{base}/files.html?{urlencode({'uid': corpus.uid(sid), 'ref': str(files) + '/'})}", wait_until="networkidle")
                expect(manager.locator("#entries")).to_contain_text("other.txt")
                assert manager.evaluate("document.querySelector('#view').value") == "grid", "pre-rename Rust spelling wins over the Python key"
                assert manager.evaluate("document.querySelector('#sort').value") == "size"
                expect(manager.locator("#order")).to_contain_text("降序")
                dump = manager.evaluate(LS_DUMP)
                assert json.loads(dump["sessiondock.files-view"])["view"] == "grid", dump
                assert json.loads(dump["sessiondock.files-clipboard"])["action"] == "copy", dump
                assert dump["sessiondock-files-view"] == seed["sessiondock-files-view"]
                assert dump["sessiondock-files-clipboard"] == seed["sessiondock-files-clipboard"]
                assert dump["sessiondock.sessiondock-files-view"] == seed["sessiondock.sessiondock-files-view"]
                assert "文件管理 · SessionDock" in manager.title()
                manager.locator("#view").select_option("list")
                manager.wait_for_function("JSON.parse(localStorage.getItem('sessiondock.files-view')).view === 'list'")
                dump = manager.evaluate(LS_DUMP)
                assert dump["sessiondock-files-view"] == seed["sessiondock-files-view"], "Python files key never written"
                assert dump["sessiondock.sessiondock-files-view"] == seed["sessiondock.sessiondock-files-view"], "old Rust spelling never written"
                assert not [k for k in dump if k.startswith("sessiondock.sessiondock-files-") and k != "sessiondock.sessiondock-files-view"], dump
                context.close()

                # 5: a fresh browser has nothing to migrate and stays at the defaults.
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
                assert not [k for k in dump if k.startswith("sessiondock")], dump
                assert all(k.startswith("sessiondock.") for k in dump), dump
                # PWA identity: the shell is SessionDock and the manifest is served as such.
                assert page.evaluate("document.querySelector('meta[name=apple-mobile-web-app-title]').content") == "SessionDock"
                manifest = page.evaluate("fetch('manifest.webmanifest').then(r => r.json())")
                assert manifest["name"] == "SessionDock" and manifest["short_name"] == "SessionDock", manifest
                worker = page.evaluate("fetch('service-worker.js').then(r => r.text())")
                assert "const CACHE_PREFIX = 'sessiondock-shell-'" in worker
                fresh.close()
                assert not errors, errors
            finally:
                browser.close()
        assert all(path.read_bytes() == before for path, before in native_before.items())
        print("PASS prefs migration browser: seeded sessiondock.* preferences applied and copied once to sessiondock.*, "
              "later writes only under the new prefix, existing Rust keys win, files.html migrates both older "
              "spellings, a fresh browser keeps the defaults, PWA identity is SessionDock")


if __name__ == "__main__":
    main()
