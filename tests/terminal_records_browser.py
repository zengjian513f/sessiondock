#!/usr/bin/env python3
"""Real recordings-page acceptance; isolated free shells only.

Build sessiondock and ptyhost first; SESSIONDOCK_TEST_WEB_DIR selects another
frontend directory. The test opens
only its fixtures' recordings page, never creates descendants, and exercises
list filtering, selection, live follow, resize, read-only viewing, exit, reload
and fit by normal browser clicks and keyboard input. Terminal assertions read
the actual imported xterm buffer through globalThis.__records and the DOM.
"""
from contextlib import ExitStack
import os
from pathlib import Path
import tempfile
import uuid
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, Corpus, isolated_server
from host_identity import guarded, host, request


PROBE = """() => {
  const OriginalWebSocket = window.WebSocket;
  window.__recordsProbe = {sockets: []};
  window.WebSocket = class extends OriginalWebSocket {
    constructor(...args) {
      super(...args);
      if (new URL(args[0], location.href).pathname !== new URL('api/term/records/attach', location.href).pathname) return;
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


def expect_selected(page, item, record_id):
    expect(item).to_have_attribute("aria-selected", "true")
    expect(page.locator('#records [aria-selected="true"]')).to_have_count(1)
    expect(page.locator("#records")).to_have_attribute("aria-activedescendant", "rec-" + record_id)
    assert parse_qs(urlsplit(page.url).query).get("id") == [record_id], page.url


def wait_recorded_size(page, cols, rows):
    page.wait_for_function(
        "size => __records.term()?.cols === size[0] && __records.term()?.rows === size[1]",
        arg=[cols, rows])
    expect(page.locator("#size")).to_have_text(f"{cols}×{rows}")


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
        archived_instance = "synthetic-" + uuid.uuid4().hex
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            with ExitStack() as cleanup:
                process, record = cleanup.enter_context(host(root, instance))
                archived_process, archived_record = cleanup.enter_context(
                    host(root, archived_instance, name="synthetic-records-archived"))
                with isolated_server(corpus, BINARY, host_dir=root / "host") as (base, _):
                    if os.environ.get('SESSIONDOCK_TEST_PREFIX'):
                        from frontend_entry_browser import prefixed_proxy
                        prefixed, target = cleanup.enter_context(prefixed_proxy())
                        target.url, base = base, prefixed.rstrip('/')
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
                    live_item = items.filter(has_text="synthetic-identity-host")
                    archived_item = items.filter(has_text="synthetic-records-archived")
                    expect(items).to_have_count(2, timeout=10000)
                    expect(live_item.locator(".badge")).to_have_text("进行中")
                    expect(archived_item.locator(".badge")).to_have_text("进行中")
                    expect(page.locator("#empty")).to_be_visible()
                    record_id = live_item.get_attribute("data-id")
                    archived_id = archived_item.get_attribute("data-id")
                    assert record_id and archived_id and record_id != archived_id

                    # Exit after server startup: its startup cleanup must not remove
                    # the second host's synthetic agent metadata recording.
                    request(archived_record, guarded(archived_instance, {"op": "send", "text": "next\rquit\r"}))
                    archived_process.wait(timeout=8)
                    page.locator("#refresh").click()
                    expect(archived_item.locator(".badge")).to_have_text("已结束", timeout=8000)
                    page.locator("#live-only").check()
                    expect(items).to_have_count(1)
                    expect(archived_item).to_have_count(0)
                    expect(live_item.locator(".badge")).to_have_text("进行中")
                    print("PASS a: list and live-only filter", flush=True)

                    live_item.click()
                    expect_selected(page, live_item, record_id)
                    expect(page.locator("#status")).to_contain_text("进行中")
                    wait_recorded_size(page, 80, 24)
                    wait_buffer(page, "RS_SHELL_READY")
                    assert page.evaluate("__records.socket()?.readyState === WebSocket.OPEN")
                    print("PASS b", flush=True)

                    request(record, guarded(instance, {"op": "send", "text": "ping\r"}))
                    wait_buffer(page, "RS_PING_OK", timeout=10000)
                    print("PASS c", flush=True)

                    page.locator("#live-only").uncheck()
                    expect(items).to_have_count(2)
                    archived_item.click()
                    expect_selected(page, archived_item, archived_id)
                    wait_buffer(page, "RS_AFTER_RESTART")
                    wait_buffer(page, "RS_SHELL_DONE")
                    expect(page.locator("#status")).to_contain_text("已结束")
                    assert "RS_PING_OK" not in page.evaluate(XTERM_TEXT)
                    page.wait_for_function("__recordsProbe.sockets[0]?.close != null")

                    # Filtering the selected ended row keeps its replay visible;
                    # normal list keyboard navigation opens the remaining live row.
                    page.locator("#live-only").check()
                    expect(items).to_have_count(1)
                    expect(page.locator("#records[aria-activedescendant]")).to_have_count(0)
                    wait_buffer(page, "RS_SHELL_DONE")
                    page.locator("#records").focus()
                    page.keyboard.press("ArrowDown")
                    expect_selected(page, live_item, record_id)
                    wait_buffer(page, "RS_PING_OK")
                    assert "RS_AFTER_RESTART" not in page.evaluate(XTERM_TEXT)
                    assert "RS_SHELL_DONE" not in page.evaluate(XTERM_TEXT)
                    page.wait_for_function("__recordsProbe.sockets[1]?.close != null")
                    page.locator("#live-only").uncheck()
                    expect(items).to_have_count(2)
                    expect_selected(page, live_item, record_id)
                    print("PASS selection: click, filtered keyboard, isolated replay, old sockets closed", flush=True)

                    resized = request(record, {"op": "resize", "cols": 100, "rows": 30})
                    if resized.get("ok") is False:
                        resized = request(record, guarded(instance, {"op": "resize", "cols": 100, "rows": 30}))
                    assert resized.get("ok") is True, resized
                    wait_recorded_size(page, 100, 30)
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
                    assert page.evaluate("__recordsProbe.sockets.every(socket => socket.sent.length === 0)"), \
                        "read-only selection or keyboard input sent data to its socket"
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
                    expect(live_item.locator(".badge")).to_have_text("已结束", timeout=8000)
                    print("PASS f", flush=True)

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

                    expect_selected(page, live_item, record_id)
                    wait_recorded_size(page, 100, 30)
                    page.locator("#fit").click()
                    expect(page.locator("#fit")).to_have_attribute("aria-pressed", "true")
                    expect(page.locator("#xterm")).to_have_class("fit")
                    page.wait_for_function("__records.term()?.cols !== 100", timeout=2000)
                    expect(page.locator("#size")).to_have_text("100×30")
                    page.reload(wait_until="networkidle")
                    wait_buffer(page, "RS_SHELL_DONE")
                    expect_selected(page, live_item, record_id)
                    expect(page.locator("#fit")).to_have_attribute("aria-pressed", "true")
                    expect(page.locator("#xterm")).to_have_class("fit")
                    page.wait_for_function("__records.term()?.cols !== 100", timeout=2000)
                    expect(page.locator("#size")).to_have_text("100×30")
                    page.locator("#fit").click()
                    expect(page.locator("#fit")).to_have_attribute("aria-pressed", "false")
                    expect(page.locator("#xterm")).not_to_have_class("fit")
                    wait_recorded_size(page, 100, 30)
                    page.reload(wait_until="networkidle")
                    wait_buffer(page, "RS_SHELL_DONE")
                    expect(page.locator("#fit")).to_have_attribute("aria-pressed", "false")
                    expect(page.locator("#xterm")).not_to_have_class("fit")
                    wait_recorded_size(page, 100, 30)
                    print("PASS h: fit preference reload and exact recorded size restored", flush=True)

                    assert not page_errors, page_errors
                    assert page.evaluate("__recordsProbe.sockets.every(socket => socket.sent.length === 0)"), \
                        "read-only playback sent data to its socket"
                    print("PASS i", flush=True)
        finally:
            browser.close()
    print("PASS terminal records browser: filter, click/keyboard selection, live follow, resize, read-only, exit, reload, fit preference, exact dimensions, no pageerror",
          flush=True)


if __name__ == "__main__":
    main()
