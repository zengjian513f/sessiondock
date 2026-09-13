#!/usr/bin/env python3
"""What survives a Web restart on the same isolated directories.

Stars, lifecycle receipts and a live host persist; history-page grants and
delivery epochs do not. Synthetic fixtures, loopback only.
"""
from __future__ import annotations
import argparse, json, os, socket, stat, subprocess, tempfile, time
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode
from urllib.request import Request
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, isolated_server

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
SHELL = ('stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; '
         'while IFS= read -r c; do case "$c" in quit) exit 0 ;; *) printf "RS_UNKNOWN\\n" ;; esac; done')
N = 700


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


def qstat(rec):
    return f"/api/term/new-status?record_id={rec['record_id']}&instance_id={rec['instance_id']}"


def wait_run(opener, base, rec, want=True, timeout=8):
    deadline, last, raw = time.monotonic() + timeout, {}, b""
    while time.monotonic() < deadline:
        last, raw = call(opener, base, "GET", qstat(rec))
        if bool(last.get("running")) is want:
            return last
        time.sleep(0.05)
    fail("new-status", f"running={last.get('running')!r} want {want}", raw)


@contextmanager
def serve(corpus, binary, delivery, **kw):
    orig = subprocess.Popen
    def popen(*a, **k):
        env = dict(k.get("env") or {})
        env["SESSIONDOCK_DELIVERY_DIR"] = str(delivery)
        k["env"] = env
        return orig(*a, **k)
    subprocess.Popen = popen
    try:
        with isolated_server(corpus, binary, **kw) as pair:
            yield pair
    finally:
        subprocess.Popen = orig


def initialize(binary, flag, directory):
    directory.mkdir(mode=0o700)
    directory.chmod(0o700)
    with socket.socket() as occupied:
        occupied.bind(("127.0.0.1", 0))
        env = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
        env.update(PATH="/usr/bin:/bin", SESSIONDOCK_BIND="127.0.0.1:%d" % occupied.getsockname()[1])
        out = subprocess.run([str(binary), flag, str(directory)], cwd=REPO, env=env,
                             capture_output=True, timeout=20)
    if out.returncode:
        fail(flag, out.stderr.decode("utf-8", "replace") or out.stdout.decode("utf-8", "replace"))


def modes(*roots):
    leftover = []
    for root in roots:
        for path in [root, *root.rglob("*")]:
            info = path.lstat()
            mode, want = stat.S_IMODE(info.st_mode), 0o700 if stat.S_ISDIR(info.st_mode) else 0o600
            if mode != want:
                fail("modes", f"{path.name} {oct(mode)} want {oct(want)}")
            if path.name.startswith((".metadata-tmp-", ".lifecycle-tmp-")):
                leftover.append(path.name)
    if leftover:
        fail("temps", leftover)
    passed("state/lifecycle 0600/0700 no temps")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    host_ok = os.name == "posix" and PTYHOST.is_file()
    if not host_ok:
        print("SKIP host restart: target/debug/ptyhost is not built", flush=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-restart-state-") as tmp:
        root = Path(tmp)
        for name in ("host", "work", "state", "claude", "codex", "grok"):
            (root / name).mkdir(mode=0o700, parents=True, exist_ok=True)
            (root / name).chmod(0o700)
        initialize(args.binary, "--initialize-lifecycle", root / "ledger")
        initialize(args.binary, "--initialize-delivery", root / "delivery")
        corpus, sid = Corpus(root), "claude-restart"
        corpus.put(sid, "claude", [claude_row(sid, "user" if i % 2 == 0 else "assistant", f"e{i}",
                       None if i == 0 else f"e{i-1}", f"m{i:04d}") for i in range(N)], [])
        uid, rec, kw = corpus.uid(sid), None, {"state_dir": root / "state"}
        if host_ok:
            cfg = root / "launcher.json"
            cfg.touch(mode=0o600)
            cfg.write_text(json.dumps({
                "schema": 2, "host_binary": str(PTYHOST.resolve()), "host_dir": str(root / "host"),

                "adapters": [{"id": "synthetic-shell-v1", "source": "codex",
                              "executable": str(Path("/bin/sh").resolve()), "args": ["-c", SHELL],
                              "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}}]}))
            cfg.chmod(0o600)
            kw.update(host_dir=root / "host", lifecycle_dir=root / "ledger", launcher_config=cfg)
        enc, box = quote(uid, safe=":"), "/api/session/outbox?uid=" + quote(uid, safe=":")
        with serve(corpus, args.binary, root / "delivery", **kw) as (base, opener):
            saved, raw = call(opener, base, "POST", "/api/session/star", {"uid": uid, "starred": True})
            if saved.get("ok") is not True or saved.get("starred") is not True:
                fail("star", saved, raw)
            win, raw = call(opener, base, "GET", "/api/messages/" + enc + "?window=1")
            partial = win.get("partial") or {}
            cursor = partial.get("cursor")
            if not (isinstance(cursor, str) and len(cursor) == 32 and (partial.get("omitted") or 0) > 0):
                fail("cursor", partial, raw)
            payload, raw = call(opener, base, "GET", box)
            epoch = (payload.get("outbox_version") or {}).get("epoch")
            if not isinstance(epoch, str) or not epoch:
                fail("epoch", payload, raw)
            if host_ok:
                rec, raw = call(opener, base, "POST", "/api/term/create",
                                {"source": "codex", "cwd": str(root / "work"), "request_id": "restart-shell-1"})
                rec = wait_run(opener, base, rec)
        with serve(corpus, args.binary, root / "delivery", **kw) as (base, opener):
            listed, raw = call(opener, base, "GET", "/api/sessions?force=1")
            row = next((s for s in listed.get("sessions") or [] if s.get("uid") == uid), None)
            if not row or row.get("starred") is not True:
                fail("star persist", row, raw)
            passed("star persists after Web restart")
            gone, raw = call(opener, base, "GET",
                             "/api/messages/" + enc + "/page?" + urlencode({"cursor": cursor}), want=404)
            if gone.get("code") != "session_error":
                fail("page 404", gone.get("code"), raw)
            passed("history page cursor 404 after restart")
            payload, raw = call(opener, base, "GET", box)
            new_epoch = (payload.get("outbox_version") or {}).get("epoch")
            if not isinstance(new_epoch, str) or new_epoch == epoch:
                fail("epoch restart", payload, raw)
            passed("delivery outbox epoch changed after restart")
            if rec is not None:
                try:
                    deadline, last, raw = time.monotonic() + 8, {}, b""
                    while time.monotonic() < deadline:
                        listed, raw = call(opener, base, "GET", "/api/term/list")
                        last = next((p for p in listed.get("pending") or []
                                     if p.get("record_id") == rec["record_id"]), None) or {}
                        if last.get("instance_id") == rec["instance_id"] and last.get("running") is True:
                            break
                        time.sleep(0.05)
                    else:
                        fail("list running", last, raw)
                    st, raw = call(opener, base, "GET", qstat(rec))
                    if st.get("running") is not True:
                        fail("status running", st, raw)
                    passed("lifecycle receipt same ids running after restart")
                    deadline, host, raw = time.monotonic() + 8, {}, b""
                    while time.monotonic() < deadline:
                        live, raw = call(opener, base, "GET", "/api/live?force=1")
                        host = next((h for h in ((live.get("managed") or {}).get("hosts") or [])
                                     if h.get("instance_id") == rec["instance_id"]), None) or {}
                        proc = host.get("process") or {}
                        if (live.get("enabled") is True and host.get("liveness") == "running"
                                and proc.get("status") == "verified" and proc.get("child")):
                            break
                        time.sleep(0.05)
                    else:
                        fail("live identity", host, raw)
                    passed("live instance running with fresh identity")
                    body = {"record_id": rec["record_id"], "instance_id": rec["instance_id"]}
                    call(opener, base, "POST", "/api/term/kill", body)
                    wait_run(opener, base, rec, want=False)
                    passed("kill instance through B")
                finally:
                    try:
                        call(opener, base, "POST", "/api/term/kill",
                             {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})
                    except SystemExit:
                        pass
        modes(root / "state", root / "ledger")


if __name__ == "__main__":
    main()
