#!/usr/bin/env python3
"""OpenCode sessions started by another session's tool shell nest under it.

BUG-20260929-045938-557b87: `opencode run` launched from a Claude Bash tool names
no session on its command line, so its session never reached spawner discovery.
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
import sys
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import get_json
import spawned_by_suite as base_suite
from spawned_by_suite import BINARY, BTIME, HZ, P_SID, build, proc_pid, server_with_env

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fake_opencode_composer as fake  # noqa: E402

RUN_START = "2026-09-20T00:00:00Z"
SPAWNED = "ses_0f0000000000spawnedByClaude"  # born while `opencode run` runs in /work/oc
CHILD = "ses_0f0000000000subagentOfRunXx"    # its OpenCode subagent (parent_id)
OLDER = "ses_0f0000000000olderThanTheRun"    # same directory, born before the process
MIXED = "ses_0f0000000000twoCandidatesXx"    # /work/mix: a spawned run and a user's own TUI


def ms(text):
    return int(datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp() * 1000)


def ticks(text):
    return int((ms(text) / 1000 - BTIME) * HZ)


def seed(db):
    connection = sqlite3.connect(db)
    connection.executescript(fake.SCHEMA)
    t = ms(RUN_START)
    for project, work in (("ocproject", "/work/oc"), ("mixproject", "/work/mix")):
        connection.execute("INSERT INTO project VALUES (?, ?, ?, ?, '[]')", (project, work, t, t))
    sessions = [
        (SPAWNED, "ocproject", None, "/work/oc", "compare luna", ms("2026-09-21T10:00:00Z")),
        (CHILD, "ocproject", SPAWNED, "/work/oc", "subagent", ms("2026-09-21T10:05:00Z")),
        (OLDER, "ocproject", None, "/work/oc", "older", ms("2026-09-01T10:00:00Z")),
        (MIXED, "mixproject", None, "/work/mix", "mixed", ms("2026-09-21T11:00:00Z")),
    ]
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
        for pid in (900, 901, 902, 903):
            base_suite.START[pid] = ticks(RUN_START) + pid
        # P's Bash tool shell runs two `opencode run` jobs; a user's own TUI also sits in /work/mix.
        inherited = (("CLAUDE_CODE_SESSION_ID", P_SID),)
        proc_pid(proc, 900, "bash", ["bash", "run.sh"], 100, env=inherited, cwd="/work/p")
        proc_pid(proc, 901, "opencode", ["/home/u/.opencode/bin/opencode", "run", "--standalone", "claude: go"],
                 900, env=inherited, cwd="/work/oc")
        proc_pid(proc, 902, "opencode", ["/home/u/.opencode/bin/opencode", "run", "go"], 900,
                 env=inherited, cwd="/work/mix")
        proc_pid(proc, 903, "opencode", ["opencode"], 1, cwd="/work/mix")
        db = root / "opencode.db"
        seed(db)
        state = root / "state"
        state.mkdir(mode=0o700)
        env = {"SESSIONDOCK_PROC_ROOT": proc, "SESSIONDOCK_STATE_DIR": state,
               "SESSIONDOCK_GROK_ACTIVE": root / "no-active.json",
               "SESSIONDOCK_OPENCODE_DB": db, "SESSIONDOCK_OPENCODE_ROOT": root / "mirror"}
        with server_with_env(corpus, env, args.binary) as (base, opener):
            sids = (SPAWNED, CHILD, OLDER, MIXED)
            for _ in range(100):
                rows = {r["sid"]: r for r in get_json(opener, base, "/api/sessions?force=1")["sessions"]}
                if all(sid in rows for sid in sids):
                    break
                __import__("time").sleep(0.1)
            else:
                raise AssertionError(f"mirrored OpenCode rows missing: {sorted(rows)}")
            get_json(opener, base, "/api/live?force=1")
            rows = {r["sid"]: r for r in get_json(opener, base, "/api/sessions?force=1")["sessions"]}
            got = {sid: rows[sid].get("spawned_by") for sid in sids}
            assert got == {SPAWNED: {"source": "claude", "sid": P_SID}, CHILD: None, OLDER: None,
                           MIXED: None}, got
            saved = json.loads((state / "session-metadata.json").read_text())["sessions"]
            assert saved[rows[SPAWNED]["uid"]]["spawned_by"] == {"source": "claude", "sid": P_SID}, saved
            spawned, parent = rows[SPAWNED]["uid"], uids[P_SID]
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
                    expect(item).to_have_attribute("data-depth", "0")
                    page.locator("#nest-toggle").click()
                    expect(item).to_have_attribute("data-depth", "1")
                    # The OpenCode row sits right under its Claude spawner.
                    order = page.evaluate("() => [...document.querySelectorAll('#side .item[data-uid]')]"
                                          ".map(n => n.dataset.uid)")
                    assert order.index(spawned) > order.index(parent), order
                    for sid in (OLDER, MIXED):
                        expect(page.locator(f'#side .item[data-uid="{rows[sid]["uid"]}"]')) \
                            .to_have_attribute("data-depth", "0")
                    item.click()
                    expect(page.locator("#msgs")).to_contain_text("compare luna prompt")
                    assert not errors, errors
                    context.close()
                browser.close()
    print("PASS opencode spawn: `opencode run` from a Claude tool shell nests under Claude; subagent child, "
          "older session and an ambiguous directory stay roots; Chromium nesting/open, desktop + 390px")


if __name__ == "__main__":
    main()
