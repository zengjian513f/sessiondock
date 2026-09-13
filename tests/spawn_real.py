#!/usr/bin/env python3
# run_validation: skip  (real-cli operator run: whether haiku actually executes the grok command is up to the model; run by hand per batch)
"""Real-CLI acceptance: Claude -p spawns grok -p; the Rust service lists that
Grok session on the real ~/.grok/sessions root and reports spawned_by when
the Linux /proc scan still saw grok.

Rules: cheapest models only (Claude `claude-haiku-4-5-20251001` `--effort low`,
Grok `grok-4.6` `--reasoning-effort low`); isolated `CLAUDE_CONFIG_DIR` reuses
credentials read-only; proxy passthrough; Claude JSONL assistant records carry
the exact model id; release binary read-only with `SESSIONDOCK_PROC_SCAN=1` and
a temp state dir; GET `/api/sessions?force=1` must list the new Grok row;
`spawned_by` is `{"source":"claude","sid":U}` when recorded, else presence only
(grok -p may have finished); delete only the created `U.jsonl`, the new Grok
session dir, and temp dirs. SKIP when `claude`/`grok` is absent or login fails.
`--dry-run` prints the exact commands and exits 0 without touching homes.
"""
from __future__ import annotations

import argparse, hashlib, json, os, shlex, shutil, socket, subprocess, tempfile, time, uuid
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, build_opener

from history_parity import BINARY as DEBUG_BINARY, NoRedirects, REPO

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
MODEL = "claude-haiku-4-5-20251001"
GROK_BIN, GROK_ROOT = Path.home() / ".local/bin/grok", Path.home() / ".grok/sessions"
CLAUDE_PROJECTS = Path.home() / ".claude/projects"
PROMPT = ("Run exactly this shell command and reply DONE: "
          "~/.local/bin/grok -p 'Reply OK' -m grok-4.6 --output-format plain "
          "--disable-web-search --max-turns 1 --reasoning-effort low")
PROXY_KEYS = ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
              "http_proxy", "https_proxy", "all_proxy", "no_proxy")
MARKERS = ("events.jsonl", "summary.json", "chat_history.jsonl")


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def skip(reason):
    print(f"SKIP spawn_real: {reason}", flush=True)
    raise SystemExit(0)


def passthrough():
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
           "LANG": os.environ.get("LANG", "C.UTF-8"), "HOME": str(Path.home())}
    env.update({key: os.environ[key] for key in PROXY_KEYS if key in os.environ})
    return env


def isolated_config(tmp):
    """Private CLAUDE_CONFIG_DIR; copies the login read-only. Never writes ~/.claude."""
    real = Path(os.environ.get("CLAUDE_CONFIG_DIR", Path.home() / ".claude"))
    config = tmp / "claude-config"
    config.mkdir(mode=0o700)
    copied = False
    if (real / ".credentials.json").is_file():
        shutil.copy2(real / ".credentials.json", config / ".credentials.json")
        copied = True
    top = Path.home() / ".claude.json"
    if top.is_file():
        shutil.copy2(top, config / ".claude.json")
    (config / "projects").mkdir(mode=0o700)
    return config, copied


def trust_project(config, area):
    top = config / ".claude.json"
    try:
        document = json.loads(top.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        document = {}
    document["hasCompletedOnboarding"] = True
    projects = document.setdefault("projects", {})
    projects[str(area)] = {**projects.get(str(area), {}), "hasTrustDialogAccepted": True,
                           "allowedTools": [], "hasClaudeMdExternalIncludesApproved": False,
                           "hasClaudeMdExternalIncludesWarningShown": False}
    top.write_text(json.dumps(document, ensure_ascii=False, indent=2)); top.chmod(0o600)


def claude_argv(claude, sid):
    return [claude, "-p", PROMPT, "--model", MODEL, "--effort", "low",
            "--session-id", sid, "--allowedTools", "Bash"]


def session_dirs(root):
    found = set()
    if not root.is_dir():
        return found
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        if set(filenames) & set(MARKERS):
            found.add(Path(dirpath).resolve()); dirnames.clear()
    return found


def find_jsonl(sid, *roots):
    for root in roots:
        if root is not None and root.is_dir():
            hits = sorted(root.glob(f"**/{sid}.jsonl"))
            if hits:
                return hits[0]
    return None


@contextmanager
def server_with_env(executable, extra_env):
    executable = executable.resolve(strict=True)
    environment = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    environment.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}",
                        "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"), **extra_env})
    opener = build_opener(ProxyHandler({}), NoRedirects())
    with tempfile.TemporaryFile(mode="w+b") as log:
        process = subprocess.Popen([str(executable)], cwd=REPO, env=environment,
                                   stdout=log, stderr=log)
        try:
            for _ in range(150):
                if process.poll() is not None:
                    fail("server", f"exited early ({process.returncode})")
                try:
                    with opener.open(base + "/api/health", timeout=1) as resp:
                        resp.read(256); break
                except (OSError, URLError):
                    time.sleep(0.05)
            else:
                fail("server", "health check timed out")
            yield base, opener
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill(); process.wait(timeout=5)


def pull(opener, base, route, timeout=30):
    try:
        with opener.open(base + route, timeout=timeout) as resp:
            raw, code = resp.read(8 * 1024 * 1024 + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    if code != 200:
        fail(route, f"HTTP {code}", raw)
    if len(raw) > 8 * 1024 * 1024:
        fail(route, "oversized response")
    try:
        return json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)


def print_dry_run(binary):
    cmd = claude_argv(shutil.which("claude") or "claude", "<minted-uuid>")
    print(shlex.join(str(p) for p in cmd))
    print("cwd=<temp-directory>")
    print("CLAUDE_CONFIG_DIR=<temp>/claude-config  # credentials copied read-only")
    print(f"SESSIONDOCK_PROC_SCAN=1 SESSIONDOCK_CLAUDE_ROOT=<isolated-or-real>/projects "
          f"SESSIONDOCK_GROK_ROOT={GROK_ROOT} SESSIONDOCK_CODEX_ROOT=<temp>/codex "
          f"SESSIONDOCK_STATE_DIR=<temp>/state SESSIONDOCK_BIND=127.0.0.1:<ephemeral> "
          f"SESSIONDOCK_WEB_DIR={REPO / 'legacy-web'} {binary}")
    print("GET /api/live")
    print("GET /api/sessions?force=1")
    print(f"rm {CLAUDE_PROJECTS}/<encoded-cwd>/<minted-uuid>.jsonl")
    print(f"rm -r {GROK_ROOT}/<new-session-dir>")
    passed("dry-run")


def remove_created(jsonl, grok_dir, sid, before):
    if jsonl is not None and jsonl.is_file() and jsonl.name == f"{sid}.jsonl":
        jsonl.unlink(); print(f"deleted {jsonl}", flush=True)
    if grok_dir is None:
        return
    root, path = GROK_ROOT.resolve(), grok_dir.resolve()
    if path == root or not path.is_relative_to(root) or path in before:
        fail("cleanup", f"refusing to delete {path}")
    shutil.rmtree(path); print(f"deleted {path}", flush=True)


def run(binary):
    if not Path(binary).is_file():
        fail("binary", f"not found: {binary}")
    passed("binary")
    claude = shutil.which("claude")
    if not claude:
        skip("`claude` binary is not on PATH")
    claude = os.path.realpath(claude); passed("claude")
    if not GROK_BIN.is_file():
        skip("`grok` binary is not at ~/.local/bin/grok")
    passed("grok")
    sid, jsonl, grok_dir, before = str(uuid.uuid4()), None, None, session_dirs(GROK_ROOT)
    with tempfile.TemporaryDirectory(prefix="sessiondock-spawn-real-") as temporary:
        tmp = Path(temporary).resolve()
        try:
            config, copied = isolated_config(tmp)
            if not copied:
                skip("no ~/.claude/.credentials.json to reuse (not logged in)")
            work = tmp / "work"; work.mkdir(mode=0o700); trust_project(config, work)
            env = {**passthrough(), "CLAUDE_CONFIG_DIR": str(config)}
            try:
                done = subprocess.run(claude_argv(claude, sid), cwd=work, env=env,
                                      stdin=subprocess.DEVNULL, capture_output=True, timeout=300)
            except (OSError, subprocess.TimeoutExpired) as error:
                fail("spawn", str(error))
            detail = (done.stderr.decode("utf-8", "replace").strip()
                      or done.stdout.decode("utf-8", "replace").strip() or "nonzero exit")
            if done.returncode != 0:
                low = detail.lower()
                if "403" in detail or "not allowed" in low or "not logged in" in low:
                    skip(f"isolated `claude` login failed: {detail[:200]}")
                fail("spawn", detail[:400])
            passed("spawn")
            jsonl = find_jsonl(sid, config / "projects", CLAUDE_PROJECTS)
            if jsonl is None:
                fail("jsonl", f"{sid}.jsonl not under isolated projects or {CLAUDE_PROJECTS}")
            print(f"claude jsonl={jsonl}", flush=True); passed("jsonl")
            created = sorted(session_dirs(GROK_ROOT) - before, key=lambda p: p.stat().st_mtime,
                             reverse=True)
            if not created:
                fail("grok-dir", f"no new session directory under {GROK_ROOT}")
            grok_dir = next((p for p in created if (p / "events.jsonl").exists()), created[0])
            print(f"grok session={grok_dir} new={len(created)}", flush=True); passed("grok-dir")
            rows = [json.loads(line) for line in jsonl.read_text().splitlines() if line.strip()]
            models = [r.get("message", {}).get("model", "") for r in rows if r.get("type") == "assistant"]
            if not any(m == MODEL for m in models):
                fail("model", f"expected {MODEL} in {models}")
            passed("model")
            state, codex = tmp / "state", tmp / "codex"
            state.mkdir(mode=0o700); codex.mkdir(mode=0o700)
            extra = {"SESSIONDOCK_PROC_SCAN": "1", "SESSIONDOCK_CLAUDE_ROOT": str(jsonl.parent.parent),
                     "SESSIONDOCK_GROK_ROOT": str(GROK_ROOT), "SESSIONDOCK_CODEX_ROOT": str(codex),
                     "SESSIONDOCK_STATE_DIR": str(state)}
            with server_with_env(Path(binary), extra) as (base, opener):
                try:
                    with opener.open(base + "/api/live", timeout=10) as resp:
                        resp.read(65536)
                except (OSError, URLError, HTTPError):
                    pass
                listed = pull(opener, base, "/api/sessions?force=1")
            uid = "grok:" + hashlib.sha1(str(grok_dir.resolve()).encode()).hexdigest()[:16]
            sessions = listed.get("sessions") or []
            row = next((r for r in sessions if r.get("uid") == uid), None)
            if row is None:
                row = next((r for r in sessions if r.get("source") == "grok"
                            and r.get("sid") == grok_dir.name), None)
            if row is None:
                fail("grok-row", f"Grok uid {uid} not in {len(sessions)} rows")
            parent = row.get("spawned_by")
            print(f"observed grok uid={row.get('uid')} sid={row.get('sid')!r} "
                  f"spawned_by={parent!r} claude_sid={sid}", flush=True)
            passed("grok-row")
            if parent:
                if parent.get("source") != "claude" or parent.get("sid") != sid:
                    fail("spawned_by", f"want claude/{sid}, got {parent}")
                passed("spawned_by")
            else:
                print("PASS spawned_by: not recorded (grok may have finished before the scan)",
                      flush=True)
        finally:
            remove_created(jsonl, grok_dir, sid, before)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--dry-run", action="store_true",
                        help="print the exact commands and exit 0 without starting anything")
    args = parser.parse_args()
    (print_dry_run if args.dry_run else run)(args.binary)


if __name__ == "__main__":
    main()
