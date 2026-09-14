#!/usr/bin/env python3
"""Legacy console raw HTTP input under the Rust `terminal_input` capability.

One isolated ptyhost runs a fixed free shell with synthetic native metadata.
Desktop: while a wheel scroll request is still pending, xterm keystrokes take
the legacy HTTP `term/send` path (raw text, no Enter semantics) and the shell's
reply renders in the same xterm. Mobile 390px: the on-screen key bar sends
named keys over HTTP. After the shell exits, the rendered tail stays, the key
bar issues no request for the vanished instance and nothing reclaims it (HTTP
403/409/410 refusals are covered by the Rust `terminal_input` suite). The
reliable-send composer stays hidden.
"""
from contextlib import contextmanager
import base64
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
from urllib.parse import urlsplit
import uuid

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, REPO, Corpus, codex_message, codex_row, isolated_server
from host_identity import request as host_request
from terminal_browser import stop

XTERM_TEXT = """() => [...T.views.values()].map(view => {
  const buffer = view.term?.buffer?.active;
  return buffer ? Array.from({length:buffer.length}, (_,i) =>
    buffer.getLine(i)?.translateToString(true) || '').join('\\n').trimEnd() : '';
}).join('\\n')"""

# Same fixed loop as the other console tests plus two named-key cases: a lone
# Tab line and an Up-arrow line, so HTTP key input is observable as shell output.
SHELL = """stty -echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  case "$command" in
    ping) printf 'RS_PING_OK\\n' ;;
    osc52) printf 'RS_OSC52_OK\\n' ;;
    "\t") printf 'RS_TAB_OK\\n' ;;
    *"[A") printf 'RS_UP_OK\\n' ;;
    quit) printf 'RS_SHELL_DONE\\n'; exit 0 ;;
    *) printf 'RS_UNKNOWN_INPUT\\n' ;;
  esac
done
"""


@contextmanager
def host(root, instance, uid):
    name = "synthetic-input-host"
    environment = {key: value for key, value in os.environ.items() if key in {"PATH", "LANG", "LC_ALL", "LC_CTYPE"}}
    environment["TERM"] = "xterm-256color"
    metadata = {"source": "codex", "sid": "synthetic-native-sid", "uid": uid, "instance_id": instance}
    process = subprocess.Popen([str(REPO / "target/debug/ptyhost"), "--dir", str(root / "host"), "run", "--name", name,
        "--cwd", str(root / "work"), "--cols", "80", "--rows", "24", "--meta", json.dumps(metadata), "--",
        shutil.which("sh"), "-c", SHELL], cwd=root / "work", env=environment,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    record = None
    try:
        deadline = time.monotonic() + 5
        path = root / "host" / (name + ".json")
        while time.monotonic() < deadline:
            assert process.poll() is None, "synthetic host exited early"
            if path.is_file():
                record = json.loads(path.read_text())
                if record["host_pid"] == process.pid:
                    try:
                        if "RS_SHELL_READY" in host_request(record, {"op": "capture", "styled": False}).get("text", ""):
                            break
                    except (OSError, json.JSONDecodeError):
                        pass
            time.sleep(.03)
        else:
            raise AssertionError("isolated shell startup timeout")
        yield process, record
    finally:
        stop(process)  # Only the exact test child, never a name or guessed PID.


def xterm_contains(page, needle, timeout=10000):
    page.wait_for_function("needle => (" + XTERM_TEXT + ")().includes(needle)", arg=needle, timeout=timeout)


def open_console(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator("#a-term")).to_be_visible()
    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false")
    if not page.locator("#termpane").is_visible():
        page.locator("#a-term").click()
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    xterm_contains(page, "RS_SHELL_READY")


def main():
    with tempfile.TemporaryDirectory(prefix="sessiondock-terminput-") as temporary:
        root = Path(temporary)
        for name in ["host", "work", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = "synthetic-native-sid"
        corpus.put(sid, "codex", [codex_row("session_meta", {"id": sid, "cwd": str(root / "work")}),
                                  codex_message("user", "Synthetic raw terminal input")], [])
        uid = corpus.uid(sid)
        native = corpus.paths[sid].read_bytes()
        instance = "synthetic-" + uuid.uuid4().hex
        with host(root, instance, uid) as (process, _), isolated_server(corpus, BINARY, host_dir=root / "host") as (base, _), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                errors, dialogs, sends, scrolls = [], [], [], []

                def observe(request):
                    path = urlsplit(request.url).path
                    if path == "/api/term/send":
                        sends.append(request.post_data_json)
                    elif path == "/api/term/scroll":
                        scrolls.append(request.post_data_json)

                context.on("request", observe)
                page = context.new_page()
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("dialog", lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
                page.goto(base, wait_until="networkidle")
                capabilities = page.evaluate("SessionDockCapabilities.config")
                assert capabilities["terminal_input"] is True and capabilities["outbox"] is False, capabilities
                open_console(page, uid)
                expect(page.locator("#composer")).to_be_hidden()

                # ---- Remote OSC 52 copy: Claude emits this after a mouse
                # selection. The embedding page writes it to the browser
                # clipboard, then ordinary Ctrl+V follows xterm's paste path.
                origin = f"{urlsplit(base).scheme}://{urlsplit(base).netloc}"
                context.grant_permissions(["clipboard-read", "clipboard-write"], origin=origin)
                page.evaluate("navigator.clipboard.writeText('sentinel')")
                write_terminal = """payload => new Promise(resolve =>
                  [...T.views.values()][0].term.write(payload, resolve))"""
                page.evaluate(write_terminal, "\x1b]52;c;?\x07")
                page.evaluate(write_terminal, "\x1b]52;c;not-base64!\x1b\\")
                assert page.evaluate("navigator.clipboard.readText()") == "sentinel"
                copied = "osc52"
                encoded = base64.b64encode(copied.encode()).decode()
                page.evaluate(write_terminal, f"\x1b]52;c;{encoded}\x1b\\")
                page.wait_for_function("expected => navigator.clipboard.readText().then(text => text === expected)", arg=copied)
                keyboard = page.locator("#termpane .xterm-helper-textarea")
                keyboard.press("Control+V")
                keyboard.press("Enter")
                xterm_contains(page, "RS_OSC52_OK")

                # ---- Desktop: keystrokes while a wheel request is pending go over HTTP.
                held = []
                page.route("**/api/term/scroll", lambda route: held.append(route))
                box = page.locator("#xterm").bounding_box()
                page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
                page.mouse.wheel(0, -120)
                deadline = time.monotonic() + 5
                while not held and time.monotonic() < deadline:
                    page.wait_for_timeout(25)
                assert held, "the legacy wheel handler must request term/scroll for a ptyhost row"
                keyboard.press_sequentially("ping")
                keyboard.press("Enter")
                assert not sends, "no send may fire before the pending scroll settles"
                for route in held:
                    route.continue_()
                page.unroute("**/api/term/scroll")
                xterm_contains(page, "RS_PING_OK")
                typed = [body.get("data") for body in sends]
                assert typed == ["p", "i", "n", "g", "\r"], typed
                for body in sends:
                    assert body["uid"] == uid and body["instance_id"] == instance and len(body["token"]) == 64, body
                    assert "text" not in body and "keys" not in body and "enter" not in body, body
                assert scrolls and scrolls[0]["name"] == page.evaluate("T.name") and scrolls[0]["up"] is True, scrolls
                assert not dialogs, dialogs
                # The wheel path is a legacy no-op for ptyhost: pos 0, xterm own buffer.
                assert page.evaluate("[...T.views.values()][0].scrollPos") == 0
                # Ordinary typing goes back to the WebSocket once nothing is pending.
                before = len(sends)
                keyboard.press_sequentially("ping")
                keyboard.press("Enter")
                page.wait_for_function("(" + XTERM_TEXT + ")().split('RS_PING_OK').length >= 3")
                assert len(sends) == before, sends[before:]

                # ---- Mobile 390px: the key bar sends named keys over HTTP.
                page.set_viewport_size({"width": 390, "height": 844})
                open_console(page, uid)
                keys = page.locator("#termpane .term-keys")
                expect(keys).to_be_visible()
                sends.clear()
                keys.locator('[data-term-key="Tab"]').click()
                deadline = time.monotonic() + 5
                while len(sends) < 1 and time.monotonic() < deadline:
                    page.wait_for_timeout(25)
                keyboard.press("Enter")
                xterm_contains(page, "RS_TAB_OK")
                keys.locator('[data-term-key="Up"]').click()
                keyboard.press("Enter")
                xterm_contains(page, "RS_UP_OK")
                assert [body.get("keys") for body in sends] == [["Tab"], ["Up"]], sends
                assert all(body["uid"] == uid and body["instance_id"] == instance for body in sends), sends
                assert not dialogs, dialogs

                # ---- After exit: the rendered tail stays, the key bar cannot type
                # into a vanished instance, and nothing reclaims or relaunches.
                claims = []
                context.on("request", lambda request: claims.append(request.url)
                           if urlsplit(request.url).path == "/api/term/claim" else None)
                keyboard.press_sequentially("quit")
                keyboard.press("Enter")
                xterm_contains(page, "RS_SHELL_DONE")
                page.wait_for_function("[...T.views.values()].some(v => v.ended)", timeout=10000)
                for _ in range(100):
                    if process.poll() is not None:
                        break
                    time.sleep(.05)
                assert process.poll() == 0, process.poll()
                page.wait_for_function("instance => !(T.list || []).some(row => row.instance_id === instance)",
                                       arg=instance, timeout=10000)
                # Batch 44 WP-E: a full exit closes the pane (Python parity);
                # the key bar is gone with it, so nothing can type, reclaim or
                # relaunch into the vanished instance.
                expect(page.locator("#termpane")).to_be_hidden()
                before = len(sends)
                page.wait_for_timeout(600)
                assert len(sends) == before and not dialogs, (sends[before:], dialogs)
                assert not claims, claims
                assert "RS_SHELL_DONE" in page.evaluate(XTERM_TEXT)
                assert page.evaluate("T.ws") is None or page.evaluate("T.ws.readyState") != 1
                assert not errors, errors
                context.close()
                assert corpus.paths[sid].read_bytes() == native
            finally:
                browser.close()
    print("PASS terminal input browser: OSC 52 clipboard + Ctrl+V, terminal_input capability, "
          "desktop HTTP text while scroll pending, mobile key bar HTTP keys, exact lease body, "
          "no input/reclaim after exit, composer hidden, native fixture unchanged")


if __name__ == "__main__":
    main()
