#!/usr/bin/env python3
"""Export a committed, allowlisted source snapshot without private Git history.

    python3 deploy/export_public.py --ref HEAD --out /path/to/new-directory --draft

The manifest and public documentation are taken from the selected commit too.
The destination must not exist. This command never pushes, creates a remote,
copies .git, or overwrites an existing public checkout.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FORBIDDEN_PARTS = {".git", ".runtime", "target", "__pycache__", ".remember", "node_modules"}
PERSONAL_PATH = re.compile(r"/(?:home|Users)/(?!user/|example/)[^/\s]+/(?:Projects|Desktop|Documents)/")


def git(*args: str) -> bytes:
    return subprocess.check_output(["git", *args], cwd=ROOT)


def path_ok(value: str) -> str:
    path = PurePosixPath(value)
    if path.is_absolute() or not path.parts or ".." in path.parts or "\\" in value:
        raise ValueError(f"invalid manifest path: {value}")
    if any(part in FORBIDDEN_PARTS or part == ".env" or part.startswith(".env.")
           or part.endswith(".local") for part in path.parts):
        raise ValueError(f"private/runtime path cannot be exported: {value}")
    return path.as_posix()


def matches(path: str, rules: list[str]) -> bool:
    return any(path.startswith(rule) if rule.endswith("/") else path == rule for rule in rules)


def tree(commit: str) -> dict[str, tuple[str, str]]:
    result = {}
    for entry in git("ls-tree", "-r", "-z", commit).split(b"\0"):
        if not entry:
            continue
        meta, name = entry.split(b"\t", 1)
        mode, kind, oid = meta.decode().split()
        result[name.decode()] = (mode, oid)
    return result


def blob(commit: str, name: str) -> bytes:
    return git("show", f"{commit}:{name}")


def write_files(entries: dict, outputs: dict[str, str], destination: Path) -> None:
    # One batch process avoids one Git subprocess per file. Only allowlisted blobs
    # are ever read into the staging directory; private paths are not extracted.
    with subprocess.Popen(["git", "cat-file", "--batch"], cwd=ROOT,
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE) as reader:
        assert reader.stdin is not None and reader.stdout is not None
        for source, output in sorted(outputs.items()):
            mode, oid = entries[source]
            if mode not in {"100644", "100755"}:
                raise ValueError(f"only regular tracked files are exportable: {source}")
            path_ok(output)
            reader.stdin.write((oid + "\n").encode())
            reader.stdin.flush()
            header = reader.stdout.readline().decode().strip().split()
            if len(header) != 3 or header[1] != "blob":
                raise ValueError(f"cannot read source blob: {source}")
            data = reader.stdout.read(int(header[2]))
            if reader.stdout.read(1) != b"\n":
                raise ValueError(f"incomplete source blob: {source}")
            target = destination / output
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            target.chmod(0o755 if mode == "100755" else 0o644)
        reader.stdin.close()
        if reader.wait() != 0:
            raise ValueError("git cat-file failed")


def check_snapshot(destination: Path) -> None:
    # This is a focused known-risk check, not a promise to detect every secret.
    # Errors name files only; they never print a possible sensitive value.
    findings = []
    for path in sorted(destination.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(destination).as_posix()
        path_ok(rel)
        try:
            content = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        if PERSONAL_PATH.search(content) or re.search(r"-----BEGIN (?:[A-Z]+ )?PRIVATE KEY-----", content):
            findings.append(rel)
    if findings:
        raise ValueError("review private path/key material in: " + ", ".join(findings))
    for script in ("check_docs_links.py", "check_agents_md.py"):
        subprocess.run([sys.executable, "-B", str(destination / "tests" / script)],
                       cwd=destination, check=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ref", default="HEAD", help="committed source revision, including manifest and overlays")
    parser.add_argument("--out", type=Path, required=True, help="new destination directory (must not exist)")
    parser.add_argument("--draft", action="store_true", help="prepare an explicitly unpublished preview, even without LICENSE")
    parser.add_argument("--init-git", action="store_true", help="initialize a new independent Git repository; no commit or remote")
    args = parser.parse_args()
    destination = args.out.absolute()
    if destination.exists() or destination.is_symlink():
        parser.error("--out must name a new directory; existing checkouts are never overwritten")
    commit = git("rev-parse", "--verify", "--end-of-options", f"{args.ref}^{{commit}}").decode().strip()
    entries = tree(commit)
    manifest = json.loads(blob(commit, "public/manifest.json"))
    if manifest.get("version") != 1:
        parser.error("unsupported public export manifest version")
    include, optional = manifest["include"], manifest["optional"]
    exclude, overlays = manifest["exclude"], manifest["overlays"]
    for name in [*include, *optional, *exclude, *overlays, *overlays.values()]:
        path_ok(name)
    for name in include:
        if not any(matches(path, [name]) for path in entries):
            parser.error(f"required public content is missing from selected commit: {name}")
    outputs = {path: path for path in entries
               if matches(path, include + optional) and not matches(path, exclude)}
    for source, target in overlays.items():
        if source not in entries:
            parser.error(f"public overlay is missing from selected commit: {source}")
        outputs = {src: dst for src, dst in outputs.items() if dst != target}
        outputs[source] = target
    if len(set(outputs.values())) != len(outputs):
        parser.error("manifest maps multiple files to the same public path")
    licensed = any(name in outputs.values() for name in ("LICENSE", "LICENSE.md", "LICENSE.txt"))
    if not licensed and not args.draft:
        parser.error("project LICENSE is absent: choose a license before publication, or prepare with --draft")

    destination.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".sessiondock-public-", dir=destination.parent))
    try:
        write_files(entries, outputs, staging)
        validation = staging / "docs/validation.md"
        with validation.open("a", encoding="utf-8") as doc:
            for script in sorted((staging / "tests").glob("*.py")):
                doc.write(f"\n- [{script.name}](../tests/{script.name})")
            doc.write("\n")
        if args.draft:
            (staging / "PUBLICATION-DRAFT.txt").write_text(
                "UNPUBLISHED PREPARATION SNAPSHOT\n"
                "Review licensing, imported-code rights and snapshot contents before publication.\n"
                "This marker is present because export used --draft; it does not grant a license.\n",
                encoding="utf-8")
        check_snapshot(staging)
        if args.init_git:
            subprocess.run(["git", "init", "--initial-branch=main", str(staging)], check=True,
                           stdout=subprocess.DEVNULL)
        # Recheck before rename so a concurrent creator cannot have its empty
        # destination silently replaced by POSIX rename semantics.
        if destination.exists() or destination.is_symlink():
            raise ValueError("destination appeared during export; no files replaced")
        staging.rename(destination)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    print(f"exported {len(outputs)} committed files to {destination}")
    print(f"source revision (private-side log only): {commit}")
    print("draft preparation only" if args.draft else "source snapshot ready for publication review")
    print("No history, remote, commit, push or deployment was copied or performed.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, OSError, subprocess.CalledProcessError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
