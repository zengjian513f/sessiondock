#!/usr/bin/env python3
"""Legacy stop control under the Rust `session_stop` capability.

A synthetic Codex session is resumed through the existing console button
(`/api/term/takeover`, fake CLI = free shell that exits on Ctrl-D). The header
"停止会话" action then posts `/api/session/stop` with a `request_id`, the page
shows which stage ended the instance (graceful), the console turns into the
exit explanation and the action flips to "删除会话". A session that has no
running instance succeeds as a no-op. The
mobile (390 px) sidebar long-press menu stops a fresh resume the same way. No
model binary, native CLI home or production host is touched.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.parse import urlsplit
from playwright.sync_api import sync_playwright, expect
from history_parity import REPO, BINARY, Corpus, claude_row, codex_row, codex_message, isolated_server

CODEX_SID = "8f3c1d2e-4a5b-4c6d-8e7f-90a1b2c3d4e5"
FAKE_CODEX = """#!/bin/sh
printf 'FAKE_CODEX_ARGV'
for arg in "$@"; do printf ' [%s]' "$arg"; done
printf '\\n'
exec /bin/sh -c 'stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; while IFS= read -r line; do case "$line" in quit) exit 0 ;; *) printf "RS_INPUT_OK\\n" ;; esac; done'
"""
def session_action(page):
    button = page.locator("#a-session-action")
    if not button.is_visible():
        page.locator("#a-more").click()
    expect(button).to_be_visible()
    return button


def wait_xterm(page, text):
    page.wait_for_function(
        "text => [...T.views.values()].some(v => v.term?.buffer?.active && Array.from({length: v.term.buffer.active.length},"
        " (_, i) => v.term.buffer.active.getLine(i)?.translateToString() || '').join('\\n').includes(text))",
        arg=text, timeout=15000)


def main():
    if os.name != "posix":
        raise SystemExit("Real launch acceptance currently requires POSIX; no Windows/macOS claim.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-session-stop-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "work", "work/codex-area", "ledger", "bin", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        corpus.put(CODEX_SID, "codex", [codex_row("session_meta", {"id": CODEX_SID, "cwd": str(root / "work/codex-area")}),
            codex_message("user", "Synthetic managed stop target")], [])
        corpus.put("synthetic-unrelated-claude", "claude", [
            claude_row("synthetic-unrelated-claude", "user", "u0", None, "Synthetic session without instance"),
            claude_row("synthetic-unrelated-claude", "assistant", "a0", "u0", "Synthetic answer")], [])
        native = {name: path.read_bytes() for name, path in corpus.paths.items()}
        codex_uid = corpus.uid(CODEX_SID)
        other_uid = corpus.uid("synthetic-unrelated-claude")
        (root / "bin/fake-codex").write_text(FAKE_CODEX)
        (root / "bin/fake-codex").chmod(0o700)
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [
                {"id": "codex-cli-v1", "source": "codex", "executable": str(root / "bin/fake-codex"),
                 "args": [], "resume_args": ["resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}}]}))
        initialized = subprocess.run([str(BINARY), "--initialize-lifecycle", str(root / "ledger")],
            cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        assert initialized.returncode == 0, initialized.stderr.decode()
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration) as (base, opener):
                    errors, dialogs, stops = [], [], []

                    def watch(context):
                        context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                        context.on("request", lambda request: stops.append(request.post_data_json)
                            if urlsplit(request.url).path == "/api/session/stop" else None)
                        page = context.new_page()
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        def on_dialog(dialog):
                            dialogs.append((dialog.type, dialog.message))
                            dialog.accept()
                        page.on("dialog", on_dialog)
                        page.goto(base, wait_until="networkidle")
                        assert page.evaluate("SessionDockCapabilities.config.session_stop") is True
                        return page

                    # ---- Desktop: resume, then stop through the header action.
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    page = watch(context)
                    page.locator(f'#side .item[data-uid="{codex_uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic managed stop target")
                    # Before any instance exists the action is delete, never stop.
                    expect(session_action(page)).to_have_attribute("aria-label", "删除会话")
                    page.keyboard.press("Escape")
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover") as taken:
                        page.locator("#a-term").click()
                    resumed = taken.value.json()
                    assert taken.value.status == 200 and resumed["launch_kind"] == "resume", resumed
                    expect(page.locator("#termpane")).to_be_visible()
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                    wait_xterm(page, "RS_SHELL_READY")
                    page.wait_for_function("uid => (T.list || []).some(row => row.uid === uid && row.instance_id)", arg=codex_uid)
                    action = session_action(page)
                    expect(action).to_have_attribute("aria-label", "停止会话")
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/stop") as stopped:
                        action.click()
                    assert dialogs[-1][0] == "confirm" and "停止会话" in dialogs[-1][1], dialogs
                    reply = stopped.value.json()
                    assert stopped.value.status == 200, stopped.value.text()
                    assert reply["stage"] == "graceful" and reply["stopped"] is True and reply["tmux"] is False, reply
                    assert reply["instance_id"] == resumed["instance_id"] and reply["record_id"] == resumed["record_id"], reply
                    assert stops[-1]["uid"] == codex_uid and stops[-1].get("request_id"), stops[-1]
                    notice = page.locator("#session-stop-notice")
                    expect(notice).to_be_visible()
                    expect(notice).to_contain_text("CLI 已在收到 Ctrl-D 后退出")
                    # The pane closes on exit and the console
                    # button stays usable as "接管会话" because the source has a
                    # resume-capable profile; the exit explanation is remembered,
                    # the instance leaves the list and the header action flips
                    # back to delete.
                    page.wait_for_function("uid => T.ended.has(uid)", arg=codex_uid, timeout=15000)
                    expect(page.locator("#termpane")).to_be_hidden()
                    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false", timeout=15000)
                    page.wait_for_function("uid => !(T.list || []).some(row => row.uid === uid)", arg=codex_uid, timeout=15000)
                    assert page.evaluate("uid => S.live.has(uid)", codex_uid) is False
                    expect(session_action(page)).to_have_attribute("aria-label", "删除会话")
                    page.keyboard.press("Escape")
                    # The notice really hides (the shared toast class forces display:flex).
                    page.evaluate("showSessionStopNotice('')")
                    expect(notice).to_be_hidden()
                    deadline = time.monotonic() + 10
                    while list((root / "host").glob("*.json")) and time.monotonic() < deadline:
                        time.sleep(0.05)
                    assert not list((root / "host").glob("*.json")), "host record must be cleaned after the exit"
                    live = json.loads(opener.open(base + "/api/live?force=1", timeout=10).read())
                    assert live["managed"]["sessions"][codex_uid]["state"] == "exited", live["managed"]["sessions"]

                    # ---- A session without any running instance follows Python:
                    # stopping succeeds as a no-op and the stale live marker clears.
                    # `S.live` can hold stale entries in Rust mode (no live poll), so the
                    # stop control can still be reached for such a session.
                    page.locator(f'#side .item[data-uid="{other_uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic session without instance")
                    expect(session_action(page)).to_have_attribute("aria-label", "删除会话")
                    page.keyboard.press("Escape")
                    page.evaluate("uid => { S.live.add(uid); paintLive(); }", other_uid)
                    action = session_action(page)
                    expect(action).to_have_attribute("aria-label", "停止会话")
                    before = len(dialogs)
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/stop") as stopped:
                        action.click()
                    assert stopped.value.status == 200, stopped.value.text()
                    result = stopped.value.json()
                    assert result.get("ok") is True and result.get("stopped") is False, result
                    assert result.get("external_detection") == "proc_scan", result
                    expect(notice).to_be_visible()
                    expect(notice).to_contain_text("停止请求已处理")
                    assert len(dialogs) == before + 1 and dialogs[-1][0] == "confirm", dialogs[before:]
                    assert not errors, errors
                    context.close()

                    # ---- Mobile: a fresh resume, stopped from the sidebar long-press menu.
                    mobile = browser.new_context(viewport={"width": 390, "height": 844}, service_workers="block", has_touch=True)
                    page = watch(mobile)
                    page.locator(f'#side .item[data-uid="{codex_uid}"]').click()
                    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false")
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover") as taken:
                        page.locator("#a-term").click()
                    second = taken.value.json()
                    assert taken.value.status == 200 and second["action"] == "started", second
                    assert second["instance_id"] != resumed["instance_id"]
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                    wait_xterm(page, "RS_SHELL_READY")
                    page.locator(".mobile-back").first.click()
                    item = page.locator(f'#side .item[data-uid="{codex_uid}"]')
                    expect(item).to_be_visible()
                    box = item.bounding_box()
                    item.dispatch_event("pointerdown", {"pointerType": "touch", "clientX": box["x"] + 20, "clientY": box["y"] + 10, "bubbles": True})
                    menu = page.locator("#item-menu")
                    expect(menu).to_be_visible(timeout=3000)
                    stop_item = menu.locator('[data-act="stop"]')
                    expect(stop_item).to_be_visible()
                    bounds = menu.bounding_box()
                    assert bounds and bounds["x"] >= 0 and bounds["x"] + bounds["width"] <= 391, bounds
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/stop") as stopped:
                        stop_item.click()
                    reply = stopped.value.json()
                    assert stopped.value.status == 200 and reply["stage"] == "graceful", reply
                    assert reply["instance_id"] == second["instance_id"], reply
                    expect(page.locator("#session-stop-notice")).to_contain_text("CLI 已在收到 Ctrl-D 后退出")
                    page.wait_for_function("uid => !(T.list || []).some(row => row.uid === uid)", arg=codex_uid, timeout=15000)
                    assert not errors, errors
                    mobile.close()
                    assert {name: path.read_bytes() for name, path in corpus.paths.items()} == native
            finally:
                browser.close()
                # Cleanup only explicitly created instances in this private
                # fixture, protected by their full immutable launch envelope.
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
    print("PASS session stop browser: session_stop capability, desktop header action stops a resumed managed "
          "instance (graceful, request_id, exit explanation, action flips to delete, /api/live exited), "
          "no-op for a session without a running instance, 390px long-press menu stop, native bytes unchanged")


if __name__ == "__main__":
    main()
