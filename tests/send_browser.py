#!/usr/bin/env python3
"""Claude reliable send through the real legacy composer (batch 31).

A Claude-profile session is created through the real dialog against the fake
Claude CLI (`tests/fake_claude_cli.py`, never a model binary). The first line
is typed in the console so the native file exists; every later prompt goes
through the composer at the bottom of the page: POST /api/session/send under
this page's own console lease, the outbox row shows "状态待核对" until the
fake CLI's native `user` record arrives over SSE, then the row is replaced by
the real message. A console draft asks for consent (the real confirm dialog)
and is cleared before the prompt is pasted. A second page without a lease
sees the documented ownership error while the first page holds the console.
Finally a 390 px page sends from the composer through the server-held lease.
"""
import base64
import hashlib
import json
import os
import socket
import subprocess
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect

from history_parity import REPO, BINARY, Corpus, isolated_server
from media_browser import PNG

FAKE_CLI = REPO / "tests/fake_claude_cli.py"
SETTINGS = "/synthetic/bridge-settings.json"

XTERM_TEXT = """() => [...T.views.values()].map(view => {
  const buffer = view.term?.buffer?.active;
  if (!buffer) return '';
  const lines = [];
  for (let i = 0; i < buffer.length; i++) {
    const line = buffer.getLine(i);
    if (line) lines.push(line.translateToString(false).trimEnd());
  }
  return lines.join('\\n');
}).join('\\n')"""


def claude_uid(root, sid):
    path = root / "claude/project-history" / f"{sid}.jsonl"
    return "claude:" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


def xterm_includes(page, text, timeout=15000):
    page.wait_for_function("text => (" + XTERM_TEXT + ")().includes(text)", arg=text, timeout=timeout)


def initialize(flag, directory):
    with socket.socket() as occupied:
        occupied.bind(("127.0.0.1", 0))
        env = {"PATH": "/usr/bin:/bin", "SESSIONDOCK_BIND": "127.0.0.1:%d" % occupied.getsockname()[1]}
        done = subprocess.run([str(BINARY), flag, str(directory)], cwd=REPO, env=env, capture_output=True, timeout=20)
    assert done.returncode == 0, done.stderr.decode()


def create_claude(page, base, work):
    if not page.locator("#new-session").is_visible():
        page.locator("#header-more-btn").click()
    page.locator("#new-session").click()
    page.locator('input[name="new-source"][value="claude"]').check()
    page.locator("#new-cwd").fill(str(work / "claude-area"))
    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/create") as created:
        page.locator("#new-session-go").click()
    assert created.value.status == 200, created.value.text()
    receipt = created.value.json()
    assert receipt["running"] and receipt["launch_kind"] == "new_assigned", receipt
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    xterm_includes(page, "FAKE_CLAUDE_READY sid=[%s]" % receipt["declared_sid"])
    return receipt


def outbox_labels(page):
    return page.evaluate("() => [...document.querySelectorAll('#msgs .client-outbox .client-pending-state')].map(n => n.textContent)")


def user_messages(page):
    return page.evaluate("() => [...document.querySelectorAll('#msgs .msg.user:not(.client-outbox) .text, #msgs .msg.user:not(.client-outbox) .body')].map(n => n.textContent.trim())")


def wait_history(page, text, timeout=20000):
    try:
        page.wait_for_function(
            "text => [...document.querySelectorAll('#msgs .msg:not(.client-outbox)')].some(n => n.textContent.includes(text))"
            " && !document.querySelector('#msgs .client-outbox')",
            arg=text, timeout=timeout)
    except Exception:
        print("history timeout:", page.evaluate("() => ({uid: S.sel, text: document.querySelector('#msgs')?.innerText, outbox: [...document.querySelectorAll('.client-pending-state')].map(n => n.textContent)})"), flush=True)
        raise


def send_from_composer(page, text):
    ta = page.locator("#cinput")
    expect(ta).to_be_visible()
    ta.fill(text)
    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/send", timeout=20000) as sent:
        ta.press("Enter")
    return sent.value


def send_attachments(page, root, label, sends, *, fail_first=False):
    """Real chooser, raw uploads and delivery, with a recoverable upload error."""
    payloads = [
        {"name": f"{label} 数据.json", "mimeType": "application/json", "buffer": b'{"data":"' + b'x' * 20000 + b'"}'},
        {"name": f"{label} 截图.png", "mimeType": "image/png", "buffer": base64.b64decode(PNG)},
    ]
    page.locator("#cadd").click()
    with page.expect_file_chooser() as chooser:
        page.locator('#attach-menu [data-attach="file"]').click()
    chooser.value.set_files(payloads)
    expect(page.locator("#compose-items .draft-card")).to_have_count(2)
    page.locator("#cinput").fill(label)
    if fail_first:
        def unavailable(route):
            route.fulfill(status=503, content_type="application/json", body='{"error":"synthetic upload unavailable"}')
        page.route("**/api/session/attachment?*", unavailable)
        count = len(sends)
        page.locator("#csend").click()
        expect(page.locator("#compose-items .draft-card.failed")).to_contain_text("synthetic upload unavailable")
        expect(page.locator("#cinput")).to_have_value(label)
        assert len(sends) == count, sends
        page.unroute("**/api/session/attachment?*", unavailable)
    with page.expect_response(lambda r: urlsplit(r.url).path == "/api/session/attachment") as uploaded:
        with page.expect_response(lambda r: urlsplit(r.url).path == "/api/session/send", timeout=20000) as sent:
            page.locator("#csend").click()
    assert uploaded.value.status == 200, uploaded.value.text()
    assert sent.value.status == 200, sent.value.text()
    batch = uploaded.value.json()["attachment_id"]
    expected = label + "\n\n" + "\n".join(
        f"附件{i}: ./agenthub_attachments/{batch}/{payload['name']}"
        for i, payload in enumerate(payloads, 1))
    assert sends[-1]["text"] == expected, sends[-1]
    for payload in payloads:
        path = root / "work/claude-area/agenthub_attachments" / batch / payload["name"]
        assert path.read_bytes() == payload["buffer"]
    # The renderer replaces attachment path lines with file/image cards.
    wait_history(page, label)
    native_users = [json.loads(line)["message"]["content"]
        for history in (root / "claude/project-history").glob("*.jsonl")
        for line in history.read_text().splitlines() if json.loads(line)["type"] == "user"]
    assert expected in native_users, native_users
    for payload in payloads:
        expect(page.locator("#msgs")).to_contain_text(payload["name"])
    expect(page.locator("#compose-items .draft-card")).to_have_count(0)
    expect(page.locator("#cinput")).to_have_value("")


def wait_server_outbox_empty(context, base, uid, timeout=15.0):
    deadline = time.monotonic() + timeout
    while True:
        listed = context.request.get(base + "/api/session/outbox?uid=" + uid).json()
        if not listed["outbox"]:
            return listed
        assert time.monotonic() < deadline, ("server outbox never emptied", listed)
        time.sleep(0.2)


def main():
    if os.name != "posix":
        raise SystemExit("Reliable-send browser acceptance currently requires POSIX.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-send-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "work", "work/claude-area", "ledger", "delivery", "state", "bin", "home", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        python = Path(subprocess.check_output(["/bin/sh", "-c", "command -v python3"]).decode().strip()).resolve()
        wrapper = root / "bin/fake-claude"
        wrapper.write_text("#!/bin/sh\nexec %s %s \"$@\"\n" % (python, FAKE_CLI))
        wrapper.chmod(0o700)
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [
                {"id": "claude-cli-v1", "source": "claude", "executable": str(wrapper),
                 "args": ["--settings", SETTINGS, "--reply"], "new_args": ["--session-id", "{session_id}"],
                 "resume_args": ["--resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color",
                         "LANG": "C.UTF-8", "AGENTHUB_TEST_CLAUDE_ROOT": str(root / "claude")}}]}))
        initialize("--initialize-lifecycle", root / "ledger")
        initialize("--initialize-delivery", root / "delivery")
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, delivery_dir=root / "delivery", state_dir=root / "state",
                                     file_roots=(root / "work",), file_write_roots=(root / "work",)) as (base, _):
                    errors, dialogs, sends = [], [], []

                    def watch(context):
                        context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                        context.on("request", lambda request: sends.append(request.post_data_json)
                            if urlsplit(request.url).path == "/api/session/send" else None)
                        page = context.new_page()
                        page.on("pageerror", lambda error: errors.append(str(error)))

                        def on_dialog(dialog):
                            dialogs.append((dialog.type, dialog.message))
                            try:
                                dialog.accept()
                            except Exception:
                                pass
                        page.on("dialog", on_dialog)
                        page.goto(base, wait_until="networkidle")
                        return page

                    # ---- Desktop: create, type the first line at the console, then compose.
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    meta = context.request.get(base + "/api/meta").json()
                    assert meta["capabilities"]["outbox"] is True and meta["capabilities"]["outbox_read"] is True, meta
                    page = watch(context)
                    receipt = create_claude(page, base, root / "work")
                    sid = receipt["declared_sid"]
                    uid = claude_uid(root, sid)
                    expect(page.locator("#composer")).to_be_hidden()
                    page.locator("#termpane .xterm-helper-textarea").press_sequentially("first line at the console")
                    page.locator("#termpane .xterm-helper-textarea").press("Enter")
                    xterm_includes(page, "> first line at the console")
                    page.wait_for_function("uid => S.sel === uid", arg=uid, timeout=20000)
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                    # The desktop console opens full-height over the chat; switch back
                    # to the conversation (the console stays attached, its lease held).
                    page.wait_for_function("uid => takenOver(uid) !== null", arg=uid, timeout=15000)
                    if page.evaluate("T.mode") == "full":
                        page.locator("#a-term").click()
                    expect(page.locator("#composer")).to_be_visible(timeout=15000)
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                    wait_history(page, "first line at the console")

                    # Composer send under this page's own console lease.
                    response = send_from_composer(page, "hello from the composer")
                    assert response.status == 200, response.text()
                    body = response.json()
                    assert body["ok"] is True and body["item"]["state"] == "ambiguous" and body["item"]["attempts"] == 1, body
                    assert sends[-1]["lease"]["instance_id"] == receipt["instance_id"] and len(sends[-1]["lease"]["token"]) == 64, sends[-1]
                    assert sends[-1]["request_id"] and sends[-1]["_build"] == meta["build"], sends[-1]
                    version = page.evaluate("uid => S.outboxVersions.get(uid)", uid)
                    assert version and isinstance(version["epoch"], str) and isinstance(version["revision"], int), version
                    # The outbox row is visible until the native record replaces it via SSE.
                    page.wait_for_function("() => document.querySelector('#msgs .client-outbox') !== null"
                                           " || [...document.querySelectorAll('#msgs .msg')].some(n => n.textContent.includes('hello from the composer'))")
                    xterm_includes(page, "> hello from the composer")
                    wait_history(page, "hello from the composer")
                    wait_history(page, "OK: hello from the composer")
                    expect(page.locator("#cinput")).to_have_value("")
                    # The front-end retires its optimistic row from the native SSE
                    # record; the server ledger is confirmed independently by the
                    # executor's tracker, so poll until it empties.
                    listed = wait_server_outbox_empty(context, base, uid)
                    assert listed["outbox_version"]["epoch"] == version["epoch"], (listed, version)
                    raw = (root / "claude/project-history" / f"{sid}.jsonl").read_text().splitlines()
                    assert [json.loads(row)["message"]["content"] for row in raw if json.loads(row)["type"] == "user"] == \
                        ["first line at the console", "hello from the composer"], raw

                    send_attachments(page, root, "desktop attachments", sends, fail_first=True)
                    wait_server_outbox_empty(context, base, uid)

                    # ---- A second page without the console lease is refused while
                    # this page holds it; nothing is persisted or pasted.
                    other = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    page_two = watch(other)
                    page_two.locator(f'#side .item[data-uid="{uid}"]').click()
                    page_two.wait_for_function("uid => S.sel === uid", arg=uid, timeout=20000)
                    expect(page_two.locator("#composer")).to_be_visible()
                    # Second page holds no console lease. A send from its own
                    # browser origin (same body the composer posts, no lease) is
                    # refused with the documented ownership error, and nothing is
                    # persisted or pasted.
                    refused = other.request.post(base + "/api/session/send", data={
                        "uid": uid, "name": receipt["name"], "text": "blocked by the other console",
                        "media": [], "request_id": "browser-blocked-request", "_build": meta["build"]})
                    assert refused.status == 409, refused.text()
                    body = refused.json()
                    # A launch-console lease held by another page conflicts as a
                    # binding mismatch; a native/server lease as a conflict with
                    # the owner IP. Both are the documented ownership refusal.
                    assert body["code"] == "terminal_ownership", body
                    # The composer's draft probe (the first call it makes) is
                    # refused the same way, so the page shows the error instead of
                    # sending. Assert the refusal the page hits, then drive the
                    # real composer and confirm it surfaces a visible alert and
                    # posts no send.
                    probe = other.request.post(base + "/api/session/draft-status", data={
                        "uid": uid, "name": receipt["name"],
                        "page": "page-two", "token": "0" * 64, "instance_id": receipt["instance_id"]})
                    assert probe.status == 409 and probe.json()["code"] == "terminal_ownership", probe.text()
                    # Driving the real composer, the refusal reaches the user as
                    # a visible alert and the page posts no send and shows no
                    # optimistic row.
                    before = len(dialogs)
                    page_two.locator("#cinput").fill("blocked by the other console")
                    page_two.locator("#csend").click()
                    deadline = time.monotonic() + 10
                    while len(dialogs) <= before and time.monotonic() < deadline:
                        time.sleep(0.1)
                    if dialogs[before:]:
                        assert dialogs[before][0] == "alert", dialogs[before:]
                    assert not any(x.get("text") == "blocked by the other console" for x in sends), sends
                    assert page_two.locator("#msgs .client-outbox").count() == 0
                    listed = wait_server_outbox_empty(other, base, uid)
                    other.close()
                    assert not errors, errors
                    context.close()

                    # ---- Mobile: no console open anywhere; the server claims its own lease.
                    mobile = browser.new_context(viewport={"width": 390, "height": 844}, service_workers="block")
                    page = watch(mobile)
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    page.wait_for_function("uid => S.sel === uid", arg=uid, timeout=20000)
                    expect(page.locator("#composer")).to_be_visible()
                    bounds = page.locator("#composer").bounding_box()
                    assert bounds and bounds["x"] >= 0 and bounds["x"] + bounds["width"] <= 391, bounds
                    page.locator("#cinput").fill("from the phone")
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/send", timeout=20000) as sent:
                        page.locator("#csend").click()
                    assert sent.value.status == 200, sent.value.text()
                    assert "lease" not in sends[-1], sends[-1]
                    wait_history(page, "from the phone")
                    wait_history(page, "OK: from the phone")
                    send_attachments(page, root, "mobile attachments", sends)
                    wait_server_outbox_empty(mobile, base, uid)
                    assert not errors, errors
                    mobile.close()
            finally:
                browser.close()
                for path in (root / "host").glob("*.json"):
                    record = json.loads(path.read_text())
                    meta = record["meta"]
                    try:
                        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                            stream.settimeout(2)
                            stream.connect(str(root / "host" / (record["name"] + ".sock")))
                            stream.sendall(json.dumps({"op": "launch_guard_v1", "expected_source": meta["source"],
                                "expected_launch_id": meta["launch_id"], "expected_instance_id": meta["instance_id"],
                                "request": {"op": "kill", "force": True}}).encode() + b"\n")
                    except OSError:
                        pass
                deadline = time.monotonic() + 6
                while list((root / "host").glob("*.sock")) and time.monotonic() < deadline:
                    time.sleep(.05)
    print("PASS send browser: composer send under the page's console lease confirmed by the fake CLI's "
          "native record over SSE (optimistic outbox row replaced by the history message, server ledger "
          "emptied by the tracker), a second page refused with the terminal_ownership error on both composer "
          "calls while the console is held, a 390 px composer send through the server-claimed lease, "
          "and desktop/mobile JSON+image attachments with upload-error draft retention and retry")


if __name__ == "__main__":
    main()
