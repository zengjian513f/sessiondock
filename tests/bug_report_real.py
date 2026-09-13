#!/usr/bin/env python3
# run_validation: real-cli
"""Real Claude CLI bug-report worker acceptance (batch 41), cheapest configuration.

Runs in the normal validation sweep per AGENTS.md with the cheapest Claude
configuration only (`--model claude-haiku-4-5-20251001 --effort low`, the full
dated ID). Everything is isolated and deleted afterwards: a private
`CLAUDE_CONFIG_DIR` that copies the credential file read-only (the real
~/.claude is never written), a throwaway git repository as the worker's cwd,
private bundle/audit/lifecycle/host directories, one worker session. The
server is started with the bug-report variables, `POST /api/bug-report` starts
the worker, and the suite asserts that the prompt was confirmed from the real
native `user` record of the declared session, that the assistant record names
exactly the cheapest model, then kills the instance and removes the bundle,
the session files and every temporary directory. Proxy variables pass through
like the other real-CLI suites. Skips with a printed reason when `claude` is
absent or cannot authenticate a standalone call.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bug_report_http_suite import BINARY, PTYHOST, REPO, call, server, wait_final  # noqa: E402
from send_claude_real import (MODEL, isolated_config, logged_in, passthrough,  # noqa: E402
                              trust_project)

DESCRIPTION = "真实 CLI 验收：这是一次自动化检查，请只回复 OK，不要读取文件，不要修改任何内容。"


def skip(reason):
    print(f"SKIP bug_report_real: {reason}", flush=True)
    sys.exit(0)


def main():
    claude = shutil.which("claude")
    if not claude:
        skip("`claude` binary is not on PATH")
    claude = os.path.realpath(claude)
    if not PTYHOST.is_file():
        skip("ptyhost is not built (cargo build -p ptyhost)")
    import send_claude_real
    send_claude_real.CLAUDE = claude
    binary = BINARY.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-bug-report-real-") as temporary:
        tmp = Path(temporary).resolve()
        config, copied = isolated_config(tmp)
        if not copied:
            skip("no ~/.claude/.credentials.json to reuse (not logged in)")
        for name in ("host", "ledger", "audit", "reports", "home", "codex", "grok"):
            (tmp / name).mkdir(mode=0o700)
        (tmp / "work").mkdir(mode=0o755)
        repo = tmp / "work/repo"
        repo.mkdir(mode=0o755)
        subprocess.run(["git", "init", "-q", str(repo)], check=False, capture_output=True, timeout=15)
        (repo / "README.md").write_text("throwaway repository for the bug-report worker acceptance\n")
        trust_project(config, repo)
        import uuid
        ok, detail = logged_in(config, repo, str(uuid.uuid4()))
        if not ok:
            skip(f"isolated `claude` login probe failed: {detail}")
        print(f"login probe ok: {detail!r}", flush=True)
        # The real CLI writes sessions under <config>/projects/<cwd>/<sid>.jsonl.
        (tmp / "claude").symlink_to(config / "projects")
        launcher = tmp / "launcher.json"
        launcher.touch(mode=0o600)
        launcher.write_text(json.dumps({
            "schema": 2, "host_binary": str(PTYHOST.resolve()), "host_dir": str(tmp / "host"),
            "adapters": [],
            "profiles": [{
                "id": "claude-real-v1", "source": "claude", "executable": claude,
                "args": ["--model", MODEL, "--effort", "low", "--permission-mode", "plan"],
                "new_args": ["--session-id", "{session_id}"],
                "resume_args": ["--resume", "{sid}"],
                "env": {**passthrough(), "HOME": str(tmp / "work"), "CLAUDE_CONFIG_DIR": str(config)},

            }],
            "bug_report_profiles": {"claude": "claude-real-v1"}}))
        launcher.chmod(0o600)
        init = subprocess.run([str(binary), "--initialize-lifecycle", str(tmp / "ledger")], cwd=REPO,
                              env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")},
                              capture_output=True, timeout=30)
        assert init.returncode == 0, init.stderr.decode("utf-8", "replace")
        extra = {"SESSIONDOCK_LAUNCHER_CONFIG": str(launcher),
                 "SESSIONDOCK_BUG_REPORT_DIR": str(tmp / "reports"),
                 "SESSIONDOCK_BUG_REPORT_REPO": str(repo)}
        worker = None
        report_id = None
        with server(binary, tmp, extra) as (base, opener):
            try:
                meta, raw = call(opener, base, "GET", "/api/meta")
                assert meta["capabilities"]["bug_report"] is True, meta
                reply, raw = call(opener, base, "POST", "/api/bug-report",
                                  {"description": DESCRIPTION, "source": "claude",
                                   "page_id": "real-page", "_build": meta["build"]}, want=202)
                worker = reply["worker"]
                report_id = reply["report_id"]
                path = Path(reply["path"])
                assert worker["source"] == "claude" and worker["sid"], reply
                print(f"report {report_id} created; worker {worker['name']} sid={worker['sid']}", flush=True)
                final = wait_final(path, "real worker", timeout=120)
                print("manifest status:", final["status"], json.dumps(final.get("injection"), ensure_ascii=False),
                      flush=True)
                assert final["status"] == "submitted", json.dumps(final, ensure_ascii=False)
                confirmed = final["confirmed_from"]
                assert confirmed["method"] == "native_user_record", confirmed

                def session_file():
                    return next((config / "projects").glob(f"*/{worker['sid']}.jsonl"), None)
                jsonl = session_file()
                assert jsonl is not None, "confirmed but no session file"
                rows = [json.loads(line) for line in jsonl.read_text(encoding="utf-8").splitlines() if line.strip()]
                users = [r for r in rows if r.get("type") == "user"
                         and isinstance(r.get("message", {}).get("content"), str)]
                assert any(report_id in r["message"]["content"] for r in users), \
                    f"prompt not found in native user records: {[r['message']['content'][:60] for r in users]}"
                print("prompt confirmed from the native user record", flush=True)
                # The model flag reached the CLI: wait for the first assistant record.
                deadline = time.monotonic() + 90
                models = []
                while time.monotonic() < deadline and not models:
                    rows = [json.loads(line) for line in jsonl.read_text(encoding="utf-8").splitlines()
                            if line.strip()]
                    models = [r.get("message", {}).get("model", "") for r in rows if r.get("type") == "assistant"]
                    if not models:
                        time.sleep(1.0)
                assert models, "no assistant record within 90 s"
                assert any(m == MODEL for m in models), f"model assertion failed: expected {MODEL} in {models}"
                print(f"models seen in the real JSONL: {sorted(set(m for m in models if m))}", flush=True)
                # The worker's pending row is decorated as a bug-report launch.
                listing, raw = call(opener, base, "GET", "/api/term/list")
                assert any(row.get("record_id") == worker["record_id"] for row in listing.get("pending") or []), raw
            finally:
                if worker:
                    try:
                        killed, raw = call(opener, base, "POST", "/api/term/kill",
                                           {"record_id": worker["record_id"], "instance_id": worker["instance_id"]},
                                           want=None)
                        print("instance killed:", killed.get("state"), flush=True)
                    except SystemExit as error:
                        print("kill failed:", error, flush=True)
                if report_id:
                    shutil.rmtree(tmp / "reports" / report_id, ignore_errors=True)
                    print(f"report directory removed: {not (tmp / 'reports' / report_id).exists()}", flush=True)
        # Session files live only in the isolated config copy inside the temp dir.
        created = list((config / "projects").glob("*/*.jsonl"))
        for path in created:
            path.unlink()
        print(f"session files removed: {len(created)}", flush=True)
    print(f"PASS bug_report_real: real Claude ({MODEL}, effort low) worker launched through the bug-report "
          f"profile, prompt pasted and confirmed from the native user record, model ID asserted from the JSONL, "
          f"instance killed, bundle and session files deleted, temp dirs removed")


if __name__ == "__main__":
    main()
