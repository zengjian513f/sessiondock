#!/usr/bin/env python3
"""Headless create-and-discard for Claude, Codex, Grok and SSH.

Each source is created through the real new-session dialog, given unsent
composer input, then removed through the header action. A reload and a
second page must not rebuild the row from drafts, pending receipts or
Grok's empty summary.json.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY as DEBUG_BINARY, Corpus, REPO, isolated_server

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
DEFAULT_BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
SOURCES = ("claude", "codex", "grok", "shell")

STAY = """#!/bin/sh
stty -echo 2>/dev/null
printf 'READY\\n'
while IFS= read -r line; do
  case "$line" in quit) exit 0 ;; *) printf 'OK\\n' ;; esac
done
"""

GROK_NEW = r"""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
from urllib.parse import quote
sid = ""
args = sys.argv[1:]
for i, arg in enumerate(args):
    if arg == "--session-id" and i + 1 < len(args):
        sid = args[i + 1]
root = Path(os.environ["SESSIONDOCK_TEST_GROK_ROOT"])
cwd = str(Path(os.getcwd()).resolve())
folder = root / quote(cwd, safe="") / (sid or "unknown")
folder.mkdir(parents=True, exist_ok=True)
(folder / "summary.json").write_text(
    json.dumps({"info": {"id": sid or "unknown", "cwd": cwd}}), encoding="utf-8")
sys.stdout.write("GROK_READY\n")
sys.stdout.flush()
while True:
    line = sys.stdin.readline()
    if not line or line.strip() == "quit":
        break
"""


def fail(area, why, body=""):
    text = body if isinstance(body, str) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:400]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def wait_native(context, base, sid, timeout=4):
    if not sid:
        return None
    deadline = time.monotonic() + timeout
    last = {}
    while time.monotonic() < deadline:
        last = context.request.get(base + "/api/sessions").json()
        for row in last.get("sessions") or []:
            if row.get("sid") == sid:
                return row
        time.sleep(0.05)
    return None


def leftover(context, base, rec, pending_uid, native_uid):
    listed = context.request.get(base + "/api/term/list").json()
    pending = [row for row in listed.get("pending") or []
               if row.get("record_id") == rec["record_id"]]
    drafts = context.request.get(base + "/api/session/conversation/drafts").json()
    draft_uids = {row.get("uid") for row in drafts.get("drafts") or []}
    sessions = context.request.get(base + "/api/sessions").json().get("sessions") or []
    native = [row for row in sessions
              if row.get("uid") == native_uid
              or (rec.get("declared_sid") and row.get("sid") == rec.get("declared_sid"))]
    return pending, {uid for uid in (pending_uid, native_uid) if uid and uid in draft_uids}, native


def assert_gone(context, base, page, rec, pending_uid, native_uid, where):
    pending, drafts, native = leftover(context, base, rec, pending_uid, native_uid)
    if pending:
        fail(where, "pending receipt still listed", pending)
    if drafts:
        fail(where, "conversation/drafts still lists the session", drafts)
    if native:
        fail(where, "native session still listed", native)
    for uid in (pending_uid, native_uid):
        if uid:
            expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(0)


def create_source(page, source, work):
    expect(page.locator("#new-session")).to_be_visible()
    page.locator("#new-session").click()
    expect(page.locator("#new-session-dialog")).to_be_visible()
    page.locator(f'#new-session-form label:has(input[name="new-source"][value="{source}"])').click()
    page.locator("#new-cwd").fill(str(work))
    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/create") as created:
        page.locator("#new-session-go").click()
    response = created.value
    if response.status != 200:
        fail(f"{source} create", f"HTTP {response.status}", response.text())
    receipt = response.json()
    if not receipt.get("running"):
        fail(f"{source} create", "not running", receipt)
    pending_uid = "tmux:" + receipt["name"]
    page.wait_for_function("uid => S.sel === uid", arg=pending_uid, timeout=10000)
    return receipt, pending_uid


def discard_selected(page):
    action = page.locator("#a-session-action")
    if not action.is_visible():
        page.locator("#a-more").click()
    expect(action).to_have_attribute("aria-label", "删除会话")
    action.click()


def run_source(page, context, base, source, work):
    receipt, pending_uid = create_source(page, source, work)
    passed(f"{source}: created running launch_kind={receipt.get('launch_kind')}")

    composer = page.locator("#composer")
    if composer.is_visible() and "hidden" not in (composer.get_attribute("class") or ""):
        page.locator("#cinput").fill("审计下所有")
        page.wait_for_function(
            "text => composerDrafts.get(S.sel)?.text === text", arg="审计下所有", timeout=10000)
        passed(f"{source}: unsent composer input retained")
    else:
        passed(f"{source}: composer hidden (no retained input)")

    native = wait_native(context, base, receipt.get("declared_sid"))
    native_uid = native["uid"] if native else None
    if native_uid:
        page.evaluate("async () => { if (typeof loadSessions === 'function') await loadSessions(true); }")
        try:
            page.wait_for_function(
                "uid => (typeof sidebarSessions === 'function' ? sidebarSessions() : []).some(s => s.uid === uid)",
                arg=native_uid, timeout=8000)
        except Exception:
            dump = page.evaluate("""() => ({
                sessions: (S.sessions || []).map(s => ({uid: s.uid, sid: s.sid, title: s.title})),
                pending: (typeof pendingTmuxSessions === 'function' ? pendingTmuxSessions() : []).map(
                    s => ({uid: s.uid, sid: s.sid, title: s.title})),
                side: [...document.querySelectorAll('#side .item')].map(
                    n => ({uid: n.dataset.uid, text: (n.innerText || '').slice(0, 80)}))
            })""")
            fail(f"{source} native row", f"catalog uid {native_uid} missing from sidebar", dump)
        row = page.locator(f'#side .item[data-uid="{native_uid}"]')
        expect(row).to_have_count(1)
        row.click()
        page.wait_for_function("uid => S.sel === uid", arg=native_uid, timeout=10000)
        passed(f"{source}: native row {native_uid} appeared before first send")
    else:
        passed(f"{source}: no native row before first send")

    # SSH receipts survive exit for recording replay, so they first offer
    # stop. Unused native AI rows still offer direct discard after the merge.
    row = page.locator(f'#side .item[data-uid="{native_uid or pending_uid}"]')
    row.click(button="right")
    menu_stop = page.locator('#item-menu [data-act="stop"]')
    menu_delete = page.locator('#item-menu [data-act="delete"]')
    if source == "shell":
        expect(menu_stop).to_be_visible()
        expect(menu_delete).to_be_hidden()
        with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/kill") as stopped:
            menu_stop.click()
        assert stopped.value.status == 200, stopped.value.text()
        page.wait_for_function(
            "id => T.pending.some(row => row.record_id === id && row.running === false)",
            arg=receipt["record_id"], timeout=15000)
        row.click(button="right")
        passed("shell: sidebar stop exits the session and retains its row")
    expect(menu_stop).to_be_hidden()
    expect(menu_delete).to_be_visible()
    expect(menu_delete).to_have_text("删除会话" if source == "shell" else "丢弃会话")
    page.keyboard.press("Escape")
    discard_selected(page)
    deadline = time.monotonic() + 8
    last = None
    while time.monotonic() < deadline:
        last = leftover(context, base, receipt, pending_uid, native_uid)
        if not any(last):
            break
        time.sleep(0.05)
    else:
        fail(f"{source} discard", "still present after header action", last)
    assert_gone(context, base, page, receipt, pending_uid, native_uid, f"{source} after discard")
    status = context.request.get(
        base + "/api/term/new-status",
        params={"record_id": receipt["record_id"], "instance_id": receipt["instance_id"]})
    body = status.json()
    if status.status != 200 or body.get("discarded") is not True:
        fail(f"{source} discard", "receipt not discarded", body)
    passed(f"{source}: header action removed pending, drafts and native files")

    page.reload(wait_until="networkidle")
    page.wait_for_function("typeof sidebarSessions === 'function'")
    assert_gone(context, base, page, receipt, pending_uid, native_uid, f"{source} after reload")
    passed(f"{source}: reload did not rebuild the row")

    other = context.new_page()
    other.goto(base, wait_until="networkidle")
    other.wait_for_function("typeof sidebarSessions === 'function'")
    assert_gone(context, base, other, receipt, pending_uid, native_uid, f"{source} second page")
    other.close()
    passed(f"{source}: second page did not rebuild the row")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    args = parser.parse_args()
    if os.name != "posix":
        print("SKIP pending_create_discard_browser: POSIX required", flush=True)
        return
    if not PTYHOST.is_file():
        print("SKIP pending_create_discard_browser: target/debug/ptyhost is not built", flush=True)
        return
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="sessiondock-create-discard-") as tmp:
        root = Path(tmp).resolve()
        for name in ("host", "work", "ledger", "bin", "claude", "codex", "grok", "state", "trash", "home"):
            (root / name).mkdir(mode=0o700)
        (root / "bin" / "stay").write_text(STAY)
        (root / "bin" / "stay").chmod(0o700)
        (root / "bin" / "fake-grok.py").write_text(GROK_NEW)
        (root / "bin" / "fake-grok.py").chmod(0o700)
        cfg = root / "launcher.json"
        cfg.touch(mode=0o600)
        cfg.write_text(json.dumps({
            "schema": 2,
            "host_binary": str(PTYHOST.resolve()),
            "host_dir": str(root / "host"),
            "adapters": [],
            "profiles": [
                {"id": "claude-cli-v1", "source": "claude",
                 "executable": str(root / "bin" / "stay"),
                 "args": [], "new_args": ["--session-id", "{session_id}"],
                 "resume_args": ["--resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}},
                {"id": "codex-cli-v1", "source": "codex",
                 "executable": str(root / "bin" / "stay"),
                 "args": [], "resume_args": ["resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}},
                {"id": "grok-cli-v1", "source": "grok",
                 "executable": str(Path(sys.executable).resolve()),
                 "args": [str(root / "bin" / "fake-grok.py")],
                 "new_args": ["--session-id", "{session_id}"],
                 "resume_args": ["--resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color",
                         "PYTHONUNBUFFERED": "1",
                         "SESSIONDOCK_TEST_GROK_ROOT": str(root / "grok")}},
            ],
        }))
        cfg.chmod(0o600)
        init = subprocess.run(
            [str(binary), "--initialize-lifecycle", str(root / "ledger")],
            cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        if init.returncode:
            fail("initialize-lifecycle", init.stderr.decode() or init.stdout.decode())
        corpus = Corpus(root)
        with isolated_server(
            corpus, binary, state_dir=root / "state", host_dir=root / "host",
            lifecycle_dir=root / "ledger", launcher_config=cfg, trash_dir=root / "trash",
        ) as (base, _), sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                context = browser.new_context(
                    viewport={"width": 1280, "height": 900}, service_workers="block")
                context.route(
                    "**/*",
                    lambda route: route.continue_() if route.request.url.startswith(base + "/")
                    else route.abort())
                page = context.new_page()
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("dialog", lambda dialog: dialog.accept())
                page.goto(base, wait_until="networkidle")
                for source in SOURCES:
                    run_source(page, context, base, source, root / "work")
                if errors:
                    fail("pageerror", errors)
            finally:
                browser.close()
    print("PASS pending_create_discard_browser: claude, codex, grok, shell create+discard stay gone",
          flush=True)


if __name__ == "__main__":
    main()
