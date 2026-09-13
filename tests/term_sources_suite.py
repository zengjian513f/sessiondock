#!/usr/bin/env python3
"""Bug-report worker profiles must not hide a source's interactive CLI.

GET /api/term/list still publishes sources/resume_sources for claude, codex and
grok when each source has both an interactive profile and a cheapest-model
worker named in bug_report_profiles. POST /api/term/create selects the
interactive CLI by source, matching Python; legacy adapter_id input is ignored.
"""
from __future__ import annotations
import argparse, json, os, subprocess, tempfile, time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, isolated_server

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
SHELL = ('stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; '
         'while IFS= read -r c; do case "$c" in quit) exit 0 ;; *) printf "RS_UNKNOWN\\n" ;; esac; done')
FAKE = "#!/bin/sh\nprintf 'FAKE_CLAUDE_ARGV'; for a in \"$@\"; do printf ' [%s]' \"$a\"; done; printf '\\n'; exec /bin/sh -c '" + SHELL + "'\n"
CHECKS = 0


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    global CHECKS
    CHECKS += 1
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
    if want is not None and code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        return (json.loads(raw) if raw else {}), raw, code
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)


def qstat(rec):
    return f"/api/term/new-status?record_id={rec['record_id']}&instance_id={rec['instance_id']}"


def wait_run(opener, base, rec, want=True, timeout=8):
    deadline, last, raw = time.monotonic() + timeout, {}, b""
    while time.monotonic() < deadline:
        last, raw, _ = call(opener, base, "GET", qstat(rec))
        if bool(last.get("running")) is want:
            return last
        time.sleep(0.05)
    fail("new-status", f"running={last.get('running')!r} want {want}", raw)


def kill(opener, base, rec):
    call(opener, base, "POST", "/api/term/kill",
         {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})


def profile(pid, source, exe, work, home, extra):
    row = {
        "id": pid, "source": source, "executable": str(exe.resolve()),
        "resume_args": extra["resume"],
        "env": {"PATH": "/usr/bin:/bin", "HOME": home},

    }
    if extra.get("args"):
        row["args"] = extra["args"]
    if extra.get("new"):
        row["new_args"] = extra["new"]
    return row


def run(opener, base, work):
    listed, raw, _ = call(opener, base, "GET", "/api/term/list")
    want = {"claude": True, "codex": True, "grok": True}
    if listed.get("sources") != want:
        fail("list sources", listed.get("sources"), raw)
    if listed.get("resume_sources") != want:
        fail("list resume_sources", listed.get("resume_sources"), raw)
    passed("GET /api/term/list sources resume_sources all true")

    rec, raw, _ = call(opener, base, "POST", "/api/term/create",
                       {"source": "codex", "cwd": work, "request_id": "term-sources-1"})
    if rec.get("launch_kind") != "new_pending":
        fail("create no adapter_id", rec.get("launch_kind"), raw)
    rec = wait_run(opener, base, rec)
    kill(opener, base, rec)
    passed("POST /api/term/create no adapter_id launch_kind=new_pending")

    rec, raw, _ = call(opener, base, "POST", "/api/term/create",
                       {"source": "codex", "cwd": work, "request_id": "term-sources-worker",
                        "adapter_id": "codex-bug-report-v1"})
    if rec.get("launch_kind") != "new_pending":
        fail("ignored worker adapter", rec, raw)
    rec = wait_run(opener, base, rec)
    kill(opener, base, rec)
    passed("POST /api/term/create ignores worker adapter_id and selects interactive source")

    rec, raw, _ = call(opener, base, "POST", "/api/term/create",
                       {"source": "codex", "cwd": work, "request_id": "term-sources-missing",
                        "adapter_id": "missing-v1"})
    if rec.get("launch_kind") != "new_pending":
        fail("ignored missing adapter", rec, raw)
    rec = wait_run(opener, base, rec)
    kill(opener, base, rec)
    passed("POST /api/term/create ignores unknown adapter_id")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    if os.name != "posix":
        print("SKIP term_sources_suite: POSIX required", flush=True)
        return
    if not PTYHOST.is_file():
        print("SKIP term_sources_suite: target/debug/ptyhost is not built", flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-term-sources-") as tmp:
        root = Path(tmp)
        for name in ("host", "work", "work/claude-area", "ledger", "bin", "home",
                     "claude", "codex", "grok"):
            (root / name).mkdir(mode=0o700, parents=True, exist_ok=True)
            (root / name).chmod(0o700)
        corpus = Corpus(root)
        fakes = {}
        for src in ("claude", "codex", "grok"):
            fake = root / "bin" / f"fake-{src}"
            fake.write_text(FAKE)
            fake.chmod(0o700)
            fakes[src] = fake
        work, home = str(root / "work"), str(root / "home")
        extras = {
            "claude": {"resume": ["--resume", "{sid}"], "new": ["--session-id", "{session_id}"]},
            "codex": {"resume": ["resume", "{sid}"]},
            "grok": {"resume": ["--resume", "{sid}"]},
        }
        workers = {
            "claude": ["--model", "claude-haiku-4-5-20251001", "--effort", "low"],
            "codex": ["-m", "gpt-5.6-luna", "-c", 'model_reasoning_effort="low"'],
            "grok": ["-m", "grok-4.6", "--reasoning-effort", "low"],
        }
        profiles = []
        for src in ("claude", "codex", "grok"):
            profiles.append(profile(f"{src}-cli-v1", src, fakes[src], work, home, extras[src]))
            wextra = {**extras[src], "args": workers[src]}
            profiles.append(profile(f"{src}-bug-report-v1", src, fakes[src], work, home, wextra))
        cfg = root / "launcher.json"
        cfg.touch(mode=0o600)
        cfg.write_text(json.dumps({
            "schema": 2, "host_binary": str(PTYHOST.resolve()), "host_dir": str(root / "host"),
            "adapters": [], "profiles": profiles,
            "bug_report_profiles": {
                "claude": "claude-bug-report-v1",
                "codex": "codex-bug-report-v1",
                "grok": "grok-bug-report-v1",
            },
        }))
        cfg.chmod(0o600)
        init = subprocess.run([str(args.binary), "--initialize-lifecycle", str(root / "ledger")],
                              cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        if init.returncode:
            fail("initialize-lifecycle", init.stderr.decode() or init.stdout.decode())
        with isolated_server(corpus, args.binary, host_dir=root / "host",
                             lifecycle_dir=root / "ledger", launcher_config=cfg) as (base, opener):
            run(opener, base, work)
    print(f"term_sources_suite: {CHECKS} checks passed")


if __name__ == "__main__":
    main()
