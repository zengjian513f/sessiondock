"""Test gate for deploy/deploy.py: map changed files to validation suites, run them.

`--test none` skips; `full` is the default sweep of tests/run_validation.py; `affected` maps
`git diff --name-only <base>..HEAD` through RULES and runs the union.  No suite name is
hard-coded: each entry is a fnmatch pattern resolved against `run_validation.py --list` at run
time, a `tests/*.py` script outside the sweep, or FULL.  Base commit: the oldest
`etc/deployed-commit` of the targets, else `origin/main`, else `HEAD~1`; `--test-base REF` wins.
"""
from __future__ import annotations

import fnmatch
import json
import re
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "tests" / "run_validation.py"
MODES = ("none", "affected", "full")
FULL = "<full>"                       # target sentinel: run the whole default sweep
RUST = ["cargo_*"]                    # run_validation has no per-crate Rust lanes
DOCS = ["tests/check_docs_links.py", "tests/check_agents_md.py"]
PTYHOST = RUST + ["terminal*", "term_*", "lifecycle*", "cutover*", "host*", "native_*", "managed_*", "send_*",
                  "live_*", "session_stop_*", "pending_*", "restart_state_*", "bug_report_*", "grok_raw_send_*"]
# crates/sessiondock/src/<module>/** or src/<module>.rs -> Python suite patterns (RUST is always
# added).  A module absent here gets "<module>*"; if that matches nothing -> FULL.
MODULE_SUITES = {
    "sessions": ["history_*", "sessions_*", "messages_*", "native_*", "*_parity", "codex_*",
                 "claude_*", "grok_*", "agent_*", "orphan_*", "continued_*", "fork_*",
                 "list_rows_*", "input_history_*", "inventory_*", "debug_runs_*", "symlink_*",
                 "unicode_*", "names_*", "budget_*", "reader_pool_*", "sse_*", "rewind_*"],
    "terminal": ["terminal_*", "term_*", "managed_*", "session_stop_*", "grok_raw_send_*"],
    "lifecycle": ["lifecycle_*", "send_*", "outbox*", "pending_*", "restart_state_*", "live_*", "session_stop_*"],
    "hub": ["hub_*", "node_auth_*"], "hub_config": ["hub_*"], "bin": ["hub_*"],
    "search": ["search_*"], "media": ["media_*", "native_*"], "files": ["file*"],
    "delivery": ["delivery_*", "send_*", "outbox*"], "bug_report": ["bug_report_*"],
    "audit": ["audit_*"], "metadata": ["metadata_*", "prefs_*"], "trash": ["trash_*"],
    "runtime": ["live_*", "spawned_by_*", "managed_*", "restart_state_*", "lifecycle_*"],
    "bridge": ["claude_prompt_*", "prompt_*", "live_*"], "native_replay": ["native_*"],
    "assets": ["static_assets_*", "meta_*", "prefs_*"],
}
BROAD_MODULES = {"api", "main", "lib", "config", "security", "state", "error"}
SELF_SCRIPTS = {"tests/check_docs_links.py", "tests/check_agents_md.py", "tests/deploy_dry_run.py"}
say = lambda line: print(line, flush=True)   # noqa: E731  (progress must reach a redirected stdout live)


def matches(patterns: list[str], names: list[str]) -> bool:
    return any(fnmatch.filter(names, p) for p in patterns)


def module_rule(path: str, names: list[str]) -> list[str]:
    part = path.split("/")[3] if path.count("/") >= 3 else ""
    module = part[:-3] if part.endswith(".rs") else part
    if module in BROAD_MODULES or not module:
        return [FULL]
    patterns = MODULE_SUITES.get(module, [f"{module}*"])
    return RUST + patterns if module in MODULE_SUITES or matches(patterns, names) else [FULL]


def tests_rule(path: str, names: list[str]) -> list[str]:
    if path in SELF_SCRIPTS:
        return [path]
    if path.startswith("tests/fixtures/") or not path.endswith((".py", ".mjs")):
        return [FULL]
    stem = Path(path).stem
    for patterns in (["node_contracts"] if path.endswith(".mjs") else [stem, f"{stem}_*"],
                     [f"{stem.split('_')[0]}_*"]):
        if matches(patterns, names):
            return patterns
    return [FULL]


# (path prefix "x/", "*.ext" or exact name, targets or rule function); first match wins;
# no match -> FULL.  Documented in docs/deployment.md.
RULES = [
    (".gitignore", []), (".gitattributes", []), (".editorconfig", []),
    ("docs/", DOCS), ("*.md", DOCS),
    ("crates/ptyhost/", PTYHOST), ("crates/ptyhost-client/", PTYHOST),
    ("crates/sessiondock/src/", module_rule),
    ("crates/sessiondock/tests/fixtures/", [FULL]), ("crates/sessiondock/tests/", RUST),
    ("legacy-web/", ["node_contracts", "*_browser*", "brand_names_check"]),
    ("deploy/", ["deploy_*", "tests/deploy_dry_run.py"]),
    ("tests/", tests_rule),
    ("Cargo.toml", [FULL]), ("Cargo.lock", [FULL]), (".github/", [FULL]),
]


def rule_for(path: str, names: list[str]) -> tuple[str, list[str]]:
    for key, targets in RULES:
        hit = path.startswith(key) if key.endswith("/") else \
            path.endswith(key[1:]) if key.startswith("*.") else path == key
        if hit:
            return key, (targets(path, names) if callable(targets) else list(targets))
    return "<unmatched>", [FULL]


def resolve(targets: list[str], names: list[str]) -> tuple[set[str], set[str], bool]:
    """-> (suite names, script paths, full).  A script whose stem is a suite runs as that suite."""
    suites: set[str] = set()
    scripts: set[str] = set()
    for t in targets:
        if t == FULL:
            return set(), set(), True
        if "/" not in t:
            suites.update(fnmatch.filter(names, t))
        elif Path(t).stem in names:
            suites.add(Path(t).stem)
        elif (ROOT / t).is_file():
            scripts.add(t)
    return suites, scripts, False


def plan_for(mode: str, changed: list[str], names: list[str]) -> dict:
    plan = {"mode": mode, "changed": list(changed), "suites": [], "scripts": [], "full": mode == "full",
            "broad_by": [], "rules": {}}
    suites: set[str] = set()
    scripts: set[str] = set()
    for path in changed if mode == "affected" else []:
        plan["rules"][path], targets = rule_for(path, names)
        s, sc, full = resolve(targets, names)
        plan["broad_by"] += [path] if full else []
        suites |= s
        scripts |= sc
    plan["full"] = plan["full"] or bool(plan["broad_by"])   # scripts outside the sweep still run
    plan["suites"], plan["scripts"] = ([] if plan["full"] else sorted(suites)), sorted(scripts)
    return plan


# -- git ---------------------------------------------------------------------------------
def git(*args: str, check: bool = True) -> str:
    p = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, timeout=60, check=check)
    return p.stdout.rstrip("\r\n") if p.returncode == 0 else ""


def commit_of(ref: str) -> str | None:
    return git("rev-parse", "--verify", "-q", f"{ref}^{{commit}}", check=False) or None


def resolve_base(explicit: str | None, markers: dict[str, str | None]) -> tuple[str, str]:
    """-> (full sha, how).  markers: target name -> etc/deployed-commit text (or None)."""
    if explicit:
        if not (sha := commit_of(explicit)):
            raise ValueError(f"--test-base {explicit!r} is not a commit in this checkout")
        return sha, f"--test-base {explicit}"
    behind: list[tuple[int, str, str]] = []
    for name, text in markers.items():
        tok = ((text or "").split() or [""])[0]
        if re.fullmatch(r"[0-9a-f]{7,40}", tok) and (sha := commit_of(tok)):
            behind.append((int(git("rev-list", "--count", f"{sha}..HEAD")), sha, name))
    if behind:
        n, sha, name = max(behind)
        return sha, f"oldest etc/deployed-commit marker ({name}, {n} commits behind HEAD)"
    if sha := commit_of("origin/main"):
        return sha, "origin/main (no target reported an etc/deployed-commit marker)"
    if sha := commit_of("HEAD~1"):
        return sha, "HEAD~1 (no marker, no origin/main)"
    raise ValueError("no deployed-commit marker, no origin/main and no HEAD~1: pass --test-base REF")


def changed_files(base: str, include_dirty: bool) -> list[str]:
    files = set(git("diff", "--name-only", f"{base}..HEAD").splitlines())
    status = git("status", "--porcelain", "--untracked-files=all") if include_dirty else ""
    files.update(line[3:].split(" -> ")[-1].strip() for line in status.splitlines())
    return sorted(f for f in files if f)


def list_suites(binary: str) -> list[str]:
    """Suite names from `run_validation.py --list` (real suites are excluded there by default)."""
    out = subprocess.run([sys.executable, str(RUNNER), "--list", "--binary", binary], cwd=ROOT,
                         capture_output=True, text=True, timeout=120, check=True).stdout
    return [ln.split()[0] for ln in out.splitlines() if ln.strip() and not ln.endswith(" suites")]


# -- reporting and running -------------------------------------------------------------
def print_plan(plan: dict, base: str | None, how: str, out=say) -> None:
    out(f"test gate: mode={plan['mode']}" + (f"  base {base[:12]} ({how})" if base else ""))
    if plan["mode"] != "affected":
        return
    out(f"changed files ({len(plan['changed'])}):")
    for path in plan["changed"][:20]:
        out(f"  {path}  [{plan['rules'].get(path, '')}]")
    out(f"  ... and {len(plan['changed']) - 20} more") if len(plan["changed"]) > 20 else None
    if plan["full"]:
        out(f"selected: full sweep (broad rule hit by: {', '.join(plan['broad_by'][:8])})")
    else:
        out(f"selected suites ({len(plan['suites'])}): {', '.join(plan['suites']) or '-'}")
        out(f"extra scripts: {', '.join(plan['scripts']) or '-'}")


def stream(argv: list[str], log, deadline: float, out=say) -> int | None:
    """Run argv, echo every line and append it to `log`; None when the deadline killed it."""
    out("$ " + " ".join(argv))
    log.write("$ " + " ".join(argv) + "\n")
    proc = subprocess.Popen(argv, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, bufsize=1)
    fired: list[bool] = []
    timer = threading.Timer(max(0.0, deadline - time.monotonic()), lambda: (fired.append(True), proc.kill()))
    timer.start()
    try:
        for line in proc.stdout:
            out(line.rstrip("\n"))
            log.write(line)
            log.flush()
        return None if fired else proc.wait()
    finally:
        timer.cancel()


def failed_suites(json_path: Path, log_path: Path) -> list[tuple[str, str]]:
    try:
        rows = json.loads(json_path.read_text(encoding="utf-8")).get("results") or []
    except (OSError, ValueError):
        rows = []
    bad = [(r["name"], r.get("log") or str(log_path)) for r in rows if r.get("status") in {"FAIL", "TIMEOUT"}]
    return bad or [("run_validation (non-zero exit, see log)", str(log_path))]


def run(plan: dict, binary: str, stage: Path, timeout: float, out=say) -> dict:
    """Execute the plan -> {"result": passed|failed|timeout|skipped, "failed": [(name, log)], "log"}."""
    log_path, json_path = stage / "logs" / "tests.log", stage / "logs" / "tests.json"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    res = {"result": "skipped", "failed": [], "log": str(log_path)}
    if plan["mode"] == "none":
        out("tests skipped by --test none")
        return res
    cmds: list[tuple[str, list[str]]] = []
    if plan["full"] or plan["suites"]:
        argv = [sys.executable, str(RUNNER), "--binary", binary, "--log-dir",
                str(stage / "logs" / "validation"), "--json", str(json_path)]
        cmds.append(("run_validation", argv + ([] if plan["full"] else ["--only", ",".join(plan["suites"])])))
    cmds += [(s, [sys.executable, s]) for s in plan["scripts"]]
    if not cmds:
        out("no suite maps to these changes; nothing to test")
        return dict(res, result="passed")
    deadline = time.monotonic() + timeout
    with log_path.open("a", encoding="utf-8") as log:
        for label, argv in cmds:
            rc = stream(argv, log, deadline, out)
            if rc is None:
                res["failed"].append((f"{label} (killed by --test-timeout {timeout:.0f}s)", str(log_path)))
                return dict(res, result="timeout")
            if rc != 0:
                res["failed"] += failed_suites(json_path, log_path) if label == "run_validation" \
                    else [(label, str(log_path))]
    res["result"] = "failed" if res["failed"] else "passed"
    return res
