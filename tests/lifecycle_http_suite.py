#!/usr/bin/env python3
"""HTTP-only controlled creation: free shell plus one fake Claude CLI profile. No Chromium."""
from __future__ import annotations
import argparse, json, os, subprocess, tempfile, time, uuid
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode
from urllib.request import Request
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, isolated_server

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
SHELL = ('stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; '
         'while IFS= read -r c; do case "$c" in quit) exit 0 ;; *) printf "RS_UNKNOWN\\n" ;; esac; done')
FAKE = "#!/bin/sh\nprintf 'FAKE_CLAUDE_ARGV'; for a in \"$@\"; do printf ' [%s]' \"$a\"; done; printf '\\n'; exec /bin/sh -c '" + SHELL + "'\n"


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


def upload(opener, base, query, payload, want=200):
    route = "/api/session/attachment?" + urlencode(query)
    req = Request(base + route, data=payload, method="POST",
                  headers={"Content-Type": "text/plain"})
    try:
        with opener.open(req, timeout=20) as resp:
            raw, code = resp.read(4096), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    if code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    return json.loads(raw)


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


def receipt_ok(rec, raw, kind):
    leaked = [k for k in ("argv", "env", "token", "port", "sock", "uid", "sid", "executable") if k in rec]
    if leaked or rec.get("launch_kind") != kind or rec.get("native_binding") != "unbound":
        fail("receipt", f"kind={rec.get('launch_kind')!r} leak={leaked}", raw)


def run(opener, base, root, work, claude_cwd):
    created, body = [], {"source": "codex", "cwd": work, "request_id": "free-shell-req-1"}
    try:
        rec, raw = call(opener, base, "POST", "/api/term/create", body)
        receipt_ok(rec, raw, "fixed")
        rec = wait_run(opener, base, rec)
        created.append(rec)
        passed("POST /api/term/create free-shell launch_kind=fixed running")

        attachment_query = {"uid": "tmux:" + rec["name"], "record_id": rec["record_id"],
                            "instance_id": rec["instance_id"], "name": "新会话附件.txt"}
        wrong = upload(opener, base, {**attachment_query, "instance_id": "0" * 32},
                       b"must not be written", want=409)
        if wrong.get("code") != "launch_identity":
            fail("pending attachment wrong instance", wrong)
        uploaded = upload(opener, base, attachment_query, b"pending attachment bytes")
        target = Path(work) / uploaded["relative_path"]
        if target.read_bytes() != b"pending attachment bytes" or uploaded.get("recorded") is not False:
            fail("pending attachment", uploaded)
        compatible = upload(opener, base, {"uid": attachment_query["uid"],
                            "name": "旧页面附件.txt"}, b"legacy pending attachment")
        if (Path(work) / compatible["relative_path"]).read_bytes() != b"legacy pending attachment":
            fail("legacy pending attachment", compatible)
        passed("POST /api/session/attachment pending UID compatibility, receipt identity and wrong-instance refusal")

        listed, raw = call(opener, base, "GET", "/api/term/list")
        pending, backends = listed.get("pending") or [], listed.get("backends") or []
        if not any(row.get("record_id") == rec["record_id"] for row in pending):
            fail("list pending", pending, raw)
        if listed.get("sources") != {"claude": True, "codex": True, "grok": False}:
            fail("list sources", listed.get("sources"), raw)
        if listed.get("resume_sources") != {"claude": True, "codex": False, "grok": False}:
            fail("list resume_sources", listed.get("resume_sources"), raw)
        if (listed.get("backend") != "ptyhost" or len(backends) < 2
                or backends[0].get("name") != "ptyhost" or backends[0].get("current") is not True
                or backends[1].get("name") != "tmux" or backends[1].get("available") is not False):
            fail("list backends", backends, raw)
        passed("GET /api/term/list pending sources resume_sources backends")

        replay, _ = call(opener, base, "POST", "/api/term/create", body)
        if replay.get("record_id") != rec["record_id"]:
            fail("idempotent", f"{replay.get('record_id')} != {rec['record_id']}")
        other, raw = call(opener, base, "POST", "/api/term/create", {**body, "request_id": "free-shell-req-2"})
        receipt_ok(other, raw, "fixed")
        if other.get("record_id") == rec["record_id"]:
            fail("second request_id", "same record_id", raw)
        other = wait_run(opener, base, other)
        created.append(other)
        passed("create idempotent replay and second request_id")

        for payload in (
            {**body, "request_id": "free-shell-home-cwd", "cwd": str(root)},
            {"source": "codex", "cwd": work},
            {**body, "request_id": "free-shell-unk-adp", "adapter_id": "missing-v1"},
        ):
            accepted, raw = call(opener, base, "POST", "/api/term/create", payload)
            receipt_ok(accepted, raw, "fixed")
            created.append(wait_run(opener, base, accepted))
        passed("create accepts cwd, generated request_id and ignored adapter metadata")

        st, _ = call(opener, base, "GET", qstat(rec))
        if st.get("running") is not True:
            fail("status exact", st)
        ident, raw = call(opener, base, "GET",
                          qstat({**rec, "instance_id": "0" * 32 if rec["instance_id"] != "0" * 32 else "f" * 32}),
                          want=409)
        if ident.get("code") != "launch_identity":
            fail("wrong instance", ident.get("code"), raw)
        gone, raw = call(opener, base, "GET",
                         qstat({**rec, "record_id": "a" * 32 if rec["record_id"] != "a" * 32 else "b" * 32}),
                         want=404)
        if gone.get("code") != "launch_missing":
            fail("missing record", gone.get("code"), raw)
        passed("GET /api/term/new-status exact 200 / wrong instance 409 / missing 404")

        killed, raw = call(opener, base, "POST", "/api/term/kill",
                           {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})
        deadline, last = time.monotonic() + 6, killed
        while time.monotonic() < deadline:
            if last.get("running") is False and last.get("state") in ("exited", "cancel_requested", "uncertain"):
                break
            last, raw = call(opener, base, "GET", qstat(rec))
            time.sleep(0.05)
        else:
            fail("kill status", last, raw)
        again, raw = call(opener, base, "POST", "/api/term/kill",
                          {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})
        if again.get("record_id") != rec["record_id"] or again.get("running") is not False:
            fail("kill idempotent", again, raw)
        passed("POST /api/term/kill cancelled/exited and repeat idempotent")

        bind, raw = call(opener, base, "POST", "/api/term/bind",
                         {"record_id": other["record_id"], "instance_id": other["instance_id"],
                          "uid": "codex:ffffffffffffffff"}, want=400)
        if bind.get("code") != "invalid_launch_request":
            fail("bind unconfirmed", bind.get("code"), raw)
        passed("POST /api/term/bind without operator_confirmed 400")

        inside, raw = call(opener, base, "GET", "/api/term/complete-dir?path=" + quote(work + "/", safe="/"))
        if not any(str(item).endswith("claude-area/") for item in inside.get("directories") or []):
            fail("complete-dir inside", inside.get("directories"), raw)
        outside, raw = call(opener, base, "GET", "/api/term/complete-dir?path=" + quote(str(root) + "/", safe="/"))
        if not any(str(item).rstrip("/") == work for item in outside.get("directories") or []):
            fail("complete-dir parent", outside.get("directories"), raw)
        passed("GET /api/term/complete-dir traverses ordinary directories")

        tmux, raw = call(opener, base, "POST", "/api/term/backend", {"backend": "tmux"}, want=400)
        if tmux.get("code") != "backend_unsupported":
            fail("backend tmux", tmux.get("code"), raw)
        passed("POST /api/term/backend tmux 400 backend_unsupported")

        claude, raw = call(opener, base, "POST", "/api/term/create",
                           {"source": "claude", "cwd": claude_cwd, "request_id": "claude-cli-new-1"})
        receipt_ok(claude, raw, "new_assigned")
        sid = claude.get("declared_sid") or ""
        try:
            parsed = uuid.UUID(sid)
        except ValueError:
            fail("declared_sid", sid, raw)
        if parsed.version != 4 or sid != str(parsed):
            fail("declared_sid", sid, raw)
        claude = wait_run(opener, base, claude)
        created.append(claude)
        listed, raw = call(opener, base, "GET", "/api/term/list")
        row = next((item for item in listed.get("pending") or []
                    if item.get("record_id") == claude["record_id"]), None)
        if not row or row.get("declared_sid") != sid or row.get("launch_kind") != "new_assigned":
            fail("list claude", row or listed.get("pending"), raw)
        # PTY argv echo is WS attach / host capture only; /api/term/scroll is a no-op.
        passed("POST /api/term/create claude new_assigned UUID declared_sid (PTY echo skipped: WS-only)")
    finally:
        for rec in created:
            try:
                call(opener, base, "POST", "/api/term/kill",
                     {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})
            except SystemExit:
                pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    if os.name != "posix":
        print("SKIP lifecycle_http_suite: POSIX required", flush=True)
        return
    if not PTYHOST.is_file():
        print("SKIP lifecycle_http_suite: target/debug/ptyhost is not built", flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-lifecycle-http-") as tmp:
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
                             lifecycle_dir=root / "ledger", launcher_config=cfg,
                             file_roots=(root / "work",), file_write_roots=(root / "work",)) as (base, opener):
            run(opener, base, root, str(root / "work"), str(root / "work" / "claude-area"))


if __name__ == "__main__":
    main()
