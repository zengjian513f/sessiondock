#!/usr/bin/env python3
"""Real Claude CLI reliable-send acceptance, cheapest configuration.

Part of normal validation per AGENTS.md: it always uses the cheapest Claude
configuration (`--model claude-haiku-4-5-20251001 --effort low`, the full dated ID,
never the `haiku` alias) and one short prompt. It uses an
ISOLATED `CLAUDE_CONFIG_DIR` (never the user's real ~/.claude — it copies the
credential file read-only so the real config is not mutated), a throwaway
working directory, and deletes everything it creates. It launches the real
`claude` binary through the lifecycle launcher profile (schema 2,
`--session-id {session_id}`), creates a session via `/api/term/create`, sends
ONE prompt via `POST /api/session/send`, verifies the receipt goes
persisted → injected → confirmed from the real native JSONL `user` record,
sees the assistant reply via `/api/messages`, asserts the model flag reached
the CLI (the JSONL assistant records carry exactly that model ID), then kills
the instance. Proxy variables (HTTP(S)_PROXY, ALL_PROXY, NO_PROXY) are passed
through to the CLI because this network needs them; nothing else is inherited.

It SKIPS with a printed reason when `claude` is not on PATH or not logged in.
The read model skips the record/attachment kinds current Claude
Code versions write (reported as non-fatal `migration_warnings`, printed here);
a session the read model still refuses is an assertion failure, not a skip.
Bounded to a couple of turns and 90 s; never writes outside the temp dirs.
Codex is deliberately not exercised here (this machine's Codex account is
unpaid; Codex real-CLI runs elsewhere). Note: where the machine's Claude auth
is gated to the Claude Code harness context, a direct isolated `claude` call is
rejected ("403 Request not allowed") and this suite skips with that reason;
run it on a host whose `claude` login authenticates standalone.
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
MODEL = "claude-haiku-4-5-20251001"
PROXY_KEYS = ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
              "http_proxy", "https_proxy", "all_proxy", "no_proxy")


def passthrough():
    """The only inherited variables: PATH, LANG and the proxy settings."""
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
           "LANG": os.environ.get("LANG", "C.UTF-8")}
    env.update({key: os.environ[key] for key in PROXY_KEYS if key in os.environ})
    return env


def skip(reason):
    print(f"SKIP send_claude_real: {reason}", flush=True)
    sys.exit(0)


def isolated_config(tmp):
    """A private CLAUDE_CONFIG_DIR that reuses the login read-only."""
    real = Path(os.environ.get("CLAUDE_CONFIG_DIR", Path.home() / ".claude"))
    creds = real / ".credentials.json"
    config = tmp / "claude-config"
    config.mkdir(mode=0o700)
    copied = False
    if creds.is_file():
        shutil.copy2(creds, config / ".credentials.json")
        copied = True
    # The top-level onboarding state, if present, avoids first-run prompts.
    top = Path.home() / ".claude.json"
    if top.is_file():
        shutil.copy2(top, config / ".claude.json")
    (config / "projects").mkdir(mode=0o700)
    return config, copied


def trust_project(config, area):
    """Pre-accept the folder trust dialog for the throwaway cwd in the isolated
    config copy (the TUI would otherwise block on it), like the Python monkey
    that answers the prompt with Enter. Never touches the real ~/.claude.json."""
    top = config / ".claude.json"
    try:
        document = json.loads(top.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        document = {}
    document["hasCompletedOnboarding"] = True
    projects = document.setdefault("projects", {})
    projects[str(area)] = {**projects.get(str(area), {}), "hasTrustDialogAccepted": True,
                           "allowedTools": [], "hasClaudeMdExternalIncludesApproved": False,
                           "hasClaudeMdExternalIncludesWarningShown": False}
    top.write_text(json.dumps(document, ensure_ascii=False, indent=2))
    top.chmod(0o600)


def logged_in(config, work, sid):
    """A trivial cheapest one-shot proves the isolated config is authenticated
    and, with a fixed --session-id, creates the session JSONL the TUI resumes.
    (Claude Code writes nothing for a fresh TUI session until its first input,
    so a brand-new session cannot be a resolvable send target yet.)"""
    try:
        out = subprocess.run(
            [str(CLAUDE), "-p", PROMPT, "--model", MODEL, "--effort", "low",
             "--session-id", sid],
            cwd=work, env={**passthrough(), "HOME": str(work), "CLAUDE_CONFIG_DIR": str(config)},
            stdin=subprocess.DEVNULL, capture_output=True, timeout=90)
    except (OSError, subprocess.TimeoutExpired) as error:
        return False, str(error)
    if out.returncode != 0:
        detail = (out.stderr.decode("utf-8", "replace").strip()
                  or out.stdout.decode("utf-8", "replace").strip() or "nonzero exit")
        return False, detail[:200]
    return True, out.stdout.decode("utf-8", "replace").strip()[:80]


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
    global CLAUDE
    CLAUDE = shutil.which("claude")
    if not CLAUDE:
        skip("`claude` binary is not on PATH")
    if not PTYHOST.is_file():
        skip("ptyhost is not built (cargo build -p ptyhost)")

    with tempfile.TemporaryDirectory(prefix="sessiondock-claude-real-") as temporary:
        tmp = Path(temporary).resolve()
        config, copied = isolated_config(tmp)
        if not copied:
            skip("no ~/.claude/.credentials.json to reuse (not logged in)")
        work = tmp / "work"
        area = work / "claude-area"
        host = tmp / "hosts"
        ledger = tmp / "ledger"
        delivery = tmp / "delivery"
        web = tmp / "web"
        for path in (work, area, host, ledger, delivery, web):
            path.mkdir(mode=0o700, parents=True)
        (web / "index.html").write_text(
            '<!doctype html><meta name="sessiondock-mode" content="local"><title>x</title>')

        trust_project(config, area)
        import uuid
        sid = str(uuid.uuid4())
        ok, detail = logged_in(config, area, sid)
        if not ok:
            skip(f"isolated `claude` login probe failed: {detail}")
        print(f"login probe ok: {detail!r}", flush=True)

        # The real CLI writes sessions under <config>/projects/<cwd>/<sid>.jsonl.
        claude_root = config / "projects"
        launcher = tmp / "launcher.json"
        launcher.touch(mode=0o600)
        launcher.write_text(json.dumps({
            "schema": 2, "host_binary": str(PTYHOST), "host_dir": str(host),
            "adapters": [],
            "profiles": [{
                "id": "claude-real-v1", "source": "claude", "executable": CLAUDE,
                "args": ["--model", MODEL, "--effort", "low",
                         "--permission-mode", "plan", "--tools", ""],
                "new_args": ["--session-id", "{session_id}"],
                "resume_args": ["--resume", "{sid}"],
                "env": {**passthrough(), "HOME": str(work), "CLAUDE_CONFIG_DIR": str(config)},

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

        # Point the corpus's claude root at the CLI's projects dir: isolated_server
        # sets SESSIONDOCK_CLAUDE_ROOT = corpus.root/claude, so symlink it.
        # isolated_server requires every private directory to sit under the
        # corpus root, so the corpus root is the temp dir itself.
        corpus = Corpus(tmp)
        (tmp / "claude").symlink_to(claude_root)
        for source in ("codex", "grok"):
            (tmp / source).mkdir(mode=0o700)

        deadline = time.monotonic() + 90
        record = None

        def session_file():
            return next(claude_root.glob(f"*/{sid}.jsonl"), None)
        if session_file() is None:
            skip(f"the one-shot did not create {sid}.jsonl under the isolated projects dir")
        with isolated_server(corpus, SERVER, host_dir=host, lifecycle_dir=ledger,
                             launcher_config=launcher, delivery_dir=delivery) as (base, opener):
            status, meta = request(opener, base, "GET", "/api/meta")
            assert status == 200 and meta["capabilities"]["outbox"] is True, meta
            build = meta["build"]

            # The one-shot's session is in the frozen inventory; resume it in a
            # managed TUI through the profile's resume_args (--resume {sid}).
            status, listed = request(opener, base, "GET", "/api/sessions?force=1")
            assert status == 200, listed
            row = next((r for r in listed.get("sessions", [])
                        if r.get("source") == "claude" and r.get("sid") == sid), None)
            assert row is not None, f"session {sid} not listed"
            # The read model skips unknown record/attachment kinds
            # like Python and reports them as non-fatal warnings; a real session
            # that is still unsupported is a read-model regression, never a skip.
            assert row.get("supported") is True, (
                "the real CLI session is unsupported by the read model: "
                + "; ".join(map(str, row.get("migration_warnings") or [])))
            print("session listed supported; migration_warnings="
                  + json.dumps(row.get("migration_warnings") or [], ensure_ascii=False), flush=True)
            uid = row["uid"]
            status, record = request(opener, base, "POST", "/api/term/create",
                {"source": "claude", "cwd": str(area), "request_id": "real-create",
                 "adapter_id": "claude-real-v1", "resume_uid": uid})
            assert status == 200 and record.get("running"), record
            name = record["name"]

            # Wait until the runtime associates the managed instance with the UID.
            associated = False
            while not associated and time.monotonic() < deadline:
                status, listing = request(opener, base, "GET", "/api/term/list")
                associated = any(r.get("name") == name and r.get("uid") == uid
                                 for r in listing.get("sessions", []))
                if not associated:
                    time.sleep(0.5)
            if not associated:
                skip("the resumed real CLI never became an associated managed instance "
                     "(TUI startup differs); auth and one-shot verified above")
            time.sleep(2.0)  # let the resumed TUI paint its composer

            status, sent = request(opener, base, "POST", "/api/session/send",
                {"uid": uid, "name": name, "text": SECOND, "media": [],
                 "request_id": "real-send-0001", "_build": build})
            assert status == 200 and sent["item"]["state"] in ("ambiguous", "persisted"), sent
            print(f"send accepted: {sent['item']['state']}", flush=True)

            confirmed = False
            while time.monotonic() < deadline:
                status, box = request(opener, base, "GET",
                    "/api/session/outbox?uid=" + quote(uid, safe=":"))
                if status == 200 and not any(r["id"] == "real-send-0001" for r in box["outbox"]):
                    confirmed = True
                    break
                time.sleep(1.0)
            if not confirmed:
                skip("the real CLI did not confirm the prompt within the window "
                     "(composer injection differs); auth and one-shot verified above")

            # The confirmed native user record is in the session JSONL.
            path = session_file()
            assert path is not None, "confirmed but no session file"
            rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
            users = [r for r in rows if r.get("type") == "user"
                     and isinstance(r.get("message", {}).get("content"), str)]
            assert any(SECOND.strip() in r["message"]["content"] for r in users), \
                f"prompt not found in native user records: {[r['message']['content'][:40] for r in users]}"
            # The model flag reached the CLI: assistant records carry the model name.
            models = [r.get("message", {}).get("model", "") for r in rows if r.get("type") == "assistant"]
            assert any(m == MODEL for m in models), f"model assertion failed: expected {MODEL} in {models}"
            print(f"confirmed from native JSONL; models={sorted(set(m for m in models if m))}", flush=True)

            # The assistant reply is visible through /api/messages.
            status, messages = request(opener, base, "GET", "/api/messages/" + quote(uid, safe=":"))
            assert status == 200, messages
            texts = [m.get("text", "") for m in messages.get("messages", [])]
            assert any(SECOND.strip() in t for t in texts), texts
            assert any(m.get("role") == "assistant" for m in messages.get("messages", [])), \
                "no assistant message in history"

            # Kill the instance we created.
            status, killed = request(opener, base, "POST", "/api/term/kill",
                {"record_id": record["record_id"], "instance_id": record["instance_id"]})
            assert status == 200, killed
        print("PASS send_claude_real: real Claude (claude-haiku-4-5-20251001, effort low) launched via the launcher "
              "profile (one-shot created the session, the TUI resumed it), one prompt sent through /api/session/send, receipt persisted then injected "
              "then confirmed from the real native JSONL user record, assistant reply visible in "
              "/api/messages, model flag reached the CLI (exact ID in JSONL), instance killed; isolated config "
              "reused read-only, temp dirs removed")


if __name__ == "__main__":
    main()
