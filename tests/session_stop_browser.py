#!/usr/bin/env python3
"""Legacy stop control under the Rust `session_stop` capability.

A synthetic Codex session is resumed through the explicit start action
(`/api/term/takeover`, fake CLI = free shell that exits on Ctrl-D). The header
"停止会话" action then posts `/api/session/stop` with a `request_id`, the page
updates the existing running state without a success toast, the console closes and the action flips back to "启动会话". A session that has no
running instance succeeds as a no-op, and being idle it asks no confirmation. The
mobile (390 px) sidebar long-press menu stops a fresh resume the same way. No
model binary, native CLI home or production host is touched.
"""

import json
import os
import re
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit
from playwright.sync_api import sync_playwright, expect
from pending_create_discard_browser import create_source
from history_fixtures import REPO, BINARY, Corpus, claude_row, codex_row, codex_message, isolated_server
from private_hosts import private_hosts
from popups import on_popup

CODEX_SID = "8f3c1d2e-4a5b-4c6d-8e7f-90a1b2c3d4e5"
SLOW_SIDS = [f"00000000-0000-4000-8000-{index:012d}" for index in range(8)]
FAKE_CODEX = """#!/bin/sh
case "$2" in 00000000-0000-4000-8000-*) exec "$STOP_TEST_PYTHON" "$STOP_TEST_SCRIPT" "$@" ;; esac
printf 'FAKE_CODEX_ARGV'
for arg in "$@"; do printf ' [%s]' "$arg"; done
printf '\\n'
exec /bin/sh -c 'stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; while IFS= read -r line; do case "$line" in quit) exit 0 ;; *) printf "RS_INPUT_OK\\n" ;; esac; done'
"""
SLOW_CODEX = """import os, sys, time, tty
from pathlib import Path
tty.setraw(0)
sid = sys.argv[2]
events = Path(os.environ['STOP_TEST_EVENTS']) / sid
os.write(1, ('STOP_READY_' + sid + '\\r\\n').encode())
while True:
    key = os.read(0, 1)
    if key == b'\\x04':
        with events.open('a') as stream:
            stream.write(str(time.monotonic()) + '\\n')
"""
def session_action(page):
    button = page.locator("#a-session-toggle")
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
    with tempfile.TemporaryDirectory(prefix="sessiondock-session-stop-") as temporary, private_hosts(Path(temporary)):
        root = Path(temporary).resolve()
        for name in ["host", "work", "work/codex-area", "ledger", "bin", "claude", "codex", "grok", "events"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        corpus.put(CODEX_SID, "codex", [codex_row("session_meta", {"id": CODEX_SID, "cwd": str(root / "work/codex-area")}),
            codex_message("user", "Synthetic managed stop target")], [])
        corpus.put("synthetic-unrelated-claude", "claude", [
            claude_row("synthetic-unrelated-claude", "user", "u0", None, "Synthetic session without instance"),
            claude_row("synthetic-unrelated-claude", "assistant", "a0", "u0", "Synthetic answer")], [])
        for sid in SLOW_SIDS:
            corpus.put(sid, "codex", [codex_row("session_meta", {"id": sid, "cwd": str(root / "work/codex-area")}),
                codex_message("user", "Synthetic slow stop " + sid)], [])
        slow_uids = [corpus.uid(sid) for sid in SLOW_SIDS]
        native = {name: path.read_bytes() for name, path in corpus.paths.items()}
        codex_uid = corpus.uid(CODEX_SID)
        other_uid = corpus.uid("synthetic-unrelated-claude")
        (root / "bin/fake-codex").write_text(FAKE_CODEX)
        (root / "bin/fake-codex").chmod(0o700)
        (root / "bin/slow.py").write_text(SLOW_CODEX)
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [
                {"id": "codex-cli-v1", "source": "codex", "executable": str(root / "bin/fake-codex"),
                 "args": [], "resume_args": ["resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color",
                         "STOP_TEST_PYTHON": sys.executable, "STOP_TEST_SCRIPT": str(root / "bin/slow.py"),
                         "STOP_TEST_EVENTS": str(root / "events")}}]}))
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
                        on_popup(page, on_dialog)
                        page.goto(base, wait_until="networkidle")
                        assert page.evaluate("SessionDockCapabilities.config.session_stop") is True
                        return page

                    # ---- Desktop: resume, then stop through the header action.
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    page = watch(context)
                    page.locator(f'#side .item[data-uid="{codex_uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic managed stop target")
                    # Before any instance exists the first action starts this native session.
                    expect(session_action(page)).to_have_attribute("aria-label", "启动会话")
                    expect(session_action(page).locator('use')).to_have_attribute('href', '#i-power')
                    expect(session_action(page).locator('span')).to_be_hidden()
                    expect(session_action(page)).to_have_css('width', '28px')
                    expect(session_action(page)).to_have_text('启动会话')
                    page.keyboard.press("Escape")
                    page.route("**/api/term/takeover", lambda route:
                               route.fulfill(status=500, json={"error":"synthetic start refused"}))
                    before_start = len(dialogs)
                    session_action(page).click()
                    page.wait_for_function("!ConsoleUI.busy.has(S.sel)")
                    assert any(kind == "alert" and "synthetic start refused" in message
                               for kind, message in dialogs[before_start:]), dialogs[before_start:]
                    page.unroute("**/api/term/takeover")
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover") as taken:
                        session_action(page).click()
                    resumed = taken.value.json()
                    assert taken.value.status == 200 and resumed["launch_kind"] == "resume", resumed
                    expect(page.locator("#termpane")).to_be_visible()
                    page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                    wait_xterm(page, "RS_SHELL_READY")
                    page.wait_for_function("uid => (T.list || []).some(row => row.uid === uid && row.instance_id)", arg=codex_uid)
                    action = session_action(page)
                    expect(action).to_have_attribute("aria-label", "停止会话")
                    expect(action.locator('use')).to_have_attribute('href', '#i-power')
                    expect(action.locator('span')).to_be_hidden()
                    expect(action).to_have_css('width', '28px')
                    expect(action).to_have_text('停止会话')
                    expect(page.locator("#a-session-action")).to_have_attribute("aria-label", "删除当前会话")
                    expect(page.locator("#a-session-action")).to_have_attribute("aria-disabled", "true")
                    expect(page.locator('.dhead-actions .session-menu-action').first).to_have_attribute('id','a-session-toggle')
                    page.evaluate('''() => {
                        window.stopAttentionFlashes=[];
                        window.stopAttentionObserver=new MutationObserver(() => {
                            for (const node of document.querySelectorAll('#dlive, #side .item.sel > .ico > .item-status, #composer-input-status')) {
                                if (node.classList.contains('input-attention') && !node.classList.contains('input-question'))
                                    stopAttentionFlashes.push({id:node.id,text:node.textContent});
                            }
                        });
                        stopAttentionObserver.observe(document.body,{subtree:true,childList:true,attributes:true});
                    }''')
                    # A user-initiated failure or uncertain result requires acknowledgement.
                    for status, payload, expected in (
                        (500, {"error":"synthetic stop refused"}, "停止失败"),
                        (200, {"ok":True,"stopped":False,"stage":"uncertain"}, "停止结果不确定"),
                    ):
                        page.route("**/api/session/stop", lambda route, request, status=status, payload=payload:
                                   route.fulfill(status=status, json=payload))
                        before_errors = len(dialogs)
                        session_action(page).click()
                        page.wait_for_function("!document.querySelector('#a-session-toggle').disabled")
                        assert any(kind == "alert" and expected in message
                                   for kind, message in dialogs[before_errors:]), dialogs[before_errors:]
                        expect(page.locator("#float-stack > :visible")).to_have_count(0)
                        page.unroute("**/api/session/stop")
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/stop") as stopped:
                        action.click()
                    # No turn event yet (state unknown): the stop still asks first.
                    assert dialogs[-1][0] == "confirm" and "停止会话" in dialogs[-1][1], dialogs
                    reply = stopped.value.json()
                    assert stopped.value.status == 200, stopped.value.text()
                    assert reply["stage"] == "graceful" and reply["stopped"] is True and reply["tmux"] is False, reply
                    assert reply["instance_id"] == resumed["instance_id"] and reply["record_id"] == resumed["record_id"], reply
                    assert stops[-1]["uid"] == codex_uid and stops[-1].get("request_id"), stops[-1]
                    notice = page.locator("#float-stack > :visible")
                    expect(notice).to_have_count(0)
                    expect(page.locator("#float-stack > :visible")).to_have_count(0)
                    # The pane closes on exit and the console
                    # button stays usable as "接管会话" because the source has a
                    # resume-capable profile; the exit explanation is remembered,
                    # the instance leaves the list and the header action flips
                    # back to delete.
                    page.wait_for_function("uid => T.ended.has(uid)", arg=codex_uid, timeout=15000)
                    expect(page.locator("#termpane")).to_be_hidden()
                    assert page.evaluate("name => !T.views.has(name) && !T.openViews.has(name)", resumed["name"])
                    assert "保留" not in page.evaluate("uid => T.ended.get(uid).reason", codex_uid)
                    expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false", timeout=15000)
                    page.wait_for_function("uid => !(T.list || []).some(row => row.uid === uid)", arg=codex_uid, timeout=15000)
                    assert page.evaluate("uid => S.live.has(uid)", codex_uid) is False
                    expect(session_action(page)).to_have_attribute("aria-label", "启动会话")
                    expect(page.locator("#a-session-action")).not_to_have_attribute("aria-disabled", "true")
                    expect(page.locator('.dhead-actions .session-menu-action').first).to_have_attribute('id','a-session-toggle')
                    page.wait_for_timeout(1200)
                    assert page.evaluate('stopAttentionFlashes') == [], page.evaluate('stopAttentionFlashes')
                    page.evaluate('stopAttentionObserver.disconnect()')
                    page.keyboard.press("Escape")
                    # Successful stop does not add any floating status card.
                    expect(notice).to_have_count(0)
                    deadline = time.monotonic() + 10
                    while list((root / "host").glob("*.json")) and time.monotonic() < deadline:
                        time.sleep(0.05)
                    assert not list((root / "host").glob("*.json")), "host record must be cleaned after the exit"
                    live = json.loads(opener.open(base + "/api/live?force=1", timeout=10).read())
                    assert live["managed"]["sessions"][codex_uid]["state"] == "exited", live["managed"]["sessions"]

                    # Exit from the actual terminal, as in the diagnostic audit:
                    # normal EOF must also stay silent without a /session/stop response.
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover") as taken:
                        page.locator("#a-term").click()
                    direct = taken.value.json()
                    wait_xterm(page, "RS_SHELL_READY")
                    page.locator("#termpane .xterm-helper-textarea").focus()
                    page.keyboard.type("quit")
                    page.keyboard.press("Enter")
                    page.wait_for_function("name => !T.views.has(name)", arg=direct["name"])
                    expect(notice).to_have_count(0)
                    expect(page.locator("#float-stack > :visible")).to_have_count(0)
                    expect(page.locator("#termpane")).to_be_hidden()
                    assert page.evaluate("name => !T.openViews.has(name)", direct["name"])
                    page.wait_for_function("uid => !(T.list || []).some(row => row.uid === uid)", arg=codex_uid)
                    page.evaluate("refreshLive(true)")

                    # ---- A session without any running instance:
                    # stopping succeeds as a no-op and the stale live marker clears.
                    # `S.live` can hold stale entries in Rust mode (no live poll), so the
                    # stop control can still be reached for such a session.
                    page.locator(f'#side .item[data-uid="{other_uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic session without instance")
                    expect(session_action(page)).to_have_attribute("aria-label", "启动会话")
                    page.keyboard.press("Escape")
                    page.evaluate("uid => { S.live.add(uid); paintLive(); }", other_uid)
                    action = session_action(page)
                    expect(action).to_have_attribute("aria-label", "停止会话")
                    expect(page.locator('.dhead-actions .session-menu-action').first).to_have_attribute('id','a-session-toggle')
                    before = len(dialogs)
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/stop") as stopped:
                        action.click()
                    assert stopped.value.status == 200, stopped.value.text()
                    result = stopped.value.json()
                    assert result.get("ok") is True and result.get("stopped") is False, result
                    assert result.get("external_detection") == "proc_scan", result
                    expect(notice).to_have_count(0)
                    # Its transcript ends with a finished turn: stopping an idle session asks nothing.
                    assert page.evaluate("uid => sessionTurn(uid)", other_uid) == "idle"
                    assert len(dialogs) == before, dialogs[before:]
                    assert not errors, errors
                    # ---- Multi-select: real managed stop, a failed target, retry,
                    # and ended selections retained without an extra stop request.
                    page.locator(f'#side .item[data-uid="{codex_uid}"]').click()
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover"):
                        page.locator("#a-term").click()
                    wait_xterm(page, "RS_SHELL_READY")
                    page.wait_for_function("uid => sessionStoppable(uid)", arg=codex_uid)
                    page.evaluate("uid => { S.live.add(uid); paintLive(); }", other_uid)
                    page.locator(f'#side .item[data-uid="{codex_uid}"]').click(button="right")
                    page.locator('#item-menu [data-act="pick"]').click()
                    page.locator(f'#side .item[data-uid="{other_uid}"]').click()
                    bulk = page.locator("#side-pick-stop")
                    expect(bulk).to_have_text("停止 (2)")
                    before = len(dialogs)
                    before_stops = len(stops)
                    def fail_other(route):
                        if route.request.post_data_json["uid"] == other_uid:
                            route.fulfill(status=409, content_type="application/json",
                                          body=json.dumps({"error": "synthetic unknown host state"}))
                        else:
                            route.continue_()
                    context.route("**/api/session/stop", fail_other)
                    bulk.click()
                    expect(bulk).to_have_text("已停止 1/2")
                    expect(notice).to_have_count(0)
                    page.locator("#side-stop-summary").click()
                    expect(page.locator("#side-stop-errors")).to_contain_text("synthetic unknown host state")
                    page.wait_for_function("!sessionStopBusy")
                    page.evaluate("uid => { S.live.add(uid); paintLive(); }", other_uid)
                    expect(bulk).to_be_enabled()
                    assert len(dialogs) == before + 1, dialogs[before:]
                    assert len(stops) == before_stops + 2, stops[before_stops:]
                    assert len({entry["request_id"] for entry in stops[before_stops:]}) == 2
                    expect(page.locator("#side-picked")).to_have_text("已选 2 项")
                    context.unroute("**/api/session/stop", fail_other)
                    # Exited session remains selected, but is skipped on retry.
                    page.wait_for_function("uid => !sessionStoppable(uid)", arg=codex_uid)
                    expect(bulk).to_have_text("已停止 1/2")
                    def uncertain_stop(route):
                        route.fulfill(status=200, content_type="application/json",
                                      body=json.dumps({"ok": True, "stopped": False, "stage": "uncertain"}))
                    context.route("**/api/session/stop", uncertain_stop)
                    bulk.click()
                    expect(bulk).to_have_text("已停止 0/1")
                    expect(page.locator("#side-stop-summary")).to_have_text("未确认 1")
                    expect(notice).to_have_count(0)
                    page.wait_for_function("!sessionStopBusy")
                    assert len(stops) == before_stops + 3, "uncertain stop must not retry itself"
                    context.unroute("**/api/session/stop", uncertain_stop)
                    page.evaluate("uid => { S.live.add(uid); paintLive(); }", other_uid)
                    bulk.click()
                    expect(bulk).to_have_text("已停止 1/1")
                    expect(notice).to_have_count(0)
                    assert len(stops) == before_stops + 4 and stops[-1]["uid"] == other_uid
                    page.evaluate("() => { S.live.clear(); paintLive(); }")
                    expect(bulk).to_be_disabled()
                    expect(page.locator("#side-pick-delete")).to_be_enabled()
                    page.locator("#side-pick-cancel").click()
                    expect(page.locator("#side-tools")).to_be_hidden()
                    assert not errors, errors
                    # Pending shell receipts use instance identity and remain listed.
                    receipt, pending_uid = create_source(page, "shell", root / "work")
                    pending_status = page.locator(f'#side .item[data-uid="{pending_uid}"] > .ico > .item-status')
                    expect(pending_status).to_have_class("item-status visible tmux")
                    page.locator(f'#side .item[data-uid="{pending_uid}"]').click(button="right")
                    page.locator('#item-menu [data-act="pick"]').click()
                    expect(bulk).to_have_text("停止 (1)")
                    before = len(dialogs)
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/kill") as killed:
                        bulk.click()
                    result = killed.value.json()
                    assert killed.value.status == 200 and result["state"] == "exited", result
                    assert result["record_id"] == receipt["record_id"] and result["instance_id"] == receipt["instance_id"]
                    expect(bulk).to_have_text("已停止 1/1")
                    expect(notice).to_have_count(0)
                    page.wait_for_function("!sessionStopBusy")
                    expect(bulk).to_be_disabled()
                    expect(page.locator(f'#side .item[data-uid="{pending_uid}"]')).to_be_visible()
                    # The exited row keeps its entry but loses the running (blue) dot.
                    page.wait_for_function("uid => !document.querySelector(`#side .item[data-uid=\"${uid}\"]`).classList.contains('live')", arg=pending_uid)
                    expect(pending_status).to_be_hidden()
                    assert len(dialogs) == before + 1
                    assert not errors, errors
                    page.locator("#side-pick-cancel").click()

                    # Change concurrency through settings, then exercise the actual pool.
                    page.locator("#settings").click()
                    page.get_by_role("tab", name="功能", exact=True).click()
                    expect(page.locator("#setting-stop-concurrency")).to_have_value("6")
                    page.locator("#setting-stop-concurrency").select_option("4")
                    page.locator("#settings-dialog .modal-close").click()
                    # Eight real hosts ignore EOF: independent EOF waits overlap,
                    # and later requests fill the four configured slots as they free up.
                    for sid, uid in zip(SLOW_SIDS, slow_uids):
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                        with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover"):
                            page.locator("#a-term").click()
                        wait_xterm(page, "STOP_READY_" + sid)
                    page.wait_for_function("uids => uids.every(sessionStoppable)", arg=slow_uids)
                    page.locator(f'#side .item[data-uid="{slow_uids[0]}"]').click(button="right")
                    page.locator('#item-menu [data-act="pick"]').click()
                    for uid in slow_uids[1:]:
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(bulk).to_have_text("停止 (8)")
                    before_stops = len(stops)
                    outstanding, peak = set(), [0]
                    def on_stop_request(request):
                        if urlsplit(request.url).path == "/api/session/stop":
                            outstanding.add(request)
                            peak[0] = max(peak[0], len(outstanding))
                    def on_stop_response(response):
                        outstanding.discard(response.request)
                        if urlsplit(response.url).path == "/api/session/stop" and not response.ok:
                            print(f"STOP response {response.status}: {response.text()}", flush=True)
                    context.on("request", on_stop_request)
                    context.on("response", on_stop_response)
                    started = time.monotonic()
                    bulk.click()
                    expect(bulk).to_have_text("已停止 0/8")
                    expect(bulk).to_have_attribute("aria-busy", "true")
                    expect(page.locator("#side-pick-cancel")).to_be_disabled()
                    expect(notice).to_have_count(0)
                    # Observe the rendered UI, not an earlier state mutation.
                    expect(bulk).to_have_text(re.compile(r"已停止 [1-7]/8"), timeout=15000)
                    expect(bulk).to_have_text("已停止 8/8", timeout=15000)
                    page.wait_for_function("!sessionStopBusy")
                    elapsed = time.monotonic() - started
                    assert len(stops) == before_stops + 8 and peak[0] == 4, (stops[before_stops:], peak)
                    events = [list(map(float, (root / "events" / sid).read_text().splitlines())) for sid in SLOW_SIDS]
                    assert all(len(ticks) == 2 for ticks in events), events
                    # Other lifecycle writes remain barriers, so admission need
                    # not start all four together; independent EOF waits must overlap.
                    ordered = sorted(events, key=lambda ticks: ticks[0])
                    assert ordered[1][0] < ordered[0][1], events
                    assert elapsed < 16, elapsed  # Serial escalation requires at least 8 * 2.4 seconds.
                    expect(bulk).to_be_disabled()
                    expect(notice).to_have_count(0)
                    expect(page.locator("#side-stop-details")).to_be_hidden()
                    expect(page.locator("#side-picked")).to_have_text("已选 8 项")
                    page.locator(f'#side .item[data-uid="{slow_uids[0]}"]').click()
                    expect(bulk).to_have_text("停止")
                    context.remove_listener("request", on_stop_request)
                    context.remove_listener("response", on_stop_response)
                    print(f"PASS parallel stop: 8 real hosts in {elapsed:.2f}s, configured peak 4 requests, overlapping EOF waits, inline progress", flush=True)
                    page.locator("#side-pick-cancel").click()

                    # Two actual pages stopping the same instance must not
                    # duplicate its EOF sequence or escalate concurrently.
                    page.locator(f'#side .item[data-uid="{slow_uids[0]}"]').click()
                    (root / "events" / SLOW_SIDS[0]).unlink()
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover") as taken:
                        page.locator("#a-term").click()
                    duplicate_target = taken.value.json()
                    page.wait_for_function("""([name, text]) => {
                      const b = T.views.get(name)?.term?.buffer?.active;
                      return b && Array.from({length:b.length}, (_,i) => b.getLine(i)?.translateToString() || '').join('\\n').includes(text);
                    }""", arg=[duplicate_target["name"], "STOP_READY_" + SLOW_SIDS[0]])
                    other_page = context.new_page()
                    on_popup(other_page, lambda dialog: dialog.accept())
                    other_page.on("pageerror", lambda error: errors.append(str(error)))
                    other_page.goto(base, wait_until="networkidle")
                    other_page.locator(f'#side .item[data-uid="{slow_uids[0]}"]').click()
                    other_page.wait_for_function("uid => sessionStoppable(uid)", arg=slow_uids[0])
                    first_action, second_action = session_action(page), session_action(other_page)
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/stop") as first_stop, \
                            other_page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/stop") as duplicate_stop:
                        first_action.click()
                        second_action.click()
                    stages = sorted([first_stop.value.json()["stage"], duplicate_stop.value.json()["stage"]])
                    assert stages == ["already_exited", "stopped"], stages
                    assert len((root / "events" / SLOW_SIDS[0]).read_text().splitlines()) == 2
                    other_page.close()
                    assert not errors, errors
                    print("PASS duplicate stop: two browser pages, one EOF/escalation sequence for the same instance", flush=True)
                    context.close()

                    # ---- Mobile: start through the sidebar, then stop from long press.
                    mobile = browser.new_context(viewport={"width": 390, "height": 844}, service_workers="block", has_touch=True)
                    page = watch(mobile)
                    page.locator(f'#side .item[data-uid="{codex_uid}"]').click(button="right")
                    start_item=page.locator('#item-menu [data-act="stop"]')
                    expect(start_item).to_have_text('启动会话')
                    expect(start_item).not_to_have_attribute('aria-disabled','true')
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover") as taken:
                        start_item.click()
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
                    # Complete the long-press gesture, including its suppressed
                    # click, so the next deliberate row click can open a session.
                    item.dispatch_event("pointerup", {"pointerType": "touch", "bubbles": True})
                    item.dispatch_event("click", {"bubbles": True})
                    stop_item = menu.locator('[data-act="stop"]')
                    expect(stop_item).to_be_visible()
                    expect(stop_item).not_to_have_attribute('aria-disabled', 'true')
                    delete_item = menu.locator('[data-act="delete"]')
                    expect(delete_item).to_be_visible()
                    expect(delete_item).to_have_attribute('aria-disabled', 'true')
                    delete_item.click(force=True)
                    expect(menu).to_be_visible()
                    expect(menu.locator('[data-act="group"]')).to_be_visible()
                    expect(menu.locator('button:visible')).to_have_count(10)
                    expect(menu.locator('button:visible').first).to_have_attribute('data-act','stop')
                    bounds = menu.bounding_box()
                    assert bounds and bounds["x"] >= 0 and bounds["x"] + bounds["width"] <= 391, bounds
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/session/stop") as stopped:
                        stop_item.click()
                    reply = stopped.value.json()
                    assert stopped.value.status == 200 and reply["stage"] == "graceful", reply
                    assert reply["instance_id"] == second["instance_id"], reply
                    expect(page.locator("#float-stack > :visible")).to_have_count(0)
                    page.wait_for_function("uid => !(T.list || []).some(row => row.uid === uid)", arg=codex_uid, timeout=15000)
                    assert not errors, errors
                    # Wait for the stop refresh before opening another menu;
                    # paintLive closes a menu rendered from the old running row.
                    page.wait_for_function("uid => !sessionStoppable(uid)", arg=codex_uid)
                    # Use a fresh real host for the mobile bulk stop. A synthetic
                    # S.live marker can be removed by the concurrent live poll.
                    item.click()
                    if not page.locator("#a-term").is_visible():
                        page.locator("#a-more").click()
                    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover"):
                        page.locator("#a-term").click()
                    wait_xterm(page, "RS_SHELL_READY")
                    page.wait_for_function("uid => sessionStoppable(uid)", arg=codex_uid)
                    page.locator(".mobile-back").first.click()
                    # The multi-select stop action fits the mobile toolbar.
                    item.click(button="right")
                    page.locator('#item-menu [data-act="pick"]').click()
                    expect(page.locator("#side-pick-stop")).to_be_visible()
                    expect(page.locator("#side-pick-stop")).to_be_enabled()
                    page.locator("#side-pick-stop").click()
                    expect(page.locator("#side-pick-stop")).to_have_text("已停止 1/1")
                    page.wait_for_function("!sessionStopBusy")
                    expect(page.locator("#float-stack > :visible")).to_have_count(0)
                    for selector in ("#side-pick-stop", "#side-pick-delete", "#side-pick-cancel"):
                        bounds = page.locator(selector).bounding_box()
                        assert bounds and bounds["x"] >= 0 and bounds["x"] + bounds["width"] <= 391, bounds
                    page.locator("#side-pick-cancel").click()
                    # Resume once more, then stop the real private host via the
                    # cleanup dialog. Age only the synthetic transcript mtime.
                    old_time = time.time() - 3 * 86400
                    os.utime(corpus.paths[CODEX_SID], (old_time, old_time))
                    item.click()
                    session_action(page).click()
                    wait_xterm(page, "RS_SHELL_READY")
                    page.wait_for_function("uid => sessionStoppable(uid)", arg=codex_uid)
                    page.locator(".mobile-back").first.click()
                    page.evaluate('() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))')
                    if not page.locator('#session-cleanup').is_visible():
                        page.locator('#header-more-btn').click()
                    page.locator('#session-cleanup').click()
                    expect(page.locator('#session-cleanup-status')).to_contain_text('找到 1 个')
                    with page.expect_response(lambda response: urlsplit(response.url).path == '/api/session/stop') as cleaned:
                        page.locator('#session-cleanup-start').click()
                    reply = cleaned.value.json()
                    assert cleaned.value.status == 200 and reply['stage'] == 'graceful', reply
                    expect(page.locator('#session-cleanup-status')).to_contain_text('已停止 1/1，失败 0，未确认 0')
                    expect(page.locator('#session-cleanup-close')).to_have_text('完成')
                    page.locator('#session-cleanup-close').click()
                    page.wait_for_function("uid => !sessionStoppable(uid)", arg=codex_uid)
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
          "instance (graceful, request_id, exit explanation, action flips back to start with independent delete, /api/live exited), "
          "no-op, bulk partial failure/retry/uncertainty, concurrent stop and duplicate serialization, pending shell retained, "
          "390px long-press menu and toolbar, native bytes unchanged")


if __name__ == "__main__":
    main()
