#!/usr/bin/env python3
"""Reproducible Linux release package of one commit.

    deploy/release.py [--ref REF] [--out DIR] [--check] [--component resource-agent]

Builds `sessiondock`, `sessiondock-hub`, `sessiondock-transfer` and `ptyhost`
from `git archive REF` in a fresh temporary directory (never the working tree),
with build paths remapped and SOURCE_DATE_EPOCH set to the commit time, then
writes `sessiondock-<version>-<short>-linux-<arch>.tar.gz` with bin/, web/
(web), README.md and manifest.json. Archive entries are
sorted with fixed owner, mode and mtime, so the same commit and toolchain give
the same bytes. `--check` builds twice in different directories and fails if
the two packages differ. resource-agent ships separately (docs/release.md).
"""
from __future__ import annotations

import argparse
import gzip
import io
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BINARIES = ["sessiondock", "sessiondock-hub", "sessiondock-transfer", "ptyhost"]
PACKAGES = ["sessiondock", "ptyhost"]


def components(component: str) -> tuple[list[str], list[str]]:
    return (["resource-agent"], ["resource-agent"]) if component == "resource-agent" else (PACKAGES, BINARIES)


def public_document(src: Path, name: str) -> bytes:
    """The private source uses public overlays; public snapshots already applied them."""
    overlay = src / "public" / name
    return (overlay if overlay.is_file() else src / name).read_bytes()


def run(argv, **kw) -> str:
    return subprocess.run(argv, check=True, capture_output=True, text=True, **kw).stdout.strip()


def git(*args: str) -> str:
    return run(["git", *args], cwd=ROOT)


def cargo() -> str:
    found = shutil.which("cargo") or os.path.expanduser("~/.cargo/bin/cargo")
    if not Path(found).exists():
        sys.exit("error: cargo not found")
    return found


def build(commit: str, epoch: str, work: Path, component: str = "sessiondock") -> dict[str, bytes]:
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
    packages, binaries = components(component)
    for package in packages:
        argv += ["-p", package]
    for name in binaries:
        argv += ["--bin", name]
    print(f"build {commit[:12]} in {work}", flush=True)
    subprocess.run(argv, cwd=src, env=env, check=True)
    files = {f"bin/{name}": (target / "release" / name).read_bytes() for name in binaries}
    if component == "resource-agent":
        files["README.md"] = (
            "# Optional Linux resource collector\n\n"
            "This package contains `bin/resource-agent` and service templates in `deploy/`.\n"
            "It does not install or start any service automatically.\n\n"
            "See [installation and configuration](docs/resource-agent.md). For this prebuilt\n"
            "package, use `bin/resource-agent` wherever the guide names\n"
            "`target/release/resource-agent`; no Rust or BPF compiler is needed at runtime.\n"
        ).encode()
        guide = (src / "docs/resource-agent.md").read_text(encoding="utf-8")
        files["docs/resource-agent.md"] = guide.split("## Validate the integration", 1)[0].encode()
        for name in ("resource-agent.service", "sys-fs-resctrl.mount"):
            files[f"deploy/{name}"] = (src / "deploy" / name).read_bytes()
    else:
        for path in sorted((src / "web").rglob("*")):
            if path.is_file():
                files["web/" + path.relative_to(src / "web").as_posix()] = path.read_bytes()
        files["README.md"] = (
            "# SessionDock binary package\n\n"
            "Extract the package and run from its top-level directory:\n\n"
            "```sh\nSESSIONDOCK_WEB_DIR=web ./bin/sessiondock\n```\n\n"
            "Open http://127.0.0.1:8741. The initial list is empty until you explicitly\n"
            "configure native session roots with `SESSIONDOCK_CLAUDE_ROOT`,\n"
            "`SESSIONDOCK_CODEX_ROOT` or `SESSIONDOCK_GROK_ROOT`. The package contains\n"
            "no user sessions, synthetic fixtures, or local configuration. It does not\n"
            "read `.env` automatically. Use `./bin/sessiondock --check-config` before\n"
            "starting, and keep metadata in a private `SESSIONDOCK_STATE_DIR`.\n\n"
            "The frontend is in `web/`; set an absolute `SESSIONDOCK_WEB_DIR` when\n"
            "starting from another working directory. Restart after changing assets.\n"
            "No Rust, Node.js, Clang or BPF build tools are needed to run these binaries.\n\n"
            "The ordinary listener must stay on loopback. For other devices, use an\n"
            "authenticated reverse proxy, configure `SESSIONDOCK_PUBLIC_HOSTS`, and\n"
            "support WebSocket upgrades and SSE. Host checks do not authenticate users.\n\n"
            "`bin/sessiondock-hub` is the optional multi-node service; consult its\n"
            "`--help` for registration commands. `bin/ptyhost` is an independent\n"
            "terminal host: always pass an explicit private `--dir`. Stopping the web\n"
            "service does not terminate terminal hosts. `bin/sessiondock-transfer`\n"
            "provides explicit session transfer operations; consult `--help`.\n\n"
            "Labdesk, FileDock and the Linux collector are optional and not installed\n"
            "by this package. See [integration configuration](docs/integrations.md).\n"
            "The [collector guide](docs/resource-agent.md) describes the separate\n"
            "collector package and its optional source build; its Cargo commands\n"
            "require a source checkout. This package never deploys or starts services.\n"
        ).encode()
        files["docs/integrations.md"] = public_document(src, "docs/integrations.md")
        guide = (src / "docs/resource-agent.md").read_text(encoding="utf-8")
        files["docs/resource-agent.md"] = guide.split("## Validate the integration", 1)[0].encode()
        for name in ("resource-agent.service", "sys-fs-resctrl.mount"):
            files[f"deploy/{name}"] = (src / "deploy" / name).read_bytes()
    for name in ("LICENSE", "LICENSE.md", "LICENSE.txt", "NOTICE.md"):
        if (src / name).is_file():
            files[name] = (src / name).read_bytes()
    if component == "sessiondock" and (src / "public/NOTICE.md").is_file():
        files["NOTICE.md"] = (src / "public/NOTICE.md").read_bytes()
    return files


def package(files: dict[str, bytes], meta: dict, epoch: int, name: str) -> bytes:
    files = dict(files)
    version = str(meta['source_date_epoch'])
    if meta.get("component", "sessiondock") == "sessiondock":
        files['web/release-version.txt'] = (version + '\n').encode()
    for binary in meta['binaries']:
        files[f'bin/{binary}.version'] = (version + '\n').encode()
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
    parser.add_argument("--component", choices=("sessiondock", "resource-agent"), default="sessiondock",
                        help="build the core package or the separate optional Linux collector")
    args = parser.parse_args()
    commit = git("rev-parse", f"{args.ref}^{{commit}}")
    short, epoch = commit[:8], int(git("show", "-s", "--format=%ct", commit))
    version = tomllib.loads(git("show", f"{commit}:Cargo.toml"))["workspace"]["package"]["version"]
    if args.component == "resource-agent" and platform.system() != "Linux":
        parser.error("resource-agent packaging requires Linux")
    arch = platform.machine()
    name = f"{args.component}-{version}-{short}-linux-{arch}"
    meta = {"commit": commit, "version": version, "target": f"{arch}-unknown-linux-gnu",
            "source_date_epoch": epoch, "rustc": run(["rustc", "-V"]) if shutil.which("rustc")
            else run([os.path.expanduser("~/.cargo/bin/rustc"), "-V"]),
            "component": args.component, "binaries": components(args.component)[1]}
    rounds = 2 if args.check else 1
    packages = []
    for round_ in range(rounds):
        with tempfile.TemporaryDirectory(prefix=f"sessiondock-release-{round_}-") as work:
            packages.append(package(build(commit, str(epoch), Path(work), args.component), meta, epoch, name))
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    path = out / f"{name}.tar.gz"
    path.write_bytes(packages[0])
    print(f"package {path} version={version} source_date_epoch={epoch}")
    if args.check:
        if packages[0] != packages[1]:
            (out / f"{name}.second.tar.gz").write_bytes(packages[1])
            print(f"error: second build differs; kept as {name}.second.tar.gz",
                  file=sys.stderr)
            return 1
        print("reproducible: two builds in different directories are byte-identical")
    return 0


if __name__ == "__main__":
    sys.exit(main())
