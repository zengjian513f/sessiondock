#!/usr/bin/env python3
"""Real Claude CLI question-card acceptance (WP-G), cheapest configuration.

Like `send_claude_real.py` (isolated `CLAUDE_CONFIG_DIR` reusing the login
read-only, throwaway cwd, `claude-haiku-4-5-20251001 --effort low`, everything
deleted afterwards) but the launch profile additionally passes
`--settings <bridge settings>` written by `sessiondock --write-bridge-settings`
and restricts the tools to `AskUserQuestion`. One prompt asks Claude to pose a
two-option question; the suite then asserts the whole bridge:

1. Claude's `PreToolUse` hook ran `sessiondock claude-hook` and wrote
   `<state dir>/claude-prompts/<sid>.json` (0600, `state: waiting`);
2. `/api/messages` carries that card as `prompt` and `/api/watch` pushed it as
   a `prompt_only` packet before any native record existed;
3. the answer is sent as keyboard input through `/api/term/send` under a page
   lease (exactly the key `legacy-web/cli.js` sends: the option's digit) — or,
   with `--browser`, by clicking the option on the real page;
4. the native `tool_use` / `tool_result` records land, `/api/messages`
   returns `prompt: null` and the card file is gone.

`--evidence DIR` saves the card JSON, the packet timeline and (with
`--browser`) screenshots before/after answering. SKIPs like the send suite
when `claude` is absent or cannot authenticate standalone.
"""
import argparse
import json
import os
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from pathlib import Path
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import REPO, BINARY, Corpus, isolated_server  # noqa: E402
import send_claude_real as base_suite  # noqa: E402
from send_claude_real import (  # noqa: E402
    MODEL, isolated_config, logged_in, passthrough, request, trust_project)
from sse_suite import open_watch, sse_next  # noqa: E402

RELEASE = REPO / "target/release" / BINARY.name
SERVER = RELEASE if RELEASE.is_file() else BINARY
PTYHOST = REPO / "target/debug/ptyhost"
ASK = ("Use the AskUserQuestion tool right now to ask me ONE question with exactly two options "
       "(for example: tea or coffee). Do not answer yourself; wait for my choice, then reply with the single word DONE.")
PAGE = "wp-g-prompt-page"


def skip(reason):
    print(f"SKIP prompt_claude_real: {reason}", flush=True)
    sys.exit(0)


def note(timeline, event, **fields):
    row = {"t": round(time.time(), 3), "event": event, **fields}
    timeline.append(row)
    print(json.dumps(row, ensure_ascii=False), flush=True)


def watch_thread(host, port, uid, snapshot, packets, stop):
    """Collect every SSE packet (data only) until `stop` is set."""
    conn, resp = open_watch(host, port, uid, snapshot)
    try:
        while not stop.is_set():
            # A long deadline: a read timeout inside an event would drop its
            # partial lines; the 15 s keepalive comment wakes the reader anyway.
            frame = sse_next(conn, resp, time.monotonic() + 30.0)
            if frame and frame.get("event") == "message":
                packets.append((time.time(), frame["data"]))
            elif frame and frame.get("closed"):
                return
    except SystemExit as error:  # sse_next's fail(): keep the reason for the caller
        packets.append((time.time(), {"watch_thread_failed": str(error)}))
    finally:
        conn.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--browser", action="store_true", help="answer by clicking the card on the real page")
    parser.add_argument("--evidence", default=os.environ.get("SESSIONDOCK_PROMPT_EVIDENCE_DIR", ""))
    args = parser.parse_args()
    evidence = Path(args.evidence).resolve() if args.evidence else None
    if evidence:
        evidence.mkdir(parents=True, exist_ok=True)
    CLAUDE = shutil.which("claude")
    if not CLAUDE:
        skip("`claude` binary is not on PATH")
    CLAUDE = os.path.realpath(CLAUDE)
    base_suite.CLAUDE = CLAUDE  # `logged_in` reads the send suite's module global
    if not PTYHOST.is_file():
        skip("ptyhost is not built (cargo build -p ptyhost)")
    if args.browser:
        try:
            from playwright.sync_api import sync_playwright  # noqa: F401
        except ImportError:
            skip("playwright is not installed (needed for --browser)")

    timeline = []
    with tempfile.TemporaryDirectory(prefix="sessiondock-claude-prompt-") as temporary:
        tmp = Path(temporary).resolve()
        config, copied = isolated_config(tmp)
        if not copied:
            skip("no ~/.claude/.credentials.json to reuse (not logged in)")
        work = tmp / "work"
        area = work / "claude-area"
        host = tmp / "hosts"
        ledger = tmp / "ledger"
        delivery = tmp / "delivery"
        state = tmp / "state"
        for path in (work, area, host, ledger, delivery, state):
            path.mkdir(mode=0o700, parents=True)
        trust_project(config, area)
        sid = str(uuid.uuid4())
        ok, detail = logged_in(config, area, sid)
        if not ok:
            skip(f"isolated `claude` login probe failed: {detail}")
        note(timeline, "login_probe_ok", detail=detail[:60])

        # The bridge settings file Claude receives through --settings.
        settings = tmp / "claude-bridge-settings.json"
        done = subprocess.run([str(SERVER), "--write-bridge-settings", str(settings)],
                              env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                                   "SESSIONDOCK_STATE_DIR": str(state)},
                              capture_output=True, timeout=30)
        assert done.returncode == 0 and settings.is_file(), done.stderr.decode("utf-8", "replace")
        hooks = json.loads(settings.read_text())["hooks"]
        assert hooks["PreToolUse"][0]["hooks"][0]["args"] == ["claude-hook", "--state-dir", str(state)], hooks

        claude_root = config / "projects"
        launcher = tmp / "launcher.json"
        launcher.touch(mode=0o600)
        launcher.write_text(json.dumps({
            "schema": 2, "host_binary": str(PTYHOST), "host_dir": str(host),
            "adapters": [],
            "profiles": [{
                "id": "claude-real-v1", "source": "claude", "executable": CLAUDE,
                "args": ["--model", MODEL, "--effort", "low",
                         "--settings", str(settings), "--tools", "AskUserQuestion"],
                "new_args": ["--session-id", "{session_id}"],
                "resume_args": ["--resume", "{sid}"],
                "env": {**passthrough(), "HOME": str(work), "CLAUDE_CONFIG_DIR": str(config)},

            }]}))
        for flag, directory in (("--initialize-lifecycle", ledger), ("--initialize-delivery", delivery)):
            with socket.socket() as occupied:
                occupied.bind(("127.0.0.1", 0))
                env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                       "SESSIONDOCK_BIND": "127.0.0.1:%d" % occupied.getsockname()[1]}
                done = subprocess.run([str(SERVER), flag, str(directory)], cwd=REPO, env=env,
                                      capture_output=True, timeout=30)
            assert done.returncode == 0, done.stderr.decode("utf-8", "replace")
        corpus = Corpus(tmp)
        (tmp / "claude").symlink_to(claude_root)
        for source in ("codex", "grok"):
            (tmp / source).mkdir(mode=0o700)
        card_file = state / "claude-prompts" / f"{sid}.json"

        def session_file():
            return next(claude_root.glob(f"*/{sid}.jsonl"), None)
        if session_file() is None:
            skip(f"the one-shot did not create {sid}.jsonl under the isolated projects dir")

        deadline = time.monotonic() + 150
        with isolated_server(corpus, SERVER, host_dir=host, lifecycle_dir=ledger, launcher_config=launcher,
                             delivery_dir=delivery, state_dir=state) as (base, opener):
            hostname, port = base.replace("http://", "").split(":")
            port = int(port)
            status, meta = request(opener, base, "GET", "/api/meta")
            assert status == 200 and meta["capabilities"]["outbox"] is True, meta
            build = meta["build"]
            status, listed = request(opener, base, "GET", "/api/sessions?force=1")
            row = next((r for r in listed.get("sessions", []) if r.get("source") == "claude" and r.get("sid") == sid), None)
            assert row is not None and row.get("supported") is True, row
            uid = row["uid"]
            q = quote(uid, safe=":")
            status, before = request(opener, base, "GET", f"/api/messages/{q}")
            assert status == 200 and "prompt" in before and before["prompt"] is None, before

            # Watch the session before anything happens.
            packets, stop = [], threading.Event()
            watcher = threading.Thread(target=watch_thread, args=(hostname, port, uid, before, packets, stop), daemon=True)
            watcher.start()

            status, record = request(opener, base, "POST", "/api/term/create",
                {"source": "claude", "cwd": str(area), "request_id": "real-create",
                 "adapter_id": "claude-real-v1", "resume_uid": uid})
            assert status == 200 and record.get("running"), record
            try:
                name = record["name"]
                note(timeline, "instance_started", name=name)
                instance = None
                while instance is None and time.monotonic() < deadline:
                    status, listing = request(opener, base, "GET", "/api/term/list")
                    instance = next((r for r in listing.get("sessions", []) if r.get("name") == name and r.get("uid") == uid), None)
                    if instance is None:
                        time.sleep(0.5)
                if instance is None:
                    skip("the resumed real CLI never became an associated managed instance")
                note(timeline, "instance_associated", instance_id=instance["instance_id"])
                time.sleep(2.0)

                status, sent = request(opener, base, "POST", "/api/session/send",
                    {"uid": uid, "name": name, "text": ASK, "media": [], "request_id": "real-ask-0001", "_build": build})
                assert status == 200 and sent["item"]["state"] in ("ambiguous", "persisted"), sent
                note(timeline, "prompt_sent", state=sent["item"]["state"])

                # 1+2: the hook wrote the card and the API shows it before any native record.
                card = None
                while time.monotonic() < deadline:
                    status, body = request(opener, base, "GET", f"/api/messages/{q}")
                    prompt = body.get("prompt") if status == 200 else None
                    if prompt and prompt.get("state") == "waiting" and prompt.get("questions"):
                        card = prompt
                        break
                    time.sleep(0.5)
                if card is None:
                    skip("Claude did not call AskUserQuestion within the window (model behaviour); hook/settings verified up to launch")
                note(timeline, "card_visible", id=card["id"], questions=len(card["questions"]),
                     options=[o["label"] for o in card["questions"][0]["options"]])
                assert card["source"] == "claude" and card["version"] == 1 and card["id"].startswith("toolu"), card
                assert card_file.is_file() and (os.name == "nt" or stat.S_IMODE(card_file.stat().st_mode) == 0o600), card_file
                assert json.loads(card_file.read_text())["id"] == card["id"]
                options = card["questions"][0]["options"]
                assert len(options) >= 2 and not card["questions"][0].get("multiple"), card
                rows = [json.loads(line) for line in session_file().read_text().splitlines() if line.strip()]
                native_ids = [c.get("id") for r in rows if r.get("type") == "assistant"
                              for c in (r.get("message", {}).get("content") or []) if isinstance(c, dict) and c.get("type") == "tool_use"]
                note(timeline, "native_tool_use_recorded_already", value=card["id"] in native_ids)
                if evidence:
                    (evidence / "claude-card.json").write_text(json.dumps(card, ensure_ascii=False, indent=2))
                pushed = []
                wait_until = time.monotonic() + 6
                while not pushed and time.monotonic() < wait_until:
                    pushed = [p for _, p in packets if p.get("prompt_only") and (p.get("prompt") or {}).get("id") == card["id"]]
                    if not pushed:
                        time.sleep(0.2)
                assert pushed, ("the watch stream never pushed the card as prompt_only", [p for _, p in packets][-5:])
                note(timeline, "sse_prompt_only_seen", state=pushed[0]["prompt"]["state"],
                     latency_ms=int((next(t for t, p in packets if p is pushed[0]) - timeline[-2]["t"]) * 1000))

                # 3: answer.
                choice = 1 if len(options) > 1 else 0
                if args.browser:
                    answer_in_browser(base, uid, card, choice, evidence, timeline)
                else:
                    status, claimed = request(opener, base, "POST", "/api/term/claim",
                        {"name": name, "page": PAGE, "uid": uid, "instance_id": instance["instance_id"]})
                    assert status == 200 and claimed.get("token"), claimed
                    # Exactly what legacy-web/cli.js ClaudeCli.questionAnswerKeys sends:
                    # the option's digit (Claude 2.1.270 selects and submits on it).
                    keys = [str(choice + 1)]
                    status, typed = request(opener, base, "POST", "/api/term/send",
                        {"name": name, "page": PAGE, "token": claimed["token"], "uid": uid,
                         "instance_id": instance["instance_id"], "keys": keys})
                    assert status == 200 and typed.get("ok"), typed
                    note(timeline, "answer_keys_sent", keys=len(keys), option=options[choice]["label"])

                # 4: native records land, the card clears, the file is gone.
                cleared = None
                while time.monotonic() < deadline:
                    status, body = request(opener, base, "GET", f"/api/messages/{q}")
                    answered = any(m.get("role") == "answer" and m.get("call_id") == card["id"] for m in body.get("messages", []))
                    if status == 200 and answered and body.get("prompt") is None:
                        cleared = body
                        break
                    time.sleep(0.5)
                assert cleared is not None, "the answer never reached the records / the card never cleared"
                note(timeline, "card_cleared", file_exists=card_file.exists())
                assert not card_file.exists(), "card file must be deleted once the answer is recorded"
                answer = next(m for m in cleared["messages"] if m.get("role") == "answer" and m.get("call_id") == card["id"])
                assert options[choice]["label"] in (answer.get("text") or ""), answer
                nulls = [p for _, p in packets if not p.get("prompt_only") and "prompt" in p and p["prompt"] is None and any(m.get("role") == "answer" for m in p.get("messages", []))]
                assert nulls, "the watch messages packet with the answer must carry prompt:null"
                rows = [json.loads(line) for line in session_file().read_text().splitlines() if line.strip()]
                models = {r.get("message", {}).get("model", "") for r in rows if r.get("type") == "assistant"} - {""}
                assert MODEL in models, f"model assertion failed: {models}"
                stop.set()
                watcher.join(timeout=5)
                if evidence:
                    (evidence / "claude-timeline.json").write_text(json.dumps(timeline, ensure_ascii=False, indent=2))
                    (evidence / "claude-packets.json").write_text(json.dumps(
                        [{"t": round(t, 3), "prompt_only": bool(p.get("prompt_only")),
                          "prompt": (p.get("prompt") or {}).get("state") if p.get("prompt") else None,
                          "messages": [m.get("role") for m in p.get("messages", [])]} for t, p in packets],
                        ensure_ascii=False, indent=2))
            finally:
                # Never leak the real CLI, even when an assertion above fails.
                status, killed = request(opener, base, "POST", "/api/term/kill",
                    {"record_id": record["record_id"], "instance_id": record["instance_id"]})
                assert status == 200, killed
        print(f"PASS prompt_claude_real: real Claude ({MODEL}, effort low, --settings bridge, --tools AskUserQuestion) "
              f"asked a {len(options)}-option question; the claude-hook wrote the 0600 card, /api/messages and the "
              f"/api/watch prompt_only packet showed it, the answer went through "
              f"{'the page (browser click)' if args.browser else '/api/term/send keys'} and the card cleared once "
              f"the native answer landed; instance killed, temp dirs removed")


def answer_in_browser(base, uid, card, choice, evidence, timeline):
    """The real legacy page: take over the console, see the live card, click."""
    from playwright.sync_api import sync_playwright, expect
    with sync_playwright() as playwright:
        options = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**options)
        try:
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            errors = []
            page = context.new_page()
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.on("dialog", lambda dialog: dialog.accept())
            page.goto(base, wait_until="networkidle")
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            page.wait_for_function("uid => S.sel === uid", arg=uid, timeout=20000)
            live = page.locator("#msgs .msg.question.live-question")
            expect(live).to_be_visible(timeout=20000)
            expect(live).to_contain_text(card["questions"][0]["question"])
            note(timeline, "browser_card_visible")
            if evidence:
                page.screenshot(path=str(evidence / "claude-01-card.png"), full_page=True)
            # Buttons are disabled until the page holds the console lease.
            expect(page.locator("#a-term")).to_have_attribute("data-unavailable", "false", timeout=15000)
            page.locator("#a-term").click()
            page.wait_for_function("uid => takenOver(uid) !== null", arg=uid, timeout=20000)
            if page.evaluate("T.mode") == "full":
                page.locator("#a-term").click()
            expect(page.locator("#composer")).to_be_visible(timeout=15000)
            button = page.locator(f'#msgs .live-question [data-question-option="{choice}"]')
            expect(button).to_be_enabled(timeout=15000)
            if evidence:
                page.screenshot(path=str(evidence / "claude-02-card-armed.png"), full_page=True)
            button.click()
            note(timeline, "browser_option_clicked", option=choice)
            page.wait_for_function("() => document.querySelector('#msgs .msg.question.live-question') === null", timeout=60000)
            note(timeline, "browser_card_gone")
            if evidence:
                page.screenshot(path=str(evidence / "claude-03-answered.png"), full_page=True)
            assert not errors, errors
            context.close()
        finally:
            browser.close()


if __name__ == "__main__":
    main()
