#!/usr/bin/env python3
"""Headless create-and-discard for Claude, Codex, Grok and SSH.

Each source is created through the real new-session dialog, given unsent
composer input, then removed through the header action. A reload and a
second page must not rebuild the row from drafts, pending receipts or
Grok's empty summary.json.
"""
from __future__ import annotations

from browser_runtime import js, scoped_frontend
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY as DEBUG_BINARY, Corpus, REPO, isolated_server
from popups import on_popup

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


def pending_live_identity(browser, base):
    """Observed pending liveness belongs to a UID and receipt/host identity."""
    if not scoped_frontend():
        return
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    try:
        page = context.new_page()
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.goto(base, wait_until="networkidle")
        page.wait_for_function("window.SessionDockRuntime?.terminal.state.listLoaded && "
                               "window.SessionDockRuntime.core.state.catalog.sig")
        # Accepted receipt/list state, with polling paused so it cannot repair
        # an incorrect paint before we inspect it. Simultaneous receipts have
        # separate UIDs/instances; their observations must remain independent.
        page.evaluate("""() => {
          const runtime = window.SessionDockRuntime;
          runtime.core.network.pause('idle');
          const receipt = {name: 'observed-pending', source: 'codex', cwd: '/synthetic/pending',
            title: 'Observed pending', record_id: 'observed-record', instance_id: 'observed-instance',
            started: 1700000000, stale: false, running: true, state: 'running'};
          const neighbor = {...receipt, name: 'neighbor-pending', title: 'Neighbor pending',
            record_id: 'neighbor-record', instance_id: 'neighbor-instance'};
          runtime.terminal.state.pending = [receipt, neighbor];
          runtime.terminal.state.list = [neighbor];
          runtime.core.state.sidebar.closed.clear();
        }""")
        row = page.locator('#side .item[data-uid="tmux:observed-pending"]')
        neighbor = page.locator('#side .item[data-uid="tmux:neighbor-pending"]')
        live = re.compile(r'\blive\b')
        live_tmux = re.compile(r'\blive-tmux\b')
        visible = re.compile(r'\bvisible\b')
        expect(row).to_have_class(live)
        expect(neighbor).to_have_class(live)
        row_handle, neighbor_handle = row.element_handle(), neighbor.element_handle()
        page.evaluate("window.SessionDockRuntime.status.paintLive()")
        expect(row).not_to_have_class(live)
        expect(row).not_to_have_class(live_tmux)
        expect(row.locator('.item-status')).not_to_have_class(visible)
        expect(neighbor).to_have_class(live)
        expect(neighbor.locator('.item-status')).to_have_class(visible)
        page.evaluate("""() => {
          const runtime = window.SessionDockRuntime;
          window.__observedPendingRows = runtime.core.pending.pendingTmuxSessions();
          window.__pendingGroupings = 0;
          const groupBy = runtime.sidebarView.groupBy;
          runtime.sidebarView.groupBy = (...args) => { __pendingGroupings++; return groupBy(...args); };
        }""")
        # Nesting reprojections do not call paintLive: a row-object-keyed
        # observation would fall back to !stale and incorrectly resurrect it.
        nesting = page.get_by_role('button', name='分层显示', exact=True)
        for pressed in ('true', 'false'):
            before = page.evaluate('__pendingGroupings')
            nesting.click()
            expect(nesting).to_have_attribute('aria-pressed', pressed)
            assert page.evaluate('__pendingGroupings') > before
            expect(row).not_to_have_class(live)
            expect(row.locator('.item-status')).not_to_have_class(visible)
            expect(neighbor).to_have_class(live)
        page.locator('#livecount').click()
        expect(page.locator('#livecount')).to_have_attribute('aria-checked', 'true')
        expect(row).to_have_count(1)
        expect(row).not_to_have_class(live)
        page.locator('#allcount').click()
        expect(page.locator('#allcount')).to_have_attribute('aria-checked', 'true')
        before = page.evaluate('__pendingGroupings')
        page.evaluate("""async () => {
          const runtime = window.SessionDockRuntime, uid = 'tmux:observed-pending';
          const receipt = runtime.terminal.state.pending.find(row => 'tmux:' + row.name === uid);
          const draft = runtime.composer.restoreComposerDraftRecord({text: 'unsent', session: receipt}, uid);
          runtime.composer.composerDrafts.set(uid, draft);
          await new Promise(requestAnimationFrame);
          draft.text = 'changed unsent draft';
          draft.session = {...receipt, title: 'Draft-only title'};
          draft.inputStatus = {state: 'starting', code: 'cli_starting'};
          await new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)));
        }""")
        assert page.evaluate('__pendingGroupings') == before
        assert page.evaluate("window.SessionDockRuntime.core.pending.pendingTmuxSessions() === __observedPendingRows")
        expect(row.locator('.t')).to_have_text('Observed pending')
        expect(row).not_to_have_class(live)
        expect(row.locator('.item-status')).not_to_have_class(visible)
        assert row.evaluate('(node, old) => node === old', row_handle)
        assert neighbor.evaluate('(node, old) => node === old', neighbor_handle)
        # A decoded receipt with changed metadata gets a new projected object,
        # but the same UID/instance keeps the already observed inactive state.
        page.evaluate("""() => {
          const terminal = window.SessionDockRuntime.terminal.state;
          terminal.pending = terminal.pending.map(row => row.name === 'observed-pending'
            ? {...row, title: 'Updated receipt title'} : row);
        }""")
        expect(row.locator('.t')).to_have_text('Updated receipt title')
        assert page.evaluate("window.SessionDockRuntime.core.pending.pendingTmuxSessions()[0] !== __observedPendingRows[0]")
        expect(row).not_to_have_class(live)
        expect(row.locator('.item-status')).not_to_have_class(visible)
        expect(neighbor).to_have_class(live)
        assert row.evaluate('(node, old) => node === old', row_handle)
        # Same endpoint/UID and record, new instance: it starts with the normal
        # receipt default until observed, rather than inheriting old false.
        # Receipts without an instance use record identity with the same rule.
        for identity in ({'instance_id': 'replacement-instance', 'record_id': 'observed-record'},
                         {'instance_id': None, 'record_id': 'record-only-a'},
                         {'instance_id': None, 'record_id': 'record-only-b'}):
            page.evaluate("""identity => {
              const terminal = window.SessionDockRuntime.terminal.state;
              terminal.pending = terminal.pending.map(row => {
                if (row.name !== 'observed-pending') return row;
                const next = {...row, ...identity};
                if (next.instance_id === null) delete next.instance_id;
                return next;
              });
            }""", identity)
            expect(row).to_have_class(live)
            expect(row).to_have_class(live_tmux)
            expect(row.locator('.item-status')).to_have_class(visible)
            assert row.evaluate('(node, old) => node === old', row_handle)
            page.evaluate("window.SessionDockRuntime.status.paintLive()")
            expect(row).not_to_have_class(live)
            expect(row.locator('.item-status')).not_to_have_class(visible)
            expect(neighbor).to_have_class(live)
            assert neighbor.evaluate('(node, old) => node === old', neighbor_handle)
        assert not errors, errors
        passed('pending: inactive observation survives controls/drafts; UID, instance and record isolation')
    finally:
        context.close()


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
    page.wait_for_function(js("uid => S.sel === uid", 'uid => runtime.core.state.selection.sel === uid'), arg=pending_uid, timeout=10000)
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
            js("text => composerDrafts.get(S.sel)?.text === text", 'text => runtime.composer.composerDrafts.get(runtime.core.state.selection.sel)?.text === text'), arg="审计下所有", timeout=10000)
        passed(f"{source}: unsent composer input retained")
    else:
        passed(f"{source}: composer hidden (no retained input)")

    if source == "codex":
        page.wait_for_function(js("""() => {
            const draft = composerDrafts.get(S.sel);
            return draft && draft.savedVersion > 0 && draft.savedVersion === draft.editVersion;
        }""", """() => {
            const draft = runtime.composer.composerDrafts.get(runtime.core.state.selection.sel);
            return draft && draft.savedVersion > 0 && draft.savedVersion === draft.editVersion;
        }"""))
        # No native JSONL exists yet. Restore from the durable launch receipt,
        # including when it arrives after the native catalog on reload.
        for width in (1280, 390):
            page.set_viewport_size({"width": width, "height": 900})
            if width == 390:
                page.locator(f'#side .item[data-uid="{pending_uid}"]').click()
            held = []
            def hold_term_list(route):
                held.append((route, route.fetch()))
            page.route("**/api/term/list", hold_term_list)
            if scoped_frontend():
                # ESM composes the terminal controller with the core. Delay its
                # actual HTTP receipt, so the catalog must win startup first.
                page.reload(wait_until="domcontentloaded")
                page.wait_for_function("window.SessionDockRuntime?.core.state.catalog.sig")
                assert page.evaluate("window.SessionDockRuntime.terminal.state.listLoaded") is False
                assert page.evaluate("window.SessionDockRuntime.terminal.state.pending.length") == 0
            else:
                scripts = []
                def hold_term_script(route):
                    scripts.append((route, route.fetch()))
                page.route("**/term.js?*", hold_term_script)
                page.reload(wait_until="commit")
                page.wait_for_function(js("typeof S !== 'undefined' && S.sig", "typeof runtime.core.state !== 'undefined' && runtime.core.state.catalog.sig"))
                assert page.evaluate(js("typeof T", 'typeof runtime.terminal.state')) == "undefined"
                assert scripts, "term.js was not requested"
                for route, response in scripts:
                    route.fulfill(response=response)
                page.unroute("**/term.js?*", hold_term_script)
                page.wait_for_load_state("domcontentloaded")
                page.wait_for_function(js("S.sig && typeof T !== 'undefined'", "runtime.core.state.catalog.sig && typeof runtime.terminal.state !== 'undefined'"))
            page.wait_for_timeout(100)
            assert page.evaluate(js("S.sel", 'runtime.core.state.selection.sel')) is None
            assert held, "term/list was not requested"
            for route, response in held:
                route.fulfill(response=response)
            page.unroute("**/api/term/list", hold_term_list)
            try:
                page.wait_for_function(js("uid => S.sel === uid", 'uid => runtime.core.state.selection.sel === uid'), arg=pending_uid, timeout=10000)
            except Exception:
                print(page.evaluate(js("() => ({sel:S.sel, saved:store.get('sel'), mobile:store.get('mobilePage'), pending:T.pending, error:T.listError})", "() => ({sel:runtime.core.state.selection.sel, saved:runtime.core.preferences.get('sel'), mobile:runtime.core.preferences.get('mobilePage'), pending:runtime.terminal.state.pending, error:runtime.terminal.state.listError})")), flush=True)
                raise
            expect(page.locator("#cinput")).to_have_value("审计下所有")
            expect(page.locator("#a-term")).to_be_visible()
            expect(page.locator("#termpane")).to_be_hidden()
            assert page.evaluate(js("T.pending.find(row => 'tmux:' + row.name === S.sel).record_id", "runtime.terminal.state.pending.find(row => 'tmux:' + row.name === runtime.core.state.selection.sel).record_id")) == receipt["record_id"]
            passed(f"codex: reload restores pending receipt and draft at {width}px")
        # Returning to the mobile list is a saved choice, not a request to
        # reopen the last detail when the delayed receipt finally arrives.
        page.locator("#detail .mobile-back").click()
        page.reload(wait_until="networkidle")
        assert page.evaluate(js("S.sel", 'runtime.core.state.selection.sel')) is None
        page.locator(f'#side .item[data-uid="{pending_uid}"]').click()
        page.wait_for_function(js("uid => S.sel === uid", 'uid => runtime.core.state.selection.sel === uid'), arg=pending_uid)
        page.set_viewport_size({"width": 1280, "height": 900})
        passed("codex: mobile list choice survives reload")

    native = wait_native(context, base, receipt.get("declared_sid"))
    native_uid = native["uid"] if native else None
    if native_uid:
        page.evaluate(js("async () => { if (typeof loadSessions === 'function') await loadSessions(true); }", "async () => { if (typeof runtime.core.list.loadSessions === 'function') await runtime.core.list.loadSessions(true); }"))
        try:
            page.wait_for_function(
                js("uid => (typeof sidebarSessions === 'function' ? sidebarSessions() : []).some(s => s.uid === uid)", "uid => (typeof runtime.core.index.sidebarSessions === 'function' ? runtime.core.index.sidebarSessions() : []).some(s => s.uid === uid)"),
                arg=native_uid, timeout=8000)
        except Exception:
            dump = page.evaluate(js("""() => ({
                sessions: (S.sessions || []).map(s => ({uid: s.uid, sid: s.sid, title: s.title})),
                pending: (typeof pendingTmuxSessions === 'function' ? pendingTmuxSessions() : []).map(
                    s => ({uid: s.uid, sid: s.sid, title: s.title})),
                side: [...document.querySelectorAll('#side .item')].map(
                    n => ({uid: n.dataset.uid, text: (n.innerText || '').slice(0, 80)}))
            })""", """() => ({
                sessions: (runtime.core.state.catalog.sessions || []).map(s => ({uid: s.uid, sid: s.sid, title: s.title})),
                pending: (typeof runtime.core.pending.pendingTmuxSessions === 'function' ? runtime.core.pending.pendingTmuxSessions() : []).map(
                    s => ({uid: s.uid, sid: s.sid, title: s.title})),
                side: [...document.querySelectorAll('#side .item')].map(
                    n => ({uid: n.dataset.uid, text: (n.innerText || '').slice(0, 80)}))
            })"""))
            fail(f"{source} native row", f"catalog uid {native_uid} missing from sidebar", dump)
        row = page.locator(f'#side .item[data-uid="{native_uid}"]')
        expect(row).to_have_count(1)
        row.click()
        page.wait_for_function(js("uid => S.sel === uid", 'uid => runtime.core.state.selection.sel === uid'), arg=native_uid, timeout=10000)
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
        expect(menu_delete).to_be_visible()
        expect(menu_delete).to_have_attribute("aria-disabled", "true")
        with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/kill") as stopped:
            menu_stop.click()
        assert stopped.value.status == 200, stopped.value.text()
        page.wait_for_function(
            js("id => T.pending.some(row => row.record_id === id && row.running === false)", 'id => runtime.terminal.state.pending.some(row => row.record_id === id && row.running === false)'),
            arg=receipt["record_id"], timeout=15000)
        row.click(button="right")
        passed("shell: sidebar stop exits the session and retains its row")
    expect(menu_stop).to_be_visible()
    expect(menu_stop).to_have_attribute("aria-disabled", "true")
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
    page.wait_for_function(js("typeof sidebarSessions === 'function'", "typeof runtime.core.index.sidebarSessions === 'function'"))
    assert_gone(context, base, page, receipt, pending_uid, native_uid, f"{source} after reload")
    passed(f"{source}: reload did not rebuild the row")

    other = context.new_page()
    other.goto(base, wait_until="networkidle")
    other.wait_for_function(js("typeof sidebarSessions === 'function'", "typeof runtime.core.index.sidebarSessions === 'function'"))
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
        for name in ("host", "work", "ledger", "delivery", "bin", "claude", "codex", "grok", "state", "trash", "home"):
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
                pending_live_identity(browser, base)
                context = browser.new_context(
                    viewport={"width": 1280, "height": 900}, service_workers="block")
                context.route(
                    "**/*",
                    lambda route: route.continue_() if route.request.url.startswith(base + "/")
                    else route.abort())
                page = context.new_page()
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                on_popup(page, lambda dialog: dialog.accept())
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
