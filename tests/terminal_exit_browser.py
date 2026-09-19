#!/usr/bin/env python3
"""Real legacy console exit/error acceptance; isolated free shell and native fixture.

Build sessiondock and ptyhost first. The test opens only its fixture's slave
descriptor, never creates descendants, and exercises console controls by normal
browser clicks and keyboard input. WebSocket instrumentation only observes wire
events; terminal assertions read the actual xterm/grid buffer and DOM.
"""
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
from urllib.parse import urlsplit
import uuid

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
from host_identity import host


PROBE = """() => {
  const OriginalWebSocket = window.WebSocket;
  window.__exitProbe = {sockets: []};
  window.WebSocket = class extends OriginalWebSocket {
    constructor(...args) {
      super(...args);
      if (new URL(args[0], location.href).pathname !== '/api/term/attach') return;
      const state = {close:null, sent:[], received:''};
      this.__exitState = state;
      __exitProbe.sockets.push(state);
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
      if (this.__exitState) {
        const binary = typeof data !== 'string';
        const text = binary ? new TextDecoder().decode(data) : data;
        this.__exitState.sent.push({binary, text, afterClose:!!this.__exitState.close});
      }
      return super.send(data);
    }
  };
}"""

XTERM_TEXT = """() => [...T.views.values()].map(view => {
  const buffer = view.term?.buffer?.active;
  return buffer ? Array.from({length:buffer.length}, (_,i) =>
    buffer.getLine(i)?.translateToString(true) || '').join('\\n').trimEnd() : '';
}).join('\\n')"""

SNAPSHOT = """uid => ({
  errors: ConsoleUI.errors.get(uid) || '',
  paneVisible: !document.querySelector('#termpane').classList.contains('hidden'),
  inlineNotice: document.querySelector('#term-output-notice').textContent,
  buttonUnavailable: document.querySelector('#a-term').dataset.unavailable,
  text: [...T.views.values()].map(view => {
    const buffer = view.term?.buffer?.active;
    return buffer ? Array.from({length:buffer.length}, (_,i) =>
      buffer.getLine(i)?.translateToString(true) || '').join('\\n').trimEnd() : '';
  }).join('\\n'),
  sockets: __exitProbe.sockets,
  reconnectPending: [...T.views.values()].some(view => !!view.reconnectTimer),
})"""


def scenario(root, browser, incomplete, renderer):
    for name in ["host", "work", "claude", "codex", "grok"]:
        (root / name).mkdir(mode=0o700)
    corpus = Corpus(root)
    sid = "synthetic-native-sid"
    corpus.put(sid, "codex", [
        codex_row("session_meta", {"id":sid, "cwd":str(root / "work")}),
        codex_message("user", "Synthetic terminal exit acceptance"),
    ], [])
    uid = corpus.uid(sid)
    native = corpus.paths[sid].read_bytes()
    instance = "synthetic-" + uuid.uuid4().hex
    tty_file = root / "fixture.tty"
    with host(root, instance, uid=uid, tty_file=tty_file) as (process, _), ExitStack() as cleanup:
        slave = None
        if incomplete:
            descriptor = os.open(tty_file.read_text().strip(), os.O_WRONLY | os.O_NOCTTY)
            slave = cleanup.enter_context(os.fdopen(descriptor, "wb", buffering=0))
        with isolated_server(corpus, BINARY, host_dir=root / "host") as (base, _):
            context = browser.new_context(viewport={"width":1280,"height":900}, service_workers="block")
            context.add_init_script(
                "localStorage.setItem('sessiondock.consoleRenderer', JSON.stringify(%s))" % json.dumps(renderer))
            cleanup.callback(context.close)
            context.route("**/*", lambda route: route.continue_()
                          if route.request.url.startswith(base + "/") else route.abort())
            claims, page_errors, dialogs = [], [], []

            def observe_request(request):
                if urlsplit(request.url).path == "/api/term/claim":
                    payload = request.post_data_json
                    claims.append({key:payload.get(key) for key in ["uid", "instance_id", "force"]})

            context.on("request", observe_request)
            context.add_init_script("(" + PROBE + ")()")
            page = context.new_page()
            page.on("pageerror", lambda error: page_errors.append(str(error)))
            page.on("dialog", lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
            page.goto(base, wait_until="networkidle")
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            expect(page.locator("#msgs")).to_contain_text("Synthetic terminal exit acceptance")
            expect(page.locator("#a-term")).to_be_visible()
            expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false")
            page.locator("#a-term").click()
            expect(page.locator("#termpane")).to_be_visible()
            page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
            page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_SHELL_READY')")
            assert page.evaluate("[...T.views.values()].every(view => view.grid === %s)" % ("true" if renderer == "grid" else "false"))
            assert len(claims) == 1 and claims[0]["uid"] == uid and claims[0]["instance_id"] == instance, claims
            keyboard = page.locator("#termpane .xterm-helper-textarea")
            keyboard.press_sequentially("quit")
            keyboard.press("Enter")
            page.wait_for_function("__exitProbe.sockets[0].received.includes('RS_SHELL_DONE')")
            if slave is not None:
                # Owned shell has written its final output; retain its slave and
                # add delayed terminal output without spawning any descendants.
                page.wait_for_timeout(600)
                slave.write(b"RS_DELAYED_FINAL_TAIL\n")
                page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_DELAYED_FINAL_TAIL')")
            page.wait_for_function("__exitProbe.sockets[0].close !== null", timeout=10000)
            # Let normal live polling and the original 500 ms reconnect delay run.
            page.wait_for_timeout(1500)
            observed = page.evaluate(SNAPSHOT, uid)
            notice = page.locator('#termpane #term-output-notice')
            if incomplete and renderer == "grid":
                expect(notice).to_be_visible()
                expect(notice).to_contain_text("PTY drain timeout")
                assert notice.evaluate("el => { const r = el.getBoundingClientRect(); return document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) === el; }")
            else:
                expect(notice).to_be_hidden()
            auto_dialogs = list(dialogs)
            automatic_claims = len(claims)
            expect(page.locator("#a-term")).to_be_visible()
            assert page.locator("#a-term").get_attribute("disabled") is None
            page.locator("#a-term").hover()
            observed["toast"] = page.locator("#console-toast").text_content()
            page.mouse.move(0, 0)
            page.locator("#a-term").focus()
            observed["focusToast"] = page.locator("#console-toast").text_content()
            before_click = len(dialogs)
            page.locator("#a-term").click()
            page.wait_for_timeout(100)
            observed["clickDialogs"] = dialogs[before_click:]
            observed["autoDialogs"] = auto_dialogs
            observed["automaticClaims"] = automatic_claims
            observed["claimsAfterClick"] = len(claims)
            observed["pageErrors"] = page_errors
            if incomplete:
                page.set_viewport_size({"width":390,"height":844})
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                expect(page.locator("#a-term")).to_be_visible()
                assert page.locator("#a-term").get_attribute("disabled") is None
                before_click = len(dialogs)
                page.locator("#a-term").click()
                page.wait_for_timeout(100)
                observed["mobileClickDialogs"] = dialogs[before_click:]
                observed["claimsAfterMobileClick"] = len(claims)
            print(json.dumps({"renderer": renderer, "scenario":"incomplete" if incomplete else "normal", **observed}, ensure_ascii=False), flush=True)

            problems = []
            def check(ok, message):
                if not ok:
                    problems.append(message)

            close = observed["sockets"][0]["close"]
            check(close["code"] == (1011 if incomplete else 1000), "wrong WebSocket exit code")
            check(automatic_claims == 1 and len(claims) == 1, "exit or explanation click reclaimed ownership")
            check(len(observed["sockets"]) == 1 and not observed["reconnectPending"], "exit scheduled an automatic reconnect")
            check(not auto_dialogs, "exit/reconnect produced an unsolicited alert")
            check(not page_errors, "browser JavaScript error")
            check(observed["buttonUnavailable"] == "true", "exited console is not gray/unavailable")
            check(observed["errors"] and observed["errors"] in observed["toast"], "hover hides the exit explanation")
            check(observed["errors"] and observed["errors"] in observed["focusToast"], "focus hides the exit explanation")
            outgoing = [event for state in observed["sockets"] for event in state["sent"] if event["binary"]]
            check("".join(event["text"] for event in outgoing) == "quit\r", "unexpected terminal input beyond the user's quit")
            check(not any(event["afterClose"] for event in outgoing), "terminal sent data after close")
            check("RS_SHELL_DONE" in observed["text"], "final shell output was removed from xterm")
            if incomplete:
                why = "PTY drain timeout"
                check(why in close["reason"], "incomplete exit lost the specific wire reason")
                check(why in observed["errors"], "ConsoleUI.errors lost the incomplete reason")
                check(observed["paneVisible"] and "RS_DELAYED_FINAL_TAIL" in observed["text"], "delayed tail is no longer visible")
                check(why in (observed["inlineNotice"] if renderer == "grid" else observed["text"]),
                      "console does not visibly explain incomplete output")
                check(any(why in dialog for dialog in observed["clickDialogs"]), "console click hides the specific incomplete reason")
                check(any(why in dialog for dialog in observed["mobileClickDialogs"]), "mobile console click hides the specific incomplete reason")
            else:
                message = observed["errors"].lower() + observed["text"].lower()
                check(not any(word in message for word in ["incomplete", "truncated", "输出不完整", "截断"]), "normal EOF was mislabeled incomplete")
            check(corpus.paths[sid].read_bytes() == native, "native fixture was modified")
            check(process.wait(timeout=5) == 0, "synthetic host did not exit cleanly")
            if not incomplete:
                # A new host with the same name and native UID is a different
                # instance. Live polling may offer it, but must not restore the
                # retired instance's saved layout/lease without a user click.
                replacement = "synthetic-" + uuid.uuid4().hex
                with host(root, replacement, uid=uid) as (new_process, _):
                    page.wait_for_function("instance => T.list.some(row => row.instance_id === instance)", arg=replacement)
                    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false")
                    page.wait_for_timeout(600)
                    check(len(claims) == 1, "replacement inherited the exited instance's claim")
                    check(page.evaluate("uid => !T.ended.has(uid) && !ConsoleUI.errors.has(uid)", uid), "replacement retained stale exit diagnostic")
                    page.locator("#a-term").click()
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                    page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_SHELL_READY')")
                    check(len(claims) == 2 and claims[-1]["instance_id"] == replacement, "manual replacement claim is not pinned to the new instance")
                    check("RS_SHELL_DONE" not in page.evaluate(XTERM_TEXT), "replacement reused the exited xterm buffer")
                    keyboard = page.locator("#termpane .xterm-helper-textarea")
                    keyboard.press_sequentially("ping")
                    keyboard.press("Enter")
                    page.wait_for_function("(" + XTERM_TEXT + ")().includes('RS_PING_OK')")
                    check(new_process.poll() is None and not page_errors, "replacement console failed")
                    check(corpus.paths[sid].read_bytes() == native, "replacement modified native fixture")
            return problems


def main():
    if os.name != "posix":
        raise SystemExit("This isolated real-shell acceptance currently requires POSIX.")
    failures = []
    with tempfile.TemporaryDirectory(prefix="sessiondock-terminal-exit-") as temporary, sync_playwright() as playwright:
        launch = {"headless":True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            for renderer in ["xterm", "grid"]:
                for incomplete in [True, False]:
                    root = Path(temporary) / (renderer + ("-incomplete" if incomplete else "-normal"))
                    root.mkdir(mode=0o700)
                    failures.extend(f"{renderer}: {problem}" for problem in scenario(root, browser, incomplete, renderer))
        finally:
            browser.close()
    assert not failures, "; ".join(failures)
    print("PASS terminal exit browser: xterm and grid, complete UID/instance, retained tail and visible error, hover/focus/click explanation, no automatic reclaim/input, normal EOF, explicit manual replacement, native fixture unchanged")


if __name__ == "__main__":
    main()
