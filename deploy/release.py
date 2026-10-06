#!/usr/bin/env python3
"""Reproducible Linux release package of one commit.

    deploy/release.py [--ref REF] [--out DIR] [--check]

Builds `sessiondock`, `sessiondock-hub`, `sessiondock-transfer` and `ptyhost`
from `git archive REF` in a fresh temporary directory (never the working tree),
with build paths remapped and SOURCE_DATE_EPOCH set to the commit time, then
writes `sessiondock-<version>-<short>-linux-<arch>.tar.gz` with bin/, web/
(legacy-web), README.md, manifest.json and SHA256SUMS. Archive entries are
sorted with fixed owner, mode and mtime, so the same commit and toolchain give
the same bytes. `--check` builds twice in different directories and fails if
the two packages differ. resource-agent ships separately (docs/release.md).
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BINARIES = ["sessiondock", "sessiondock-hub", "sessiondock-transfer", "ptyhost"]
PACKAGES = ["sessiondock", "ptyhost"]


def run(argv, **kw) -> str:
    return subprocess.run(argv, check=True, capture_output=True, text=True, **kw).stdout.strip()


def git(*args: str) -> str:
    return run(["git", *args], cwd=ROOT)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def cargo() -> str:
    found = shutil.which("cargo") or os.path.expanduser("~/.cargo/bin/cargo")
    if not Path(found).exists():
        sys.exit("error: cargo not found")
    return found


def build(commit: str, epoch: str, work: Path) -> dict[str, bytes]:
    """Every package file (path -> bytes) built from `commit` under `work`."""
    src, target = work / "src", work / "target"
    src.mkdir(parents=True)
    with subprocess.Popen(["git", "archive", "--format=tar", commit], cwd=ROOT, stdout=subprocess.PIPE) as archive:
        subprocess.run(["tar", "-x", "-C", str(src)], stdin=archive.stdout, check=True)
    if archive.returncode != 0:
        sys.exit("error: git archive failed")
    cargo_home = Path(os.environ.get("CARGO_HOME", os.path.expanduser("~/.cargo")))
    remap = [f"--remap-path-prefix={src}=/build/sessiondock",
             f"--remap-path-prefix={cargo_home}=/cargo",
             f"--remap-path-prefix={work}=/build"]
    env = dict(os.environ, CARGO_TARGET_DIR=str(target), SOURCE_DATE_EPOCH=epoch, CARGO_INCREMENTAL="0",
               RUSTFLAGS=" ".join(remap))
    argv = [cargo(), "build", "--release", "--locked"]
    for package in PACKAGES:
        argv += ["-p", package]
    for name in BINARIES:
        argv += ["--bin", name]
    print(f"build {commit[:12]} in {work}", flush=True)
    subprocess.run(argv, cwd=src, env=env, check=True)
    files = {f"bin/{name}": (target / "release" / name).read_bytes() for name in BINARIES}
    for path in sorted((src / "legacy-web").rglob("*")):
        if path.is_file():
            files["web/" + path.relative_to(src / "legacy-web").as_posix()] = path.read_bytes()
    files["README.md"] = (src / "README.md").read_bytes()
    return files


def package(files: dict[str, bytes], meta: dict, epoch: int, name: str) -> bytes:
    files = dict(files)
    files["SHA256SUMS"] = "".join(f"{sha256(data)}  {path}\n" for path, data in sorted(files.items())).encode()
    files["manifest.json"] = (json.dumps(meta, indent=2, sort_keys=True) + "\n").encode()
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as tar:
        dirs = sorted({str(Path(path).parent) for path in files if "/" in path}
                      | {str(p) for path in files for p in Path(path).parents if str(p) != "."})
        for entry in [name] + [f"{name}/{d}" for d in dirs]:
            info = tarfile.TarInfo(entry)
            info.type, info.mode, info.mtime = tarfile.DIRTYPE, 0o755, epoch
            tar.addfile(info)
        for path, data in sorted(files.items()):
            info = tarfile.TarInfo(f"{name}/{path}")
            info.size, info.mtime = len(data), epoch
            info.mode = 0o755 if path.startswith("bin/") else 0o644
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            tar.addfile(info, io.BytesIO(data))
    out = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=out, mtime=epoch, compresslevel=9) as gz:
        gz.write(raw.getvalue())
    return out.getvalue()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ref", default="HEAD")
    parser.add_argument("--out", default=str(ROOT / "target" / "release-packages"))
    parser.add_argument("--check", action="store_true", help="build twice and require identical packages")
    args = parser.parse_args()
    commit = git("rev-parse", f"{args.ref}^{{commit}}")
    short, epoch = commit[:8], int(git("show", "-s", "--format=%ct", commit))
    version = run([cargo(), "metadata", "--no-deps", "--format-version", "1", "--locked"], cwd=ROOT)
    version = next(p["version"] for p in json.loads(version)["packages"] if p["name"] == "sessiondock")
    arch = platform.machine()
    name = f"sessiondock-{version}-{short}-linux-{arch}"
    meta = {"commit": commit, "version": version, "target": f"{arch}-unknown-linux-gnu",
            "source_date_epoch": epoch, "rustc": run(["rustc", "-V"]) if shutil.which("rustc")
            else run([os.path.expanduser("~/.cargo/bin/rustc"), "-V"]), "binaries": BINARIES}
    rounds = 2 if args.check else 1
    packages = []
    for round_ in range(rounds):
        with tempfile.TemporaryDirectory(prefix=f"sessiondock-release-{round_}-") as work:
            packages.append(package(build(commit, str(epoch), Path(work)), meta, epoch, name))
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    path = out / f"{name}.tar.gz"
    path.write_bytes(packages[0])
    (out / f"{name}.tar.gz.sha256").write_text(f"{sha256(packages[0])}  {path.name}\n")
    print(f"package {path} sha256 {sha256(packages[0])}")
    if args.check:
        if packages[0] != packages[1]:
            (out / f"{name}.second.tar.gz").write_bytes(packages[1])
            print(f"error: second build differs ({sha256(packages[1])}); kept as {name}.second.tar.gz",
                  file=sys.stderr)
            return 1
        print("reproducible: two builds in different directories are byte-identical")
    return 0


if __name__ == "__main__":
    sys.exit(main())
