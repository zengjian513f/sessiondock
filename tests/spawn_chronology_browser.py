#!/usr/bin/env python3
"""Old Grok sessions must not acquire newer parents or memory-flush activity.

Synthetic native files + /proc, private state, real Chromium clicks and SSE.
No real CLI or production data is used.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import sync_playwright, expect
from history_parity import encoded, get_json
from spawned_by_suite import BINARY, build, server_with_env, G_SID, G2_SID, P_SID, Q_SID

OLD = "2026-09-01T10:00:00.000Z"
END = "2026-09-01T10:05:00.000Z"
NEW = "2026-09-26T14:45:34.442Z"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-spawn-chronology-") as tmp:
        root = Path(tmp)
        corpus, uids, proc = build(root)
        directory = corpus.paths[G_SID]
        summary = directory / "summary.json"
        doc = json.loads(summary.read_text())
        doc.update(created_at=OLD, last_active_at=NEW, updated_at=NEW)
        summary.write_text(json.dumps(doc))
        events = directory / "events.jsonl"
        events.write_bytes(encoded({"type": "turn_started", "ts": OLD}) +
                           encoded({"type": "turn_ended", "ts": END}))
        state = root / "state"
        state.mkdir(mode=0o700)
        saved = state / "session-metadata.json"
        invalid = {"source": "claude", "sid": P_SID}
        saved.write_text(json.dumps({"schema_version": 1, "revision": 1, "sessions": {
            uids[G_SID]: {"spawned_by": invalid, "starred": True, "starred_at": 1},
        }}))
        saved.chmod(0o600)
        env = {"SESSIONDOCK_PROC_ROOT": proc, "SESSIONDOCK_STATE_DIR": state,
               "SESSIONDOCK_GROK_ACTIVE": root / "no-active.json"}
        with server_with_env(corpus, env, args.binary) as (base, opener):
            get_json(opener, base, "/api/live?force=1")
            rows = get_json(opener, base, "/api/sessions?force=1")["sessions"]
            old = next(r for r in rows if r["uid"] == uids[G_SID])
            assert "spawned_by" not in old, old
            assert old["updated"] == END and old["starred"], old
            child = next(r for r in rows if r["uid"] == uids[G2_SID])
            assert child["spawned_by"] == {"source": "claude", "sid": Q_SID}, child
            disk = json.loads(saved.read_text())["sessions"][uids[G_SID]]
            assert disk["invalid_spawned_by"] == invalid and "spawned_by" not in disk, disk
            with sync_playwright() as pw:
                launch = {"headless": True}
                if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                    launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
                browser = pw.chromium.launch(**launch)
                context = browser.new_context(viewport={"width": 1280, "height": 900})
                context.route("**/*", lambda route: route.continue_()
                              if route.request.url.startswith(base + "/") else route.abort())
                page = context.new_page()
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base)
                item = page.locator(f'#side .item[data-uid="{uids[G_SID]}"]')
                expect(item).to_be_visible()
                page.locator("#nest-toggle").click()
                expect(item).to_have_attribute("data-depth", "0")
                item.click()
                expect(page.locator("#msgs")).to_contain_text("hello G")
                page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                # Maintenance changes the native summary and adds synthetic chat
                # context, but must not move either list or detail activity time.
                doc["last_active_at"] = "2026-09-27T14:45:34.442Z"
                summary.write_text(json.dumps(doc))
                with (directory / "chat_history.jsonl").open("ab") as out:
                    out.write(encoded({"type": "user", "synthetic_reason": "system_reminder",
                                       "content": "Background maintenance only"}))
                get_json(opener, base, "/api/sessions?force=1")
                page.evaluate("pollSessions()")
                assert get_json(opener, base, "/api/messages/" + uids[G_SID])["meta"]["updated"] == END
                # Event-only growth must invalidate the inventory cache and SSE
                # metadata without appending any chat bytes or changing summary.
                with events.open("ab") as out:
                    out.write(encoded({"type": "turn_started", "ts": NEW}))
                get_json(opener, base, "/api/sessions?force=1")
                page.evaluate("pollSessions()")
                page.wait_for_function("([uid, ts]) => S.sessions.find(s => s.uid === uid)?.updated === ts",
                                       arg=[uids[G_SID], NEW])
                assert get_json(opener, base, "/api/messages/" + uids[G_SID])["meta"]["updated"] == NEW
                expect(page.locator("#msgs")).not_to_contain_text("Background maintenance only")
                # An explicit user attachment still works, even when the target
                # is younger: only automatic birth inference is constrained.
                item.click(button="right")
                expect(page.locator("#item-menu")).to_be_visible()
                page.locator('#item-menu [data-act="attach"]').click()
                page.wait_for_function("S.nestAttach")
                page.locator(f'#side .item[data-uid="{uids[P_SID]}"]').click()
                expect(item).to_have_attribute("data-depth", "1")
                assert not errors, errors
                browser.close()
            # Repeated scans cannot recreate the invalid relation.
            get_json(opener, base, "/api/live?force=1")
            assert "spawned_by" not in json.loads(saved.read_text())["sessions"][uids[G_SID]]
        # The repair persists after restart, retaining the saved evidence/star.
        with server_with_env(corpus, env, args.binary) as (base, opener):
            get_json(opener, base, "/api/live?force=1")
            disk = json.loads(saved.read_text())["sessions"][uids[G_SID]]
            assert "spawned_by" not in disk and disk["invalid_spawned_by"] == invalid and disk["starred"]
            assert disk["nest_parent"] == invalid
        print("PASS spawn chronology repair, valid children, native activity, Chromium nesting/open/SSE, restart")


if __name__ == "__main__":
    main()
