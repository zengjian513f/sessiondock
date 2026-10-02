#!/usr/bin/env python3
"""OpenCode sessions started by another session's tool shell nest under it.

BUG-20260929-045938-557b87: `opencode run` launched from a Claude Bash tool names
no session on its command line, so its session never reached spawner discovery.
BUG-20261001-073106-7ac242: relation migration dropped that pairing from
`nest_parent` initialization, including Codex-launched meeting sessions.
The scan now pairs a new top-level OpenCode session with the OpenCode processes
running in its directory when it was born; they must all lead to one spawner.

Synthetic /proc tree, synthetic OpenCode database and mirror, private state,
real Chromium clicks. No real CLI or production data is used.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import sqlite3
import shutil
import subprocess
import sys
import tempfile
import time

from playwright.sync_api import expect, sync_playwright
from history_parity import codex_row, get_json
from history_parity import REPO
from trash_browser import Dialogs, click_delete
import spawned_by_suite as base_suite
from spawned_by_suite import BINARY, BTIME, HZ, P_SID, build, proc_pid, server_with_env

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fake_opencode_composer as fake  # noqa: E402

RUN_START = "2026-09-20T00:00:00Z"
SPAWNED = "ses_0f0000000000spawnedByClaude"  # born while `opencode run` runs in /work/oc
CHILD = "ses_0f0000000000subagentOfRunXx"    # its OpenCode subagent (parent_id)
OLDER = "ses_0f0000000000olderThanTheRun"    # same directory, born before the process
CODEX_PARENT = "codex-opencode-parent"
CODEX_RUNS = [f"ses_0f0000000000codexFanout{i}" for i in range(3)]
CONFLICT = "ses_0f0000000000twoSpawnersXx"
MIXED = "ses_0f0000000000twoCandidatesXx"    # /work/mix: a spawned run and a user's own TUI
ISOLATED = [f"ses_0f0000000000isolatedFanout{i}" for i in range(6)]
ISOLATED_CHILD = "ses_0f0000000000isolatedChildXx"
ISOLATED_OLDER = "ses_0f0000000000isolatedOlderXx"
ISOLATED_UNSEEN = "ses_0f0000000000isolatedUnseenX"


def isolated_stores(root):
    """Six sandbox homes, identical native cwd/project, no remaining processes."""
    script = root / "controller" / "run.py"
    script.parent.mkdir()
    script.write_text("# Recorded launcher: /opt/opencode run --standalone\n")
    for i, sid in enumerate(ISOLATED):
        db = root / f"worker-{i}" / "home" / ".local/share/opencode/opencode.db"
        db.parent.mkdir(parents=True)
        seed(db)
        with sqlite3.connect(db) as connection:
            connection.execute("DELETE FROM session_message")
            connection.execute("DELETE FROM session_v2")
            born = ms("2026-09-21T10:00:00Z")
            entries = [(sid, None, born)]
            if i == 0:
                entries += [(ISOLATED_CHILD, sid, born + 1),
                            (ISOLATED_OLDER, None, ms("2026-09-01T10:00:00Z")),
                            (ISOLATED_UNSEEN, None, born)]
            for native, parent, created in entries:
                connection.execute(
                    "INSERT INTO session_v2 (id, project_id, parent_id, slug, directory, title, version,"
                    " model, time_created, time_updated) VALUES (?, 'sandbox', ?, 'slug', '/workspace', ?,"
                    " '2.0.18', ?, ?, ?)", (native, parent, f"isolated {native}", json.dumps(fake.MODEL), created, created))
                connection.execute("INSERT INTO session_message VALUES (?, ?, 'user', 1, ?, ?, ?)",
                    (f"msg_{native}", native, created, created,
                     json.dumps({"text": f"isolated prompt {native}", "time": {"created": created}})))
    return codex_row("event_msg", {"type": "item_completed",
        "started_at_ms": ms("2026-09-21T09:59:59Z"), "completed_at_ms": ms("2026-09-21T10:10:00Z"),
        "item": {"type": "CommandExecution", "status": "completed",
            "command": ["zsh", "-lc", f"timeout 600s python -u {script}"],
            "stdout": "\n".join(json.dumps({"sessionID": native}) for native in (*ISOLATED, ISOLATED_OLDER))}})


def ms(text):
    return int(datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp() * 1000)


def ticks(text):
    return int((ms(text) / 1000 - BTIME) * HZ)


def open_menu(page, item):
    item.scroll_into_view_if_needed()
    page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
    item.click(button="right")


def detach(page, item):
    open_menu(page, item)
    with page.expect_response(lambda response: "/api/session/nest" in response.url) as saved:
        page.locator('#item-menu [data-act="detach"]').click()
    assert saved.value.ok, saved.value.text()
    expect(item).to_have_attribute("data-depth", "0")
    page.evaluate("async () => await pollSessions()")


def seed(db):
    connection = sqlite3.connect(db)
    connection.executescript(fake.SCHEMA)
    t = ms(RUN_START)
    for project, work in (("ocproject", "/work/oc"), ("mixproject", "/work/mix"),
                          ("codexproject", "/work/codex"), ("conflictproject", "/work/conflict")):
        connection.execute("INSERT INTO project VALUES (?, ?, ?, ?, '[]')", (project, work, t, t))
    sessions = [
        (SPAWNED, "ocproject", None, "/work/oc", "compare luna", ms("2026-09-21T10:00:00Z")),
        (CHILD, "ocproject", SPAWNED, "/work/oc", "subagent", ms("2026-09-21T10:05:00Z")),
        (OLDER, "ocproject", None, "/work/oc", "older", ms("2026-09-01T10:00:00Z")),
        (MIXED, "mixproject", None, "/work/mix", "mixed", ms("2026-09-21T11:00:00Z")),
    ]
    sessions += [(sid, "codexproject", None, "/work/codex", f"codex fanout {i}",
                  ms("2026-09-21T10:00:00Z")) for i, sid in enumerate(CODEX_RUNS)]
    sessions.append((CONFLICT, "conflictproject", None, "/work/conflict", "conflict",
                     ms("2026-09-21T10:00:00Z")))
    for sid, project, parent, work, title, created in sessions:
        connection.execute(
            "INSERT INTO session_v2 (id, project_id, parent_id, slug, directory, title, version, agent,"
            " model, time_created, time_updated) VALUES (?, ?, ?, 'slug', ?, ?, '2.0.18', 'build', ?, ?, ?)",
            (sid, project, parent, work, title, json.dumps(fake.MODEL), created, created + 10))
        data = {"time": {"created": created}, "text": f"{title} prompt", "files": [], "agents": []}
        connection.execute("INSERT INTO session_message VALUES (?, ?, 'user', 1, ?, ?, ?)",
                           (f"msg_{sid}", sid, created, created, json.dumps(data)))
    connection.commit()
    connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-opencode-spawn-") as tmp:
        root = Path(tmp)
        corpus, uids, proc = build(root)
        corpus.put(CODEX_PARENT, "codex", [
            codex_row("session_meta", {"id": CODEX_PARENT, "cwd": "/work/codex",
                "timestamp": "2026-09-01T00:00:00Z", "source": "cli"}),
            codex_row("response_item", {"type": "message", "role": "user",
                "content": "Codex OpenCode meeting"}), isolated_stores(root),
            codex_row("response_item", {"type": "message", "role": "user",
                "content": f"Diagnostic text mentions {ISOLATED_UNSEEN}; it is not launch evidence."})], [])
        for pid in range(900, 909):
            base_suite.START[pid] = ticks(RUN_START) + pid
        # P's Bash tool shell runs two `opencode run` jobs; a user's own TUI also sits in /work/mix.
        inherited = (("CLAUDE_CODE_SESSION_ID", P_SID),)
        proc_pid(proc, 900, "bash", ["bash", "run.sh"], 100, env=inherited, cwd="/work/p")
        proc_pid(proc, 901, "opencode", ["/home/u/.opencode/bin/opencode", "run", "--standalone", "claude: go"],
                 900, env=inherited, cwd="/work/oc")
        proc_pid(proc, 902, "opencode", ["/home/u/.opencode/bin/opencode", "run", "go"], 900,
                 env=inherited, cwd="/work/mix")
        proc_pid(proc, 903, "opencode", ["opencode"], 1, cwd="/work/mix")
        codex_env = (("CODEX_THREAD_ID", CODEX_PARENT), ("CODEX_SESSION_ID", CODEX_PARENT))
        # A detached Python dispatcher preserves Codex identity; parallel runs share one initiator.
        proc_pid(proc, 904, "python", ["python", "meeting.py"], 1, env=codex_env, cwd="/work/codex")
        for pid in (905, 906):
            proc_pid(proc, pid, "opencode", ["opencode", "run", "--standalone", "go"],
                     904, env=codex_env, cwd="/work/codex")
        proc_pid(proc, 907, "opencode", ["opencode", "run", "go"], 904,
                 env=codex_env, cwd="/work/conflict")
        proc_pid(proc, 908, "opencode", ["opencode", "run", "go"], 900,
                 env=inherited, cwd="/work/conflict")
        db = root / "opencode.db"
        seed(db)
        state = root / "state"
        state.mkdir(mode=0o700)
        launcher = root / "launcher.json"
        launcher.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [{
                "id": "opencode-cli-v1", "source": "opencode", "executable": sys.executable,
                "args": ["-c", "raise SystemExit('isolated session must not invoke the default CLI')"],
                "resume_args": ["--session", "{sid}"]}]}))
        launcher.chmod(0o600)
        (root / "host").mkdir(mode=0o700)
        initialized = subprocess.run([str(args.binary.resolve()), "--initialize-lifecycle", str(root / "ledger")],
            env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        assert initialized.returncode == 0, initialized.stderr.decode()
        env = {"SESSIONDOCK_PROC_ROOT": proc, "SESSIONDOCK_STATE_DIR": state,
               "SESSIONDOCK_GROK_ACTIVE": root / "no-active.json",
               "SESSIONDOCK_PTYHOST_DIR": root / "host", "SESSIONDOCK_LIFECYCLE_DIR": root / "ledger",
               "SESSIONDOCK_LAUNCHER_CONFIG": launcher, "SESSIONDOCK_TRASH_DIR": root / "trash",
               "SESSIONDOCK_OPENCODE_DB": db, "SESSIONDOCK_OPENCODE_ROOT": root / "mirror"}
        with server_with_env(corpus, env, args.binary) as (base, opener):
            sids = (SPAWNED, CHILD, OLDER, MIXED, CONFLICT, *CODEX_RUNS, *ISOLATED, ISOLATED_CHILD)
            for _ in range(250):
                rows = {r["sid"]: r for r in get_json(opener, base, "/api/sessions?force=1")["sessions"]}
                if all(sid in rows for sid in sids):
                    break
                __import__("time").sleep(0.1)
            else:
                raise AssertionError(f"mirrored OpenCode rows missing: {sorted(rows)}")
            deadline = time.monotonic() + 18
            while not all(rows[sid].get('nest_parent') for sid in (SPAWNED, *CODEX_RUNS, *ISOLATED)):
                assert time.monotonic() < deadline, 'OpenCode background nesting never initialized'
                time.sleep(.1)
                rows = {r["sid"]: r for r in get_json(opener, base, "/api/sessions?force=1")["sessions"]}
            get_json(opener, base, "/api/live?force=1")
            rows = {r["sid"]: r for r in get_json(opener, base, "/api/sessions?force=1")["sessions"]}
            assert rows[SPAWNED].get('nest_parent') == {'source': 'claude', 'sid': P_SID}, rows[SPAWNED]
            assert all(rows[sid].get('nest_parent') == {'source': 'codex', 'sid': CODEX_PARENT}
                       for sid in CODEX_RUNS), [rows[sid] for sid in CODEX_RUNS]
            assert all(rows[sid].get('nest_parent') == {'source': 'codex', 'sid': CODEX_PARENT}
                       for sid in ISOLATED), [rows[sid] for sid in ISOLATED]
            assert not rows[ISOLATED_CHILD].get('nest_parent')
            assert ISOLATED_OLDER not in rows and ISOLATED_UNSEEN not in rows
            assert all('nest_parent' not in rows[sid] for sid in (CHILD, OLDER, MIXED, CONFLICT))
            assert all('spawned_by' not in row and 'nest_initialized' not in row for row in rows.values())
            spawned = rows[SPAWNED]["uid"]
            with sync_playwright() as pw:
                launch = {"headless": True}
                if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                    launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
                browser = pw.chromium.launch(**launch)
                for width in (1280, 390):
                    context = browser.new_context(viewport={"width": width, "height": 900})
                    context.route("**/*", lambda route: route.continue_()
                                  if route.request.url.startswith(base + "/") else route.abort())
                    page = context.new_page()
                    errors = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(base)
                    item = page.locator(f'#side .item[data-uid="{spawned}"]')
                    expect(item).to_be_visible()
                    if not page.evaluate('S.nest'):
                        page.locator("#nest-toggle").click()
                    expect(item).to_have_attribute("data-depth", "1")
                    codex_uid = corpus.uid(CODEX_PARENT)
                    worker = page.locator(f'#side .item[data-uid="{rows[CODEX_RUNS[0]]["uid"]}"]')
                    expect(worker).to_have_attribute("data-depth", "1" if width == 1280 else "0")
                    for sid in CODEX_RUNS[1:]:
                        expect(page.locator(f'#side .item[data-uid="{rows[sid]["uid"]}"]')) \
                            .to_have_attribute("data-depth", "1")
                    for sid in ISOLATED:
                        expect(page.locator(f'#side .item[data-uid="{rows[sid]["uid"]}"]')) \
                            .to_have_attribute("data-depth", "0" if width == 390 and sid == ISOLATED[0] else "1")
                    isolated = page.locator(f'#side .item[data-uid="{rows[ISOLATED[0]]["uid"]}"]')
                    isolated.click()
                    expect(page.locator("#msgs")).to_contain_text(f"isolated prompt {ISOLATED[0]}")
                    if width == 390:
                        page.get_by_role('button', name='返回会话列表', exact=True).click()
                    if width == 1280:
                        dialogs = Dialogs(page)
                        dialogs.expect(("原沙箱中恢复或删除", True))
                        with page.expect_response(lambda response: "/api/term/takeover" in response.url) as resumed:
                            page.locator("#a-term").click()
                        assert resumed.value.status == 501, resumed.value.text()
                        dialogs.expect(("OpenCode", True), ("原沙箱中恢复或删除", True))
                        with page.expect_response(lambda response: "/api/session/" in response.url
                                                  and response.request.method == "DELETE") as removed:
                            click_delete(page, False)
                        assert removed.value.status == 501, removed.value.text()
                        expect(page.locator('dialog.app-popup[open]')).to_have_count(0)
                        dialogs.drained()
                        expect(isolated).to_be_visible()
                        assert not list((root / "host").glob("*.json")), "default CLI was launched"
                        # User detach and reattach decisions must beat every later inference.
                        detach(page, worker)
                        detach(page, isolated)
                        detach(page, item)
                        open_menu(page, item)
                        page.locator('#item-menu [data-act="attach"]').click()
                        page.locator(f'#side .item[data-uid="{codex_uid}"]').click()
                        expect(item).to_have_attribute("data-depth", "1")
                    get_json(opener, base, "/api/live?force=1")
                    page.evaluate("pollSessions()")
                    expect(worker).to_have_attribute("data-depth", "0")
                    for sid in (CHILD, OLDER, MIXED, CONFLICT):
                        expect(page.locator(f'#side .item[data-uid="{rows[sid]["uid"]}"]')) \
                            .to_have_attribute("data-depth", "0")
                    item.click()
                    expect(page.locator("#msgs")).to_contain_text("compare luna prompt")
                    assert not errors, errors
                    context.close()
                browser.close()
        # Restart with live processes, then after all workers exit: one-time choices persist.
        for exited in (False, True):
            if exited:
                for directory in proc.iterdir():
                    if directory.is_dir():
                        shutil.rmtree(directory)
            with server_with_env(corpus, env, args.binary) as (base, opener):
                get_json(opener, base, "/api/live?force=1")
                rows = {r["sid"]: r for r in get_json(opener, base, "/api/sessions?force=1")["sessions"]}
                assert rows[SPAWNED]['nest_parent'] == {'source': 'codex', 'sid': CODEX_PARENT}
                assert 'nest_parent' not in rows[CODEX_RUNS[0]]
                assert all(rows[sid]['nest_parent'] == {'source': 'codex', 'sid': CODEX_PARENT}
                           for sid in CODEX_RUNS[1:])
                assert all(rows[sid]['nest_parent'] == {'source': 'codex', 'sid': CODEX_PARENT}
                           for sid in ISOLATED[1:])
                assert 'nest_parent' not in rows[ISOLATED[0]]
                assert all('nest_parent' not in rows[sid] for sid in (CHILD, OLDER, MIXED, CONFLICT))
    print("PASS OpenCode auto-attach from Claude and detached Codex fan-out; ambiguous directories, "
          "six completed sandbox stores recovered with history and control explanation; "
          "native subagents and older sessions stay roots; Chromium open/detach/reattach desktop + 390px; "
          "manual choices survive scans, restart and exit")


if __name__ == "__main__":
    main()
