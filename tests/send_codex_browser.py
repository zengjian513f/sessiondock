#!/usr/bin/env python3
"""Codex reliable send through the real legacy composer (batch 32).

A synthetic Codex rollout is resumed through the existing console button
(`/api/term/takeover`, schema-2 profile `resume ["resume","{sid}"]`) against
the fake Codex CLI (`tests/fake_codex_cli.py`, never a model binary), which
renders a Codex-style composer (dim placeholder, braille particle glyphs,
model footer) and appends the real rollout shape for every submitted line.
Every prompt goes through the composer at the bottom of the page:
POST /api/session/send under this page's own console lease, the outbox row
shows Codex's "终端写入待核对" (Python `failed`, attempts 1) with 检查终端/移除
until the fake CLI's native user record (with its turn ID) arrives over SSE,
then the row is replaced by the real message and the server ledger row is
confirmed with the causal native text record. A second page without a lease sees the
documented ownership error while the first page holds the console. Finally a
390 px page sends from the composer through the server-held lease.
"""
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

from history_parity import REPO, BINARY, Corpus, codex_message, codex_row, isolated_server

FAKE_CLI = REPO / "tests/fake_codex_cli.py"
CODEX_SID = "6a7b8c9d-0e1f-4a2b-9c3d-4e5f6a7b8c9d"

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


def xterm_includes(page, text, timeout=15000):
    page.wait_for_function("text => (" + XTERM_TEXT + ")().includes(text)", arg=text, timeout=timeout)


def initialize(flag, directory):
    with socket.socket() as occupied:
        occupied.bind(("127.0.0.1", 0))
        env = {"PATH": "/usr/bin:/bin", "SESSIONDOCK_BIND": "127.0.0.1:%d" % occupied.getsockname()[1]}
        done = subprocess.run([str(BINARY), flag, str(directory)], cwd=REPO, env=env, capture_output=True, timeout=20)
    assert done.returncode == 0, done.stderr.decode()


def resume_codex(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.wait_for_function("uid => S.sel === uid", arg=uid, timeout=20000)
    expect(page.locator("#msgs")).to_contain_text("Synthetic codex prompt")
    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover") as taken:
        page.locator("#a-term").click()
    resumed = taken.value.json()
    assert taken.value.status == 200 and resumed["launch_kind"] == "resume", resumed
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
    xterm_includes(page, "FAKE_CODEX_TUI sid=[%s]" % CODEX_SID)
    page.wait_for_function("uid => (T.list || []).some(row => row.uid === uid && row.instance_id)", arg=uid, timeout=15000)
    return resumed


def outbox_labels(page):
    return page.evaluate("() => [...document.querySelectorAll('#msgs .client-outbox .client-pending-state')].map(n => n.textContent)")


def wait_history(page, text, timeout=20000):
    page.wait_for_function(
        "text => [...document.querySelectorAll('#msgs .msg:not(.client-outbox)')].some(n => n.textContent.includes(text))"
        " && !document.querySelector('#msgs .client-outbox')",
        arg=text, timeout=timeout)


def send_from_composer(page, text):
    ta = page.locator("#cinput")
    expect(ta).to_be_visible()
    ta.fill(text)
    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/send", timeout=20000) as sent:
        ta.press("Enter")
    return sent.value


def wait_server_outbox_empty(context, base, uid, timeout=15.0):
    deadline = time.monotonic() + timeout
    while True:
        listed = context.request.get(base + "/api/session/outbox?uid=" + uid).json()
        if not listed["outbox"]:
            return listed
        assert time.monotonic() < deadline, ("server outbox never emptied", listed)
        time.sleep(0.2)


def user_records(path):
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    return [row["payload"] for row in rows
            if row.get("type") == "response_item" and row["payload"].get("type") == "message"
            and row["payload"].get("role") == "user"]


def main():
    if os.name != "posix":
        raise SystemExit("Reliable-send browser acceptance currently requires POSIX.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-send-codex-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "work", "work/codex-area", "ledger", "delivery", "bin", "home", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        rollout = corpus.put(CODEX_SID, "codex", [
            codex_row("session_meta", {"id": CODEX_SID, "cwd": str(root / "work/codex-area")}),
            codex_message("user", "Synthetic codex prompt")], [])
        uid = corpus.uid(CODEX_SID)
        python = Path(subprocess.check_output(["/bin/sh", "-c", "command -v python3"]).decode().strip()).resolve()
        wrapper = root / "bin/fake-codex"
        wrapper.write_text("#!/bin/sh\nexec %s %s \"$@\"\n" % (python, FAKE_CLI))
        wrapper.chmod(0o700)
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [
                {"id": "codex-cli-v1", "source": "codex", "executable": str(wrapper),
                 "args": ["--reply", "-c", 'model_reasoning_effort="low"'], "new_args": [],
                 "resume_args": ["resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color",
                         "LANG": "C.UTF-8", "SESSIONDOCK_TEST_CODEX_ROOT": str(root / "codex")}}]}))
        initialize("--initialize-lifecycle", root / "ledger")
        initialize("--initialize-delivery", root / "delivery")
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, delivery_dir=root / "delivery") as (base, _):
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

                    # ---- Desktop: resume the corpus session, then compose.
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    meta = context.request.get(base + "/api/meta").json()
                    assert meta["capabilities"]["outbox"] is True and meta["capabilities"]["outbox_read"] is True, meta
                    page = watch(context)
                    resumed = resume_codex(page, uid)
                    if page.evaluate("T.mode") == "full":
                        page.locator("#a-term").click()
                    expect(page.locator("#composer")).to_be_visible(timeout=15000)
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")

                    # The driver reads the fake's particle/placeholder composer as
                    # empty (probed under this page's own console lease, as the
                    # composer does before every send).
                    probe = page.evaluate("([uid, name]) => post('api/session/draft-status', {uid, name, ...termSendLease(name)})",
                                          [uid, resumed["name"]])
                    assert probe.get("draft_state") == "empty" and not probe.get("error"), probe

                    response = send_from_composer(page, "hello from the composer")
                    assert response.status == 200, response.text()
                    body = response.json()
                    assert body["ok"] is True and body["item"]["state"] == "failed" and body["item"]["attempts"] == 1, body
                    assert body["item"]["error"] == "发送结果待核对；禁止自动重试", body
                    assert sends[-1]["lease"]["instance_id"] == resumed["instance_id"] and len(sends[-1]["lease"]["token"]) == 64, sends[-1]
                    assert sends[-1]["request_id"] and sends[-1]["_build"] == meta["build"], sends[-1]
                    version = page.evaluate("uid => S.outboxVersions.get(uid)", uid)
                    assert version and isinstance(version["epoch"], str) and isinstance(version["revision"], int), version
                    page.wait_for_function("() => document.querySelector('#msgs .client-outbox') !== null"
                                           " || [...document.querySelectorAll('#msgs .msg')].some(n => n.textContent.includes('hello from the composer'))")
                    labels = outbox_labels(page)
                    assert all(label in ("终端写入待核对", "等待终端确认", "正在写入终端") for label in labels), labels
                    xterm_includes(page, "> hello from the composer")
                    wait_history(page, "hello from the composer")
                    wait_history(page, "OK: hello from the composer")
                    expect(page.locator("#cinput")).to_have_value("")
                    listed = wait_server_outbox_empty(context, base, uid)
                    assert listed["outbox_version"]["epoch"] == version["epoch"], (listed, version)
                    users = user_records(rollout)
                    assert [u["content"][0]["text"] for u in users] == ["Synthetic codex prompt", "hello from the composer"], users
                    assert users[1]["internal_chat_message_metadata_passthrough"]["turn_id"], users[1]

                    # ---- A second page without the console lease is refused.
                    other = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    page_two = watch(other)
                    page_two.locator(f'#side .item[data-uid="{uid}"]').click()
                    page_two.wait_for_function("uid => S.sel === uid", arg=uid, timeout=20000)
                    expect(page_two.locator("#composer")).to_be_visible()
                    refused = other.request.post(base + "/api/session/send", data={
                        "uid": uid, "name": resumed["name"], "text": "blocked by the other console",
                        "media": [], "request_id": "browser-blocked-request", "_build": meta["build"]})
                    assert refused.status == 409, refused.text()
                    assert refused.json()["code"] == "terminal_ownership", refused.json()
                    probe = other.request.post(base + "/api/session/draft-status", data={
                        "uid": uid, "name": resumed["name"],
                        "page": "page-two", "token": "0" * 64, "instance_id": resumed["instance_id"]})
                    assert probe.status == 409 and probe.json()["code"] == "terminal_ownership", probe.text()
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
                    wait_server_outbox_empty(other, base, uid)
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
                    wait_server_outbox_empty(mobile, base, uid)
                    assert [u["content"][0]["text"] for u in user_records(rollout)][-1] == "from the phone"
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
    print("PASS send codex browser: resumed fake Codex CLI, composer send under the page's console lease "
          "(Codex outbox row 终端写入待核对) confirmed by the fake CLI's native user record with its turn ID over SSE "
          "(optimistic row replaced by the history message, server ledger emptied by the tracker), a second page "
          "refused with terminal_ownership on both composer calls while the console is held, and a 390 px composer "
          "send through the server-claimed lease")


if __name__ == "__main__":
    main()
