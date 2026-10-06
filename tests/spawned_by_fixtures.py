"""Shared fixture helpers extracted from the former `spawned_by_suite.py` (removed 2026-10-06
with the non-browser suites); imported by browser suites."""
# run_validation: skip
from __future__ import annotations
import hashlib, json, os, socket, subprocess, tempfile, time
from contextlib import contextmanager
from frontend_paths import frontend_dir
from urllib.error import URLError
from urllib.request import ProxyHandler, build_opener
from history_fixtures import (
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


@contextmanager
def server_with_env(corpus, extra_env, executable):
    executable = executable.resolve(strict=True)
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    environment.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}",
                        "SESSIONDOCK_WEB_DIR": str(frontend_dir())})
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


def grok_uid(path):
    return "grok:" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


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
