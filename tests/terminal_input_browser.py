#!/usr/bin/env python3
"""Legacy console raw HTTP input under the Rust `terminal_input` capability.

One isolated ptyhost runs a fixed free shell with synthetic native metadata.
Desktop: ptyhost wheel input stays in the grid console and subsequent keystrokes
use the WebSocket; the shell reply renders in the same console. Mobile 390px: the on-screen key bar sends
named keys over HTTP. After the shell exits, its final output arrives and the
pane closes; no input or reclaim reaches the vanished instance (HTTP
403/409/410 refusals are covered by the Rust `terminal_input` suite). The
reliable-send composer stays hidden. Console file paste: ignored with a hint
while the 设置 › 功能 switch is off; enabled, a clipboard image and a two-file
paste land in `<cwd>/sessiondock_attachments/<batch>/` and their relative
paths are typed into the shell as one bracketed paste.
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

from history_fixtures import BINARY, REPO, Corpus, codex_message, codex_row, isolated_server
from host_identity import request as host_request
from terminal_browser import stop
from popups import on_popup  # noqa: E402

XTERM_TEXT = """() => [...T.views.values()].map(view => {
  const buffer = view.term?.buffer?.active;
  return buffer ? Array.from({length:buffer.length}, (_,i) =>
    buffer.getLine(i)?.translateToString(true) || '').join('\\n').trimEnd() : '';
}).join('\\n')"""

PNG_B64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="

# Same fixed loop as the other console tests plus two named-key cases: a lone
# Tab line and an Up-arrow line, so HTTP key input is observable as shell output.
SHELL = """stty -echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  case "$command" in
    ping) printf 'RS_PING_OK\\n' ;;
    "\t") printf 'RS_TAB_OK\\n' ;;
    *"[A") printf 'RS_UP_OK\\n' ;;
    ./sessiondock_attachments/*) printf 'RS_PASTE_PATH %s\\n' "$command" ;;
    quit) printf 'RS_SHELL_DONE\\n'; exit 0 ;;
    *) printf 'RS_UNKNOWN_INPUT\\n' ;;
  esac
done
"""


@contextmanager
def host(root, instance, uid, history=None):
    name = "synthetic-input-host"
    environment = {key: value for key, value in os.environ.items() if key in {"PATH", "LANG", "LC_ALL", "LC_CTYPE"}}
    environment["TERM"] = "xterm-256color"
    metadata = {"source": "codex", "sid": "synthetic-native-sid", "uid": uid, "instance_id": instance}
    process = subprocess.Popen([str(REPO / "target/debug/ptyhost"), "--dir", str(root / "host"), "run", "--name", name,
        "--cwd", str(root / "work"), "--cols", "80", "--rows", "24", "--meta", json.dumps(metadata),
        *(["--history", str(history)] if history is not None else []), "--",
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


def open_console(page, uid, history=True):
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator("#a-term")).to_be_visible()
    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false")
    if not page.locator("#termpane").is_visible():
        page.locator("#a-term").click()
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    if history:
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
        with host(root, instance, uid) as (process, _), \
                isolated_server(corpus, BINARY, host_dir=root / "host", file_roots=(root / "work",),
                                file_write_roots=(root / "work",)) as (base, _), sync_playwright() as playwright:
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
                dialog_action = {"accept": True}
                on_popup(page, lambda dialog: (dialogs.append(dialog.message),
                                                  dialog.accept() if dialog_action["accept"] else dialog.dismiss()))
                page.goto(base, wait_until="networkidle")
                capabilities = page.evaluate("SessionDockCapabilities.config")
                assert capabilities["terminal_input"] is True and "outbox" not in capabilities, capabilities
                open_console(page, uid)
                expect(page.locator("#composer")).to_be_hidden()

                origin = f"{urlsplit(base).scheme}://{urlsplit(base).netloc}"
                context.grant_permissions(["clipboard-read", "clipboard-write"], origin=origin)
                keyboard = page.locator("#termpane .xterm-helper-textarea")
                # Edge's inline Compose button and text prediction stay off the IME textarea.
                assert keyboard.get_attribute("writingsuggestions") == "false"

                # ---- Desktop: ptyhost scrolling never enters the legacy HTTP path.
                box = page.locator("#xterm").bounding_box()
                page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
                page.mouse.wheel(0, -120)
                keyboard.press_sequentially("ping")
                keyboard.press("Enter")
                xterm_contains(page, "RS_PING_OK")
                assert not scrolls, scrolls
                assert not sends, sends
                assert not dialogs, dialogs
                assert page.evaluate("[...T.views.values()][0].scrollPos") == 0

                # ---- Console file paste. Off by default: a pasted image is
                # ignored with a hint and no upload. Enabled from 设置 › 功能,
                # a clipboard image lands in <cwd>/sessiondock_attachments/<batch>/
                # and its relative path is typed into the shell; two files in one
                # paste share a batch and a name with a space is escaped.
                uploads = []
                context.on("request", lambda request: uploads.append(request.url)
                           if urlsplit(request.url).path == "/api/session/attachment" else None)
                wrote = page.evaluate("""async b64 => {
                  try {
                    const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
                    await navigator.clipboard.write([new ClipboardItem({'image/png': new Blob([bytes], {type: 'image/png'})})]);
                    return true;
                  } catch (error) { return String(error); }
                }""", PNG_B64)

                def paste(files):
                    keyboard.focus()
                    if files is None and wrote is True:
                        keyboard.press("Control+V")      # the real clipboard image
                        return
                    page.evaluate("""files => {
                      const transfer = new DataTransfer();
                      for (const [name, type, b64] of files) {
                        const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
                        transfer.items.add(new File([bytes], name, {type}));
                      }
                      const target = document.querySelector('#termpane .xterm-helper-textarea');
                      target.dispatchEvent(new ClipboardEvent('paste', {clipboardData: transfer, bubbles: true, cancelable: true}));
                    }""", files or [["image.png", "image/png", PNG_B64]])

                def wait_uploads(count):
                    deadline = time.monotonic() + 10
                    while len(uploads) < count and time.monotonic() < deadline:
                        page.wait_for_timeout(50)
                    assert len(uploads) == count, uploads

                assert page.evaluate("localStorage.getItem('sessiondock.consolePasteFiles')") is None
                paste(None)
                page.wait_for_timeout(400)
                assert not uploads and not dialogs, (uploads, dialogs)
                expect(page.locator("#term-output-notice")).to_contain_text("控制台粘贴文件")
                assert not (root / "work" / "sessiondock_attachments").exists()
                expect(page.locator('#float-stack > :visible')).to_have_count(0)
                assert page.locator('#term-output-notice').evaluate("""node => {
                  const notice = node.getBoundingClientRect();
                  const screen = document.querySelector('#xterm').getBoundingClientRect();
                  return notice.top >= screen.bottom - 1;
                }""")
                expect(page.locator('#term-output-notice')).to_be_hidden(timeout=5000)
                # A denied clipboard read stays beside terminal input and clears on its own.
                page.evaluate("() => { window.savedClipboardRead = navigator.clipboard.readText; navigator.clipboard.readText = async () => { throw new Error('fixture denied'); }; }")
                page.locator('.grid-canvas:visible').click(button='right', position={'x':30, 'y':12})
                page.get_by_role('menuitem', name='粘贴', exact=True).click()
                expect(page.locator('#term-output-notice')).to_contain_text('无法读取剪贴板')
                expect(page.locator('#float-stack > :visible')).to_have_count(0)
                expect(page.locator('dialog.app-popup')).to_have_count(0)
                expect(page.locator('#term-output-notice')).to_be_hidden(timeout=5000)
                page.evaluate('() => { navigator.clipboard.readText = savedClipboardRead; }')


                page.locator("#settings").click()
                page.locator('.settings-tab[data-tab="features"]').click()
                toggle = page.locator("#setting-console-paste-files")
                expect(toggle).to_be_visible()
                toggle.check()
                page.locator("#settings-dialog .modal-close").click()
                expect(page.locator("#settings-dialog")).to_be_hidden()
                assert page.evaluate("localStorage.getItem('sessiondock.consolePasteFiles')") == "true"

                sends.clear()
                paste(None)
                wait_uploads(1)
                deadline = time.monotonic() + 10
                while not sends and time.monotonic() < deadline:
                    page.wait_for_timeout(50)
                assert sends and sends[-1].get("paste") == "./sessiondock_attachments/1/image.png ", sends
                assert sends[-1]["uid"] == uid and sends[-1]["instance_id"] == instance, sends
                saved = root / "work" / "sessiondock_attachments" / "1" / "image.png"
                # Chromium re-encodes an image written to the real clipboard, so
                # only the synthetic event keeps the exact bytes.
                assert saved.read_bytes()[:8] == b"\x89PNG\r\n\x1a\n", saved
                assert wrote is True or saved.read_bytes() == base64.b64decode(PNG_B64), saved
                keyboard.press("Enter")
                xterm_contains(page, "RS_PASTE_PATH ./sessiondock_attachments/1/image.png")
                expect(page.locator("#term-output-notice")).to_be_hidden()

                sends.clear()
                paste([["shot 2.png", "image/png", PNG_B64],
                       ["notes.txt", "text/plain", base64.b64encode(b"pasted notes\n").decode()]])
                wait_uploads(3)
                deadline = time.monotonic() + 10
                while not sends and time.monotonic() < deadline:
                    page.wait_for_timeout(50)
                assert sends and sends[-1].get("paste") == \
                    "./sessiondock_attachments/2/shot\\ 2.png ./sessiondock_attachments/2/notes.txt ", sends
                assert (root / "work" / "sessiondock_attachments" / "2" / "shot 2.png").read_bytes() == base64.b64decode(PNG_B64)
                assert (root / "work" / "sessiondock_attachments" / "2" / "notes.txt").read_bytes() == b"pasted notes\n"
                keyboard.press("Enter")
                xterm_contains(page, "RS_PASTE_PATH ./sessiondock_attachments/2/shot\\ 2.png ./sessiondock_attachments/2/notes.txt")
                assert not dialogs, dialogs
                # More than five files, or over 50 MB, asks first. Dismissed:
                # nothing is uploaded or typed. Accepted: one batch of six.
                six = [[f"shot-{i}.png", "image/png", PNG_B64] for i in range(6)]
                sends.clear()
                dialog_action["accept"] = False
                paste(six)
                assert dialogs[-1] == "粘贴了 6 个文件，共 1 KB。继续？", dialogs
                page.evaluate("""() => {
                  const transfer = new DataTransfer();
                  transfer.items.add(new File([new Uint8Array(51 * 1048576)], 'big.bin', {type: 'application/octet-stream'}));
                  document.querySelector('#termpane .xterm-helper-textarea')
                    .dispatchEvent(new ClipboardEvent('paste', {clipboardData: transfer, bubbles: true, cancelable: true}));
                }""")
                assert dialogs[-1] == "粘贴了 1 个文件，共 51.0 MB。继续？", dialogs
                page.wait_for_timeout(400)
                assert len(uploads) == 3 and not sends, (uploads, sends)
                dialog_action["accept"] = True
                paste(six)
                wait_uploads(9)
                deadline = time.monotonic() + 10
                while not sends and time.monotonic() < deadline:
                    page.wait_for_timeout(50)
                assert sends and sends[-1].get("paste") == "".join(f"./sessiondock_attachments/3/shot-{i}.png " for i in range(6)), sends
                assert sorted(path.name for path in (root / "work" / "sessiondock_attachments" / "3").iterdir()) == \
                    sorted(f"shot-{i}.png" for i in range(6))
                keyboard.press("Enter")
                xterm_contains(page, "RS_PASTE_PATH ./sessiondock_attachments/3/shot-0.png")
                dialogs.clear()
                print("console paste via", "real clipboard" if wrote is True else f"synthetic ClipboardEvent ({wrote})")

                # ---- Mobile 390px: the key bar sends named keys over HTTP.
                page.set_viewport_size({"width": 390, "height": 844})
                # A re-attached grid view starts from the host's current screen;
                # output from before the reattach is not replayed into it.
                open_console(page, uid, history=False)
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

                # A complete AI-console exit disposes the view. Observe the
                # final bytes before sending quit, instead of racing that
                # disposal with a poll of an already-removed xterm buffer.
                claims = []
                context.on("request", lambda request: claims.append(request.url)
                           if urlsplit(request.url).path == "/api/term/claim" else None)
                page.evaluate("""() => {
                    window.inputExitText = '';
                    const decoder = new TextDecoder();
                    T.ws.addEventListener('message', event => {
                        if (event.data instanceof ArrayBuffer)
                            inputExitText += decoder.decode(event.data, {stream:true});
                    });
                }""")
                keyboard.press_sequentially("quit")
                keyboard.press("Enter")
                page.wait_for_function("inputExitText.includes('RS_SHELL_DONE')")
                page.wait_for_function("uid => T.ended.has(uid) && T.views.size === 0", arg=uid, timeout=10000)
                for _ in range(100):
                    if process.poll() is not None:
                        break
                    time.sleep(.05)
                assert process.poll() == 0, process.poll()
                page.wait_for_function("instance => !(T.list || []).some(row => row.instance_id === instance)",
                                       arg=instance, timeout=10000)
                # A full exit closes the pane;
                # the key bar is gone with it, so nothing can type, reclaim or
                # relaunch into the vanished instance.
                expect(page.locator("#termpane")).to_be_hidden()
                before = len(sends)
                page.wait_for_timeout(600)
                assert len(sends) == before and not dialogs, (sends[before:], dialogs)
                assert not claims, claims
                assert page.evaluate("uid => T.ended.has(uid)", uid)
                assert page.evaluate("T.views.size") == 0
                assert page.evaluate("T.ws") is None or page.evaluate("T.ws.readyState") != 1
                assert not errors, errors
                context.close()
                assert corpus.paths[sid].read_bytes() == native
            finally:
                browser.close()
    print("PASS terminal input browser: OSC 52 clipboard + Ctrl+V, terminal_input capability, "
          "desktop local wheel + WebSocket input, console file paste (off: hint; on: batch + path), "
          "mobile key bar HTTP keys, exact lease body, "
          "no input/reclaim after exit, composer hidden, native fixture unchanged")


if __name__ == "__main__":
    main()
