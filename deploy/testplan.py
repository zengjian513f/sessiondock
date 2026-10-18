"""Scoped validation planning and execution for deployment and CI."""
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
MODES = ("none", "affected", "full")
DOCS = ["tests/check_docs_links.py", "tests/check_agents_md.py"]
PTYHOST = ["terminal*", "term_*", "lifecycle*", "cutover*", "host*", "native_*", "managed_*", "send_*",
                  "live_*", "session_stop_*", "session_freeze_*", "pending_*", "restart_state_*", "bug_report_*"]
# Backend module aliases; Rust checks are selected separately by package.
MODULE_SUITES = {
    "sessions": ["history_*", "sessions_*", "messages_*", "native_*", "*_parity", "codex_*",
                 "claude_*", "grok_*", "agent_*", "orphan_*", "continued_*", "fork_*",
                 "list_rows_*", "input_history_*", "inventory_*", "symlink_*",
                 "unicode_*", "names_*", "budget_*", "reader_pool_*", "sse_*", "rewind_*"],
    "terminal": ["terminal_*", "term_*", "managed_*", "session_stop_*", "session_freeze_*"],
    "lifecycle": ["lifecycle_*", "send_*", "pending_*", "restart_state_*", "live_*", "session_stop_*", "session_freeze_*"],
    "hub": ["hub_*", "node_auth_*"], "hub_config": ["hub_*"], "bin": ["hub_*"],
    "search": ["search_*"], "media": ["media_*", "native_*"], "files": ["file*"],
    "delivery": ["delivery_*", "send_*"], "bug_report": ["bug_report_*"],
    "audit": ["audit_*"], "metadata": ["metadata_*", "prefs_*"], "trash": ["trash_*"],
    "runtime": ["session_freeze_*", "live_*", "spawned_by_*", "managed_*", "restart_state_*", "lifecycle_*"],
    "bridge": ["claude_prompt_*", "prompt_*", "live_*"], "native_replay": ["native_*"],
    "assets": ["static_assets_*", "meta_*", "prefs_*"],
}
BROAD_MODULES = {"main", "lib", "config", "security", "state", "error"}
SELF_SCRIPTS = {"tests/check_docs_links.py", "tests/check_agents_md.py"}
say = lambda line: print(line, flush=True)   # noqa: E731  (progress must reach a redirected stdout live)


WEB_SUITES = {
    "codex-sdk": ["codex_sdk_*"], "quotas": ["quotas_browser"],
    "session-resources": ["process_links_browser"], "sidebar-resources": ["process_links_browser"],
    "session-research": ["session_research_browser"], "session-info": ["session_relations_browser", "session_resources_browser"],
    "session-card": ["list_rows_*", "sidebar_flat_scroll_browser"],
    "file": ["files_browser"], "files": ["files_browser"],
    "search-controller": ["frontend_search_state_browser"], "regex-worker": ["regex_worker_browser"],
    "term": ["terminal_browser", "terminal_grid_browser"], "term-menu": ["session_menu_refresh_browser", "session_continue_browser"],
    "page-sleep": ["page_sleep_browser"], "nodes": ["hub_browser"],
    "list-sync": ["list_delta_browser"], "groups": ["sidebar_flat_scroll_browser"],
    "popup": ["toolbar_options_browser"], "assets": ["render_assets_browser"],
    "syntax": ["render_assets_browser"], "syntax-worker": ["render_assets_browser"],
    "typography": ["content_scroll_browser", "frontend_framework_browser"], "capabilities": ["legacy_browser"],
    "cli": ["cli_diagnostics_browser"], "pwa-install": ["remaining_ui_lifecycle_browser"],
}
SMOKE = ["legacy_browser", "header_fold_browser", "toolbar_options_browser"]


def module_rule(path: str, names: list[str]) -> list[str]:
    parts = path.split("/")[3:]
    if parts and parts[0] == "api":
        parts = parts[1:]
    module = Path(parts[0]).stem if parts else ""
    if module in BROAD_MODULES or not module:
        return ["legacy_browser", "hub_browser", "node_auth_*"]
    return MODULE_SUITES.get(module, [f"{module}*"])


def tests_rule(path: str, names: list[str], root: Path) -> list[str]:
    if path in SELF_SCRIPTS:
        return [path]
    if Path(path).stem in names:
        stem = Path(path).stem
        return [name for name in names if name == stem or name.startswith(stem + '_')]
    # Follow Python imports backwards, including transitive fixture dependencies.
    imports = {}
    for source in (root / "tests").glob("*.py"):
        text = source.read_text(encoding='utf-8')
        deps = re.findall(r'^\s*from\s+([\w.]+)\s+import\s', text, re.M)
        for line in re.findall(r'^\s*import\s+([^\n#]+)', text, re.M):
            deps.extend(item.strip().split()[0] for item in line.split(',') if item.strip())
        imports[source.stem] = {name.lstrip('.').split('.')[0] for name in deps}
    affected = {Path(path).stem}
    while True:
        expanded = affected | {name for name, deps in imports.items() if deps & affected}
        if expanded == affected:
            break
        affected = expanded
    return sorted(affected & set(names))


def web_rule(path: str, patch: str) -> list[str]:
    stem = Path(path).stem
    if stem.startswith('codex-sdk'):
        return WEB_SUITES['codex-sdk']
    if stem in WEB_SUITES:
        return WEB_SUITES[stem]
    if stem in {'style', 'app', 'index'}:
        # Shared files can change one surface. Use changed lines, not context lines.
        lines = [line[1:] for line in patch.splitlines() if line[:1] in {'+', '-'}
                 and not line.startswith(('+++', '---'))]
        if lines and all(not line.strip() or 'sdk-' in line or 'CodexSdk' in line for line in lines):
            return WEB_SUITES['codex-sdk']
        if lines and all(not line.strip() or 'quota' in line.lower() for line in lines):
            return WEB_SUITES['quotas']
        return SMOKE
    if path.startswith('web/grid/'):
        return ['terminal_grid_browser', 'terminal_reflow_browser']
    if path.startswith(('web/icons/', 'web/fonts/')):
        return SMOKE
    if path.startswith('web/vendor/'):
        return ['render_assets_browser', 'terminal_browser']
    if path.endswith('.webmanifest'):
        return SMOKE
    return []


def rule_for(path: str, names: list[str], root: Path = ROOT, patch: str = '') -> tuple[str, list[str]]:
    if path.endswith('.md') or path in {'.gitignore', '.gitattributes', '.editorconfig', 'LICENSE', 'NOTICE'}:
        return 'docs', DOCS
    if path.startswith('web/'):
        return 'frontend', web_rule(path, patch)
    if path.startswith('tests/'):
        return 'test dependencies', tests_rule(path, names, root)
    if path.startswith(('crates/ptyhost/', 'crates/ptyhost-client/', 'crates/ptyhost-screen/')):
        return 'ptyhost', PTYHOST
    if path.startswith(('crates/resource-agent/', 'crates/process-links/')):
        return 'collector', ['process_links_browser']
    if path.startswith('crates/sessiondock/src/'):
        return 'module', module_rule(path, names)
    if path.startswith('crates/'):
        return 'crate integration', ['legacy_browser', 'hub_browser']
    if path in {'Cargo.toml', 'Cargo.lock'}:
        return 'workspace', ['legacy_browser', 'hub_browser', 'terminal_browser']
    if path.startswith('deploy/') and path.endswith(('.service', '.mount')):
        return 'runtime configuration', ['process_links_browser', 'deploy/check.py']
    if path.startswith(('deploy/', '.github/', 'public/')):
        return 'tooling/config', DOCS + ['deploy/check.py']
    return 'unmapped', []


def resolve(targets: list[str], names: list[str], root: Path = ROOT) -> tuple[set[str], set[str]]:
    """-> (suite names, script paths, full).  A script whose stem is a suite runs as that suite."""
    suites: set[str] = set()
    scripts: set[str] = set()
    for t in targets:
        if "/" not in t:
            suites.update(fnmatch.filter(names, t))
        elif Path(t).stem in names:
            suites.add(Path(t).stem)
        elif (root / t).is_file():
            scripts.add(t)
    return suites, scripts


def plan_for(mode: str, changed: list[str], names: list[str], *, root: Path = ROOT,
             suites: list[str] | None = None, patches: dict[str, str] | None = None) -> dict:
    unknown = set(suites or []) - set(names)
    if unknown:
        raise ValueError('unknown suites: ' + ', '.join(sorted(unknown)))
    plan = {"mode": mode, "changed": list(changed), "suites": [], "scripts": [],
            "full": mode == "full", "rules": {}, "unmapped": [], "packages": []}
    selected, scripts, packages = set(), set(), set()
    for path in changed if mode == 'affected' else []:
        plan['rules'][path], targets = rule_for(path, names, root, (patches or {}).get(path, ''))
        matched, extra = resolve(targets, names, root)
        selected |= matched
        scripts |= extra
        if not matched and not extra:
            plan['unmapped'].append(path)
        if path.startswith('crates/'):
            packages.add(path.split('/')[1])
        elif path in {'Cargo.toml', 'Cargo.lock'}:
            packages.update(['sessiondock', 'ptyhost', 'ptyhost-client'])
    if suites:
        selected = set(suites)
        plan['unmapped'] = []
    plan['suites'], plan['scripts'], plan['packages'] = sorted(selected), sorted(scripts), sorted(packages)
    return plan


# -- git ---------------------------------------------------------------------------------
def git(*args: str, check: bool = True) -> str:
    p = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, timeout=60, check=check)
    return p.stdout.rstrip("\r\n") if p.returncode == 0 else ""


def commit_of(ref: str) -> str | None:
    return git("rev-parse", "--verify", "-q", f"{ref}^{{commit}}", check=False) or None


def resolve_base(explicit: str | None, markers: dict[str, str | None], head: str = "HEAD") -> tuple[str, str]:
    """-> (full sha, how).  markers: target name -> etc/deployed-commit text (or None)."""
    if explicit:
        if not (sha := commit_of(explicit)):
            raise ValueError(f"--test-base {explicit!r} is not a commit in this checkout")
        return sha, f"--test-base {explicit}"
    behind: list[tuple[int, str, str]] = []
    for name, text in markers.items():
        tok = ((text or "").split() or [""])[0]
        if re.fullmatch(r"[0-9a-f]{7,40}", tok) and (sha := commit_of(tok)):
            behind.append((int(git("rev-list", "--count", f"{sha}..{head}")), sha, name))
    if behind:
        n, sha, name = max(behind)
        return sha, f"oldest etc/deployed-commit marker ({name}, {n} commits behind HEAD)"
    if sha := commit_of(f"{head}~1"):
        return sha, "parent of release commit (no deployment marker)"
    # Empty Git tree for an initial commit; no silent origin/main == HEAD diff.
    empty = subprocess.run(['git', 'hash-object', '-t', 'tree', '--stdin'], input='',
                           cwd=ROOT, capture_output=True, text=True, check=True, timeout=30).stdout.strip()
    return empty, 'empty tree (initial commit)'


def changed_files(base: str, included: list[str], head: str = "HEAD") -> list[str]:
    """Files changed since `base` in the stage: committed ones plus the
    uncommitted files the build named with --allow-dirty."""
    files = set(git("diff", "--name-only", base, head).splitlines())
    files.update(included)
    return sorted(f for f in files if f)


def list_suites(binary: str, root: Path = ROOT) -> list[str]:
    """Suite names from `run_validation.py --list` (real suites are excluded there by default)."""
    out = subprocess.run([sys.executable, str(root / "tests/run_validation.py"), "--list", "--binary", binary], cwd=root,
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
        out("selected: explicit full sweep")
    else:
        out(f"selected suites ({len(plan['suites'])}): {', '.join(plan['suites']) or '-'}")
        out(f"extra scripts: {', '.join(plan['scripts']) or '-'}")


def stream(argv: list[str], log, deadline: float, out=say, *, root: Path = ROOT, env=None) -> int | None:
    """Run argv, echo every line and append it to `log`; None when the deadline killed it."""
    out("$ " + " ".join(argv))
    log.write("$ " + " ".join(argv) + "\n")
    proc = subprocess.Popen(argv, cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
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


def run(plan: dict, binary: str, stage: Path, timeout: float, out=say, *, root: Path = ROOT, cargo="cargo", env=None) -> dict:
    """Execute the plan -> {"result": passed|failed|timeout|skipped, "failed": [(name, log)], "log"}."""
    log_path, json_path = stage / "logs" / "tests.log", stage / "logs" / "tests.json"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    res = {"result": "skipped", "failed": [], "log": str(log_path)}
    if plan["mode"] == "none":
        out("tests skipped by --test none")
        return res
    if plan.get('unmapped'):
        out('select --test-suites for unmapped paths: ' + ', '.join(plan['unmapped']))
        return dict(res, result='unmapped', failed=[('unmapped paths', str(log_path))])
    cmds: list[tuple[str, list[str]]] = []
    if plan.get('packages') and not plan['full']:
        flags = [arg for pkg in plan['packages'] for arg in ('-p', pkg)]
        cmds += [('fmt', [cargo, 'fmt', *flags, '--check']),
                 ('clippy', [cargo, 'clippy', *flags, '--all-targets', '--locked', '--', '-D', 'warnings'])]
    if plan["full"] or plan["suites"]:
        argv = [sys.executable, str(root / "tests/run_validation.py"), "--binary", binary, "--log-dir",
                str(stage / "logs" / "validation"), "--json", str(json_path),
                "--web-dir", str((stage / "web").resolve())]
        cmds.append(("run_validation", argv + ([] if plan["full"] else ["--only", ",".join(plan["suites"])])))
    cmds += [(s, [sys.executable, s]) for s in plan["scripts"]]
    if not cmds:
        out("no suite maps to these changes; nothing to test")
        return dict(res, result="not-needed")
    deadline = time.monotonic() + timeout
    with log_path.open("a", encoding="utf-8") as log:
        for label, argv in cmds:
            rc = stream(argv, log, deadline, out, root=root, env=env)
            if rc is None:
                res["failed"].append((f"{label} (killed by --test-timeout {timeout:.0f}s)", str(log_path)))
                return dict(res, result="timeout")
            if rc != 0:
                res["failed"] += failed_suites(json_path, log_path) if label == "run_validation" \
                    else [(label, str(log_path))]
    res["result"] = "failed" if res["failed"] else "passed"
    return res
