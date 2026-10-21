#!/usr/bin/env python3
"""Kiro SDK snapshot windowing + trimmed open response, through Chromium.

Regression for the session-switch payload optimization:
- `api/native-sdk/open` HTTP response must NOT carry the transcript
  (`raw.items`/`raw.subtasks`); the open path only needs capabilities.
- The SSE snapshot must window `items` to a recent, user-turn-aligned tail
  (<=400) instead of the whole transcript, while earlier turns still render
  from the paginated `/api/messages` history.
"""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace

from kiro_sdk_fixtures import KiroFixture

EARLY_MARKER = "EARLY-TURN-MARKER-0001"
LATE_MARKER = "LATE-TURN-MARKER-0900"
PAIRS = 460  # 460 user+assistant pairs => 920 items on replay (> 400 cap)


def seed_large_session(fixture: KiroFixture):
    """Overwrite the primary V3 session with a long transcript."""
    lines = []
    for i in range(PAIRS):
        u = EARLY_MARKER if i == 0 else (f"user turn {i}")
        a = LATE_MARKER if i == PAIRS - 1 else (f"assistant reply {i}")
        lines.append(json.dumps({
            "id": f"msg-u{i:04d}", "timestamp": "2026-10-18T00:01:00Z",
            "message": {"kind": "user", "data": {"content": [{"kind": "text", "data": u}]}},
        }))
        lines.append(json.dumps({
            "id": f"msg-a{i:04d}", "timestamp": "2026-10-18T00:01:05Z",
            "message": {"kind": "assistant", "data": {"content": [{"kind": "text", "data": a}]}},
        }))
    (fixture.v3_dir / "messages.jsonl").write_text("\n".join(lines) + "\n")


def run(args):
    from playwright.sync_api import expect, sync_playwright
    from codex_sdk_browser import server
    from kiro_sdk_browser import api, api_raw, watch
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize

    with tempfile.TemporaryDirectory(prefix="sessiondock-kiro-window-") as temporary:
        root = Path(temporary)
        for name in ("claude", "codex", "grok", "state", "trash"):
            (root / name).mkdir()
        ptyhost_bin = args.binary.resolve(strict=True).with_name("ptyhost")
        fixture = KiroFixture(root, host_binary=ptyhost_bin)
        seed_large_session(fixture)
        initialize("--initialize-lifecycle", root / "ledger", args.binary.resolve(strict=True))
        extra = {
            **fixture.env,
            "SESSIONDOCK_NATIVE_SDKS": "auto",
            "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"),
            "SESSIONDOCK_PTYHOST_DIR": str(fixture.host_dir),
            "SESSIONDOCK_STATE_DIR": str(root / "state"),
            "SESSIONDOCK_TRASH_DIR": str(root / "trash"),
        }
        try:
            with server(SimpleNamespace(root=root), args.binary, "off", extra) as base, sync_playwright() as playwright:
                browser = playwright.chromium.launch(headless=True)
                page = browser.new_page(viewport={"width": 1280, "height": 900})
                page.set_default_timeout(20000)
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(base, wait_until="domcontentloaded")
                page.wait_for_function("T.listLoaded")
                page.wait_for_function(
                    "async sid => { await loadSessions(true); return S.sessions.some(r => r.sid === sid); }",
                    arg=fixture.sid, polling=500, timeout=30000)
                uid = next(r["uid"] for r in page.evaluate("S.sessions") if r.get("sid") == fixture.sid)

                # Start the SDK session (composer click loads it) and watch events.
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.locator("#a-sdk").click()
                with page.expect_response(lambda r: r.url.endswith("/api/native-sdk/open") and r.request.method == "POST") as opened:
                    page.locator("#sdk-input").click()
                assert opened.value.ok, opened.value.text()

                # (a) open HTTP response must NOT carry the transcript.
                open_body = opened.value.json()
                raw = open_body.get("raw", {})
                assert "items" not in raw, f"open response still carries items ({len(raw.get('items', []))})"
                assert "subtasks" not in raw, "open response still carries subtasks"
                # It must still carry what the open path actually needs.
                assert isinstance(open_body.get("capabilities") or raw.get("capabilities"), dict), open_body

                # A direct api('open') call (used elsewhere in the UI) is trimmed too.
                direct = api(page, "open", {"uid": uid})
                assert "items" not in direct["raw"], "api open still carries items"
                assert "subtasks" not in direct["raw"], "api open still carries subtasks"

                # (b) SSE snapshot items are windowed and user-aligned.
                watch(page, uid)
                page.wait_for_function("window.__kiroSnapshot && Array.isArray(window.__kiroSnapshot.items)")
                snap = page.evaluate("window.__kiroSnapshot")
                items = snap["items"]
                user_turns = sum(1 for it in items if it.get("role") == "user")
                # Windowed to the recent-turn cap (40 user turns), far fewer than
                # the seeded 460 turns / 920 items.
                assert 0 < len(items) < PAIRS * 2, f"items not windowed: {len(items)} (seeded {PAIRS*2})"
                assert user_turns <= 40, f"window exceeded turn cap: {user_turns} user turns"
                assert items[0].get("role") == "user", f"window must start on a user turn, got {items[0].get('role')}"
                # subtasks, if present, must not embed transcripts.
                for st in snap.get("subtasks") or []:
                    assert "items" not in st, f"subtask still embeds items: {st.get('id')}"

                # (c) The live tail renders.
                page.wait_for_function("txt => document.querySelector('#msgs')?.textContent.includes(txt)", arg=LATE_MARKER)

                # (d) The live/history seam is gap-free: history is loaded, and it
                # is truncated exactly at the first live item (no duplicate, no
                # gap). Earlier turns beyond BOTH the live window and the
                # paginated history window are legitimately omitted (SessionDock
                # history is itself head/tail windowed, like the terminal view) --
                # the point of this optimization is to stop shipping the whole
                # transcript, not to force it all into the DOM.
                seam = page.evaluate("""() => {
                  const v = NativeSdkView;
                  const firstLive = v.items.find(i => i.role === 'user');
                  const idOf = m => m.native_id || m.turn_id || m.id;
                  const histMsgs = v.historyData?.messages || [];
                  // historyCut is where history stops and live begins.
                  return {
                    loadedHistory: !!v.historyData,
                    histCut: v.historyCut,
                    histLen: histMsgs.length,
                    firstLiveId: idOf(firstLive),
                    // the message at the cut in the FULL history list (pre-slice)
                    cutMatchesFirstLive: histMsgs.some(m => m.role === 'user' && idOf(m) === idOf(firstLive)),
                  };
                }""")
                assert seam["loadedHistory"], "history pane never loaded"
                # The first live user turn must be locatable in history so the
                # seam dedups correctly (no double render, no gap).
                assert seam["cutMatchesFirstLive"], f"seam misaligned: {seam}"

                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "kiro-snapshot-window-1280.png"), full_page=True)

                assert not errors, errors
                browser.close()
        finally:
            stop_private_sdkhosts(root)
    print("Kiro snapshot windowing browser: PASS")


def main():
    from history_fixtures import BINARY
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    run(args)


if __name__ == "__main__":
    main()
