#!/usr/bin/env python3
"""HTTP-only contract of POST /api/session/stop for managed instances (batch 29). No Chromium."""
from __future__ import annotations
import argparse, json, os, subprocess, tempfile, time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, codex_message, codex_row, isolated_server

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
CODEX_SID = "8f3c1d2e-4a5b-4c6d-8e7f-90a1b2c3d4e5"
MARK = "AH_STOP_HTTP_SUITE"
FAKE = (
    "#!/bin/sh\nprintf 'FAKE_CODEX_ARGV'\n"
    'for a in "$@"; do printf \' [%s]\' "$a"; done\nprintf \'\\n\'\n'
    "exec /bin/sh -c '"
    f"{MARK}=1; stty -echo 2>/dev/null; printf \"RS_SHELL_READY\\n\"; "
    "while IFS= read -r line; do case \"$line\" in quit) exit 0 ;; *) printf \"RS_UNKNOWN\\n\" ;; esac; done'\n"
)


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def call(opener, base, method, route, body=None, want=200):
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
    if code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        return (json.loads(raw) if raw else {}), raw
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)


def leftovers():
    needle, pids = MARK.encode(), []
    for pid in Path("/proc").iterdir():
        if not pid.name.isdigit():
            continue
        try:
            if needle in (pid / "cmdline").read_bytes():
                pids.append(pid.name)
        except OSError:
            pass
    return pids


def wait_listed(opener, base, uid, instance, timeout=10):
    deadline, last, raw = time.monotonic() + timeout, {}, b""
    while time.monotonic() < deadline:
        last, raw = call(opener, base, "GET", "/api/term/list")
        if any(row.get("uid") == uid and row.get("instance_id") == instance
               for row in last.get("sessions") or []):
            return last
        time.sleep(0.05)
    fail("term/list", "instance never listed as running", raw)


def has_uid(listed, uid):
    return any(row.get("uid") == uid for row in listed.get("sessions") or []) or any(
        row.get("declared_uid") == uid for row in listed.get("pending") or [])


def run(opener, base, uid, other, cwd):
    n = 0
    meta, raw = call(opener, base, "GET", "/api/meta")
    if (meta.get("capabilities") or {}).get("session_stop") is not True:
        fail("capability", "session_stop", raw)
    passed("capability")
    n += 1

    err, raw = call(opener, base, "POST", "/api/session/stop",
                    {"uid": "codex:0000000000000000"}, want=404)
    if err.get("code") != "session_missing":
        fail("unknown uid", err.get("code"), raw)
    passed("unknown uid")
    n += 1

    err, raw = call(opener, base, "POST", "/api/session/stop", {"uid": uid}, want=501)
    if err.get("code") != "session_stop_unmanaged":
        fail("unmanaged", err.get("code"), raw)
    listed, raw = call(opener, base, "GET", "/api/term/list")
    if has_uid(listed, uid):
        fail("unmanaged", "term/list still has the uid", raw)
    passed("unmanaged")
    n += 1

    for payload in ({}, {"uid": "x", "bogus": 1}):
        err, raw = call(opener, base, "POST", "/api/session/stop", payload, want=400)
        if payload == {} and err.get("code") != "invalid_stop_request":
            fail("bad body", err.get("code"), raw)
    passed("bad body")
    n += 1

    rec, raw = call(opener, base, "POST", "/api/term/create",
                    {"source": "codex", "resume_uid": uid, "cwd": cwd, "request_id": "stop-suite-1"})
    wait_listed(opener, base, uid, rec.get("instance_id"))
    stop_body = {"uid": uid, "request_id": "stop-req-1"}
    started = time.monotonic()
    stopped, raw = call(opener, base, "POST", "/api/session/stop", stop_body)
    elapsed = time.monotonic() - started
    if (stopped.get("ok") is not True or stopped.get("stage") != "graceful"
            or stopped.get("stopped") is not True or stopped.get("graceful_attempts") != 1
            or stopped.get("replayed") is not False or stopped.get("tmux") is not False
            or stopped.get("external_detection") != "not_implemented"
            or stopped.get("uid") != uid or not stopped.get("name") or not stopped.get("instance_id")
            or elapsed >= 2.4):
        fail("graceful", f"elapsed={elapsed:.3f}s body={stopped}", raw)
    passed("graceful")
    n += 1

    replay, raw = call(opener, base, "POST", "/api/session/stop", stop_body)
    if replay.get("replayed") is not True or replay.get("stage") != "graceful":
        fail("replay", replay, raw)
    conflict, raw = call(opener, base, "POST", "/api/session/stop",
                         {"uid": other, "request_id": "stop-req-1"}, want=409)
    if conflict.get("code") != "stop_request_conflict":
        fail("replay", conflict.get("code"), raw)
    passed("replay")
    n += 1

    again, raw = call(opener, base, "POST", "/api/session/stop", {"uid": uid})
    if again.get("stage") != "already_exited" or again.get("graceful_attempts") != 0:
        fail("already exited", again, raw)
    passed("already exited")
    n += 1

    deadline, last, raw = time.monotonic() + 10, {}, b""
    while time.monotonic() < deadline:
        last, raw = call(opener, base, "GET", "/api/live?force=1")
        row = ((last.get("managed") or {}).get("sessions") or {}).get(uid) or {}
        sessions = ((last.get("managed") or {}).get("sessions") or {})
        if row.get("state") == "exited" or (uid not in sessions and uid not in (last.get("uids") or [])):
            passed("live")
            return n + 1
        time.sleep(0.1)
    fail("live", "uid not exited or absent", raw)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    if os.name != "posix":
        print("SKIP session_stop_http_suite: POSIX required", flush=True)
        return
    if not PTYHOST.is_file():
        print("SKIP session_stop_http_suite: target/debug/ptyhost is not built", flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-session-stop-http-") as tmp:
        root = Path(tmp)
        for name in ("host", "work", "ledger", "bin", "claude", "codex", "grok"):
            (root / name).mkdir(mode=0o700, parents=True, exist_ok=True)
            (root / name).chmod(0o700)
        corpus, cwd, fake = Corpus(root), str(root / "work"), root / "bin" / "fake-codex"
        corpus.put(CODEX_SID, "codex", [
            codex_row("session_meta", {"id": CODEX_SID, "cwd": cwd}),
            codex_message("user", "Synthetic managed stop target")], [])
        corpus.put("synthetic-unrelated-claude", "claude", [
            claude_row("synthetic-unrelated-claude", "user", "u0", None, "unmanaged"),
            claude_row("synthetic-unrelated-claude", "assistant", "a0", "u0", "answer")], [])
        uid, other = corpus.uid(CODEX_SID), corpus.uid("synthetic-unrelated-claude")
        fake.write_text(FAKE)
        fake.chmod(0o700)
        cfg = root / "launcher.json"
        cfg.touch(mode=0o600)
        cfg.write_text(json.dumps({
            "schema": 2, "host_binary": str(PTYHOST.resolve()), "host_dir": str(root / "host"),
            "cwd_roots": [cwd], "adapters": [],
            "profiles": [{"id": "codex-cli-v1", "source": "codex", "executable": str(fake.resolve()),
                          "args": [], "new_args": [], "resume_args": ["resume", "{sid}"],
                          "env": {"PATH": "/usr/bin:/bin", "HOME": "/synthetic/codex-home"},
                          "cwd_roots": [cwd]}]}))
        cfg.chmod(0o600)
        init = subprocess.run([str(args.binary), "--initialize-lifecycle", str(root / "ledger")],
                              cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        if init.returncode:
            fail("initialize-lifecycle", init.stderr.decode() or init.stdout.decode())
        with isolated_server(corpus, args.binary, host_dir=root / "host",
                             lifecycle_dir=root / "ledger", launcher_config=cfg) as (base, opener):
            n = run(opener, base, uid, other, cwd)
        with isolated_server(corpus, args.binary, host_dir=root / "host") as (base, opener):
            meta, raw = call(opener, base, "GET", "/api/meta")
            if (meta.get("capabilities") or {}).get("session_stop") is not False:
                fail("no lifecycle", "session_stop", raw)
            err, raw = call(opener, base, "POST", "/api/session/stop", {"uid": uid}, want=501)
            if err.get("code") != "not_implemented":
                fail("no lifecycle", err.get("code"), raw)
            passed("no lifecycle")
            n += 1
        alive = leftovers()
        if alive:
            fail("cleanup", f"fake CLI still alive pids={alive}")
        print(f"PASS session_stop_http_suite: {n} scenarios", flush=True)


if __name__ == "__main__":
    main()
