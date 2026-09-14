#!/usr/bin/env python3
"""HTTP-only contract of GET /api/live: empty inventory,
cache hit/force miss, bound free-shell running then exited; then the explicit
`/proc` scan (Python live.py) over a synthetic process tree: uids,
tmux_uids (with the Python 16cc89c CLI barrier: a `grok -p` under a pane's
claude is live but not managed), started_at, the scan cache and `spawned_by`
recording; finally continued-in pane inheritance against a real ptyhost pane
whose synthetic subtree runs the continued session. No Chromium."""
from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener

from history_parity import (BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, codex_message, codex_row, encoded,
                            isolated_server)
from provider_parity import NoRedirects

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
SHELL = (
    'stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; '
    'while IFS= read -r c; do case "$c" in quit) exit 0 ;; *) printf "RS_UNKNOWN\\n" ;; esac; done'
)
EXIT_EVIDENCE = {"host_exit", "exit_receipt", "identity_gone"}  # docs/processes.md three-state


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def call(opener, base, method, route, body=None, want=200):
    request = Request(base + route, data=None if body is None else json.dumps(body).encode(), method=method)
    if body is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with opener.open(request, timeout=20) as resp:
            raw, code = resp.read(2 * 1024 * 1024 + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    except (URLError, TimeoutError, OSError) as err:
        fail(route, str(err))
    if code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)
    return payload, raw


def redacted(raw):
    text = raw.decode("utf-8", "replace") if isinstance(raw, (bytes, bytearray)) else str(raw)
    for needle in ("token", "sock", "argv"):
        if needle in text:
            fail("redaction", needle, raw)


def wait_run(opener, base, rec, timeout=8):
    deadline = time.monotonic() + timeout
    last, raw = {}, b""
    while time.monotonic() < deadline:
        last, raw = call(opener, base, "GET",
                         f"/api/term/new-status?record_id={rec['record_id']}&instance_id={rec['instance_id']}")
        if last.get("running") is True:
            return last
        time.sleep(0.05)
    fail("new-status", f"running={last.get('running')!r}", raw)


def wait_live(opener, base, pred, timeout=5):
    deadline = time.monotonic() + timeout
    last, raw = {}, b""
    while time.monotonic() < deadline:
        last, raw = call(opener, base, "GET", "/api/live?force=1")
        redacted(raw)
        if pred(last):
            return last, raw
        time.sleep(0.1)
    fail("live", "timeout", raw)


def session(body, uid):
    return ((body.get("managed") or {}).get("sessions") or {}).get(uid) or {}


def run(opener, base, uid, other, work):
    body, raw = call(opener, base, "GET", "/api/live")
    managed = body.get("managed") or {}
    cache = managed.get("cache") or {}
    if (body.get("enabled") is not True or body.get("known") is not True or body.get("uids") != []
            or managed.get("sessions") != {} or (managed.get("unlisted") or {}).get("state") != "unknown"
            or cache.get("ttl_ms") != 2000 or body.get("partial") is not False
            or managed.get("external_detection") != "proc_scan"
            or ((body.get("scan") or {}).get("stats") or {}).get("processes") != 1):
        fail("empty inventory", "enabled/known/uids/sessions/unlisted/cache/native scan", raw)
    passed("GET /api/live empty inventory enabled/known, sessions {}, unlisted.unknown, ttl_ms 2000")

    time.sleep(0.05)
    hit, raw = call(opener, base, "GET", "/api/live")
    cache = (hit.get("managed") or {}).get("cache") or {}
    if cache.get("hit") is not True or not (cache.get("age_ms") or 0) > 0:
        fail("cache hit", cache, raw)
    forced, raw = call(opener, base, "GET", "/api/live?force=1")
    if ((forced.get("managed") or {}).get("cache") or {}).get("hit") is not False:
        fail("force miss", (forced.get("managed") or {}).get("cache"), raw)
    passed("GET /api/live cache hit (age_ms>0) and ?force=1 miss")

    rec, raw = call(opener, base, "POST", "/api/term/create",
                    {"source": "codex", "cwd": work, "request_id": "live-http-free-shell-1"})
    rec = wait_run(opener, base, rec)
    bound, raw = call(opener, base, "POST", "/api/term/bind",
                      {"record_id": rec["record_id"], "instance_id": rec["instance_id"],
                       "uid": uid, "operator_confirmed": True})
    if bound.get("native_binding") != "confirmed" and (bound.get("binding") or {}).get("state") != "confirmed":
        fail("bind", bound.get("native_binding") or bound.get("binding"), raw)
    live, raw = wait_live(opener, base, lambda d: session(d, uid).get("state") == "running", timeout=8)
    row, at = session(live, uid), (live.get("started_at") or {}).get(uid)
    if (row.get("evidence") != "host_info" or uid not in (live.get("uids") or [])
            or not isinstance(at, (int, float)) or isinstance(at, bool)):
        fail("running", f"row={row} started_at={at!r} uids={live.get('uids')}", raw)
    if other in ((live.get("managed") or {}).get("sessions") or {}):
        fail("unrelated running", "present in sessions", raw)
    redacted(raw)
    passed("GET /api/live bound free-shell running evidence=host_info, uids, started_at number")

    call(opener, base, "POST", "/api/term/kill",
         {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})
    ended, raw = wait_live(
        opener, base,
        lambda d: session(d, uid).get("state") == "exited" and session(d, uid).get("evidence") in EXIT_EVIDENCE)
    if other in ((ended.get("managed") or {}).get("sessions") or {}):
        fail("unrelated exited", "present in sessions (would look stopped)", raw)
    if session(ended, other).get("state") == "exited":
        fail("unrelated exited", session(ended, other), raw)
    passed("POST /api/term/kill → exited with documented evidence within 5s")
    passed("unrelated session absent from sessions (unknown, never exited)")
    passed("envelope contains no token/sock/argv")


# ---------------------------------------------------------------- proc scan
# The `/proc` scan (Python live.py) over a synthetic tree
# (SESSIONDOCK_PROC_ROOT), Python tests/test_live.py FakeProc shape.
BTIME = 1_700_000_000
SID_A = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa"   # claude, resumed by pid 100
SID_B = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb"   # claude, stopped; orphan helper keeps its id
SID_C = "cccccccc-3333-4333-8333-cccccccccccc"   # codex, rollout held open by pid 300
SID_D = "dddddddd-4444-4444-8444-dddddddddddd"   # grok -p under claude A, events.jsonl open
SID_E = "eeeeeeee-5555-4555-8555-eeeeeeeeeeee"   # claude -p spawned by claude A's tool shell
SID_F = "ffffffff-6666-4666-8666-ffffffffffff"   # claude inside a tmux started from A
SID_H = "0aaaaaaa-7777-4777-8777-aaaaaaaaaaaa"   # grok -p spawned by F's tool shell inside F's tmux pane
# Continued-in case (real ptyhost pane): origin claude → daemon → next claude; origin also spawns a grok -p.
SID_ORIGIN = "0bbbbbbb-8888-4888-8888-bbbbbbbbbbbb"
SID_NEXT = "0ccccccc-9999-4999-8999-cccccccccccc"
SID_CHILD = "0ddddddd-aaaa-4aaa-8aaa-dddddddddddd"


class FakeProc:
    """A controllable /proc tree: only the files the scan and the ancestry walks read."""

    def __init__(self, root: Path):
        self.root = root
        root.mkdir(parents=True, exist_ok=True)
        (root / "stat").write_text(f"cpu  1 2 3 4\nbtime {BTIME}\nprocesses 1\n")

    def add(self, pid, comm, ppid, cmdline, env=None, fds=None, start_ticks=0, cwd=None):
        d = self.root / str(pid)
        (d / "fd").mkdir(parents=True)
        (d / "cmdline").write_bytes(cmdline.replace(" ", "\0").encode() + b"\0")
        (d / "stat").write_text(
            f"{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {start_ticks} 0 0\n")
        (d / "environ").write_bytes(b"".join(f"{k}={v}".encode() + b"\0" for k, v in (env or {}).items()))
        for fd, target in (fds or {}).items():
            os.symlink(str(target), d / "fd" / str(fd))
        if cwd is not None:
            os.symlink(str(cwd), d / "cwd")


@contextmanager
def scan_server(binary, roots, state, proc_root, grok_active=None, managed=None):
    """isolated_server with a synthetic native process table;
    `managed` = (host_dir, lifecycle_dir, launcher_config) adds the private ptyhost inventory."""
    executable = Path(binary).resolve(strict=True)
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    environment.update({
        "SESSIONDOCK_BIND": f"127.0.0.1:{port}", "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"),
        "SESSIONDOCK_STATE_DIR": str(state),
        "SESSIONDOCK_PROC_ROOT": str(proc_root)})
    if grok_active is not None:
        environment["SESSIONDOCK_GROK_ACTIVE"] = str(grok_active)
    if managed is not None:
        host_dir, lifecycle_dir, launcher_config = managed
        environment.update({"SESSIONDOCK_PTYHOST_DIR": str(host_dir), "SESSIONDOCK_LIFECYCLE_DIR": str(lifecycle_dir),
                            "SESSIONDOCK_LAUNCHER_CONFIG": str(launcher_config)})
    for source, path in roots.items():
        environment["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(path)
    opener = build_opener(ProxyHandler({}), NoRedirects())
    with tempfile.TemporaryFile(mode="w+b") as log:
        process = subprocess.Popen([str(executable)], cwd=REPO, env=environment, stdout=log, stderr=log)
        try:
            for _ in range(150):
                if process.poll() is not None:
                    log.seek(0)
                    fail("scan server", f"exited early ({process.returncode})", log.read()[-2000:])
                try:
                    with opener.open(base + "/api/health", timeout=2) as resp:
                        resp.read(256)
                    break
                except (OSError, URLError):
                    time.sleep(0.05)
            else:
                fail("scan server", "health timeout")
            yield base, opener
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


def grok_session(grok_root: Path, sid: str, title: str) -> Path:
    """A headless-style Grok session directory: summary, one chat line and the events.jsonl `grok -p` keeps open."""
    grok = grok_root / "%2Ftmp%2Fproject" / sid
    grok.mkdir(parents=True)
    (grok / "summary.json").write_text(json.dumps({
        "info": {"id": sid, "cwd": "/tmp/project"}, "generated_title": title,
        "created_at": "2026-09-12T10:05:00Z", "updated_at": "2026-09-12T10:06:00Z"}))
    (grok / "chat_history.jsonl").write_bytes(encoded({"type": "user", "content": "hi", "prompt_index": 1,
                                                       "timestamp": "2026-09-12T10:05:00Z"}))
    (grok / "events.jsonl").write_text("{}\n")
    return grok


def write_roots(home: Path):
    """Seven sessions under a fake home whose paths carry the markers the fd rule needs."""
    claude_root, codex_root, grok_root = home / ".claude/projects", home / ".codex/sessions", home / ".grok/sessions"
    paths = {}
    for sid in (SID_A, SID_B, SID_E, SID_F):
        path = claude_root / "-tmp-project" / f"{sid}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(encoded(claude_row(sid, "user", "u0", None, f"Synthetic {sid[:8]}", cwd="/tmp/project"))
                         + encoded(claude_row(sid, "assistant", "a0", "u0", "ok", cwd="/tmp/project")))
        paths[sid] = path
    codex = codex_root / "2026/09/12" / f"rollout-2026-09-12T10-00-00-{SID_C}.jsonl"
    codex.parent.mkdir(parents=True)
    codex.write_bytes(encoded(codex_row("session_meta", {"id": SID_C, "cwd": "/tmp/project", "timestamp": "2026-09-12T10:00:00Z"}))
                      + encoded(codex_message("user", "Synthetic codex")))
    paths[SID_C] = codex
    paths[SID_D] = grok_session(grok_root, SID_D, "Synthetic grok")
    paths[SID_H] = grok_session(grok_root, SID_H, "Synthetic grok in F's pane")
    return {"claude": claude_root, "codex": codex_root, "grok": grok_root}, paths


def write_tree(proc_root: Path, paths):
    proc = FakeProc(proc_root)
    proc.add(1, "systemd", 0, "/sbin/init")
    proc.add(4856, "systemd", 1, "/usr/lib/systemd/systemd --user")
    # A: resumed claude + its tool shell; the shell is not a second instance.
    proc.add(100, "claude", 4856, f"/home/x/.local/bin/claude --resume {SID_A}", start_ticks=1_000)
    proc.add(101, "bash", 100, "bash /tmp/claude-1000/x/tool.sh",
             {"CLAUDE_CODE_SESSION_ID": SID_A, "CLAUDE_PID": "100"}, start_ticks=1_100)
    # B: the CLI is gone; an orphan helper adopted by systemd --user still carries the id.
    proc.add(200, "bash", 4856, "bash /tmp/claude-1000/y/dispatch.sh", {"CLAUDE_CODE_SESSION_ID": SID_B})
    proc.add(201, "sleep", 200, "sleep 15", {"CLAUDE_CODE_SESSION_ID": SID_B})
    # C: codex keeps the rollout open (fd), no id on the command line.
    proc.add(300, "codex", 4856, "codex resume-something", fds={7: paths[SID_C]}, start_ticks=2_000)
    # D: headless grok -p spawned by A's tool shell, inherits A's id, holds events.jsonl.
    proc.add(400, "grok", 101, "/home/x/.grok/bin/grok -p do the task --cwd /tmp/project",
             {"CLAUDE_CODE_SESSION_ID": SID_A, "CLAUDE_PID": "100"},
             fds={36: paths[SID_D] / "events.jsonl"}, start_ticks=3_000)
    # E: child claude spawned by A (own id on the command line, A's in the environment).
    proc.add(500, "claude", 101, f"claude -p --session-id {SID_E}",
             {"CLAUDE_CODE_SESSION_ID": SID_A, "CLAUDE_PID": "100"}, start_ticks=4_000)
    # F: inside a tmux whose server was first started from A: the server is a boundary.
    proc.add(600, "tmux: server", 1, "tmux -L sessiondock", {"CLAUDE_CODE_SESSION_ID": SID_A, "CLAUDE_PID": "100"})
    proc.add(601, "bash", 600, "bash")
    proc.add(602, "claude", 601, f"claude --session-id {SID_F}", start_ticks=5_000)
    # H: headless grok -p spawned by F's tool shell inside F's pane. It is live and
    # spawned_by F, but F's claude between it and the tmux server is a barrier
    # (Python 16cc89c `is_cli_process`): the console is F's, H is not in tmux_uids.
    proc.add(603, "bash", 602, "bash /tmp/claude-1000/f/tool.sh",
             {"CLAUDE_CODE_SESSION_ID": SID_F, "CLAUDE_PID": "602"}, start_ticks=5_100)
    proc.add(604, "grok", 603, "/home/x/.grok/bin/grok -p summarize --cwd /tmp/project",
             {"CLAUDE_CODE_SESSION_ID": SID_F, "CLAUDE_PID": "602"},
             fds={36: paths[SID_H] / "events.jsonl"}, start_ticks=6_000)
    return proc


def uid_of(source, path):
    import hashlib
    return source + ":" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


def rows_by_uid(opener, base):
    body, raw = call(opener, base, "GET", "/api/sessions?force=1")
    rows = body.get("sessions")
    if not isinstance(rows, list):
        fail("/api/sessions", "sessions not a list", raw)
    return {row["uid"]: row for row in rows}, [row["uid"] for row in rows], raw


def scan_case(binary: Path, root: Path):
    home = root / "home"
    roots, paths = write_roots(home)
    uids = {sid: uid_of("grok" if sid in (SID_D, SID_H) else "codex" if sid == SID_C else "claude", paths[sid])
            for sid in (SID_A, SID_B, SID_C, SID_D, SID_E, SID_F, SID_H)}
    state = root / "scan-state"
    state.mkdir(mode=0o700)
    state.chmod(0o700)
    proc_root = root / "proc"
    write_tree(proc_root, paths)
    with scan_server(binary, roots, state, proc_root) as (base, opener):
        meta, raw = call(opener, base, "GET", "/api/meta")
        if (meta.get("capabilities") or {}).get("live") is not True:
            fail("meta", "capabilities.live should be true with the scan on", raw)
        passed("GET /api/meta capabilities.live true on a supported platform")
        body, raw = call(opener, base, "GET", "/api/live")
        redacted(raw)
        expect_live = {uids[SID_A], uids[SID_C], uids[SID_D], uids[SID_E], uids[SID_F], uids[SID_H]}
        if set(body.get("uids") or []) != expect_live or uids[SID_B] in (body.get("uids") or []):
            fail("scan uids", f"{body.get('uids')} want {sorted(expect_live)}", raw)
        by_uid, order, _ = rows_by_uid(opener, base)
        if body["uids"] != [uid for uid in order if uid in expect_live]:
            fail("scan uids order", f"{body['uids']} vs list order {order}", raw)
        # H runs under F's pane but F's claude is a barrier: only F is managed.
        if body.get("tmux_uids") != [uids[SID_F]]:
            fail("tmux_uids", f"{body.get('tmux_uids')} want [{uids[SID_F]}] (H behind F's CLI barrier)", raw)
        want_started = {uids[SID_A]: BTIME + 10.0, uids[SID_C]: BTIME + 20.0, uids[SID_D]: BTIME + 30.0,
                        uids[SID_E]: BTIME + 40.0, uids[SID_F]: BTIME + 50.0, uids[SID_H]: BTIME + 60.0}
        if body.get("started_at") != want_started:
            fail("started_at", f"{body.get('started_at')} want {want_started}", raw)
        if (body.get("enabled"), body.get("known"), body.get("partial")) != (True, True, False) \
                or "unavailable_reason" in body or body.get("managed") is not None:
            fail("scan envelope", {k: body.get(k) for k in ("enabled", "known", "partial", "unavailable_reason", "managed")}, raw)
        scan = body.get("scan") or {}
        stats, cache = scan.get("stats") or {}, scan.get("cache") or {}
        # 14 entries; 9 command lines mention a CLI (the three helper scripts live under /tmp/claude-1000).
        # The 10 s watch loop ticks at startup, so the three spawners may already be on disk (0) or
        # get written by this call (3); never a second time.
        if scan.get("enabled") is not True or stats.get("processes") != 14 or stats.get("matched") != 9 \
                or cache.get("ttl_ms") != 3000 or scan.get("spawned_recorded") not in (0, 3):
            fail("scan report", scan, raw)
        passed("GET /api/live scan: uids (cmdline/env/fd/orphan/cross-family), tmux_uids (CLI barrier), started_at, envelope")
        hit, raw = call(opener, base, "GET", "/api/live")
        if ((hit.get("scan") or {}).get("cache") or {}).get("hit") is not True:
            fail("scan cache hit", hit.get("scan"), raw)
        forced, raw = call(opener, base, "GET", "/api/live?force=1")
        if ((forced.get("scan") or {}).get("cache") or {}).get("hit") is not False \
                or (forced.get("scan") or {}).get("spawned_recorded") != 0:
            fail("scan force miss", forced.get("scan"), raw)
        passed("GET /api/live scan cache hit, ?force=1 miss, spawners recorded once")
        by_uid, _, raw = rows_by_uid(opener, base)
        want = {uids[SID_D]: {"source": "claude", "sid": SID_A}, uids[SID_E]: {"source": "claude", "sid": SID_A},
                uids[SID_H]: {"source": "claude", "sid": SID_F}}
        got = {uid: row.get("spawned_by") for uid, row in by_uid.items() if "spawned_by" in row}
        if got != want:
            fail("spawned_by rows", f"{got} want {want}", raw)
        passed("/api/sessions rows carry spawned_by {source, sid} for the two grok -p and the child claude only")
        disk = json.loads((state / "session-metadata.json").read_text())
        if {uid: row.get("spawned_by") for uid, row in disk["sessions"].items()} != want:
            fail("spawned_by disk", disk)
        passed("session-metadata.json rows hold spawned_by (write once)")
    # Everything exited: the relation survives, liveness does not.
    empty = root / "proc-empty"
    FakeProc(empty).add(1, "systemd", 0, "/sbin/init")
    with scan_server(binary, roots, state, empty) as (base, opener):
        body, raw = call(opener, base, "GET", "/api/live")
        if body.get("uids") != [] or body.get("started_at") != {} or body.get("tmux_uids") != []:
            fail("scan after exit", body, raw)
        by_uid, _, raw = rows_by_uid(opener, base)
        if by_uid[uids[SID_E]].get("spawned_by") != {"source": "claude", "sid": SID_A}:
            fail("spawned_by after restart", by_uid[uids[SID_E]].get("spawned_by"), raw)
        passed("restart with an empty tree: uids [] but spawned_by persists")
    # Explicit Grok active file: a listed id is live without a pid.
    active = root / "active_sessions.json"
    active.write_text(json.dumps([{"session_id": SID_D, "pid": 99}]))
    with scan_server(binary, roots, state, empty, grok_active=active) as (base, opener):
        body, raw = call(opener, base, "GET", "/api/live")
        if body.get("uids") != [uids[SID_D]] or body.get("started_at") != {} or body.get("tmux_uids") != []:
            fail("grok active", body, raw)
        passed("SESSIONDOCK_GROK_ACTIVE lists a Grok session live without a pid or start time")


FAKE_CLAUDE = (
    "#!/bin/sh\nprintf 'FAKE_CLAUDE_ARGV'\n"
    'for a in "$@"; do printf \' [%s]\' "$a"; done\nprintf \'\\n\'\n'
    f"exec /bin/sh -c '{SHELL}'\n"
)


def continued_case(binary: Path, root: Path, ptyhost: Path):
    """Python 16cc89c `_pane_for_session`: a managed pane resumes ORIGIN; the synthetic tree hangs
    ORIGIN's claude, the daemon child running NEXT's claude (ORIGIN's row says continued_in → NEXT)
    and a `grok -p` (CHILD) under that real pane root. NEXT inherits the pane, CHILD never does."""
    home = root / "continued-home"
    claude_root, codex_root, grok_root = home / ".claude/projects", home / ".codex/sessions", home / ".grok/sessions"
    codex_root.mkdir(parents=True)
    paths = {}
    for sid, tail in ((SID_ORIGIN, [{"type": "continued-in", "sessionId": SID_ORIGIN, "continuedInSessionId": SID_NEXT,
                                     "timestamp": "2026-09-12T10:10:00Z"}]),
                      (SID_NEXT, [])):
        path = claude_root / "-tmp-project" / f"{sid}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(encoded(claude_row(sid, "user", "u0", None, f"Synthetic {sid[:8]}", cwd="/tmp/project"))
                         + encoded(claude_row(sid, "assistant", "a0", "u0", "ok", cwd="/tmp/project"))
                         + b"".join(encoded(row) for row in tail))
        paths[sid] = path
    paths[SID_CHILD] = grok_session(grok_root, SID_CHILD, "Synthetic grok spawned in the pane")
    uids = {SID_ORIGIN: uid_of("claude", paths[SID_ORIGIN]), SID_NEXT: uid_of("claude", paths[SID_NEXT]),
            SID_CHILD: uid_of("grok", paths[SID_CHILD])}
    roots = {"claude": claude_root, "codex": codex_root, "grok": grok_root}
    for name in ("continued-host", "continued-work", "continued-ledger", "continued-state"):
        (root / name).mkdir(mode=0o700)
        (root / name).chmod(0o700)
    fake = root / "continued-work" / "fake-claude"
    fake.write_text(FAKE_CLAUDE)
    fake.chmod(0o700)
    cfg = root / "continued-launcher.json"
    cfg.touch(mode=0o600)
    cfg.write_text(json.dumps({
        "schema": 2, "host_binary": str(ptyhost.resolve()), "host_dir": str(root / "continued-host"),
        "adapters": [],
        "profiles": [{"id": "claude-cli-v1", "source": "claude", "executable": str(fake.resolve()),
                      "args": [], "new_args": ["--session-id", "{session_id}"], "resume_args": ["--resume", "{sid}"],
                      "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}}]}))
    cfg.chmod(0o600)
    init = subprocess.run([str(binary), "--initialize-lifecycle", str(root / "continued-ledger")],
                          cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
    if init.returncode:
        fail("initialize-lifecycle", init.stderr.decode() or init.stdout.decode())
    proc = FakeProc(root / "proc-continued")
    proc.add(1, "systemd", 0, "/sbin/init")
    managed = (root / "continued-host", root / "continued-ledger", cfg)
    with scan_server(binary, roots, root / "continued-state", proc.root, managed=managed) as (base, opener):
        rec, raw = call(opener, base, "POST", "/api/term/create",
                        {"source": "claude", "resume_uid": uids[SID_ORIGIN], "cwd": str(root / "continued-work"),
                         "request_id": "live-http-continued-1"})
        rec = wait_run(opener, base, rec)
        try:
            live, raw = wait_live(opener, base, lambda d: session(d, uids[SID_ORIGIN]).get("state") == "running", timeout=8)
            pane = session(live, uids[SID_ORIGIN]).get("pid")
            if not isinstance(pane, int) or pane <= 1:
                fail("continued pane", f"managed pid {pane!r}", raw)
            if live.get("tmux_uids") != [uids[SID_ORIGIN]]:
                fail("continued before", f"tmux_uids {live.get('tmux_uids')} want [{uids[SID_ORIGIN]}]", raw)
            # The pane root is the real child; everything below it is synthetic.
            proc.add(pane, "sh", 1, "sh -c claude", start_ticks=100)
            proc.add(9001, "claude", pane, f"claude --session-id {SID_ORIGIN}", start_ticks=1_000)
            proc.add(9002, "node", 9001, "node /usr/lib/claude/daemon.js", {"CLAUDE_CODE_SESSION_ID": SID_ORIGIN},
                     start_ticks=2_000)
            proc.add(9003, "claude", 9002, f"claude --resume {SID_NEXT}", start_ticks=3_000)
            proc.add(9004, "grok", 9001, "grok -p do the task", {"CLAUDE_CODE_SESSION_ID": SID_ORIGIN},
                     fds={36: paths[SID_CHILD] / "events.jsonl"}, start_ticks=4_000)
            body, raw = call(opener, base, "GET", "/api/live?force=1")
            redacted(raw)
            got = body.get("uids") or []
            if any(uids[sid] not in got for sid in (SID_ORIGIN, SID_NEXT, SID_CHILD)):
                fail("continued uids", f"{got} want origin/next/child", raw)
            _, order, _ = rows_by_uid(opener, base)
            want_tmux = [uid for uid in order if uid in (uids[SID_ORIGIN], uids[SID_NEXT])]
            if body.get("tmux_uids") != want_tmux:
                fail("continued tmux_uids", f"{body.get('tmux_uids')} want {want_tmux} (next inherits, grok child never)", raw)
            if session(body, uids[SID_ORIGIN]).get("state") != "running" or uids[SID_NEXT] in (body.get("managed") or {}).get("sessions", {}):
                fail("continued managed", "origin must stay the only managed instance", raw)
            passed("GET /api/live continued-in: next inherits the origin's pane (tmux_uids), the spawned grok -p does not")
        finally:
            call(opener, base, "POST", "/api/term/kill",
                 {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--ptyhost", type=Path, default=PTYHOST,
                        help="ptyhost binary for the managed-instance part (skipped when absent)")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-live-http-") as tmp:
        root = Path(tmp)
        if not args.ptyhost.is_file():
            print(f"SKIP live_http_suite managed part: {args.ptyhost} is not built", flush=True)
            scan_case(args.binary, root)
            return
        for name in ("host", "work", "ledger", "claude", "codex", "grok"):
            (root / name).mkdir(mode=0o700)
            (root / name).chmod(0o700)
        empty_proc = root / "proc-managed"
        FakeProc(empty_proc).add(1, "systemd", 0, "/sbin/init")
        corpus = Corpus(root)
        corpus.put("synthetic-live-sid", "codex", [
            codex_row("session_meta", {"id": "synthetic-live-sid", "cwd": str(root / "work")}),
            codex_message("user", "Synthetic live bound session")], [])
        corpus.put("synthetic-unrelated-sid", "codex", [
            codex_row("session_meta", {"id": "synthetic-unrelated-sid", "cwd": str(root / "work")}),
            codex_message("user", "Synthetic unrelated session")], [])
        uid, other = corpus.uid("synthetic-live-sid"), corpus.uid("synthetic-unrelated-sid")
        cfg = root / "launcher.json"
        cfg.touch(mode=0o600)
        cfg.write_text(json.dumps({
            "host_binary": str(args.ptyhost.resolve()),
            "host_dir": str(root / "host"),

            "adapters": [{
                "id": "synthetic-shell-v1", "source": "codex",
                "executable": str(Path("/bin/sh").resolve()),
                "args": ["-c", SHELL],
                "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"},
            }],
        }))
        cfg.chmod(0o600)
        init = subprocess.run(
            [str(args.binary), "--initialize-lifecycle", str(root / "ledger")],
            cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        if init.returncode:
            fail("initialize-lifecycle", init.stderr.decode() or init.stdout.decode())
        with isolated_server(corpus, args.binary, extra_env={
                "SESSIONDOCK_PROC_ROOT": str(empty_proc)}) as (base, opener):
            body, raw = call(opener, base, "GET", "/api/live")
            if (body.get("uids"), body.get("known"), body.get("partial"), body.get("managed")) != ([], True, False, None):
                fail("native-empty", body, raw)
            meta, raw = call(opener, base, "GET", "/api/meta")
            if (meta.get("capabilities") or {}).get("live") is not True:
                fail("meta", "capabilities.live must expose native discovery", raw)
            passed("GET /api/live empty native inventory, capabilities.live true")
        with isolated_server(corpus, args.binary, host_dir=root / "host",
                             lifecycle_dir=root / "ledger", launcher_config=cfg,
                             extra_env={"SESSIONDOCK_PROC_ROOT": str(empty_proc)}) as (base, opener):
            run(opener, base, uid, other, str(root / "work"))
        scan_case(args.binary, root)
        continued_case(args.binary, root, args.ptyhost)


if __name__ == "__main__":
    main()
