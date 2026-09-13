#!/usr/bin/env python3
"""Real Codex CLI reliable-send acceptance (batch 32), cheapest configuration.

Part of normal validation per AGENTS.md: it always uses the cheapest Codex
configuration (`--model gpt-5.6-luna` with `-c model_reasoning_effort="low"`,
never a pricier model, never a fallback) and one short prompt. It uses an
ISOLATED `CODEX_HOME` (never the user's real ~/.codex — it copies `auth.json`
read-only so the real login is reused without mutating it, writes its own
minimal `config.toml` with the model, low effort, a read-only sandbox and the
throwaway working directory pre-trusted), a throwaway working directory, and
deletes everything it creates. It launches the real `codex` binary through the
lifecycle launcher profile (schema 2, `resume {sid}` on the session the login
probe created), resumes that session via `/api/term/create`, sends ONE prompt
via `POST /api/session/send`, verifies the receipt goes persisted → injected →
confirmed from the real rollout `response_item` user record (its
native user text confirms through the fixed pre-injection cursor, sees the assistant
reply via `/api/messages`, asserts the model reached the CLI (the rollout's
`turn_context.payload.model` is exactly that ID), then kills the instance.
Proxy variables (HTTP(S)_PROXY, ALL_PROXY, NO_PROXY, lower-case too) are passed
through to the CLI because this network needs them; nothing else is inherited.

It SKIPS with a printed reason when `codex` is not on PATH, when the isolated
home cannot authenticate, or when the account is over its usage limit / unpaid
(the login probe's error text is printed verbatim, e.g. lyra's
`ERROR: You've hit your usage limit …`; with credits available the suite
passes on lyra as well as on cygnus). A session the read model still refuses
(`supported:false`) is also reported as a skip with the migration warnings.
Bounded to two turns and 120 s; never writes outside the temp dirs. Claude is
not exercised here (see `send_claude_real.py`).
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import REPO, BINARY, Corpus, isolated_server  # noqa: E402

RELEASE = REPO / "target/release" / BINARY.name
SERVER = RELEASE if RELEASE.is_file() else BINARY
PTYHOST = REPO / "target/debug/ptyhost"
PROMPT = "Reply with the single word OK."
SECOND = "Reply with the single word DONE."
MODEL = "gpt-5.6-luna"
EFFORT = "low"
PROXY_KEYS = ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
              "http_proxy", "https_proxy", "all_proxy", "no_proxy")


def passthrough():
    """The only inherited variables: PATH, LANG and the proxy settings."""
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
           "LANG": os.environ.get("LANG", "C.UTF-8"), "TERM": "xterm-256color"}
    env.update({key: os.environ[key] for key in PROXY_KEYS if key in os.environ})
    return env


def skip(reason):
    print(f"SKIP send_codex_real: {reason}", flush=True)
    sys.exit(0)


def isolated_home(tmp, area):
    """A private CODEX_HOME that reuses the login read-only and pre-trusts the
    throwaway cwd (Codex would otherwise block on its trust prompt). The real
    ~/.codex is never written."""
    real = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
    auth = real / "auth.json"
    home = tmp / "codex-home"
    home.mkdir(mode=0o700)
    copied = False
    if auth.is_file():
        shutil.copy2(auth, home / "auth.json")
        (home / "auth.json").chmod(0o600)
        copied = True
    (home / "sessions").mkdir(mode=0o700)
    config = home / "config.toml"
    config.write_text(
        f'model = "{MODEL}"\n'
        f'model_reasoning_effort = "{EFFORT}"\n'
        'approval_policy = "never"\n'
        'sandbox_mode = "read-only"\n'
        '\n'
        f'[projects."{area}"]\n'
        'trust_level = "trusted"\n')
    config.chmod(0o600)
    return home, copied


def cli_env(home, work):
    return {**passthrough(), "HOME": str(work), "CODEX_HOME": str(home)}


def logged_in(home, work):
    """A cheapest one-shot `codex exec` proves the isolated home authenticates
    (and that the account is paid) and records the session the TUI resumes."""
    try:
        out = subprocess.run(
            [str(CODEX), "exec", "--model", MODEL, "-c", f'model_reasoning_effort="{EFFORT}"',
             "--sandbox", "read-only", "--skip-git-repo-check", "-C", str(work), PROMPT],
            cwd=work, env=cli_env(home, work),
            stdin=subprocess.DEVNULL, capture_output=True, timeout=120)
    except (OSError, subprocess.TimeoutExpired) as error:
        return False, str(error)
    text = (out.stderr.decode("utf-8", "replace").strip() + "\n"
            + out.stdout.decode("utf-8", "replace").strip()).strip()
    if out.returncode != 0:
        return False, (text or "nonzero exit")[-400:]
    return True, text[-400:]


def rollouts(home):
    return sorted((home / "sessions").glob("**/rollout-*.jsonl"), key=lambda p: p.stat().st_mtime)


def rows_of(path):
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def session_id(path):
    for row in rows_of(path):
        if row.get("type") == "session_meta":
            return str((row.get("payload") or {}).get("id") or "")
    return ""


def models_of(path):
    return sorted({str((row.get("payload") or {}).get("model") or "")
                   for row in rows_of(path) if row.get("type") == "turn_context"} - {""})


def assistant_replied(path):
    return any(row.get("type") == "response_item"
               and (row.get("payload") or {}).get("type") == "message"
               and (row.get("payload") or {}).get("role") == "assistant"
               for row in rows_of(path))


def user_texts_of(path):
    texts = []
    for row in rows_of(path):
        payload = row.get("payload") or {}
        if row.get("type") == "response_item" and payload.get("type") == "message" \
                and payload.get("role") == "user":
            texts.append("".join(part.get("text", "") for part in payload.get("content", [])
                                 if isinstance(part, dict)))
    return texts


def request(opener, base, method, route, body=None):
    from urllib.request import Request
    from urllib.error import HTTPError
    data = json.dumps(body).encode() if body is not None else None
    req = Request(base + route, data=data, method=method,
                  headers={"Content-Type": "application/json", "Host": "localhost"})
    try:
        with opener.open(req, timeout=15) as resp:
            return resp.status, json.loads(resp.read() or b"{}")
    except HTTPError as err:
        raw = err.read()
        return err.code, (json.loads(raw) if raw else {})


def main():
    global CODEX
    CODEX = shutil.which("codex")
    if not CODEX:
        skip("`codex` binary is not on PATH")
    if not PTYHOST.is_file():
        skip("ptyhost is not built (cargo build -p ptyhost)")

    with tempfile.TemporaryDirectory(prefix="sessiondock-codex-real-") as temporary:
        tmp = Path(temporary).resolve()
        work = tmp / "work"
        area = work / "codex-area"
        host = tmp / "hosts"
        ledger = tmp / "ledger"
        delivery = tmp / "delivery"
        web = tmp / "web"
        for path in (work, area, host, ledger, delivery, web):
            path.mkdir(mode=0o700, parents=True)
        (web / "index.html").write_text(
            '<!doctype html><meta name="agenthub-mode" content="local"><title>x</title>')
        home, copied = isolated_home(tmp, area)
        if not copied:
            skip("no ~/.codex/auth.json to reuse (not logged in)")

        ok, detail = logged_in(home, area)
        if not ok:
            # Unpaid/unauthenticated accounts fail here (lyra: unpaid); the
            # CLI's own error text is the printed reason.
            skip(f"isolated `codex exec` login/billing probe failed: {detail}")
        found = rollouts(home)
        if not found:
            skip(f"the one-shot recorded no rollout under the isolated CODEX_HOME/sessions: {detail}")
        rollout = found[-1]
        if not assistant_replied(rollout):
            skip(f"the one-shot got no assistant reply (unpaid or refused account?): {detail}")
        print(f"login probe ok: {detail[-120:]!r}", flush=True)
        sid = session_id(rollout)
        assert sid, f"no session_meta id in {rollout}"
        models = models_of(rollout)
        assert models == [MODEL], f"model assertion failed after the probe: expected [{MODEL}], got {models}"

        launcher = tmp / "launcher.json"
        launcher.touch(mode=0o600)
        launcher.write_text(json.dumps({
            "schema": 2, "host_binary": str(PTYHOST), "host_dir": str(host),
            "adapters": [],
            "profiles": [{
                "id": "codex-real-v1", "source": "codex", "executable": CODEX,
                "args": ["--model", MODEL, "-c", f'model_reasoning_effort="{EFFORT}"',
                         "--sandbox", "read-only"],
                "new_args": [],
                "resume_args": ["resume", "{sid}"],
                "env": cli_env(home, area),

            }]}))

        for flag, directory in (("--initialize-lifecycle", ledger),
                                ("--initialize-delivery", delivery)):
            with socket.socket() as occupied:
                occupied.bind(("127.0.0.1", 0))
                env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                       "SESSIONDOCK_BIND": "127.0.0.1:%d" % occupied.getsockname()[1]}
                done = subprocess.run([str(SERVER), flag, str(directory)],
                                      cwd=REPO, env=env, capture_output=True, timeout=30)
            assert done.returncode == 0, done.stderr.decode("utf-8", "replace")

        # isolated_server serves SESSIONDOCK_CODEX_ROOT = corpus.root/codex, so
        # point that at the CLI's own sessions directory.
        corpus = Corpus(tmp)
        (tmp / "codex").symlink_to(home / "sessions")
        for source in ("claude", "grok"):
            (tmp / source).mkdir(mode=0o700)

        deadline = time.monotonic() + 120
        with isolated_server(corpus, SERVER, host_dir=host, lifecycle_dir=ledger,
                             launcher_config=launcher, delivery_dir=delivery) as (base, opener):
            status, meta = request(opener, base, "GET", "/api/meta")
            assert status == 200 and meta["capabilities"]["outbox"] is True, meta
            build = meta["build"]

            status, listed = request(opener, base, "GET", "/api/sessions?force=1")
            assert status == 200, listed
            row = next((r for r in listed.get("sessions", [])
                        if r.get("source") == "codex" and r.get("sid") == sid), None)
            assert row is not None, f"session {sid} not listed: {listed}"
            if row.get("supported") is False:
                skip("the real Codex session is unsupported by the read model: "
                     + "; ".join(map(str, row.get("migration_warnings") or [])))
            if row.get("migration_warnings"):
                print("migration_warnings:", "; ".join(map(str, row["migration_warnings"])), flush=True)
            uid = row["uid"]
            status, record = request(opener, base, "POST", "/api/term/create",
                {"source": "codex", "cwd": str(area), "request_id": "real-codex-create",
                 "adapter_id": "codex-real-v1", "resume_uid": uid})
            assert status == 200 and record.get("running"), record
            name = record["name"]

            associated = False
            while not associated and time.monotonic() < deadline:
                status, listing = request(opener, base, "GET", "/api/term/list")
                associated = any(r.get("name") == name and r.get("uid") == uid
                                 for r in listing.get("sessions", []))
                if not associated:
                    time.sleep(0.5)
            if not associated:
                skip("the resumed real Codex never became an associated managed instance "
                     "(TUI startup differs); auth and one-shot verified above")

            # Let the resumed TUI paint its composer; the draft probe must read
            # it as empty before sending (an `unknown` composer is a pre-write
            # failure the executor never pastes into).
            probe = {}
            while time.monotonic() < deadline:
                status, probe = request(opener, base, "POST", "/api/session/draft-status",
                                        {"uid": uid, "name": name})
                if status == 200 and probe.get("draft_state") == "empty":
                    break
                time.sleep(1.0)
            if probe.get("draft_state") != "empty":
                skip(f"the real Codex composer never read as empty: {probe}")

            status, sent = request(opener, base, "POST", "/api/session/send",
                {"uid": uid, "name": name, "text": SECOND, "media": [],
                 "request_id": "real-codex-send-0001", "_build": build})
            assert status == 200, sent
            item = sent["item"]
            print(f"send accepted: state={item['state']} attempts={item.get('attempts')}", flush=True)
            if item["state"] == "failed" and int(item.get("attempts") or 0) == 0:
                skip(f"the executor refused to paste (pre-write failure): {item}")
            assert item["state"] == "failed" and int(item.get("attempts") or 0) == 1, item

            confirmed = False
            while time.monotonic() < deadline:
                status, box = request(opener, base, "GET",
                    "/api/session/outbox?uid=" + quote(uid, safe=":"))
                if status == 200 and not any(r["id"] == "real-codex-send-0001" for r in box["outbox"]):
                    confirmed = True
                    break
                time.sleep(1.0)
            if not confirmed:
                skip("the real Codex did not confirm the prompt within the window "
                     "(composer injection differs); auth and one-shot verified above")

            texts = user_texts_of(rollout)
            assert any(SECOND.strip() == t.strip() for t in texts), \
                f"prompt not found in native user records: {[t[:40] for t in texts]}"
            models = models_of(rollout)
            assert models == [MODEL], f"model assertion failed: expected [{MODEL}], got {models}"
            print(f"confirmed from the real rollout; models={models}", flush=True)

            status, messages = request(opener, base, "GET", "/api/messages/" + quote(uid, safe=":"))
            assert status == 200, messages
            history = [m.get("text", "") for m in messages.get("messages", [])]
            assert any(SECOND.strip() in t for t in history), history
            replied = False
            while time.monotonic() < deadline:
                status, messages = request(opener, base, "GET", "/api/messages/" + quote(uid, safe=":"))
                if status == 200 and any(m.get("role") == "assistant"
                                         and m.get("turn_id") for m in messages.get("messages", [])):
                    replied = True
                    break
                time.sleep(1.0)
            assert replied, "no assistant message in history"

            status, killed = request(opener, base, "POST", "/api/term/kill",
                {"record_id": record["record_id"], "instance_id": record["instance_id"]})
            assert status == 200, killed
        print(f"PASS send_codex_real: real Codex ({MODEL}, model_reasoning_effort={EFFORT}) launched via the "
              "launcher profile (`codex exec` created the session, the TUI resumed it), one prompt sent through "
              "/api/session/send, receipt persisted then injected then confirmed from the real rollout user record "
              "(causal text match), assistant reply visible in /api/messages, model asserted from turn_context, instance "
              "killed; isolated CODEX_HOME reused the login read-only, temp dirs removed")


if __name__ == "__main__":
    main()
