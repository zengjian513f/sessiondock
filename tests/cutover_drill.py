#!/usr/bin/env python3
# run_validation: skip
"""Isolated rehearsal of docs/replacement-checklist.md §4/§5 (switch + rollback).

Operator-run (not in run_validation). Proves a Web restart never kills the
ptyhost host and that Python data is untouched: SIGTERM the Rust server,
`ptyhost --dir list` still shows the same instance (checklist §5 step 1 / §6
rollback), fake Python `session-meta.json` bytes and mtime are unchanged, the
Rust state dir contains no `session-meta.json`; restart on the same dirs still
serves `/api/health` and the synthetic session. `/api/meta` reports `hub:false`.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, build_opener

from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, NoRedirects, claude_row, get_json

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
HOST_NAME = "cutover-sleep"
SID = "cutover-claude"


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def mkdir(root, name, mode=0o700):
    path = root / name
    path.mkdir(mode=mode)
    path.chmod(mode)
    return path


def stop(proc, timeout=5):
    if proc is None or proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)


def host_list(ptyhost, host):
    out = subprocess.run(
        [str(ptyhost), "--dir", str(host), "list"], capture_output=True, text=True, timeout=10)
    if out.returncode:
        fail("ptyhost list", out.stderr.strip() or f"exit {out.returncode}")
    return out.stdout


def load(opener, base, route):
    try:
        return get_json(opener, base, route)
    except HTTPError as err:
        fail(route, f"HTTP {err.code}", err.read(4096))
    except (URLError, TimeoutError, OSError, json.JSONDecodeError, AssertionError) as err:
        fail(route, str(err))


def initialize(binary, flag, directory, env):
    out = subprocess.run(
        [str(binary), flag, str(directory)], cwd=REPO, env=env, capture_output=True, timeout=20)
    if out.returncode:
        fail(flag, (out.stderr or out.stdout).decode("utf-8", "replace"))


def start_host(ptyhost, host, work):
    sh = shutil.which("sh")
    if not sh:
        fail("ptyhost", "POSIX sh not found")
    env = {key: value for key, value in os.environ.items() if key in {"PATH", "LANG", "LC_ALL", "LC_CTYPE"}}
    env["TERM"] = "xterm-256color"
    proc = subprocess.Popen(
        [str(ptyhost), "--dir", str(host), "run", "--name", HOST_NAME,
         "--cwd", str(work), "--cols", "80", "--rows", "24", "--", sh, "-c", "sleep 600"],
        cwd=str(work), env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.monotonic() + 5
    listed = ""
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            fail("ptyhost", f"exited {proc.returncode}")
        listed = host_list(ptyhost, host)
        if HOST_NAME in listed:
            return proc, listed
        time.sleep(0.05)
    fail("ptyhost", "list timed out waiting for instance", listed.encode())


def start_server(binary, base_env, logs):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    env = dict(base_env)
    env["SESSIONDOCK_BIND"] = f"127.0.0.1:{port}"
    log = tempfile.TemporaryFile(mode="w+b")
    logs.append(log)
    proc = subprocess.Popen([str(binary)], cwd=REPO, env=env, stdout=log, stderr=log)
    opener = build_opener(ProxyHandler({}), NoRedirects())
    base = f"http://127.0.0.1:{port}"
    for _ in range(150):
        if proc.poll() is not None:
            log.seek(0)
            fail("boot", f"exited {proc.returncode}", log.read(1024))
        try:
            get_json(opener, base, "/api/health")
            return proc, opener, base
        except (OSError, URLError):
            time.sleep(0.05)
    fail("boot", "health timed out")


def check_listed(opener, base, area):
    payload = load(opener, base, "/api/sessions?force=1")
    sids = {row.get("sid") for row in payload.get("sessions") or []}
    if SID not in sids:
        fail(area, f"missing {SID} in {sorted(sids)}")
    passed(area)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--ptyhost", type=Path, default=PTYHOST)
    parser.add_argument("--keep", action="store_true", help="do not delete the temp dir")
    args = parser.parse_args()
    binary, ptyhost = args.binary.expanduser(), args.ptyhost.expanduser()
    if not binary.is_file():
        fail("binary", f"missing {binary}")
    if not ptyhost.is_file():
        fail("ptyhost", f"missing {ptyhost}")
    binary, ptyhost = binary.resolve(), ptyhost.resolve()
    root = Path(tempfile.mkdtemp(prefix="sessiondock-cutover-"))
    server = host_proc = None
    logs = []
    try:
        for source in ("claude", "codex", "grok"):
            mkdir(root, source)
        corpus = Corpus(root)
        corpus.put(SID, "claude", [
            claude_row(SID, "user", "u0", None, "cutover drill"),
            claude_row(SID, "assistant", "a0", "u0", "ok")], [])
        python_dir = mkdir(root, "python-data")
        python_meta = python_dir / "session-meta.json"
        python_meta.write_text('{"schema":1,"synthetic":true}\n', encoding="utf-8")
        python_stamp = (python_meta.read_bytes(), python_meta.stat().st_mtime_ns)
        state = mkdir(root, "state")
        host = mkdir(root, "host")
        delivery = mkdir(root, "delivery")
        lifecycle = mkdir(root, "lifecycle")
        audit = mkdir(root, "audit")
        work = mkdir(root, "work")
        launcher = root / "launcher.json"
        launcher.touch(mode=0o600)
        launcher.write_text(json.dumps({
            "schema": 2, "host_binary": str(ptyhost), "host_dir": str(host),
            "cwd_roots": [str(work)],
            "adapters": [{"id": "synthetic-shell-v1", "source": "codex",
                          "executable": str(Path("/bin/sh").resolve()),
                          "args": ["-c", "sleep 600"],
                          "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}}]}))
        launcher.chmod(0o600)
        passed("corpus")

        env = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
        env.update({
            "SESSIONDOCK_BIND": "127.0.0.1:0",
            "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"),
            "SESSIONDOCK_CLAUDE_ROOT": str(corpus.root / "claude"),
            "SESSIONDOCK_CODEX_ROOT": str(corpus.root / "codex"),
            "SESSIONDOCK_GROK_ROOT": str(corpus.root / "grok"),
            "SESSIONDOCK_STATE_DIR": str(state),
            "SESSIONDOCK_PTYHOST_DIR": str(host),
            "SESSIONDOCK_DELIVERY_DIR": str(delivery),
            "SESSIONDOCK_LIFECYCLE_DIR": str(lifecycle),
            "SESSIONDOCK_LAUNCHER_CONFIG": str(launcher),
            "SESSIONDOCK_AUDIT_DIR": str(audit),
        })
        initialize(binary, "--initialize-delivery", delivery, env)
        initialize(binary, "--initialize-lifecycle", lifecycle, env)

        host_proc, instance = start_host(ptyhost, host, work)
        passed("ptyhost")

        server, opener, base = start_server(binary, env, logs)
        health = load(opener, base, "/api/health")
        if health.get("status") != "ok":
            fail("health", f"status={health.get('status')!r}", json.dumps(health).encode())
        passed("health")
        check_listed(opener, base, "sessions")
        meta = load(opener, base, "/api/meta")
        caps = meta.get("capabilities") if isinstance(meta.get("capabilities"), dict) else meta
        if caps.get("hub") is not False:
            fail("hub", f"hub={caps.get('hub')!r}", json.dumps(meta).encode())
        passed("hub")

        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            fail("sigterm", "server still running after 10s")
        server = None
        leftover = host_list(ptyhost, host)
        if HOST_NAME not in leftover or instance.strip() not in leftover:
            fail("rollback host", f"instance gone; before={instance!r} after={leftover!r}")
        passed("rollback host")
        now = (python_meta.read_bytes(), python_meta.stat().st_mtime_ns)
        if now != python_stamp:
            fail("rollback python", "session-meta.json bytes or mtime changed")
        passed("rollback python")
        if (state / "session-meta.json").exists():
            fail("rollback state", "state dir contains session-meta.json")
        passed("rollback state")

        server, opener, base = start_server(binary, env, logs)
        health = load(opener, base, "/api/health")
        if health.get("status") != "ok":
            fail("restart health", f"status={health.get('status')!r}")
        passed("restart health")
        check_listed(opener, base, "restart sessions")

        stop(server)
        server = None
        stop(host_proc)
        host_proc = None
        passed("cleanup")
    finally:
        stop(server)
        stop(host_proc)
        if args.keep:
            print(f"kept {root}", flush=True)
        else:
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
