#!/usr/bin/env python3
"""Grok composer text over raw terminal input from a page without a console.

Grok has no reliable-send executor (`delivery_source_unsupported`), so the
composer text is submitted as raw terminal input.
Under the Rust `terminal_input` capability the same submit is a bracketed
`paste` and a separate `Enter` on `/api/term/send`, with an empty token and
the pane's pinned identity because the conversation view holds no lease
(`term.js termInputBody` / `termRowBinding`). One isolated ptyhost runs the
fixed free shell with synthetic Grok metadata; a 390 px page selects the Grok
session, sends "ping" from the composer, and the shell's reply is visible once
the console is opened afterwards. Nothing is claimed by the page before that.
"""
from contextlib import contextmanager
import hashlib
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

from history_parity import BINARY, REPO, Corpus, isolated_server
from host_identity import request as host_request
from send_browser import initialize
from terminal_browser import stop
from terminal_input_browser import SHELL, XTERM_TEXT, xterm_contains

FIXTURE = REPO / "crates/sessiondock/tests/fixtures/grok/project-demo/synthetic-grok"


@contextmanager
def grok_host(root, instance, uid):
    name = "synthetic-grok-host"
    environment = {key: value for key, value in os.environ.items() if key in {"PATH", "LANG", "LC_ALL", "LC_CTYPE"}}
    environment["TERM"] = "xterm-256color"
    metadata = {"source": "grok", "sid": "synthetic-grok", "uid": uid, "instance_id": instance}
    process = subprocess.Popen([str(REPO / "target/debug/ptyhost"), "--dir", str(root / "host"), "run", "--name", name,
        "--cwd", str(root / "work"), "--cols", "80", "--rows", "24", "--meta", json.dumps(metadata), "--",
        shutil.which("sh"), "-c", SHELL], cwd=root / "work", env=environment,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
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
        yield process
    finally:
        stop(process)  # Only the exact test child, never a name or guessed PID.


def main():
    with tempfile.TemporaryDirectory(prefix="sessiondock-grokraw-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "work", "claude", "codex", "grok", "delivery"]:
            (root / name).mkdir(mode=0o700)
        session = root / "grok/project-demo/synthetic-grok"
        session.mkdir(parents=True)
        for file in ("summary.json", "chat_history.jsonl"):
            shutil.copyfile(FIXTURE / file, session / file)
        uid = "grok:" + hashlib.sha1(str(session).encode()).hexdigest()[:16]
        native = (session / "chat_history.jsonl").read_bytes()
        instance = "synthetic-" + uuid.uuid4().hex
        initialize("--initialize-delivery", root / "delivery")
        corpus = Corpus(root)
        with grok_host(root, instance, uid), \
             isolated_server(corpus, BINARY, host_dir=root / "host", delivery_dir=root / "delivery") as (base, opener), \
             sync_playwright() as playwright:
            listed = json.loads(opener.open(base + "/api/sessions").read())
            assert [row["uid"] for row in listed["sessions"]] == [uid], listed
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 390, "height": 844}, service_workers="block")
                context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                errors, dialogs, sends, claims, reliable = [], [], [], [], []

                def observe(request):
                    path = urlsplit(request.url).path
                    if path == "/api/term/send":
                        sends.append(request.post_data_json)
                    elif path == "/api/term/claim":
                        claims.append(request.post_data_json)
                    elif path in ("/api/session/send", "/api/session/draft-status"):
                        reliable.append(path)

                context.on("request", observe)
                page = context.new_page()
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("dialog", lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
                page.goto(base, wait_until="networkidle")
                capabilities = page.evaluate("SessionDockCapabilities.config")
                assert capabilities["terminal_input"] is True and capabilities["outbox"] is True, capabilities
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.wait_for_function("uid => S.sel === uid", arg=uid, timeout=20000)
                page.wait_for_function("uid => takenOver(uid) !== null", arg=uid, timeout=15000)
                expect(page.locator("#composer")).to_be_visible()
                expect(page.locator("#termpane")).to_be_hidden()

                holder_context = browser.new_context(service_workers="block")
                holder = holder_context.new_page()
                holder.goto(base, wait_until="networkidle")
                holder.locator(f'#side .item[data-uid="{uid}"]').click()
                holder.locator('#a-term').click()
                holder.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                holder_token = holder.evaluate('name => T.views.get(name)?.inputLease?.token', 'synthetic-grok-host')

                # ---- The phone's composer, console closed: paste + Enter over
                # raw input, no lease, no claim, no reliable-send call.
                page.locator("#cinput").fill("ping")
                with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/send"
                                          and response.request.post_data_json.get("keys") == ["Enter"], timeout=20000) as entered:
                    page.locator("#csend").click()
                assert entered.value.status == 200 and entered.value.json()["ok"] is True, entered.value.text()
                deadline = time.monotonic() + 5
                while len(sends) < 2 and time.monotonic() < deadline:
                    page.wait_for_timeout(25)
                assert [body.get("paste") for body in sends] == ["ping", None], sends
                assert [body.get("keys") for body in sends] == [None, ["Enter"]], sends
                for body in sends:
                    assert body["token"] == "" and body["uid"] == uid and body["instance_id"] == instance, body
                    assert "record_id" not in body and "launch_id" not in body and "text" not in body, body
                assert not claims and not reliable and not dialogs, (claims, reliable, dialogs)
                expect(page.locator("#cinput")).to_have_value("")

                assert holder.evaluate('name => T.views.get(name)?.inputLease?.token', 'synthetic-grok-host') == holder_token
                assert holder.evaluate('T.ws?.readyState === WebSocket.OPEN')
                holder_context.close()

                # ---- Opening the console afterwards shows the shell answered.
                page.locator("#a-term").click()
                expect(page.locator("#termpane")).to_be_visible()
                page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                xterm_contains(page, "RS_PING_OK")
                assert len(claims) == 1 and claims[0]["uid"] == uid and claims[0]["instance_id"] == instance, claims
                assert not errors, errors
                context.close()
                assert (session / "chat_history.jsonl").read_bytes() == native
            finally:
                browser.close()
    print("PASS grok raw send browser: Grok composer text from a 390 px page without a console goes over "
          "/api/term/send as paste + Enter with an empty token and the pane identity (no claim, no "
          "reliable-send call), the shell's reply is visible once the console opens, fixture unchanged")


if __name__ == "__main__":
    main()
