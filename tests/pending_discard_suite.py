#!/usr/bin/env python3
"""Exited pending receipts can be discarded and leave /api/term/list.pending (batch 44 WP-E)."""
from __future__ import annotations
import argparse, json, os, subprocess, sys, tempfile, time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request
from history_parity import Corpus, isolated_server, REPO
from lifecycle_http_suite import (
    BINARY, FAKE, PTYHOST, SHELL, call, fail, passed, qstat, wait_run)

CHECKS = 0


def ok(area):
    global CHECKS
    CHECKS += 1
    passed(area)


def call_codes(opener, base, method, route, body=None, want=200):
    allowed = want if isinstance(want, (tuple, list, set)) else (want,)
    req = Request(base + route, data=None if body is None else json.dumps(body).encode(), method=method)
    if body is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with opener.open(req, timeout=20) as resp:
            raw, code = resp.read(2 * 1024 * 1024 + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    except (URLError, TimeoutError, OSError) as err:
        fail(route, str(err))
    if code not in allowed:
        fail(route, f"HTTP {code} (want {allowed})", raw)
    try:
        return (json.loads(raw) if raw else {}), raw, code
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)


def pending_row(listed, rec):
    return next((row for row in (listed.get("pending") or [])
                 if row.get("record_id") == rec["record_id"]), None)


def flow(opener, base, work):
    rec, raw = call(opener, base, "POST", "/api/term/create",
                    {"source": "codex", "cwd": work, "request_id": "discard-1"})
    rec = wait_run(opener, base, rec)
    if rec.get("running") is not True:
        fail("create running", rec, raw)
    ok("POST /api/term/create request_id=discard-1 running")

    killed, raw = call(opener, base, "POST", "/api/term/kill",
                       {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})
    deadline, last = time.monotonic() + 8, killed
    while time.monotonic() < deadline:
        if last.get("running") is False and last.get("state") in ("exited", "cancel_requested", "uncertain"):
            break
        last, raw = call(opener, base, "GET", qstat(rec))
        time.sleep(0.05)
    else:
        fail("kill wait", last, raw)
    ok(f"POST /api/term/kill running=false state={last.get('state')!r}")

    listed, raw = call(opener, base, "GET", "/api/term/list")
    row = pending_row(listed, rec)
    if not row:
        fail("list still pending (600s window)", listed.get("pending"), raw)
    label = row.get("label") or row.get("title") or row.get("explanation") or row.get("state")
    if not row.get("state") or not label:
        fail("pending state/label", row, raw)
    ok(f"GET /api/term/list pending state={row.get('state')!r} label={label!r}")

    call(opener, base, "POST", "/api/term/discard",
         {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})
    ok("POST /api/term/discard 200")

    listed, raw = call(opener, base, "GET", "/api/term/list")
    if pending_row(listed, rec) is not None:
        fail("list after discard", listed.get("pending"), raw)
    ok("GET /api/term/list pending no longer contains receipt")

    st, raw = call(opener, base, "GET", qstat(rec))
    flags = {k: st.get(k) for k in ("discarded", "state", "discardable", "finished_at") if k in st}
    if st.get("discarded") is not True and st.get("state") != "exited":
        fail("new-status after discard", st, raw)
    ok(f"GET /api/term/new-status still 200 fields={flags}")

    again, raw, code = call_codes(opener, base, "POST", "/api/term/discard",
                                  {"record_id": rec["record_id"], "instance_id": rec["instance_id"]},
                                  want=(200, 409))
    ok(f"POST /api/term/discard again HTTP {code} {'idempotent' if code == 200 else again.get('code')}")

    wrong = "0" * 32 if rec["instance_id"] != "0" * 32 else "f" * 32
    ident, raw = call(opener, base, "POST", "/api/term/discard",
                      {"record_id": rec["record_id"], "instance_id": wrong}, want=409)
    if ident.get("code") != "launch_identity":
        fail("wrong instance", ident.get("code"), raw)
    ok("POST /api/term/discard wrong instance_id 409 launch_identity")

    missing = "a" * 32 if rec["record_id"] != "a" * 32 else "b" * 32
    gone, raw = call(opener, base, "POST", "/api/term/discard",
                     {"record_id": missing, "instance_id": rec["instance_id"]}, want=404)
    if gone.get("code") != "launch_missing":
        fail("missing record", gone.get("code"), raw)
    ok("POST /api/term/discard unknown record_id 404 launch_missing")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    if os.name != "posix":
        print("SKIP pending_discard_suite: POSIX required", flush=True)
        return
    if not PTYHOST.is_file():
        print("SKIP pending_discard_suite: target/debug/ptyhost is not built", flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-pending-discard-") as tmp:
        root = Path(tmp)
        for name in ("host", "work", "work/claude-area", "ledger", "bin", "claude", "codex", "grok"):
            (root / name).mkdir(mode=0o700, parents=True, exist_ok=True)
            (root / name).chmod(0o700)
        corpus, fake = Corpus(root), root / "bin" / "fake-claude"
        fake.write_text(FAKE)
        fake.chmod(0o700)
        cfg = root / "launcher.json"
        cfg.touch(mode=0o600)
        cfg.write_text(json.dumps({
            "schema": 2, "host_binary": str(PTYHOST.resolve()), "host_dir": str(root / "host"),

            "adapters": [{"id": "synthetic-shell-v1", "source": "codex",
                          "executable": str(Path("/bin/sh").resolve()), "args": ["-c", SHELL],
                          "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}}],
            "profiles": [{"id": "claude-cli-v1", "source": "claude", "executable": str(fake.resolve()),
                          "args": ["--settings", "/synthetic/bridge-settings.json"],
                          "new_args": ["--session-id", "{session_id}"],
                          "resume_args": ["--resume", "{sid}"],
                          "env": {"PATH": "/usr/bin:/bin", "HOME": "/synthetic/claude-home"}}]}))
        cfg.chmod(0o600)
        init = subprocess.run([str(args.binary), "--initialize-lifecycle", str(root / "ledger")],
                              cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        if init.returncode:
            fail("initialize-lifecycle", init.stderr.decode() or init.stdout.decode())
        with isolated_server(corpus, args.binary, host_dir=root / "host",
                             lifecycle_dir=root / "ledger", launcher_config=cfg) as (base, opener):
            flow(opener, base, str(root / "work"))
    print(f"pending_discard_suite: {CHECKS} checks passed", flush=True)


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    main()
