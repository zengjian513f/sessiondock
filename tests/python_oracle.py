"""Locate and import the optional legacy Python comparison checkout safely."""
# run_validation: skip
from __future__ import annotations

import importlib
import os
from pathlib import Path
import sys


def package_dir(source: Path, required=("adapters.py",)) -> Path:
    """Return the unique immediate Python package containing required files."""
    source = Path(source).resolve(strict=True)
    candidates = []
    if all((source / name).is_file() for name in required):
        candidates.append(source)
    candidates.extend(
        child for child in source.iterdir()
        if child.is_dir() and all((child / name).is_file() for name in required)
    )
    if len(candidates) != 1:
        names = ", ".join(required)
        raise RuntimeError(f"--python-source must contain one Python package with: {names}")
    return candidates[0]


def source_file(source: Path, name: str) -> Path:
    return package_dir(source, (name,)) / name


def import_oracle_module(source: Path, leaf: str):
    package = package_dir(source, (leaf + ".py",))
    root = str(package.parent)
    if root not in sys.path:
        sys.path.insert(0, root)
    module = importlib.import_module(package.name + "." + leaf)
    expected = (package / (leaf + ".py")).resolve()
    if Path(module.__file__).resolve() != expected:
        raise RuntimeError("a different Python oracle package was already imported")
    return module


def discover_source(workspace: Path) -> Path:
    """Find the unique sibling checkout that exposes the comparison adapters."""
    workspace = Path(workspace).resolve()
    configured = os.environ.get("SESSIONDOCK_PYTHON_SOURCE")
    if configured:
        return Path(configured).expanduser().resolve()
    candidates = []
    for child in workspace.parent.iterdir():
        if not child.is_dir() or child.resolve() == workspace:
            continue
        try:
            package_dir(child, ("adapters.py", "media.py", "federation.py"))
        except (OSError, RuntimeError):
            continue
        candidates.append(child.resolve())
    if len(candidates) == 1:
        return candidates[0]
    return workspace.parent / "python-oracle"
