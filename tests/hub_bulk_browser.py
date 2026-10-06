#!/usr/bin/env python3
"""Hub multi-select: attach several sessions at once, and a slow bulk delete.

BUG-20260929-045938-557b87 follow-up: deleting several OpenCode sessions from
the hub page reported every one as "机器请求失败" while the node was still
removing them one by one (the hub waited 5 s for the node's single answer,
then dropped the request, which also stopped the node mid-batch). The pick bar
also gains 附属到…, attaching every picked session under one clicked parent.

A real hub and one real isolated node with a fake OpenCode profile whose
`session.remove` takes 2.5 s; Chromium clicks through the pick bar at desktop
and phone width. No real CLI or production data is used.
"""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO, Corpus, codex_row, isolated_server
from hub_fixtures import Hub, free_port, scoped
from private_hosts import private_hosts
from nest_tree_browser import open_item_menu
from node_auth_fixtures import node_env, TOKEN
from send_browser import initialize

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fake_opencode_composer as fake  # noqa: E402
from popups import on_popup  # noqa: E402

NID = "c" * 32
OPENCODE = [f"ses_0f0000000000bulkDeleteRow{n}" for n in range(4)]


def seed(db, work):
    connection = sqlite3.connect(db)
    connection.executescript(fake.SCHEMA)
    t = 1790500000000
    connection.execute("INSERT INTO project VALUES ('bulkproject', ?, ?, ?, '[]')", (str(work), t, t))
    for n, sid in enumerate(OPENCODE):
        created = t + n * 1000
        connection.execute(
            "INSERT INTO session_v2 (id, project_id, slug, directory, title, version, agent, model,"
            " time_created, time_updated) VALUES (?, 'bulkproject', 'slug', ?, ?, '2.0.18', 'build', ?, ?, ?)",
            (sid, str(work), f"bulk opencode {n}", json.dumps(fake.MODEL), created, created + 10))
        data = {"time": {"created": created}, "text": f"bulk prompt {n}", "files": [], "agents": []}
        connection.execute("INSERT INTO session_message VALUES (?, ?, 'user', 1, ?, ?, ?)",
                           (f"msg_bulk_{n}", sid, created, created, json.dumps(data)))
    connection.commit()
    connection.close()


def remaining(db):
    with sqlite3.connect(db) as connection:
        return {row[0] for row in connection.execute("SELECT id FROM session_v2")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-hub-bulk-") as temporary, private_hosts(Path(temporary)), sync_playwright() as pw:
        root = Path(temporary).resolve()
        corpus = Corpus(root)
        for name in ("host", "work", "ledger", "state", "home", "ids"):
            (root / name).mkdir(mode=0o700)
        (root / "ids/node-id").write_text(NID + "\n")
        corpus.put("bulk-parent", "codex", [
            codex_row("session_meta", {"id": "bulk-parent", "cwd": str(root / "work"),
                                       "timestamp": "2026-09-11T10:00:00Z"}),
            codex_row("response_item", {"type": "message", "role": "user", "content": "bulk parent"})], [])
        db = root / "opencode.db"
        seed(db, root / "work")
        env = {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "LANG": "C.UTF-8",
               "SESSIONDOCK_TEST_OPENCODE_DB": str(db), "SESSIONDOCK_TEST_OPENCODE_REMOVE_DELAY": "2.5"}
        executable = str(Path(sys.executable).resolve())
        launcher = root / "launcher.json"
        launcher.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
                                        "host_dir": str(root / "host"), "adapters": [], "profiles": [
            {"id": "opencode-cli-v1", "source": "opencode", "executable": executable,
             "args": [str(REPO / "tests/fake_opencode_composer.py")],
             "resume_args": ["--session", "{sid}"], "env": env}]}))
        launcher.chmod(0o600)
        initialize("--initialize-lifecycle", root / "ledger")
        node = SimpleNamespace(name="c", nid=NID, port=free_port(), token=TOKEN)
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = pw.chromium.launch(**launch)
        with ExitStack() as stack:
            stack.callback(browser.close)
            stack.enter_context(isolated_server(
                corpus, args.binary, host_dir=root / "host", lifecycle_dir=root / "ledger",
                launcher_config=launcher, state_dir=root / "state", trash_dir=root / "trash",
                extra_env={**node_env(root, node.port, "127.0.0.0/8"),
                           "SESSIONDOCK_OPENCODE_DB": str(db),
                           "SESSIONDOCK_OPENCODE_ROOT": str(root / "mirror")}))
            (root / "hubroot").mkdir()
            hub = Hub(args.binary.resolve().with_name("sessiondock-hub"), root / "hubroot", [node])
            hub.start()
            stack.callback(hub.stop)
            parent = scoped(NID, corpus.uid("bulk-parent"))
            for width in (1280, 390):
                context = browser.new_context(service_workers="block", viewport={"width": width, "height": 900})
                page = context.new_page()
                errors, dialogs = [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
                page.goto(f"http://127.0.0.1:{hub.port}", wait_until="networkidle")
                page.wait_for_function("n => S.sessions.length === n", arg=1 + len(remaining(db)))
                rows = page.evaluate("() => Object.fromEntries(S.sessions.map(s => [s.sid, s.uid]))")
                if not page.evaluate("S.nest"):
                    page.locator("#nest-toggle").click()
                item = lambda uid: page.locator(f'#side .item[data-uid="{uid}"]')  # noqa: E731
                if width == 1280:
                    # Multi-select 附属到…: two OpenCode rows go under the Codex row in one pick.
                    children = [rows[OPENCODE[0]], rows[OPENCODE[1]]]
                    open_item_menu(page, children[0])
                    page.locator('#item-menu [data-act="pick"]').click()
                    attach = page.locator("#side-pick-attach")
                    expect(attach).to_have_text("附属到… (1)")
                    item(children[1]).click()
                    expect(attach).to_have_text("附属到… (2)")
                    attach.click()
                    expect(page.locator("#side-picked")).to_have_text("点击要附属的会话（当前：2 个会话）")
                    expect(page.locator("#side .item.nest-source")).to_have_count(2)
                    item(children[0]).click()          # a picked row is not its own parent
                    expect(page.locator("#side .item.nest-source")).to_have_count(2)
                    item(parent).click()
                    for uid in children:
                        expect(item(uid)).to_have_attribute("data-depth", "1")
                    expect(page.locator("#side-tools")).to_be_hidden()
                    saved = json.loads((root / "state/session-metadata.json").read_text())["sessions"]
                    assert {corpus_uid: row.get("nest_parent") for corpus_uid, row in saved.items()
                            if row.get("nest_parent")} == {
                        uid.split(":", 1)[0] + ":" + uid.split("~", 1)[1]: {"source": "codex", "sid": "bulk-parent"}
                        for uid in children}, saved
                # Bulk delete that outlasts the old 5 s hub wait: every picked OpenCode row goes.
                victims = [rows[sid] for sid in OPENCODE if sid in rows]
                victims = victims[:3] if width == 1280 else victims
                open_item_menu(page, victims[0])
                page.locator('#item-menu [data-act="pick"]').click()
                for uid in victims[1:]:
                    item(uid).click()
                expect(page.locator("#side-picked")).to_have_text(f"已选 {len(victims)} 项")
                tools = page.locator("#side-tools").bounding_box()
                assert tools["x"] + tools["width"] <= width + 0.5, (tools, width)
                page.locator("#side-pick-delete").click()
                try:
                    page.wait_for_function("uids => uids.every(uid => !S.sessions.some(s => s.uid === uid))",
                                           arg=victims, timeout=40000)
                except Exception:
                    raise AssertionError(f"rows not removed; dialogs {dialogs}, db {sorted(remaining(db))}")
                gone = {sid for sid in OPENCODE if rows.get(sid) in victims}
                assert not gone & remaining(db), remaining(db)
                assert not [message for message in dialogs if "失败" in message], dialogs
                expect(page.locator("#side-tools")).to_be_hidden()
                assert not errors, errors
                context.close()
    print("PASS hub bulk: multi-select 附属到… nests two rows under one click, a picked row is not a parent; "
          "a 7.5 s and a 2.5 s OpenCode bulk delete through the hub remove every row without errors; "
          "desktop + 390px")


if __name__ == "__main__":
    main()
