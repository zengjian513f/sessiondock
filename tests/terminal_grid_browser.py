#!/usr/bin/env python3
"""Server-side grid console in the main page; isolated free shell and native fixture.

Build sessiondock and ptyhost first. The test opens the fixture session, clicks
the console button (claim + `term/attach?mode=grid` into GridTerm) and checks,
by normal clicks, typing and clipboard paste: the shell reports the same pty
size as the grid view, a viewport resize reaches the pty, clipboard text pastes
into the shell, and a second page takes the terminal over after confirmation
while the first page is told and closes its pane. Terminal assertions read the
grid buffer through the view's xterm-compatible facade. No pageerror.

Selection/copy, scrollback, file paste and exit are covered by
terminal_selection_browser, terminal_scrollback_browser and
terminal_input_browser.
"""
from contextlib import ExitStack
import os
from pathlib import Path
import tempfile
from urllib.parse import urlsplit
import uuid

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY, Corpus, codex_message, codex_row, isolated_server
from host_identity import host
from popups import on_popup
from terminal_exit_browser import XTERM_TEXT


def wait_text(page, needle, timeout=10000):
    page.wait_for_function("text => (" + XTERM_TEXT + ")().includes(text)", arg=needle, timeout=timeout)


def count(page, needle):
    return page.evaluate("text => (" + XTERM_TEXT + ")().split(text).length - 1", needle)


def view_size(page):
    return page.evaluate("() => { const v = currentTermViewObject(); return {cols: v.term.cols, rows: v.term.rows}; }")


def type_line(page, text):
    keyboard = page.locator("#termpane .xterm-helper-textarea")
    keyboard.press_sequentially(text)
    keyboard.press("Enter")


def expect_pty_size(page, size):
    before = count(page, f"{size['rows']} {size['cols']}")
    type_line(page, "size")
    page.wait_for_function(
        "a => (" + XTERM_TEXT + ")().split('\\n').filter(line => line.trim() === a.line).length > a.before",
        arg={"line": f"{size['rows']} {size['cols']}", "before": before}, timeout=10000)


def open_console(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    button = page.locator("#a-term")
    expect(button).to_have_attribute("data-unavailable", "false")
    if not page.locator("#termpane").is_visible():
        button.click()
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN", timeout=10000)
    expect(page.locator("#termpane .grid-canvas")).to_be_visible()


def main():
    if os.name != "posix":
        raise SystemExit("This isolated real-shell acceptance currently requires POSIX.")
    ptyhost = BINARY.with_name("ptyhost")
    missing = [str(path) for path in (BINARY, ptyhost) if not path.is_file()]
    if missing:
        raise SystemExit("missing built binaries (do not cargo build here): " + ", ".join(missing))
    errors = []
    with tempfile.TemporaryDirectory(prefix="sessiondock-terminal-grid-") as temporary, sync_playwright() as playwright:
        root = Path(temporary)
        for name in ["host", "work", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = "synthetic-native-sid"
        corpus.put(sid, "codex", [
            codex_row("session_meta", {"id": sid, "cwd": str(root / "work")}),
            codex_message("user", "Synthetic grid terminal acceptance"),
        ], [])
        uid = corpus.uid(sid)
        native = corpus.paths[sid].read_bytes()
        instance = "synthetic-" + uuid.uuid4().hex
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            with host(root, instance, uid=uid) as (process, _record), ExitStack() as cleanup:
                with isolated_server(corpus, BINARY, host_dir=root / "host") as (base, _):
                    if os.environ.get('SESSIONDOCK_TEST_PREFIX'):
                        from frontend_entry_browser import prefixed_proxy
                        prefixed, target = cleanup.enter_context(prefixed_proxy())
                        target.url, base = base, prefixed.rstrip('/')

                    def new_page():
                        context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                        cleanup.callback(context.close)
                        context.route("**/*", lambda route: route.continue_()
                                      if route.request.url.startswith(base + "/") else route.abort())
                        origin = f"{urlsplit(base).scheme}://{urlsplit(base).netloc}"
                        context.grant_permissions(["clipboard-read", "clipboard-write"], origin=origin)
                        page = context.new_page()
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        return context, page

                    context, page = new_page()
                    claims = []
                    context.on("request", lambda request: claims.append(request.post_data_json)
                               if urlsplit(request.url).path.endswith("/api/term/claim") else None)
                    dialogs = []
                    on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
                    page.goto(base, wait_until="networkidle")
                    open_console(page, uid)
                    wait_text(page, "RS_SHELL_READY")
                    assert claims and not any(claim.get("force") for claim in claims), claims
                    assert claims[-1]["instance_id"] == instance, claims
                    print("PASS a: console button claims and attaches the grid view", flush=True)

                    type_line(page, "ping")
                    wait_text(page, "RS_PING_OK")
                    expect_pty_size(page, view_size(page))
                    print("PASS b: typing reaches the shell; pty size equals the grid view", flush=True)

                    before = view_size(page)
                    page.set_viewport_size({"width": 900, "height": 600})
                    page.wait_for_function("prev => currentTermViewObject().term.cols !== prev",
                                           arg=before["cols"], timeout=10000)
                    resized = view_size(page)
                    expect_pty_size(page, resized)
                    page.set_viewport_size({"width": 1280, "height": 900})
                    page.wait_for_function("prev => currentTermViewObject().term.cols !== prev",
                                           arg=resized["cols"], timeout=10000)
                    print("PASS c: viewport resize follows into the pty", flush=True)

                    pings = count(page, "RS_PING_OK")
                    page.evaluate("navigator.clipboard.writeText('ping')")
                    keyboard = page.locator("#termpane .xterm-helper-textarea")
                    keyboard.focus()
                    keyboard.press("Control+V")
                    keyboard.press("Enter")
                    page.wait_for_function("n => (" + XTERM_TEXT + ")().split('RS_PING_OK').length - 1 > n",
                                           arg=pings, timeout=10000)
                    print("PASS d: clipboard text pastes into the shell", flush=True)

                    second_context, second = new_page()
                    second_dialogs = []
                    on_popup(second, lambda dialog: (second_dialogs.append(dialog.message), dialog.accept()))
                    second.goto(base, wait_until="networkidle")
                    open_console(second, uid)
                    expect(page.locator("#termpane")).to_be_hidden(timeout=10000)
                    assert any("抢占终端" in message for message in second_dialogs), second_dialogs
                    assert any("抢占" in message and "本页面的终端已关闭" in message for message in dialogs), dialogs
                    second.bring_to_front()
                    type_line(second, "next")
                    wait_text(second, "RS_AFTER_RESTART")
                    print("PASS e: second page takes over after confirmation; first page closes", flush=True)

                    assert process.poll() is None
                    assert corpus.paths[sid].read_bytes() == native
                    assert not errors, errors
        finally:
            browser.close()
    print("PASS terminal grid browser: claim/attach, typing, pty size, resize follow, paste, takeover, no pageerror",
          flush=True)


if __name__ == "__main__":
    main()
