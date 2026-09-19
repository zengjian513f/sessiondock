#!/usr/bin/env python3
"""Real legacy recordings-page acceptance; isolated free shell only.

Build sessiondock and ptyhost first. The test opens only its fixture's
recordings page, never creates descendants, and exercises list, live follow,
resize, read-only viewing, exit, reload and fit by normal browser clicks and
keyboard input. Terminal assertions read the actual imported xterm buffer
through globalThis.__records and the DOM.
"""
from contextlib import ExitStack
import os
from pathlib import Path
import tempfile
import uuid

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, Corpus, isolated_server
from host_identity import guarded, host, request


PROBE = """() => {
  const OriginalWebSocket = window.WebSocket;
  window.__recordsProbe = {sockets: []};
  window.WebSocket = class extends OriginalWebSocket {
    constructor(...args) {
      super(...args);
      if (new URL(args[0], location.href).pathname !== '/api/term/records/attach') return;
      const state = {close:null, sent:[], received:''};
      this.__recordsState = state;
      __recordsProbe.sockets.push(state);
      const decoder = new TextDecoder();
      this.addEventListener('message', event => {
        if (event.data instanceof ArrayBuffer)
          state.received += decoder.decode(new Uint8Array(event.data), {stream:true});
      });
      this.addEventListener('close', event => {
        state.close = {code:event.code, reason:event.reason};
      });
    }
    send(data) {
      if (this.__recordsState) {
        const binary = typeof data !== 'string';
        const text = binary ? new TextDecoder().decode(data) : data;
        this.__recordsState.sent.push({binary, text, afterClose:!!this.__recordsState.close});
      }
      return super.send(data);
    }
  };
}"""

XTERM_TEXT = """() => {
  const buffer = __records.term()?.buffer?.active;
  return buffer ? Array.from({length:buffer.length}, (_,i) =>
    buffer.getLine(i)?.translateToString(true) || '').join('\\n').trimEnd() : '';
}"""


def wait_buffer(page, needle, timeout=10000):
    page.wait_for_function(
        "text => (" + XTERM_TEXT + ")().includes(text)", arg=needle, timeout=timeout)


def main():
    if os.name != "posix":
        raise SystemExit("This isolated real-shell acceptance currently requires POSIX.")
    ptyhost = BINARY.with_name("ptyhost")
    missing = [str(path) for path in (BINARY, ptyhost) if not path.is_file()]
    if missing:
        raise SystemExit("missing built binaries (do not cargo build here): " + ", ".join(missing))
    page_errors = []
    with tempfile.TemporaryDirectory(prefix="sessiondock-terminal-records-") as temporary, sync_playwright() as playwright:
        root = Path(temporary)
        for name in ["host", "work", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        instance = "synthetic-" + uuid.uuid4().hex
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            with host(root, instance) as (process, record), ExitStack() as cleanup:
                with isolated_server(corpus, BINARY, host_dir=root / "host") as (base, _):
                    context = browser.new_context(
                        viewport={"width": 1280, "height": 900}, service_workers="block")
                    cleanup.callback(context.close)
                    context.route("**/*", lambda route: route.continue_()
                                  if route.request.url.startswith(base + "/") else route.abort())
                    context.add_init_script("(" + PROBE + ")()")
                    page = context.new_page()
                    page.on("pageerror", lambda error: page_errors.append(str(error)))
                    page.goto(base + "/records.html", wait_until="networkidle")
                    items = page.locator("#records li[role=option]")
                    expect(items).to_have_count(1, timeout=10000)
                    expect(items).to_contain_text("synthetic-identity-host")
                    expect(items.locator(".badge")).to_have_text("进行中")
                    print("PASS a", flush=True)

                    items.click()
                    expect(page.locator("#status")).to_contain_text("进行中")
                    expect(page.locator("#size")).to_contain_text("80×24")
                    wait_buffer(page, "RS_SHELL_READY")
                    assert page.evaluate("__records.socket()?.readyState === WebSocket.OPEN")
                    print("PASS b", flush=True)

                    request(record, guarded(instance, {"op": "send", "text": "ping\r"}))
                    wait_buffer(page, "RS_PING_OK", timeout=10000)
                    print("PASS c", flush=True)

                    resized = request(record, {"op": "resize", "cols": 100, "rows": 30})
                    if resized.get("ok") is False:
                        resized = request(record, guarded(instance, {"op": "resize", "cols": 100, "rows": 30}))
                    assert resized.get("ok") is True, resized
                    page.wait_for_function(
                        "__records.term()?.cols === 100 && __records.term()?.rows === 30")
                    expect(page.locator("#size")).to_contain_text("100×30")
                    print("PASS d", flush=True)

                    page.locator("#xterm .xterm-helper-textarea").focus()
                    page.keyboard.type("quit")
                    page.keyboard.press("Enter")
                    page.wait_for_timeout(500)
                    captured = request(record, {"op": "capture", "styled": False}).get("text", "")
                    assert "RS_UNKNOWN_INPUT" not in captured, captured
                    assert "RS_SHELL_DONE" not in captured, captured
                    assert "RS_SHELL_DONE" not in page.evaluate(XTERM_TEXT)
                    assert process.poll() is None
                    print("PASS e", flush=True)

                    request(record, guarded(instance, {"op": "send", "text": "quit\r"}))
                    page.wait_for_function(
                        """() => {
                          const text = (""" + XTERM_TEXT + """)();
                          const status = document.querySelector('#status')?.textContent || '';
                          return text.includes('RS_SHELL_DONE')
                            && status.includes('已结束')
                            && status.includes('退出码 0')
                            && status.includes('回放完成');
                        }""",
                        timeout=15000)
                    page.locator("#refresh").click()
                    expect(items.locator(".badge")).to_have_text("已结束", timeout=8000)
                    print("PASS f", flush=True)

                    record_id = items.get_attribute("data-id")
                    assert record_id
                    page.goto(base + "/records.html?id=" + record_id, wait_until="networkidle")
                    page.wait_for_function(
                        """() => {
                          const text = (""" + XTERM_TEXT + """)();
                          const status = document.querySelector('#status')?.textContent || '';
                          return text.includes('RS_SHELL_READY')
                            && text.includes('RS_PING_OK')
                            && text.includes('RS_SHELL_DONE')
                            && status.includes('已结束');
                        }""")
                    print("PASS g", flush=True)

                    page.wait_for_function(
                        "__records.term()?.cols === 100 && __records.term()?.rows === 30")
                    page.locator("#fit").click()
                    expect(page.locator("#fit")).to_have_attribute("aria-pressed", "true")
                    page.wait_for_function("__records.term()?.cols !== 100", timeout=2000)
                    page.locator("#fit").click()
                    expect(page.locator("#fit")).to_have_attribute("aria-pressed", "false")
                    print("PASS h", flush=True)

                    assert not page_errors, page_errors
                    print("PASS i", flush=True)
        finally:
            browser.close()
    print("PASS terminal records browser: list, live follow, resize, read-only, exit, reload, fit, no pageerror",
          flush=True)


if __name__ == "__main__":
    main()
