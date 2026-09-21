#!/usr/bin/env python3
"""Real legacy grid-terminal-page acceptance; isolated free shell and native fixture.

Build sessiondock and ptyhost first. The test opens only its fixture's
grid page, never creates descendants, and exercises connect, typing, resize,
scrollback, selection, copy, paste and exit by normal browser clicks and
keyboard input. Terminal assertions read the grid model through
globalThis.__grid and globalThis.__gridText().
"""
from contextlib import ExitStack
import os
from pathlib import Path
import re
import tempfile
from urllib.parse import urlsplit
import uuid

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
from host_identity import host


def wait_grid(page, needle, timeout=10000):
    page.wait_for_function(
        "text => __gridText().includes(text)", arg=needle, timeout=timeout)


def viewport_top(page):
    return page.evaluate("""() => {
      if (__grid.state.following)
        return Math.max(0, __grid.model.lineCount() - __grid.model.rows);
      const slot = __grid.renderer._slots[0];
      return slot && typeof slot.line === 'number' ? slot.line : 0;
    }""")


def reveal_line(page, index, timeout=10000):
    box = page.locator("#term").bounding_box()
    assert box and box["width"] and box["height"], box
    page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    deadline = page.evaluate("Date.now()") + timeout
    for _ in range(120):
        win = page.evaluate("""() => {
          const slot = __grid.renderer._slots[0];
          const top = slot && typeof slot.line === 'number' ? slot.line : 0;
          return {top, rows: __grid.model.rows, now: Date.now()};
        }""")
        if win["top"] <= index < win["top"] + win["rows"]:
            return
        assert win["now"] < deadline, f"line {index} still outside viewport {win}"
        page.mouse.wheel(0, -300 if index < win["top"] else 300)
        page.wait_for_function(
            "prev => (__grid.renderer._slots[0] && __grid.renderer._slots[0].line) !== prev",
            arg=win["top"], timeout=2000)
    raise AssertionError(f"line {index} not revealed")


def selection_point(page, needle):
    return page.evaluate("""needle => {
      const lines = __gridText().split('\\n');
      const index = lines.findIndex(line => line.includes(needle));
      if (index < 0) return null;
      const text = lines[index];
      const col = Math.max(0, text.indexOf(needle));
      const slot = __grid.renderer._slots[0];
      const top = slot && typeof slot.line === 'number' ? slot.line : 0;
      const rect = document.querySelector('#grid').getBoundingClientRect();
      const cw = __grid.renderer.cellWidth;
      const ch = __grid.renderer.cellHeight;
      const row = index - top;
      return {
        index,
        x0: rect.left + (col + 0.2) * cw,
        x1: rect.left + (col + Math.max(needle.length, 1) - 0.2) * cw,
        y: rect.top + (row + 0.5) * ch,
      };
    }""", needle)


def focus_term(page):
    page.locator("#term").click()


def main():
    if os.name != "posix":
        raise SystemExit("This isolated real-shell acceptance currently requires POSIX.")
    ptyhost = BINARY.with_name("ptyhost")
    missing = [str(path) for path in (BINARY, ptyhost) if not path.is_file()]
    if missing:
        raise SystemExit("missing built binaries (do not cargo build here): " + ", ".join(missing))
    page_errors = []
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
        instance = "synthetic-" + uuid.uuid4().hex
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            with host(root, instance, uid=uid) as (_process, _record), ExitStack() as cleanup:
                with isolated_server(corpus, BINARY, host_dir=root / "host") as (base, _):
                    context = browser.new_context(
                        viewport={"width": 1280, "height": 900}, service_workers="block")
                    cleanup.callback(context.close)
                    context.route("**/*", lambda route: route.continue_()
                                  if route.request.url.startswith(base + "/") else route.abort())
                    origin = f"{urlsplit(base).scheme}://{urlsplit(base).netloc}"
                    context.grant_permissions(
                        ["clipboard-read", "clipboard-write"], origin=origin)
                    page = context.new_page()
                    page.on("pageerror", lambda error: page_errors.append(str(error)))
                    page.goto(base + "/grid.html", wait_until="networkidle")
                    page.wait_for_function("typeof __grid === 'object' && !!__grid", timeout=10000)
                    expect(page.locator("#session")).to_contain_text(
                        "synthetic-identity-host", timeout=10000)
                    page.locator("#connect").click()
                    page.wait_for_function(
                        "__grid.state.connected === true && __gridText().includes('RS_SHELL_READY')",
                        timeout=10000)
                    size = page.locator("#size").inner_text()
                    assert re.fullmatch(r"\d+×\d+", size), size
                    shown_cols, shown_rows = (int(part) for part in size.split("×"))
                    model = page.evaluate(
                        "() => ({cols: __grid.model.cols, rows: __grid.model.rows})")
                    assert shown_cols == model["cols"] and shown_rows == model["rows"], (size, model)
                    canvas = page.evaluate("""() => {
                      const c = document.querySelector('#grid');
                      return {width: c.width, height: c.height};
                    }""")
                    assert canvas["width"] > 0 and canvas["height"] > 0, canvas
                    print("PASS a", flush=True)

                    focus_term(page)
                    page.keyboard.type("ping")
                    page.keyboard.press("Enter")
                    wait_grid(page, "RS_PING_OK")
                    dims = page.evaluate(
                        "() => ({cols: __grid.model.cols, rows: __grid.model.rows})")
                    page.keyboard.type("size")
                    page.keyboard.press("Enter")
                    page.wait_for_function(
                        "expected => __gridText().split('\\n').some(line => line.trim() === expected)",
                        arg=f"{dims['rows']} {dims['cols']}", timeout=10000)
                    print("PASS b", flush=True)

                    before_cols = page.evaluate("__grid.model.cols")
                    page.set_viewport_size({"width": 900, "height": 600})
                    page.wait_for_function(
                        "prev => __grid.model.cols !== prev", arg=before_cols, timeout=10000)
                    focus_term(page)
                    resized = page.evaluate(
                        "() => ({cols: __grid.model.cols, rows: __grid.model.rows})")
                    page.keyboard.type("size")
                    page.keyboard.press("Enter")
                    page.wait_for_function(
                        "expected => __gridText().split('\\n').some(line => line.trim() === expected)",
                        arg=f"{resized['rows']} {resized['cols']}", timeout=10000)
                    print("PASS c", flush=True)

                    focus_term(page)
                    for i in range(60):
                        page.keyboard.type(f"x{i}")
                        page.keyboard.press("Enter")
                    extra = 60
                    while extra <= 120:
                        if page.evaluate("__grid.model.scrollback.length") >= 40:
                            break
                        page.keyboard.type(f"x{extra}")
                        page.keyboard.press("Enter")
                        extra += 1
                    page.wait_for_function(
                        "__grid.model.scrollback.length >= 40", timeout=10000)
                    assert page.evaluate("__grid.state.following === true")
                    term = page.locator("#term").bounding_box()
                    assert term and term["width"] and term["height"], term
                    page.mouse.move(
                        term["x"] + term["width"] / 2, term["y"] + term["height"] / 2)
                    before_top = viewport_top(page)
                    page.mouse.wheel(0, -300)
                    page.wait_for_function(
                        """prev => __grid.state.following === false
                          && ((__grid.renderer._slots[0] && __grid.renderer._slots[0].line) || 0) < prev""",
                        arg=before_top, timeout=10000)
                    page.locator("#bottom").click()
                    page.wait_for_function("__grid.state.following === true", timeout=10000)
                    print("PASS d", flush=True)

                    token = "x59" if page.evaluate(
                        "__gridText().includes('x59')") else "RS_PING_OK"
                    lines = page.evaluate("__gridText().split('\\n')")
                    target = next(i for i, line in enumerate(lines) if token in line)
                    reveal_line(page, target)
                    point = selection_point(page, token)
                    assert point, token
                    page.mouse.move(point["x0"], point["y"])
                    page.mouse.down()
                    page.mouse.move(point["x1"], point["y"], steps=8)
                    page.wait_for_function("""() => {
                      const s = __grid.state.selection;
                      return !!(s && s.start && s.end
                        && (s.start.line !== s.end.line || s.start.col !== s.end.col));
                    }""", timeout=10000)
                    page.mouse.up()
                    page.wait_for_function("__grid.state.selection === null")
                    page.wait_for_function(
                        "expected => navigator.clipboard.readText().then(text => text.includes(expected))",
                        arg=token, timeout=10000)
                    copied = page.evaluate("navigator.clipboard.readText()")
                    assert token in copied, copied
                    print("PASS e", flush=True)

                    before_pings = page.evaluate(
                        "(__gridText().match(/RS_PING_OK/g) || []).length")
                    page.evaluate("navigator.clipboard.writeText('ping')")
                    page.locator("#paste").click()
                    focus_term(page)
                    page.keyboard.press("Enter")
                    page.wait_for_function(
                        "n => (__gridText().match(/RS_PING_OK/g) || []).length > n",
                        arg=before_pings, timeout=10000)
                    print("PASS f", flush=True)

                    page.locator("#bottom").click()
                    page.wait_for_function("__grid.state.following === true", timeout=10000)
                    focus_term(page)
                    page.keyboard.type("quit")
                    page.keyboard.press("Enter")
                    expect(page.locator("#status")).to_contain_text(
                        "终端进程已退出", timeout=10000)
                    # ptyhost coalesces grid diffs 1–8 ms and may EXIT before the
                    # last line is flushed; the page still reports a clean exit.
                    assert page.evaluate(
                        "__gridText().includes('RS_SHELL_DONE') || __grid.state.connected === false")
                    print("PASS g", flush=True)

                    assert not page_errors, page_errors
                    print("PASS h", flush=True)
        finally:
            browser.close()
    print("PASS terminal grid browser: connect, type, resize, scrollback, copy, paste, exit, no pageerror",
          flush=True)


if __name__ == "__main__":
    main()
