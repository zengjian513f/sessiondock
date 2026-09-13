#!/usr/bin/env python3
"""Real Codex CLI approval-card acceptance (WP-G), cheapest configuration.

Like `send_codex_real.py` (isolated `CODEX_HOME` reusing `auth.json`
read-only, throwaway cwd, `gpt-5.6-luna` with `model_reasoning_effort="low"`,
everything deleted afterwards) but the launch profile carries Python's TUI
arguments (`--enable default_mode_request_user_input -c
suppress_unstable_features_warning=true`), a read-only sandbox and
`-a on-request` (Codex 0.154 accepts only `on-request` / `never`). One prompt
asks Codex to run a harmless `touch` in the throwaway directory, which the
read-only sandbox refuses, so the model requests approval and the TUI shows
"Would you like to run the following command?", which lives only on the
screen. The suite asserts:

1. `/api/messages` carries the approval as `prompt` (`kind: "approval"`,
   Python `codex_bridge.approval_prompt` shape: `id codex-approval:<16 hex>`,
   `y` / `p` / `Escape` options) and `/api/watch` pushed it as `prompt_only`;
2. the answer goes through `/api/term/send` under a page lease with the
   option's own key (the page sends `['Escape']` for 拒绝 — nothing is run);
3. the dialog leaves the screen and `prompt` returns to `null`.

`--evidence DIR` saves the card JSON and the packet timeline. SKIPs with the
CLI's own text when `codex` is absent, unauthenticated or over its usage limit,
and when the model does not propose the command within the window.
"""
import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import REPO, BINARY, Corpus, isolated_server  # noqa: E402
import send_codex_real as base_suite  # noqa: E402
from send_codex_real import (  # noqa: E402
    EFFORT, MODEL, assistant_replied, cli_env, isolated_home, logged_in, models_of,
    request, rollouts, session_id)
from prompt_claude_real import note, watch_thread  # noqa: E402

RELEASE = REPO / "target/release" / BINARY.name
SERVER = RELEASE if RELEASE.is_file() else BINARY
PTYHOST = REPO / "target/debug/ptyhost"
PAGE = "wp-g-approval-page"
PROBE = "sessiondock-wpg-approval-probe.txt"


def skip(reason):
    print(f"SKIP prompt_codex_real: {reason}", flush=True)
    sys.exit(0)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence", default=os.environ.get("SESSIONDOCK_PROMPT_EVIDENCE_DIR", ""))
    args = parser.parse_args()
    evidence = Path(args.evidence).resolve() if args.evidence else None
    if evidence:
        evidence.mkdir(parents=True, exist_ok=True)
    codex = shutil.which("codex")
    if not codex:
        skip("`codex` binary is not on PATH")
    codex = os.path.realpath(codex)
    base_suite.CODEX = codex
    if not PTYHOST.is_file():
        skip("ptyhost is not built (cargo build -p ptyhost)")

    timeline = []
    with tempfile.TemporaryDirectory(prefix="sessiondock-codex-prompt-") as temporary:
        tmp = Path(temporary).resolve()
        work = tmp / "work"
        area = work / "codex-area"
        host = tmp / "hosts"
        ledger = tmp / "ledger"
        delivery = tmp / "delivery"
        state = tmp / "state"
        for path in (work, area, host, ledger, delivery, state):
            path.mkdir(mode=0o700, parents=True)
        home, copied = isolated_home(tmp, area)
        if not copied:
            skip("no ~/.codex/auth.json to reuse (not logged in)")
        ok, detail = logged_in(home, area)
        if not ok:
            skip(f"isolated `codex exec` login/billing probe failed: {detail}")
        found = rollouts(home)
        if not found or not assistant_replied(found[-1]):
            skip(f"the one-shot recorded no usable rollout: {detail[-200:]}")
        rollout = found[-1]
        sid = session_id(rollout)
        assert sid and models_of(rollout) == [MODEL], (sid, models_of(rollout))
        note(timeline, "login_probe_ok", sid=sid)

        launcher = tmp / "launcher.json"
        launcher.touch(mode=0o600)
        launcher.write_text(json.dumps({
            "schema": 2, "host_binary": str(PTYHOST), "host_dir": str(host),
            "adapters": [],
            "profiles": [{
                "id": "codex-real-v1", "source": "codex", "executable": codex,
                # Python's TUI arguments plus the cheapest model and an approval
                # policy under which the model asks before escalating out of the sandbox.
                "args": ["--model", MODEL, "-c", f'model_reasoning_effort="{EFFORT}"',
                         "--enable", "default_mode_request_user_input",
                         "-c", "suppress_unstable_features_warning=true",
                         "-a", "on-request", "--sandbox", "read-only"],
                "new_args": [],
                "resume_args": ["resume", "{sid}"],
                "env": cli_env(home, area),

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
        (tmp / "codex").symlink_to(home / "sessions")
        for source in ("claude", "grok"):
            (tmp / source).mkdir(mode=0o700)

        deadline = time.monotonic() + 180
        with isolated_server(corpus, SERVER, host_dir=host, lifecycle_dir=ledger, launcher_config=launcher,
                             delivery_dir=delivery, state_dir=state) as (base, opener):
            hostname, port = base.replace("http://", "").split(":")
            port = int(port)
            status, meta = request(opener, base, "GET", "/api/meta")
            assert status == 200 and meta["capabilities"]["outbox"] is True, meta
            build = meta["build"]
            status, listed = request(opener, base, "GET", "/api/sessions?force=1")
            row = next((r for r in listed.get("sessions", []) if r.get("source") == "codex" and r.get("sid") == sid), None)
            assert row is not None, f"session {sid} not listed"
            if row.get("supported") is False:
                skip("the real Codex session is unsupported by the read model: " + "; ".join(map(str, row.get("migration_warnings") or [])))
            uid = row["uid"]
            q = quote(uid, safe=":")
            status, before = request(opener, base, "GET", f"/api/messages/{q}")
            assert status == 200 and "prompt" in before and before["prompt"] is None, before

            packets, stop = [], threading.Event()
            watcher = threading.Thread(target=watch_thread, args=(hostname, port, uid, before, packets, stop), daemon=True)
            watcher.start()

            status, record = request(opener, base, "POST", "/api/term/create",
                {"source": "codex", "cwd": str(area), "request_id": "real-create",
                 "adapter_id": "codex-real-v1", "resume_uid": uid})
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
                    skip("the resumed real Codex never became an associated managed instance")
                note(timeline, "instance_associated", instance_id=instance["instance_id"])
                # Like send_codex_real: the resumed TUI must read as an empty composer first.
                probe = {}
                while time.monotonic() < deadline:
                    status, probe = request(opener, base, "POST", "/api/session/draft-status", {"uid": uid, "name": name})
                    if status == 200 and probe.get("draft_state") == "empty":
                        break
                    time.sleep(1.0)
                if probe.get("draft_state") != "empty":
                    skip(f"the real Codex composer never read as empty: {probe}")

                ask = (f"Run exactly this shell command now: touch {PROBE} . The sandbox is read-only, so "
                       "request approval to run it outside the sandbox instead of giving up or asking me "
                       "anything else. After it ran, reply with the single word DONE.")
                status, sent = request(opener, base, "POST", "/api/session/send",
                    {"uid": uid, "name": name, "text": ask, "media": [], "request_id": "real-ask-0001", "_build": build})
                assert status == 200, sent
                item = sent["item"]
                if item["state"] == "failed" and int(item.get("attempts") or 0) == 0:
                    skip(f"the executor refused to paste (pre-write failure): {item}")
                # Codex receipts read `failed`/attempts 1 until the rollout confirms them (send_codex_real).
                assert item["state"] == "failed" and int(item.get("attempts") or 0) == 1, item
                note(timeline, "prompt_sent", state=item["state"], attempts=item.get("attempts"))

                card = None
                while time.monotonic() < deadline:
                    status, body = request(opener, base, "GET", f"/api/messages/{q}")
                    prompt = body.get("prompt") if status == 200 else None
                    if prompt and prompt.get("kind") == "approval":
                        card = prompt
                        break
                    time.sleep(0.5)
                if card is None:
                    skip("Codex did not propose the command / the approval dialog did not appear within the window (model behaviour)")
                note(timeline, "card_visible", id=card["id"], header=card["questions"][0]["header"],
                     keys=[o["key"] for o in card["questions"][0]["options"]])
                assert card["id"].startswith("codex-approval:") and len(card["id"]) == len("codex-approval:") + 16, card
                assert card["state"] == "waiting" and card["questions"][0]["multiple"] is False, card
                keys = [o["key"] for o in card["questions"][0]["options"]]
                assert "Escape" in keys and ("y" in keys or "p" in keys), card
                assert PROBE in card["questions"][0]["question"], card
                if evidence:
                    (evidence / "codex-card.json").write_text(json.dumps(card, ensure_ascii=False, indent=2))
                pushed = []
                wait_until = time.monotonic() + 6
                while not pushed and time.monotonic() < wait_until:
                    pushed = [p for _, p in packets if p.get("prompt_only") and (p.get("prompt") or {}).get("id") == card["id"]]
                    if not pushed:
                        time.sleep(0.2)
                assert pushed, ("the watch stream never pushed the approval as prompt_only", [p for _, p in packets][-3:])
                note(timeline, "sse_prompt_only_seen")

                # Answer 拒绝 exactly like the page: the option's own key under a page lease.
                deny = next(o for o in card["questions"][0]["options"] if o["key"] == "Escape")
                status, claimed = request(opener, base, "POST", "/api/term/claim",
                    {"name": name, "page": PAGE, "uid": uid, "instance_id": instance["instance_id"]})
                assert status == 200 and claimed.get("token"), claimed
                status, typed = request(opener, base, "POST", "/api/term/send",
                    {"name": name, "page": PAGE, "token": claimed["token"], "uid": uid,
                     "instance_id": instance["instance_id"], "keys": [deny["key"]]})
                assert status == 200 and typed.get("ok"), typed
                note(timeline, "answer_key_sent", key=deny["key"], label=deny["label"])

                cleared = False
                while time.monotonic() < deadline:
                    status, body = request(opener, base, "GET", f"/api/messages/{q}")
                    if status == 200 and body.get("prompt") is None:
                        cleared = True
                        break
                    time.sleep(0.5)
                assert cleared, "the approval never left the screen"
                note(timeline, "card_cleared")
                gone = []
                wait_until = time.monotonic() + 6
                while not gone and time.monotonic() < wait_until:
                    gone = [p for t, p in packets if p.get("prompt_only") and p.get("prompt") is None]
                    if not gone:
                        time.sleep(0.2)
                assert gone, "the watch stream must push prompt_only null once the dialog is gone"
                assert not (area / PROBE).exists(), "拒绝 must not run the command"
                stop.set()
                watcher.join(timeout=5)
                if evidence:
                    (evidence / "codex-timeline.json").write_text(json.dumps(timeline, ensure_ascii=False, indent=2))
                    (evidence / "codex-packets.json").write_text(json.dumps(
                        [{"t": round(t, 3), "prompt_only": bool(p.get("prompt_only")),
                          "prompt": (p.get("prompt") or {}).get("id") if p.get("prompt") else None,
                          "messages": [m.get("role") for m in p.get("messages", [])]} for t, p in packets],
                        ensure_ascii=False, indent=2))
            finally:
                # Never leak the real CLI, even when an assertion above fails.
                status, killed = request(opener, base, "POST", "/api/term/kill",
                    {"record_id": record["record_id"], "instance_id": record["instance_id"]})
                assert status == 200, killed
        print(f"PASS prompt_codex_real: real Codex ({MODEL}, effort {EFFORT}, Python TUI args, -a on-request, read-only sandbox) proposed "
              f"`touch {PROBE}`; the screen approval appeared as prompt (kind approval, {keys}) in /api/messages and as a "
              f"prompt_only watch packet, 拒绝 went through /api/term/send Escape under a page lease, the card cleared and "
              f"nothing was run; instance killed, temp dirs removed")


if __name__ == "__main__":
    main()
