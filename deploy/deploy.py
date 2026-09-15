#!/usr/bin/env python3
"""SessionDock fleet deployment: build once on this machine, test, push to every target, roll back.

    deploy.py build    [--allow-dirty] [--web-from-head] [--with-ptyhost] [--web-only]
                       [--test none|affected|full] [--test-base REF] [--test-timeout S]
    deploy.py push     [--targets a,b | --all] [--stage DIR] [--web-only | --bin-only]
                       [--with-ptyhost] [--dry-run] [--parallel N] [--keep-backups N]
                       [--health-timeout S] [--targets-file PATH] [-v]
    deploy.py deploy   (build, then test, then push, with the same flags; --test defaults
                        to `affected` here and to `none` for a bare build)
    deploy.py rollback --targets X [--backup DIR] [--targets-file PATH]

Targets come from deploy/targets.local.json (gitignored, the real fleet);
deploy/targets.example.json shows the shape with placeholders. Per-kind steps live
in deploy/sdtargets/<kind>.py; the shared contract and the invariants every handler
keeps are documented in deploy/sdtargets/base.py. Fleet status is a separate
read-only tool (deploy/fleet_status.py).

Test gate (deploy/testplan.py): `--test none` ships untested, `affected` maps the files
changed since the targets' oldest `etc/deployed-commit` (or --test-base) to validation
suites, `full` runs the whole sweep. A failing suite stops before anything is uploaded
(exit 1, failing suite names and log paths printed). The stage records the mode and
result in artifacts.json; `push` prints them and never tests on this machine, while
macOS/Windows nodes that build natively run the Rust tests in their own stage() when
the recorded mode is not `none`. Tests are per platform, once each, never per node.

Per target: probe -> plan -> stage -> backup -> swap -> restart -> verify ->
write_marker -> prune_backups. Any failure after a successful backup rolls that
target back (ROLLED_BACK); an unreachable target is SKIPPED; a kind without a
working handler is UNSUPPORTED; a verified deploy whose marker/prune step failed is
WARN. The exit code is 1 unless every target is OK (PLANNED for --dry-run).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from sdtargets import Artifacts, DeployOptions, ProbeResult, Target, handler_for, load_targets  # noqa: E402
from sdtargets.base import ShellError  # noqa: E402
import testplan  # noqa: E402
from deployment_lock import DeploymentLock, repository_lock  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
DEPLOY_DIR = ROOT / "deploy"
STAGE_ROOT = ROOT / "target" / "deploy"
DIRTY_SCOPE = ["crates", "legacy-web", "Cargo.toml", "Cargo.lock"]
WEB_EXCLUDES = ["node_modules", ".DS_Store", "*.swp"]
GOOD = {"OK", "PLANNED"}
COLUMNS = ("target", "kind", "result", "build", "sha256", "ptyhost", "s", "backup")
PRINT_LOCK = threading.Lock()


def utc_now() -> datetime:
    return datetime.now(timezone.utc)


def stamp_of(dt: datetime) -> str:
    return dt.strftime("%Y%m%d-%H%M%S")


def die(msg: str, code: int = 2) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(code)


def git(*args: str, timeout: float = 60) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True,
                          timeout=timeout, check=True).stdout.strip()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def source_tree(worktree: bool) -> str:
    """Snapshot tracked working files without changing the shared Git index."""
    if not worktree:
        return git("rev-parse", "HEAD^{tree}")
    with tempfile.TemporaryDirectory(prefix="sessiondock-source-index-") as temporary:
        env = dict(os.environ, GIT_INDEX_FILE=str(Path(temporary) / "index"))
        # Copy the index so staged new source files are included. Even
        # write-tree may refresh its cache, so never run it on the shared index.
        index = Path(git("rev-parse", "--git-path", "index"))
        if not index.is_absolute():
            index = ROOT / index
        if index.is_file():
            shutil.copyfile(index, env["GIT_INDEX_FILE"])
        else:
            subprocess.run(["git", "read-tree", "HEAD"], cwd=ROOT, env=env, check=True, timeout=300)
        subprocess.run(["git", "add", "--update", "--", "."], cwd=ROOT, env=env, check=True, timeout=300)
        return subprocess.run(["git", "write-tree"], cwd=ROOT, env=env,
                              capture_output=True, text=True, check=True, timeout=300).stdout.strip()


def archive_source(path: Path, worktree: bool) -> str:
    tree = source_tree(worktree)
    subprocess.run(["git", "archive", "--format=tar", tree if worktree else "HEAD", "-o", str(path)],
                   cwd=ROOT, check=True, timeout=300)
    return tree


class Log:
    """Per-target log file; `-v` also echoes every line to stdout with a prefix."""

    def __init__(self, path: Path, name: str, echo: bool):
        path.parent.mkdir(parents=True, exist_ok=True)
        self.f, self.name, self.echo = path.open("a", encoding="utf-8"), name, echo

    def __call__(self, line: str) -> None:
        ts = utc_now().strftime("%H:%M:%S")
        self.f.write(f"{ts} {line}\n")
        self.f.flush()
        if self.echo:
            with PRINT_LOCK:
                print(f"[{self.name}] {line}", flush=True)

    def close(self) -> None:
        self.f.close()


# -- targets file ------------------------------------------------------------------------
def targets_file(args) -> Path:
    explicit = getattr(args, "targets_file", None)
    if explicit:
        return Path(explicit)
    local = DEPLOY_DIR / "targets.local.json"
    return local if local.is_file() else DEPLOY_DIR / "targets.example.json"


def build_config(args) -> dict:
    try:
        doc = json.loads(targets_file(args).read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        die(f"cannot read {targets_file(args)}: {e}")
    cfg = dict(doc.get("build", {}))
    cfg.setdefault("packages", ["sessiondock", "sessiondock-hub"])
    cfg.setdefault("ptyhost_package", "ptyhost")
    cfg["cargo"] = os.path.expanduser(cfg.get("cargo") or "~/.cargo/bin/cargo")
    return cfg


# -- build ---------------------------------------------------------------------------------
def dirty_files() -> list[str]:
    out = git("status", "--porcelain", "--", *DIRTY_SCOPE)
    return [line for line in out.splitlines() if line.strip()]


def cargo_bins(cargo: str) -> dict[str, str]:
    """bin target name -> package name, from `cargo metadata --no-deps`."""
    p = subprocess.run([cargo, "metadata", "--no-deps", "--format-version", "1", "--locked"],
                       cwd=ROOT, capture_output=True, text=True, timeout=120)
    if p.returncode != 0:
        die(f"cargo metadata failed:\n{p.stderr.strip()}")
    bins: dict[str, str] = {}
    for pkg in json.loads(p.stdout)["packages"]:
        for tgt in pkg["targets"]:
            if "bin" in tgt["kind"]:
                bins[tgt["name"]] = pkg["name"]
    return bins


def resolve_build(cfg: dict, with_ptyhost: bool) -> tuple[list[str], list[str]]:
    """Return (cargo argv, binary names) for the configured packages."""
    bins = cargo_bins(cfg["cargo"])
    wanted = list(cfg["packages"]) + ([cfg["ptyhost_package"]] if with_ptyhost else [])
    packages: list[str] = []
    names: list[str] = []
    for item in wanted:
        if item in bins.values():
            packages.append(item)
            names += [b for b, pkg in bins.items() if pkg == item]
        elif item in bins:
            packages.append(bins[item])
            names.append(item)
        else:
            die(f"build.packages entry {item!r} is neither a package nor a bin target")
    names = list(dict.fromkeys(names))
    argv = [cfg["cargo"], "build", "--release", "--locked"]
    for pkg in dict.fromkeys(packages):
        argv += ["-p", pkg]
    for name in names:
        argv += ["--bin", name]
    return argv, names


def cmd_build(args) -> Path:
    dirty = dirty_files()
    if dirty and not args.allow_dirty:
        print("refusing to build: working tree differs from HEAD in the build scope "
              "(pass --allow-dirty to build the working tree anyway):", file=sys.stderr)
        for line in dirty:
            print("  " + line, file=sys.stderr)
        sys.exit(2)
    cfg = build_config(args)
    commit, short = git("rev-parse", "HEAD"), git("rev-parse", "--short", "HEAD")
    started = utc_now()
    stage = STAGE_ROOT / f"{stamp_of(started)}-{short}"
    for n in range(2, 100):   # never reuse a stage another build made in the same second
        if not stage.exists():
            break
        stage = STAGE_ROOT / f"{stamp_of(started)}-{short}-{n}"
    for sub in ("bin", "web", "logs"):
        (stage / sub).mkdir(parents=True, exist_ok=True)
    print(f"stage {stage}")

    web_from_worktree = args.allow_dirty and not args.web_from_head
    if web_from_worktree:
        argv = ["rsync", "-a", "--delete", *(f"--exclude={e}" for e in WEB_EXCLUDES),
                str(ROOT / "legacy-web") + "/", str(stage / "web") + "/"]
        subprocess.run(argv, check=True, timeout=300)
        print("web: working tree legacy-web/ (dirty allowed)")
    else:
        with subprocess.Popen(["git", "archive", "--format=tar", "HEAD", "legacy-web"], cwd=ROOT,
                              stdout=subprocess.PIPE) as ga:
            subprocess.run(["tar", "-x", "--strip-components=1", "-C", str(stage / "web")],
                           stdin=ga.stdout, check=True, timeout=300)
        if ga.returncode != 0:
            die("git archive HEAD legacy-web failed")
        print("web: git archive HEAD legacy-web")
    if not (stage / "web" / "index.html").is_file():
        die("web snapshot has no index.html")
    snapshot_tree = archive_source(stage / "source.tar", args.allow_dirty)
    print(f"source: {'tracked working tree' if args.allow_dirty else 'HEAD'} ({snapshot_tree})")

    binaries: dict[str, Path] = {}
    sha256: dict[str, str] = {}
    cargo_argv: list[str] = []
    if not args.web_only:
        cargo_argv, names = resolve_build(cfg, args.with_ptyhost)
        log_path = stage / "logs" / "cargo-build.log"
        print(f"cargo: {' '.join(cargo_argv)}  (log {log_path.relative_to(ROOT)})")
        t0 = time.monotonic()
        with log_path.open("w", encoding="utf-8") as log:
            try:
                p = subprocess.run(cargo_argv, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                                   timeout=args.build_timeout)
            except subprocess.TimeoutExpired:
                die(f"cargo build exceeded --build-timeout {args.build_timeout:.0f}s (see {log_path})", 1)
        if p.returncode != 0:
            tail = log_path.read_text(encoding="utf-8", errors="replace").splitlines()[-30:]
            die("cargo build failed (rc=%d):\n%s" % (p.returncode, "\n".join(tail)), 1)
        print(f"cargo: done in {time.monotonic() - t0:.0f}s")
        for name in names:
            src = ROOT / "target" / "release" / name
            if not src.is_file():
                die(f"expected binary missing: {src}", 1)
            dst = stage / "bin" / name
            shutil.copy2(src, dst)
            binaries[name], sha256[name] = dst, sha256_file(dst)

    if args.allow_dirty and source_tree(True) != snapshot_tree:
        die("tracked working files changed during build; rerun to produce a consistent stage", 1)

    crates_dirty = any(not line[3:].startswith("legacy-web/") for line in dirty)
    web_dirty = any(line[3:].startswith("legacy-web/") for line in dirty)
    art = Artifacts(commit=commit, short=short,
                    dirty=(crates_dirty and not args.web_only) or (web_dirty and web_from_worktree),
                    built_at=started.isoformat(timespec="seconds"), web_dir=stage / "web",
                    binaries=binaries, sha256=sha256, source_archive=stage / "source.tar",
                    web_only=args.web_only)
    doc = {"commit": art.commit, "short": art.short, "dirty": art.dirty, "built_at": art.built_at,
           "web_dir": "web", "binaries": {n: f"bin/{n}" for n in binaries}, "sha256": sha256,
           "source_archive": "source.tar", "web_only": art.web_only, "stage": str(stage),
           "web_source": "worktree" if web_from_worktree else "HEAD", "dirty_files": dirty,
           "cargo": cargo_argv, "with_ptyhost": args.with_ptyhost,
           "source_tree": snapshot_tree, "source_source": "worktree" if args.allow_dirty else "HEAD"}
    (stage / "artifacts.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    print(f"commit {commit} ({short}) dirty={art.dirty} built_at={art.built_at}")
    for name, digest in sha256.items():
        print(f"  {name:16} {digest}  {binaries[name].stat().st_size} bytes")
    print(f"  web              {sum(1 for _ in (stage / 'web').rglob('*') if _.is_file())} files"
          f"  source.tar {(stage / 'source.tar').stat().st_size} bytes")
    return stage


# -- stage loading -----------------------------------------------------------------------
def newest_stage() -> Path | None:
    if not STAGE_ROOT.is_dir():
        return None
    stages = sorted(p for p in STAGE_ROOT.iterdir() if (p / "artifacts.json").is_file())
    return stages[-1] if stages else None


def load_stage(path: Path) -> Artifacts:
    try:
        doc = json.loads((path / "artifacts.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        die(f"not a stage directory ({e}): {path}")
    return Artifacts(commit=doc["commit"], short=doc["short"], dirty=doc["dirty"],
                     built_at=doc["built_at"], web_dir=path / doc["web_dir"],
                     binaries={n: path / rel for n, rel in doc["binaries"].items()},
                     sha256=dict(doc["sha256"]),
                     source_archive=path / doc["source_archive"] if doc.get("source_archive") else None,
                     web_only=bool(doc.get("web_only")))


def stage_test_info(stage: Path) -> dict:
    """test_* fields of artifacts.json ({} for a stage built before the test gate existed)."""
    try:
        doc = json.loads((stage / "artifacts.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return {k: v for k, v in doc.items() if k.startswith("test_")}


def placeholder_artifacts(stage: Path | None) -> Artifacts:
    """Used by `rollback`, which needs a handler but ships nothing."""
    if stage is not None:
        return load_stage(stage)
    return Artifacts(commit="0" * 40, short="0000000", dirty=False, built_at="", web_dir=Path("."),
                     binaries={}, sha256={}, source_archive=None, web_only=True)


def select_targets(args, targets: list[Target]) -> list[Target]:
    if getattr(args, "all", False):
        return targets
    names = [n.strip() for n in (args.targets or "").split(",") if n.strip()]
    if not names:
        die("pass --targets a,b or --all")
    by_name = {t.name: t for t in targets}
    missing = [n for n in names if n not in by_name]
    if missing:
        die(f"unknown targets {missing}; known: {sorted(by_name)}")
    return [by_name[n] for n in names]


# -- per-target runs ---------------------------------------------------------------------
def new_row(t: Target) -> dict:
    return {"target": t.name, "kind": t.kind, "result": "", "detail": "", "build_before": None,
            "build_after": None, "sha_before": {}, "sha_after": {}, "ptyhost_before": None,
            "ptyhost_after": None, "hosts_before": None, "hosts_after": None, "seconds": 0.0,
            "backup": None, "pruned": [], "plan": []}


def fill(row: dict, when: str, r) -> None:
    row[f"build_{when}"] = r.build
    row[f"sha_{when}"] = dict(r.binary_sha)
    row[f"ptyhost_{when}"] = len(r.ptyhost_pids)
    row[f"hosts_{when}"] = r.host_records


def wait_healthy(h, timeout: float) -> ProbeResult:
    end = time.monotonic() + timeout
    while True:
        p = h.probe()
        if (p.reachable and p.active and p.build) or time.monotonic() >= end:
            return p
        time.sleep(2.0)


def rollback_problems(before: ProbeResult, after: ProbeResult, names: list[str],
                      expect_sha: dict[str, str] | None) -> list[str]:
    out = []
    if not after.reachable:
        out.append(after.detail or "unreachable")
        return out
    if not after.active:
        out.append("service not active")
    if not after.build:
        out.append("meta not answering")
    for n in names:
        want = (expect_sha or before.binary_sha).get(n)
        if want and after.binary_sha.get(n) != want:
            out.append(f"bin/{n} sha256 {after.binary_sha.get(n, '-')[:12]} != {want[:12]}")
    dead = sorted(set(before.ptyhost_pids) - set(after.ptyhost_pids))
    if dead:
        out.append(f"ptyhost pids gone: {dead}")
    return out


def backup_shas(h, backup: str) -> dict[str, str] | None:
    """SHA-256 of every binary inside `<backup>/bin` on the target (POSIX kinds only)."""
    if h.kind.startswith("windows"):
        return None
    try:
        rc, out = h.sh.run(f'for f in {shlex.quote(backup)}/bin/*; do [ -f "$f" ] || continue; '
                           'sha256sum "$f" 2>/dev/null || shasum -a 256 "$f"; done', timeout=120)
    except ShellError:
        return None
    shas = {}
    for line in out.splitlines():
        parts = line.split()
        if len(parts) == 2 and len(parts[0]) == 64:
            shas[parts[1].rsplit("/", 1)[-1]] = parts[0]
    return shas or None


def finisher(row: dict, log: Log, t0: float):
    def finish(result: str, detail: str = "") -> dict:
        row["result"], row["detail"], row["seconds"] = result, detail, time.monotonic() - t0
        log(f"result {result}: {detail}")
        log.close()
        return row
    return finish


def run_push(t: Target, art: Artifacts, opts: DeployOptions, log_path: Path, echo: bool) -> dict:
    row, t0, log = new_row(t), time.monotonic(), Log(log_path, t.name, echo)
    finish = finisher(row, log, t0)
    log(f"push {art.short} to {t.name} ({t.kind}) prefix={t.prefix} opts={vars(opts)}")
    if not t.enabled:
        return finish("SKIPPED", "disabled in targets file")
    try:
        cls = handler_for(t.kind)
    except Exception as e:  # missing module, syntax error, unknown kind
        return finish("UNSUPPORTED", f"{type(e).__name__}: {e}")
    h = cls(t, art, opts, log)
    try:
        before = h.probe()
    except Exception as e:
        return finish("SKIPPED", f"probe raised {type(e).__name__}: {e}")
    h.before = before
    if not before.reachable:
        return finish("SKIPPED", before.detail or "unreachable")
    fill(row, "before", before)
    try:
        row["plan"] = h.plan()
    except Exception as e:
        return finish("FAILED", f"plan raised {type(e).__name__}: {e}")
    for line in row["plan"]:
        log("plan: " + line)
    if opts.dry_run:
        return finish("PLANNED", f"dry run; {len(row['plan'])} steps, nothing uploaded")
    backup = None
    try:
        h.stage()
        backup = row["backup"] = h.backup()
        h.swap()
        h.restart()
        v = h.verify()
        fill(row, "after", v)
        if not v.ok:
            raise RuntimeError("verify: " + v.detail)
    except Exception as e:
        err = f"{type(e).__name__}: {e}".splitlines()[0][:300]
        log(f"FAILED: {e}")
        if backup is None:
            return finish("FAILED", err + " (nothing was changed on the target)")
        try:
            log(f"rolling back from {backup}")
            h.rollback(backup)
            after = wait_healthy(h, opts.health_timeout)
            fill(row, "after", after)
            problems = rollback_problems(before, after, list(before.binary_sha), None)
        except Exception as e2:
            return finish("ROLLBACK_FAILED", f"{err}; rollback raised {type(e2).__name__}: {e2}")
        if problems:
            return finish("ROLLBACK_FAILED", f"{err}; after rollback: {'; '.join(problems)}")
        return finish("ROLLED_BACK", err)
    try:  # bookkeeping after a verified deploy never triggers a rollback
        h.write_marker()
        if opts.keep_backups > 0:
            row["pruned"] = h.prune_backups(opts.keep_backups)
    except Exception as e:
        return finish("WARN", f"deployed and verified, but marker/prune failed: {type(e).__name__}: {e}"[:300])
    return finish("OK", v.detail + (f"; pruned {len(row['pruned'])}" if row["pruned"] else ""))


def run_rollback(t: Target, art: Artifacts, opts: DeployOptions, log_path: Path, echo: bool,
                 backup: str | None) -> dict:
    row, t0, log = new_row(t), time.monotonic(), Log(log_path, t.name, echo)
    finish = finisher(row, log, t0)
    try:
        cls = handler_for(t.kind)
    except Exception as e:
        return finish("UNSUPPORTED", f"{type(e).__name__}: {e}")
    h = cls(t, art, opts, log)
    try:
        before = h.probe()
        if not before.reachable:
            return finish("SKIPPED", before.detail or "unreachable")
        fill(row, "before", before)
        if backup is None:
            found = h.list_backups()
            if not found:
                return finish("FAILED", "no backup-deploy-* dir found on the target; pass --backup DIR")
            backup = found[0]
        row["backup"] = backup
        expect = backup_shas(h, backup)
        log(f"rollback from {backup} (expect sha {({k: v[:12] for k, v in (expect or {}).items()})})")
        h.rollback(backup)
        after = wait_healthy(h, opts.health_timeout)
        fill(row, "after", after)
    except Exception as e:
        return finish("FAILED", f"{type(e).__name__}: {e}".splitlines()[0][:300])
    problems = rollback_problems(before, after, list((expect or {}).keys()) or list(t.binaries), expect)
    if problems:
        return finish("FAILED", "; ".join(problems))
    return finish("OK", f"restored {backup}; build {before.build} -> {after.build}")


# -- output ----------------------------------------------------------------------------------
def sha_cell(t: Target, d: dict) -> str:
    return " ".join(d.get(n, "")[:12] or "-" for n in t.binaries) if d else "-"


def print_table(rows: list[dict], targets: dict[str, Target]) -> None:
    table = [COLUMNS]
    for r in rows:
        t = targets[r["target"]]
        table.append((r["target"], r["kind"], r["result"],
                      f"{r['build_before'] or '-'}→{r['build_after'] or '-'}",
                      f"{sha_cell(t, r['sha_before'])}→{sha_cell(t, r['sha_after'])}",
                      f"{'-' if r['ptyhost_before'] is None else r['ptyhost_before']}/"
                      f"{'-' if r['ptyhost_after'] is None else r['ptyhost_after']}",
                      f"{r['seconds']:.1f}", r["backup"] or "-"))
    widths = [max(len(str(row[i])) for row in table) for i in range(len(COLUMNS))]
    print()
    for i, row in enumerate(table):
        print("  ".join(str(c).ljust(w) for c, w in zip(row, widths)).rstrip())
        if i and rows[i - 1]["detail"]:
            print(f"    {rows[i - 1]['detail'][:200]}")
    print()


def write_report(stage: Path | None, kind: str, rows: list[dict], args) -> None:
    where = (stage or STAGE_ROOT) / f"report-{stamp_of(utc_now())}.json"
    where.parent.mkdir(parents=True, exist_ok=True)
    doc = {"command": kind, "at": utc_now().isoformat(timespec="seconds"), "stage": str(stage) if stage else None,
           "args": {k: v for k, v in vars(args).items() if k != "func"}, "rows": rows}
    where.write_text(json.dumps(doc, indent=2, default=str) + "\n", encoding="utf-8")
    print(f"report {where}")


def run_all(rows_fn, targets: list[Target], parallel: int) -> list[dict]:
    with ThreadPoolExecutor(max_workers=max(1, parallel)) as pool:
        futures = [pool.submit(rows_fn, t) for t in targets]
        rows = []
        for fut in futures:
            row = fut.result()
            with PRINT_LOCK:
                print(f"{row['target']:12} {row['result']:16} {row['seconds']:6.1f}s  {row['detail'][:120]}", flush=True)
            rows.append(row)
    return rows


# -- test gate (build -> TEST -> push) -----------------------------------------------------
def probe_markers(targets: list[Target], stage: Path, args) -> dict[str, str | None]:
    """One read-only probe per enabled target: its etc/deployed-commit, for the test base."""
    art, opts = load_stage(stage), DeployOptions(log_dir=stage / "logs")

    def one(t: Target) -> tuple[str, str | None]:
        log = Log(opts.log_dir / f"{t.name}.log", t.name, args.verbose)
        try:
            r = handler_for(t.kind)(t, art, opts, log).probe()
            log(f"test-base probe: reachable={r.reachable} marker={r.deployed_commit!r}")
            return t.name, r.deployed_commit if r.reachable else None
        except Exception as e:  # unreachable / unsupported kinds simply contribute no marker
            log(f"test-base probe failed: {type(e).__name__}: {e}")
            return t.name, None
        finally:
            log.close()

    with ThreadPoolExecutor(max_workers=max(1, getattr(args, "parallel", 4))) as pool:
        return dict(pool.map(one, [t for t in targets if t.enabled]))


def run_test_gate(args, stage: Path, targets: list[Target] | None) -> None:
    """Run the suites the change needs against the stage; exit 1 before any upload on failure."""
    binary = str(stage / "bin" / "sessiondock") if (stage / "bin" / "sessiondock").is_file() \
        else "target/release/sessiondock"
    base = how = None
    changed: list[str] = []
    names: list[str] = []
    if args.test == "affected":
        try:
            base, how = testplan.resolve_base(args.test_base, probe_markers(targets, stage, args) if targets else {})
        except ValueError as e:
            die(str(e))
        changed, names = testplan.changed_files(base, args.allow_dirty), testplan.list_suites(binary)
        if args.web_only:
            # This stage ships no Rust sources or binaries. Concurrent backend
            # edits must not force their Cargo validation into a page update.
            changed = [path for path in changed if not path.startswith("crates/")
                       and path not in {"Cargo.toml", "Cargo.lock"}]
    plan = testplan.plan_for(args.test, changed, names)
    testplan.print_plan(plan, base, how or "")
    res = testplan.run(plan, binary, stage, args.test_timeout)
    doc = json.loads((stage / "artifacts.json").read_text(encoding="utf-8"))
    doc.update({"test_mode": args.test, "test_base": base, "test_full": plan["full"],
                "test_suites": plan["suites"] + plan["scripts"], "test_result": res["result"],
                "test_log": res["log"] if res["result"] != "skipped" else None})
    (stage / "artifacts.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    print(f"tests: {res['result']} (mode {args.test}" + (f", log {res['log']})" if res["result"] != "skipped" else ")"),
          flush=True)
    if res["result"] not in ("passed", "skipped"):
        print("failing suites (nothing was uploaded):", file=sys.stderr)
        for name, log in res["failed"]:
            print(f"  {name}  {log}", file=sys.stderr)
        sys.exit(1)


# -- commands --------------------------------------------------------------------------------
def cmd_push(args, stage: Path | None = None) -> int:
    if args.web_only and args.bin_only:
        die("--web-only and --bin-only exclude each other")
    stage = stage or (Path(args.stage) if args.stage else newest_stage())
    if stage is None:
        die("no stage under target/deploy; run `deploy.py build` first or pass --stage DIR")
    art = load_stage(stage)
    if art.web_only and not args.web_only:
        die(f"stage {stage.name} was built --web-only; push it with --web-only")
    if args.with_ptyhost and "ptyhost" not in art.binaries and not args.web_only:
        die(f"stage {stage.name} has no ptyhost binary; build with --with-ptyhost")
    try:
        targets = load_targets(targets_file(args))
    except (OSError, ValueError, TypeError, KeyError) as e:
        die(f"cannot load {targets_file(args)}: {e}")
    chosen = select_targets(args, targets)
    info = stage_test_info(stage)
    if info:
        print(f"stage tests: mode={info.get('test_mode')} result={info.get('test_result')} "
              f"base={(info.get('test_base') or '-')[:12]} "
              f"suites={'full sweep' if info.get('test_full') else len(info.get('test_suites') or [])}")
    else:
        print("stage tests: unknown (stage predates the test gate)")
    opts = DeployOptions(dry_run=args.dry_run, web_only=args.web_only, bin_only=args.bin_only,
                         with_ptyhost=args.with_ptyhost, keep_backups=args.keep_backups,
                         health_timeout=args.health_timeout, log_dir=stage / "logs",
                         test_mode=info.get("test_mode") or "none")
    print(f"{'DRY RUN: ' if args.dry_run else ''}push {art.short} ({'web only' if args.web_only else 'bin only' if args.bin_only else 'bin+web'}"
          f"{', +ptyhost' if args.with_ptyhost else ''}) from {stage} to {[t.name for t in chosen]} "
          f"(targets {targets_file(args)}, parallel {args.parallel})")
    rows = run_all(lambda t: run_push(t, art, opts, opts.log_dir / f"{t.name}.log", args.verbose),
                   chosen, args.parallel)
    if args.dry_run:
        for r in rows:
            if r["plan"]:
                print(f"\n== {r['target']} ({r['kind']}) plan:")
                for line in r["plan"]:
                    print("   - " + line)
    print_table(rows, {t.name: t for t in targets})
    write_report(stage, "push", rows, args)
    return 0 if all(r["result"] in GOOD for r in rows) else 1


def cmd_deploy(args) -> int:
    """build -> test -> push; the targets are loaded first so the test base can use their markers."""
    try:
        targets = load_targets(targets_file(args))
    except (OSError, ValueError, TypeError, KeyError) as e:
        die(f"cannot load {targets_file(args)}: {e}")
    chosen = select_targets(args, targets)
    stage = cmd_build(args)
    run_test_gate(args, stage, chosen)
    return cmd_push(args, stage)


def cmd_rollback(args) -> int:
    try:
        targets = load_targets(targets_file(args))
    except (OSError, ValueError, TypeError, KeyError) as e:
        die(f"cannot load {targets_file(args)}: {e}")
    chosen = select_targets(args, targets)
    if args.backup and len(chosen) != 1:
        die("--backup DIR applies to exactly one target")
    stage = Path(args.stage) if args.stage else newest_stage()
    art = placeholder_artifacts(stage)
    opts = DeployOptions(health_timeout=args.health_timeout, log_dir=(stage or STAGE_ROOT) / "logs")
    print(f"rollback {[t.name for t in chosen]} to {args.backup or 'newest backup-deploy-* on each target'}")
    rows = run_all(lambda t: run_rollback(t, art, opts, opts.log_dir / f"{t.name}.log", args.verbose, args.backup),
                   chosen, args.parallel)
    print_table(rows, {t.name: t for t in targets})
    write_report(stage, "rollback", rows, args)
    return 0 if all(r["result"] == "OK" for r in rows) else 1


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="command", required=True)

    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--targets-file", help="default deploy/targets.local.json (else targets.example.json)")
    common.add_argument("-v", "--verbose", action="store_true", help="echo per-target log lines to stdout")
    common.add_argument("--lock-timeout", type=float, metavar="SECONDS",
                        help="wait for the repository deployment lock (default: indefinitely; 0: fail immediately)")
    build_flags = argparse.ArgumentParser(add_help=False)
    build_flags.add_argument("--allow-dirty", action="store_true",
                             help="build although crates/, legacy-web/ or Cargo.* differ from HEAD (web then comes from the working tree)")
    build_flags.add_argument("--web-from-head", action="store_true",
                             help="with --allow-dirty: still snapshot legacy-web/ from HEAD, not the working tree")
    build_flags.add_argument("--build-timeout", type=float, default=3600, help="seconds for cargo build")

    def test_flags(default: str) -> argparse.ArgumentParser:   # a fresh parent per command: its own default
        tf = argparse.ArgumentParser(add_help=False)
        tf.add_argument("--test", choices=testplan.MODES, default=default,
                        help="1 none: ship untested; 2 affected: suites mapped from the files changed since the "
                             f"targets' deployed commit; 3 full: the whole sweep (default here: {default})")
        tf.add_argument("--test-base", metavar="REF",
                        help="diff base for --test affected (default: oldest etc/deployed-commit of the targets, "
                             "else origin/main, else HEAD~1)")
        tf.add_argument("--test-timeout", type=float, default=2400, help="seconds for the whole test run")
        return tf
    ship_flags = argparse.ArgumentParser(add_help=False)
    ship_flags.add_argument("--with-ptyhost", action="store_true", help="also build/ship the ptyhost binary")
    ship_flags.add_argument("--web-only", action="store_true", help="no cargo build / no binary steps")
    push_flags = argparse.ArgumentParser(add_help=False)
    push_flags.add_argument("--targets", help="comma-separated target names")
    push_flags.add_argument("--all", action="store_true", help="every target in the targets file")
    push_flags.add_argument("--stage", help="stage dir (default: newest under target/deploy)")
    push_flags.add_argument("--bin-only", action="store_true", help="skip web (build hash need not change)")
    push_flags.add_argument("--dry-run", action="store_true", help="probe, print the plan, upload nothing")
    push_flags.add_argument("--parallel", type=int, default=4)
    push_flags.add_argument("--keep-backups", type=int, default=5, help="0 = never prune")
    push_flags.add_argument("--health-timeout", type=float, default=45.0)

    p = sub.add_parser("build", parents=[common, build_flags, test_flags("none"), ship_flags],
                       help="build binaries + web snapshot into target/deploy/<stamp>-<short>/, then --test (default none)")
    p.set_defaults(func=lambda a: (run_test_gate(a, cmd_build(a), None), 0)[1])
    p = sub.add_parser("push", parents=[common, ship_flags, push_flags], help="deploy a stage to targets (never tests here)")
    p.set_defaults(func=cmd_push)
    p = sub.add_parser("deploy", parents=[common, build_flags, test_flags("affected"), ship_flags, push_flags],
                       help="build, then --test (default affected), then push")
    p.set_defaults(func=cmd_deploy)
    p = sub.add_parser("rollback", parents=[common], help="restore bin/ and web/ from a backup-deploy-* dir")
    p.add_argument("--targets", required=True, help="comma-separated target names")
    p.add_argument("--backup", help="backup dir on the target (default: newest backup-deploy-*)")
    p.add_argument("--stage", help="stage whose logs/ receives the per-target log (default: newest)")
    p.add_argument("--parallel", type=int, default=4)
    p.add_argument("--health-timeout", type=float, default=45.0)
    p.set_defaults(func=cmd_rollback)

    args = ap.parse_args(argv)
    if args.lock_timeout is not None and (args.lock_timeout < 0 or not math.isfinite(args.lock_timeout)):
        ap.error("--lock-timeout must be a finite nonnegative number")
    try:
        with DeploymentLock(repository_lock(ROOT), args.command, args.lock_timeout):
            return args.func(args)
    except TimeoutError as exc:
        die(str(exc))


if __name__ == "__main__":
    sys.exit(main())
