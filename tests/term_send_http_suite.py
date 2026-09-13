#!/usr/bin/env python3
"""HTTP-only contract of POST /api/term/send and /api/term/scroll. Isolated free shell, no Chromium."""
from __future__ import annotations
import argparse, json, os, subprocess, tempfile, time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, isolated_server

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
PAGE = "httpTermSend"
# Line-oriented sh: HTTP `data` with a trailing LF is one command (echo/touch).
SHELL = 'stty -echo 2>/dev/null; while IFS= read -r c; do /bin/sh -c "$c"; done'
ZERO, ECHO = "0" * 64, "echo RS_HTTP_OK\n"


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
    allowed = want if isinstance(want, (tuple, list, set)) else (want,)
    if code not in allowed:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        return (json.loads(raw) if raw else {}), raw, code
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)


def qstat(rec):
    return f"/api/term/new-status?record_id={rec['record_id']}&instance_id={rec['instance_id']}"


def wait_run(opener, base, rec, timeout=8):
    deadline, last, raw = time.monotonic() + timeout, {}, b""
    while time.monotonic() < deadline:
        last, raw, _ = call(opener, base, "GET", qstat(rec))
        if last.get("running") is True:
            return last
        time.sleep(0.05)
    fail("new-status", f"running={last.get('running')!r}", raw)


def ident(rec):
    return {"name": rec["name"], "record_id": rec["record_id"],
            "launch_id": rec["launch_id"], "instance_id": rec["instance_id"]}


def send_body(rec, token, extra, build=""):
    body = {**ident(rec), "page": PAGE, "token": token, **extra}
    if build:
        body["_build"] = build
    return body


def check_disabled(opener, base):
    for route, payload in (
        ("/api/term/send", {"name": "x", "page": PAGE, "token": ZERO, "keys": ["enter"]}),
        ("/api/term/scroll", {"name": "x", "up": True}),
    ):
        err, raw, _ = call(opener, base, "POST", route, payload, want=501)
        if err.get("code") != "terminal_disabled":
            fail("501", err.get("code"), raw)
    passed("POST /api/term/send and /scroll 501 terminal_disabled without host dir")


def run(opener, base, work, marker):
    rec, raw, _ = call(opener, base, "POST", "/api/term/create",
                       {"source": "codex", "cwd": work, "request_id": "term-send-http-1"})
    rec = wait_run(opener, base, rec)
    try:
        meta, _, _ = call(opener, base, "GET", "/api/meta")
        build = meta.get("build") or ""
        if not build:
            fail("meta", "missing build", json.dumps(meta).encode())

        err, raw, _ = call(opener, base, "POST", "/api/term/send",
                           send_body(rec, ZERO, {"keys": ["enter"]}), want=403)
        if err.get("code") != "terminal_ownership":
            fail("no lease", err.get("code"), raw)
        passed("send without a lease (well-formed unused token) 403 terminal_ownership")

        claimed, raw, _ = call(opener, base, "POST", "/api/term/claim",
                               {**ident(rec), "page": PAGE})
        token = claimed.get("token") or ""
        if not claimed.get("ok") or len(token) != 64 or any(c not in "0123456789abcdef" for c in token):
            fail("claim", "token/page grant", raw)

        got, raw, _ = call(opener, base, "POST", "/api/term/send",
                           send_body(rec, token, {"data": ECHO}, build))
        expect = {"ok": True, "bytes": len(ECHO.encode()), "acknowledged": True, "processed": "unknown"}
        if got != expect:
            fail("send data", got, raw)
        passed("POST /api/term/send data under launch lease 200 {ok,bytes,acknowledged,processed:unknown}")

        touch = f"touch {marker}\n"
        got, raw, _ = call(opener, base, "POST", "/api/term/send",
                           send_body(rec, token, {"data": touch}, build))
        if got != {"ok": True, "bytes": len(touch.encode()), "acknowledged": True, "processed": "unknown"}:
            fail("send touch", got, raw)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if marker.is_file():
                break
            time.sleep(0.05)
        else:
            fail("host output", f"marker not created (PTY echo is WS-only): {marker}")
        passed("host executed HTTP data (touch marker; PTY echo is WS-only)")

        forged = "a" * 64 if token != "a" * 64 else "b" * 64
        err, raw, _ = call(opener, base, "POST", "/api/term/send",
                           send_body(rec, forged, {"keys": ["enter"]}), want=409)
        if err.get("code") != "terminal_ownership":
            fail("forged", err.get("code"), raw)
        passed("forged token 409 terminal_ownership")

        err, raw, _ = call(opener, base, "POST", "/api/term/send",
                           send_body(rec, token, {"keys": ["not-a-key"]}), want=400)
        if err.get("code") != "invalid_terminal_input":
            fail("unknown key", err.get("code"), raw)
        err, raw, _ = call(opener, base, "POST", "/api/term/send",
                           send_body(rec, token, {"data": "x", "bogus": 1}, build), want=400)
        if err.get("code") != "invalid_terminal_input":
            fail("unknown field", err.get("code"), raw)
        err, raw, _ = call(opener, base, "POST", "/api/term/send",
                           send_body(rec, token, {"data": "a" * (1024 * 1024 + 1)}, build), want=413)
        if err.get("code") != "terminal_input_too_large":
            fail("17 KiB", err.get("code"), raw)
        passed("unknown key 400 / unknown JSON field 400 / 1 MiB+1 data 413")

        time.sleep(1.1)
        saw_rate = False
        for _ in range(17):
            err, raw, code = call(opener, base, "POST", "/api/term/send",
                                  send_body(rec, token, {"keys": ["escape"]}), want=(200, 429))
            if code == 429:
                if err.get("code") != "terminal_input_rate":
                    fail("rate", err.get("code"), raw)
                saw_rate = True
        if not saw_rate:
            fail("rate", "17 requests in one second produced no 429 terminal_input_rate")
        passed("17 send requests within 1s → 429 terminal_input_rate")

        got, raw, _ = call(opener, base, "POST", "/api/term/scroll",
                           {"name": rec["name"], "up": True, "lines": 3})
        if got != {"pos": 0, "scrollback": "browser"}:
            fail("scroll", got, raw)
        err, raw, _ = call(opener, base, "POST", "/api/term/scroll",
                           {"name": "absent-terminal", "up": True}, want=404)
        if err.get("code") != "terminal_missing":
            fail("scroll missing", err.get("code"), raw)
        passed("POST /api/term/scroll 200 {pos:0,scrollback:browser} / unknown name 404")

        call(opener, base, "POST", "/api/term/kill",
             {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})
        deadline, last, raw = time.monotonic() + 6, {}, b""
        while time.monotonic() < deadline:
            last, raw, _ = call(opener, base, "GET", qstat(rec))
            if last.get("running") is False:
                break
            time.sleep(0.05)
        err, raw, code = call(opener, base, "POST", "/api/term/send",
                              send_body(rec, token, {"keys": ["enter"]}), want=(403, 409, 410))
        # kill calls retire_launch first → lease gone → 403 NoLease (docs' 410 is exit while lease held).
        if err.get("code") not in ("terminal_ownership", "terminal_exited"):
            fail("after kill", f"HTTP {code} {err.get('code')}", raw)
        passed(f"send after term/kill HTTP {code} {err.get('code')} (retire→403; docs 410 needs a held lease)")
    finally:
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
        print("SKIP term_send_http_suite: POSIX required", flush=True)
        return
    if not PTYHOST.is_file():
        print("SKIP term_send_http_suite: target/debug/ptyhost is not built", flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-term-send-http-") as tmp:
        root = Path(tmp)
        for name in ("host", "work", "ledger", "claude", "codex", "grok"):
            (root / name).mkdir(mode=0o700)
            (root / name).chmod(0o700)
        corpus = Corpus(root)
        with isolated_server(corpus, args.binary) as (base, opener):
            check_disabled(opener, base)
        cfg = root / "launcher.json"
        cfg.touch(mode=0o600)
        cfg.write_text(json.dumps({
            "host_binary": str(PTYHOST.resolve()), "host_dir": str(root / "host"),
            "cwd_roots": [str(root / "work")],
            "adapters": [{"id": "synthetic-shell-v1", "source": "codex",
                          "executable": str(Path("/bin/sh").resolve()), "args": ["-c", SHELL],
                          "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}}]}))
        cfg.chmod(0o600)
        init = subprocess.run([str(args.binary), "--initialize-lifecycle", str(root / "ledger")],
                              cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        if init.returncode:
            fail("initialize-lifecycle", init.stderr.decode() or init.stdout.decode())
        with isolated_server(corpus, args.binary, host_dir=root / "host",
                             lifecycle_dir=root / "ledger", launcher_config=cfg) as (base, opener):
            run(opener, base, str(root / "work"), root / "work" / "rs_http_ok")


if __name__ == "__main__":
    main()
