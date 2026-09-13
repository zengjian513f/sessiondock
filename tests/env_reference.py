#!/usr/bin/env python3
"""Authoritative SESSIONDOCK_* tables from Config::from_env (config.rs) and
HubConfig::from_env (hub_config.rs)."""
# run_validation: skip
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "crates/sessiondock/src/config.rs"
HUB_CONFIG = ROOT / "crates/sessiondock/src/hub_config.rs"
DOCS, OUT = ROOT / "docs", ROOT / "docs" / "environment.md"
OCC = re.compile(r'(?:env::var(?:_os)?|root)\(\s*"(SESSIONDOCK_[A-Z0-9_]+)"')
STR = re.compile(r'"((?:\\.|[^"\\])*)"')
IDENT = re.compile(r"SESSIONDOCK_[A-Z0-9_]+")
INTRO = """# Environment variables

This file is produced by `tests/env_reference.py`. Regenerate:

```sh
python3 tests/env_reference.py --write
```
"""

def section(text, begin, end):
    start = text.find(begin)
    stop = text.find(end, start + len(begin)) if start >= 0 else -1
    return "" if start < 0 or stop < 0 else text[start:stop]

def msgs(blob):
    return [m.replace("\\n", " ") for m in STR.findall(blob) if len(m) > 18 and " " in m]

def config_fields(body):
    found, docs = {}, []
    for line in body.splitlines():
        stripped = line.strip()
        if stripped.startswith("///"):
            docs.append(stripped[3:].strip())
            continue
        match = re.match(r"pub\s+(\w+)\s*:\s*(.+?)\s*,?\s*$", stripped)
        if match:
            found[match.group(1)] = (match.group(2).rstrip(","), " ".join(docs))
        if stripped and not stripped.startswith("///"):
            docs = []
    return found

def occurrences(from_env):
    matches, rows, seen = list(OCC.finditer(from_env)), [], set()
    for index, match in enumerate(matches):
        var = match.group(1)
        if var in seen:
            continue
        seen.add(var)
        start = from_env.rfind("\n", 0, match.start()) + 1
        end = matches[index + 1].start() if index + 1 < len(matches) else len(from_env)
        window = from_env[start:end]
        nest = re.search(r"(\w+)\s*:\s*root\(\s*\"" + re.escape(var), window)
        cfg = re.search(r"config\.(\w+)", window)
        field = f"roots.{nest.group(1)}" if nest else (cfg.group(1) if cfg else "?")
        key = field.split(".")[-1].replace("_", " ")
        local = [m for m in msgs(window) if var in m or key in m.lower()]
        # `let x = env::var_os("VAR")[.filter(..)].ok_or_else(..)?` fails closed when unset.
        required = bool(re.match(
            r'\s*let \w+ = env::var_os\("\w+"\)(\s*\.filter\([^;]*?\))?\s*\.ok_or_else', window, re.S))
        rows.append((var, field, match.group(0).startswith("root"), local, required))
    return rows

def validate_map(body):
    grouped = {}
    for chunk in re.split(r"(?m)^        if ", body):
        found, match = msgs(chunk), re.search(r"self\s*\.\s*(\w+)", chunk)
        if found:
            grouped.setdefault(match.group(1) if match else "file_roots", []).extend(found)
    return grouped

def test_hits():
    hits = {}

    def add(var, label):
        hits.setdefault(var, [])
        if label not in hits[var]:
            hits[var].append(label)

    text = (ROOT / "tests/history_parity.py").read_text(encoding="utf-8")
    match = re.search(r"^def isolated_server\b.*?(?=\n\ndef |\Z)", text, re.M | re.S)
    body = match.group(0) if match else ""
    for var in IDENT.findall(body):
        add(var, "isolated_server")
    for name in re.findall(r'\("([A-Z][A-Z0-9_]+)"', body):
        add("SESSIONDOCK_" + name, "isolated_server")
    if "source.upper()" in body:
        for src in re.findall(r'"(claude|codex|grok)"', body):
            add(f"SESSIONDOCK_{src.upper()}_ROOT", "isolated_server")
    for path in sorted((ROOT / "crates/sessiondock/tests").glob("*.rs")):
        for var in IDENT.findall(path.read_text(encoding="utf-8")):
            add(var, path.name)
    return hits

def collect():
    """Rows of the node service (`sessiondock`, `Config`)."""
    src = CONFIG.read_text(encoding="utf-8")
    vmap = validate_map(section(src, "pub fn validate", "\n    pub(crate) fn validate_launcher"))
    vmap.setdefault("delivery_dir", []).extend(
        msgs(section(src, "fn validate_delivery_directory", "\n#[cfg(test)]")))
    return rows_of(src, "pub struct Config", vmap)

def collect_hub():
    """Rows of the hub (`sessiondock-hub`, `HubConfig`)."""
    src = HUB_CONFIG.read_text(encoding="utf-8")
    return rows_of(src, "pub struct HubConfig", validate_map(section(src, "pub fn validate", "\n#[cfg(test)]")))

def rows_of(src, struct, vmap):
    from_env = section(src, "pub fn from_env", "\n    pub fn validate")
    types = config_fields(section(src, struct, "\nimpl Default"))
    tests, occ = test_hits(), occurrences(from_env)
    docs = {var: [] for var, *_ in occ}
    for path in sorted(DOCS.glob("*.md")):
        if path.name != OUT.name:
            text = path.read_text(encoding="utf-8")
            for var in docs:
                if var in text:
                    docs[var].append(path.name)
    rows = []
    for var, field, uses_root, local, required in occ:
        doc = types.get(field.split(".")[0], ("", ""))[1] if "." not in field else ""
        typ = types.get(field, ("", ""))[0]
        req = "yes" if required else "no (Option)" if "." in field or "Option" in typ else "no (default)"
        parts, seen = ([doc.split(". ")[0].rstrip(".")] if doc else []), set()
        if uses_root:
            parts.append("must not be empty; must be a directory")
        for msg in local + vmap.get(field.split(".")[-1], []):
            compact = " ".join(msg.split())
            if compact not in seen:
                seen.add(compact)
                parts.append(compact)
        text = "; ".join(parts)
        rows.append({
            "var": var, "field": field, "required": req,
            "validation": text if len(text) <= 220 else text[:217].rstrip() + "...",
            "tests": ", ".join(tests.get(var, [])) or "no",
            "docs": ", ".join(f"[{n}]({n})" for n in docs[var]),
            "doc_files": docs[var],
        })
    return rows

def table(rows):
    lines = ["| Variable | Field | Required | Validation | Set by tests | Docs |",
             "| --- | --- | --- | --- | --- | --- |"]
    for row in rows:
        cells = [f"`{row['var']}`", f"`{row['field']}`", row["required"],
                 row["validation"], row["tests"], row["docs"] or "—"]
        lines.append("| " + " | ".join(c.replace("|", "\\|") for c in cells) + " |")
    return lines

def render(rows, hub_rows):
    lines = [INTRO.rstrip(), "", "## Node service (`sessiondock`, `Config::from_env`)", ""]
    lines += table(rows)
    lines += ["", "## Hub (`sessiondock-hub`, `HubConfig::from_env`)", ""]
    lines += table(hub_rows)
    return "\n".join(lines).rstrip() + "\n"

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="write docs/environment.md")
    parser.add_argument("--check", action="store_true", help="exit 1 if undocumented")
    args = parser.parse_args(argv)
    rows, hub_rows = collect(), collect_hub()
    missing = [row["var"] for row in rows + hub_rows if not row["doc_files"]]
    if args.check:
        for var in missing:
            print(f"used in config.rs/hub_config.rs but mentioned in no docs/*.md: {var}")
        code = 1 if missing else 0
    else:
        try:
            sys.stdout.write(render(rows, hub_rows))
        except BrokenPipeError:
            return 0
        code = 0
    if args.write:
        OUT.write_text(render(rows, hub_rows), encoding="utf-8")
    return code

if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        raise SystemExit(0)
