#!/usr/bin/env python3
"""HTTP contract for process scan + spawned_by over a synthetic /proc tree.

SESSIONDOCK_PROC_ROOT points at a Linux-shaped tree
(Python live.py e5b023a rules, verified against `live._scan`/`spawn_parents`
over this very fixture): a CLI main process is argv0 claude/codex/grok only —
`node …/cli.js --resume K` is not one, its --resume id still counts, and its
CLAUDE_CODE_SESSION_ID is attributed to the nearest CLI ancestor (here P's
`claude`, pid 100), so K's started_at is pid 100's start; sid from
--session-id/--resume, else family env CLAUDE_CODE_SESSION_ID /
CODEX_COMPANION_SESSION_ID / GROK_SESSION_ID (a main process never takes a
cross-family inherited id); an fd on a *.jsonl under a configured read root
marks it live (Rust's one widening of Python's literal home markers); an
orphan helper (no CLI ancestor) does not. GET /api/live is the Python shape
{uids, tmux_uids, started_at} plus enabled:true; started_at is
btime+starttime/100. spawned_by is written once into session-metadata.json
(and GET /api/sessions) from the CLI process or an ancestor within 16 levels
via another listed session's main process or CLAUDE_CODE_SESSION_ID /
CODEX_THREAD_ID / CODEX_SESSION_ID / GROK_SESSION_ID / CLAUDE_PID. Unset
The default process scan is enabled on Linux.

CLI barrier (Python 16cc89c `live.is_cli_process`): Q is a claude inside a
tmux pane whose tool shell spawned `grok -p` (G2, events.jsonl open). G2 is
live and spawned_by Q, but Q's claude between G2 and the tmux server means the
console is Q's: tmux_uids lists Q only, never G2.
"""
from __future__ import annotations

import sys

import argparse, hashlib, json, os, shutil, socket, subprocess, tempfile, time
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, build_opener

from history_parity import (
    BINARY as DEBUG_BINARY, REPO, Corpus, NoRedirects, claude_row, codex_message,
    codex_row, encoded, get_json)

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
P_SID = "0aaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
K_SID = "0bbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
T_SID = "0ccccccc-cccc-4ccc-8ccc-cccccccccccc"
G_SID = "0ddddddd-dddd-4ddd-8ddd-dddddddddddd"
ORPHAN = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"
Q_SID = "0fffffff-ffff-4fff-8fff-ffffffffffff"   # claude in a tmux pane
G2_SID = "0abababa-baba-4bab-8aba-babababababa"  # grok -p spawned by Q's tool shell
BTIME, HZ = 1_700_000_000, 100
START = {100: 10_000, 200: 20_000, 300: 30_000, 400: 40_000, 500: 50_000,
         600: 60_000, 601: 60_100, 700: 70_000, 701: 70_100, 800: 80_000}
PARENT = {
    K_SID: {"source": "claude", "sid": P_SID},
    T_SID: {"source": "claude", "sid": K_SID},
    G_SID: {"source": "claude", "sid": P_SID},
    G2_SID: {"source": "claude", "sid": Q_SID},
}


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    extra = f"; {text[:240]}" if text else ""
    raise SystemExit(f"FAIL {area}: {why}{extra}")


def passed(area):
    print(f"PASS {area}", flush=True)


@contextmanager
def server_with_env(corpus, extra_env, executable):
    executable = executable.resolve(strict=True)
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    environment.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}",
                        "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web")})
    for source in ("claude", "codex", "grok"):
        environment["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(corpus.root / source)
    environment.update({key: str(value) for key, value in extra_env.items()})
    opener = build_opener(ProxyHandler({}), NoRedirects())
    with tempfile.TemporaryFile(mode="w+b") as log:
        process = subprocess.Popen([str(executable)], cwd=REPO, env=environment, stdout=log, stderr=log)
        try:
            for _ in range(150):
                if process.poll() is not None:
                    raise AssertionError(f"isolated Rust server exited early ({process.returncode})")
                try:
                    get_json(opener, base, "/api/health")
                    break
                except (OSError, URLError):
                    time.sleep(0.05)
            else:
                raise AssertionError("isolated Rust health check timed out")
            yield base, opener
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
                    raise AssertionError("isolated Rust server did not shut down within five seconds")


def fetch(opener, base, route):
    try:
        return get_json(opener, base, route)
    except HTTPError as err:
        fail(route, f"HTTP {err.code}", err.read(4096))
    except (URLError, OSError, TimeoutError, AssertionError, json.JSONDecodeError) as err:
        fail(route, str(err))


def by_sid(opener, base):
    body = fetch(opener, base, "/api/sessions?force=1")
    rows = body.get("sessions")
    if not isinstance(rows, list):
        fail("/api/sessions", "sessions is not a list", json.dumps(body).encode())
    return {row.get("sid"): row for row in rows if isinstance(row, dict)}


def grok_uid(path):
    return "grok:" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


def epoch(pid):
    return BTIME + START[pid] / HZ


def proc_pid(proc, pid, comm, argv, ppid, *, env=(), cwd="/work", fds=None):
    directory = proc / str(pid)
    (directory / "fd").mkdir(parents=True)
    after = ["S", str(ppid), *(["0"] * 17), str(START[pid])]
    (directory / "stat").write_text(f"{pid} ({comm}) {' '.join(after)}\n")
    (directory / "cmdline").write_bytes(b"\0".join(item.encode() for item in argv) + b"\0")
    blob = b"\0".join(f"{key}={value}".encode() for key, value in env)
    (directory / "environ").write_bytes(blob + (b"\0" if blob else b""))
    (directory / "cwd").symlink_to(cwd)
    for fd, target in (fds or {}).items():
        (directory / "fd" / str(fd)).symlink_to(target)


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)

    def turn(sid, ts, cwd, text):
        uid = sid[:8]
        corpus.put(sid, "claude", [
            claude_row(sid, "user", uid + "-u", None, text + " q", cwd=cwd, timestamp=ts),
            claude_row(sid, "assistant", uid + "-a", uid + "-u", text + " a", cwd=cwd, timestamp=ts)], [])

    turn(P_SID, "2026-09-11T10:00:00Z", "/work/p", "P")
    turn(K_SID, "2026-09-11T11:00:00Z", "/work/k", "K")
    turn(Q_SID, "2026-09-11T14:00:00Z", "/work/q", "Q")
    meta = codex_row("session_meta", {
        "id": T_SID, "session_id": T_SID, "cwd": "/work/t", "timestamp": "2026-09-11T12:00:00Z"})
    meta["timestamp"] = "2026-09-11T12:00:00Z"
    corpus.put(T_SID, "codex", [meta, codex_message("user", "T q", 1)], [])
    gdir = root / "grok/%2Fwork%2Fg" / G_SID
    gdir.mkdir(parents=True)
    (gdir / "summary.json").write_text(json.dumps({
        "generated_title": "Grok G", "info": {"id": G_SID, "cwd": "/work/g"},
        "created_at": "2026-09-11T13:00:00Z", "updated_at": "2026-09-11T13:00:00Z"}))
    (gdir / "chat_history.jsonl").write_bytes(encoded({
        "type": "user", "content": "hello G", "prompt_index": 0, "timestamp": "2026-09-11T13:00:00Z"}))
    events = gdir / "events.jsonl"
    events.write_bytes(encoded({"type": "event", "content": "open"}))
    corpus.paths[G_SID] = gdir
    g2dir = root / "grok/%2Fwork%2Fq" / G2_SID
    g2dir.mkdir(parents=True)
    (g2dir / "summary.json").write_text(json.dumps({
        "generated_title": "Grok G2 in Q's pane", "info": {"id": G2_SID, "cwd": "/work/q"},
        "created_at": "2026-09-11T14:30:00Z", "updated_at": "2026-09-11T14:30:00Z"}))
    (g2dir / "chat_history.jsonl").write_bytes(encoded({
        "type": "user", "content": "hello G2", "prompt_index": 0, "timestamp": "2026-09-11T14:30:00Z"}))
    events2 = g2dir / "events.jsonl"
    events2.write_bytes(encoded({"type": "event", "content": "open"}))
    corpus.paths[G2_SID] = g2dir
    proc = root / "proc"
    proc.mkdir()
    (proc / "stat").write_text(f"cpu 0 0 0 0\nbtime {BTIME}\n")
    proc_pid(proc, 100, "claude", ["claude", "--session-id", P_SID], 1,
             cwd="/work/p", fds={3: corpus.paths[P_SID]})
    proc_pid(proc, 200, "node", ["node", "/usr/lib/claude/cli.js", "--resume", K_SID], 100,
             env=(("CLAUDE_CODE_SESSION_ID", K_SID),), cwd="/work/k")
    proc_pid(proc, 300, "codex", ["codex"], 200,
             env=(("CODEX_COMPANION_SESSION_ID", T_SID),), cwd="/work/t")
    proc_pid(proc, 400, "grok", ["grok", "-p", "hello"], 1,
             env=(("CLAUDE_CODE_SESSION_ID", P_SID),), cwd="/work/g", fds={5: events})
    proc_pid(proc, 500, "bash", ["bash"], 1,
             env=(("CLAUDE_CODE_SESSION_ID", ORPHAN),), cwd="/tmp")
    # Q's pane: tmux server → shell → claude Q → tool shell → grok -p G2.
    proc_pid(proc, 600, "tmux: server", ["tmux", "-L", "sessiondock"], 1, cwd="/work/q")
    proc_pid(proc, 601, "bash", ["bash"], 600, cwd="/work/q")
    proc_pid(proc, 700, "claude", ["claude", "--session-id", Q_SID], 601, cwd="/work/q")
    proc_pid(proc, 701, "bash", ["bash", "/tmp/claude-1000/q/tool.sh"], 700,
             env=(("CLAUDE_CODE_SESSION_ID", Q_SID), ("CLAUDE_PID", "700")), cwd="/work/q")
    proc_pid(proc, 800, "grok", ["grok", "-p", "summarize"], 701,
             env=(("CLAUDE_CODE_SESSION_ID", Q_SID), ("CLAUDE_PID", "700")), cwd="/work/q",
             fds={5: events2})
    uids = {P_SID: corpus.uid(P_SID), K_SID: corpus.uid(K_SID),
            T_SID: corpus.uid(T_SID), G_SID: grok_uid(gdir),
            Q_SID: corpus.uid(Q_SID), G2_SID: grok_uid(g2dir)}
    return corpus, uids, proc


def expect_spawned(rows, area):
    for sid, parent in PARENT.items():
        got = (rows.get(sid) or {}).get("spawned_by")
        if got != parent:
            fail(area, f"{sid} spawned_by={got!r} want {parent}")
    if "spawned_by" in (rows.get(P_SID) or {}):
        fail(area, "P has spawned_by")


def run_scan(opener, base, uids, proc, state):
    live = fetch(opener, base, "/api/live")
    got, want = set(live.get("uids") or []), set(uids.values())
    if live.get("enabled") is not True or got != want:
        fail("live", f"enabled={live.get('enabled')!r} uids={sorted(got)} want {sorted(want)}",
             json.dumps(live).encode())
    passed("GET /api/live enabled:true uids={P,K,T,G,Q,G2}")
    # Q's claude is a barrier between G2 and the tmux server: the pane is Q's alone.
    if live.get("tmux_uids") != [uids[Q_SID]]:
        fail("tmux_uids", f"{live.get('tmux_uids')} want [{uids[Q_SID]}] (G2 behind Q's CLI barrier)",
             json.dumps(live).encode())
    passed("GET /api/live tmux_uids={Q}: the grok -p under Q's claude is live but not that pane's session")
    started = live.get("started_at") if isinstance(live.get("started_at"), dict) else {}
    # K: pid 200 is `node …` (not a CLI main), so Python's started_at comes from
    # pid 100, the CLI ancestor its inherited CLAUDE_CODE_SESSION_ID resolves to.
    expect = {uids[P_SID]: epoch(100), uids[K_SID]: epoch(100),
              uids[T_SID]: epoch(300), uids[G_SID]: epoch(400),
              uids[Q_SID]: epoch(700), uids[G2_SID]: epoch(800)}
    for uid, ts in expect.items():
        if uid not in started or float(started[uid]) != ts:
            fail("started_at", f"{uid} {started.get(uid)!r} want {ts}", json.dumps(started).encode())
    passed("GET /api/live started_at btime+starttime/100")
    expect_spawned(by_sid(opener, base), "sessions spawned_by")
    passed("GET /api/sessions K←P T←K G←P G2←Q, P has no spawned_by")
    path = state / "session-metadata.json"
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as err:
        fail("metadata", str(err))
    rows = data.get("sessions") if isinstance(data, dict) else None
    if not isinstance(rows, dict):
        fail("metadata", "sessions is not an object", path.read_bytes()[:240])
    for sid, parent in PARENT.items():
        row = rows.get(uids[sid]) if isinstance(rows.get(uids[sid]), dict) else {}
        if row.get("spawned_by") != parent:
            fail("metadata", f"{sid} {row.get('spawned_by')!r} want {parent}")
    passed("session-metadata.json contains K,T,G spawned_by")
    shutil.rmtree(proc / "200")
    live = fetch(opener, base, "/api/live?force=1")
    got = set(live.get("uids") or [])
    if uids[K_SID] in got or got != want - {uids[K_SID]}:
        fail("live-after", f"uids={sorted(got)} want {sorted(want - {uids[K_SID]})}",
             json.dumps(live).encode())
    expect_spawned(by_sid(opener, base), "write-once spawned_by")
    passed("pid 200 gone: K not live, spawned_by unchanged")


def run_default_scan(opener, base):
    live = fetch(opener, base, "/api/live")
    if live.get("enabled") is not sys.platform.startswith("linux"):
        fail("default-scan", f"enabled={live.get('enabled')!r}", json.dumps(live).encode())
    passed("GET /api/live uses the default platform process scan")
    bad = [sid for sid, row in by_sid(opener, base).items() if row.get("spawned_by")]
    if bad:
        fail("disabled-spawned_by", f"rows {bad} still have spawned_by")
    passed("unrelated default scan does not invent synthetic lineage")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR",
                        help="write the synthetic corpus/proc tree into DIR, print paths, exit 0")
    args = parser.parse_args()
    if args.fixtures_only is not None:
        root = args.fixtures_only.resolve()
        root.mkdir(parents=True, exist_ok=True)
        build(root)
        for path in sorted(p for p in root.rglob("*") if p.is_file() or p.is_symlink()):
            print(path)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-spawned-by-") as tmp:
        root = Path(tmp)
        corpus, uids, proc = build(root)
        state = root / "state"
        state.mkdir(mode=0o700)
        state.chmod(0o700)
        scan = {"SESSIONDOCK_PROC_ROOT": str(proc),
                "SESSIONDOCK_STATE_DIR": str(state)}
        with server_with_env(corpus, scan, args.binary) as (base, opener):
            run_scan(opener, base, uids, proc, state)
        off = root / "state-off"
        off.mkdir(mode=0o700)
        off.chmod(0o700)
        with server_with_env(corpus, {"SESSIONDOCK_STATE_DIR": str(off)}, args.binary) as (base, opener):
            run_default_scan(opener, base)


if __name__ == "__main__":
    main()
