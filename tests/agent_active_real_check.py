#!/usr/bin/env python3
# run_validation: skip
"""Operator-run read-only check of agent_items[].active/created/updated and
continued_in against the Python adapters on the real CLI history roots.

Does not start a server. GET <rust>/api/sessions?force=1 and call
list_sessions() per source (this script never opens session files).
Per-agent ``active`` must match; ``created``/``updated`` equal as UTC
instants within ±1 s; row ``continued_in`` must match when both sides
list the uid. Requires --i-understand-this-reads-real-histories.
Never writes under the native roots.
"""
from __future__ import annotations

import argparse
import json
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, build_opener

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from provider_parity import adapter_module, load_adapters
from python_oracle import discover_source, package_dir

SOURCES = ("claude", "codex", "grok")
PYTHON_SOURCE = discover_source(Path(__file__).resolve().parents[1])
HOME_ROOTS = {
    "claude": Path.home() / ".claude" / "projects",
    "codex": Path.home() / ".codex" / "sessions",
    "grok": Path.home() / ".grok" / "sessions",
}
PLAN = ("Operator-run read-only agent_items.active/created/updated + continued_in "
        "parity vs Python adapters on the real CLI roots "
        "(~/.claude/projects, ~/.codex/sessions, ~/.grok/sessions). "
        "GET a running Rust /api/sessions?force=1; never open session files here; "
        "never writes under the roots. Pass --i-understand-this-reads-real-histories to run.")


def die(msg, code=1):
    print(msg, file=sys.stderr, flush=True)
    raise SystemExit(code)


def clip(v, n=96):
    t = v if isinstance(v, str) else json.dumps(v, ensure_ascii=False, default=str)
    return t if len(t) <= n else t[:n] + "…"


def parse_ts(v):
    if not v:
        return None
    try:
        return datetime.fromisoformat(str(v).replace("Z", "+00:00")).astimezone(timezone.utc)
    except (TypeError, ValueError):
        return None


def same_ts(a, b):
    if a == b:
        return True
    ta, tb = parse_ts(a), parse_ts(b)
    if ta is None or tb is None:
        return False
    return abs((ta - tb).total_seconds()) <= 1.0


def bind_adapters(python_source, roots, fixture_root):
    inst = load_adapters(python_source, fixture_root=fixture_root)
    mod = adapter_module(inst)
    for src, path in roots.items():
        setattr(mod, src.upper() + "_ROOT", path.resolve(strict=True))
    mod.media.register_path = lambda *a, **k: None
    for name, methods in (("claude", ("list_sessions",)), ("grok", ("list_sessions",)),
                          ("codex", ("list_sessions", "scan_sessions", "_find_session_path"))):
        cls = getattr(mod, name.title() + "Adapter")
        for method in methods:
            setattr(inst[name], method, getattr(cls, method).__get__(inst[name]))
    return inst


def pull(url):
    opener = build_opener(ProxyHandler({}))
    try:
        with opener.open(url, timeout=120) as resp:
            raw, code = resp.read(), resp.status
    except HTTPError as err:
        die(f"FAIL fetch: HTTP {err.code}: {err.read(4096)[:240]!r}")
    except (URLError, TimeoutError, OSError) as err:
        die(f"FAIL fetch: {url}: {err}")
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError as err:
        die(f"FAIL fetch: response is not JSON: {err}")
    if code != 200:
        die(f"FAIL fetch: HTTP {code}")
    if not isinstance(payload, dict) or not isinstance(payload.get("sessions"), list):
        die("FAIL fetch: expected {sessions: [...]} JSON")
    return payload["sessions"]


def continued(row):
    v = row.get("continued_in")
    return v if isinstance(v, str) and v else None


def agents(row):
    out = {}
    for item in row.get("agent_items") or []:
        if isinstance(item, dict) and item.get("id"):
            out[str(item["id"])] = item
    return out


def relevant(row):
    return bool(agents(row) or continued(row))


def emit(rep, kind, source, line):
    rep["counts"][kind] += 1
    bucket = rep["by_source"].setdefault(source, {"PASS": 0, "DIFF": 0})
    bucket[kind] += 1
    text = f"{kind} {line}"
    rep["lines"].append(text)
    print(text, flush=True)


def compare_one(uid, py, rs):
    notes = []
    pa, ra = agents(py), agents(rs)
    for aid in sorted(set(pa) | set(ra)):
        left, right = pa.get(aid), ra.get(aid)
        if not left or not right:
            notes.append(f"{uid} agent {aid} missing python={left is not None} rust={right is not None}")
            continue
        if left.get("active") != right.get("active"):
            notes.append(f"{uid} agent {aid} active python={left.get('active')!r} rust={right.get('active')!r}")
        for field in ("created", "updated"):
            if not same_ts(left.get(field), right.get(field)):
                notes.append(
                    f"{uid} agent {aid} {field} python={clip(left.get(field))} rust={clip(right.get(field))}")
    pc, rc = continued(py), continued(rs)
    if pc != rc:
        notes.append(f"{uid} continued_in python={clip(pc)} rust={clip(rc)}")
    return notes


def python_rows(adapters):
    out = {}
    for src in SOURCES:
        ad = adapters[src]
        # Python's index finalizes the raw per-file rows itself: Codex list_sessions()
        # already includes finalize_sessions(), Claude's does not (continued_in).
        rows = ad.list_sessions() or []
        if src == "claude" and hasattr(ad, "finalize_sessions"):
            rows = ad.finalize_sessions(rows)
        for row in rows:
            uid = row.get("uid")
            if uid:
                out[uid] = row
    return out


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust", default="http://127.0.0.1:8741")
    parser.add_argument("--python-source", type=Path, default=PYTHON_SOURCE)
    parser.add_argument("--json", type=Path, dest="json_out")
    parser.add_argument("--i-understand-this-reads-real-histories", action="store_true", dest="consent")
    args = parser.parse_args(argv)
    if not args.consent:
        print(PLAN, flush=True)
        for src, path in HOME_ROOTS.items():
            print(f"  {src}: {path}", flush=True)
        raise SystemExit(2)
    py_src = args.python_source if args.python_source.is_absolute() else Path.cwd() / args.python_source
    try:
        package_dir(py_src, ("adapters.py",))
    except (OSError, RuntimeError):
        die(f"FAIL python-source: missing adapters.py under {py_src}")
    roots = {src: path for src, path in HOME_ROOTS.items() if path.is_dir()}
    if not roots:
        die("FAIL roots: none of ~/.claude/projects, ~/.codex/sessions, ~/.grok/sessions exist")
    if args.json_out:
        dest = args.json_out.resolve()
        if any(dest.is_relative_to(path.resolve()) for path in roots.values()):
            die("FAIL json: --json must not be under a native root")
    url = args.rust.rstrip("/") + "/api/sessions?force=1"
    with tempfile.TemporaryDirectory(prefix="sessiondock-agent-active-") as tmp:
        fixture = Path(tmp)
        for name in SOURCES:
            (fixture / name).mkdir()
        adapters = bind_adapters(py_src, roots, fixture)
        py = python_rows(adapters)
    rust_list = pull(url)
    rs = {row.get("uid"): row for row in rust_list if isinstance(row, dict) and row.get("uid")}
    rep = {"counts": {"PASS": 0, "DIFF": 0},
           "by_source": {src: {"PASS": 0, "DIFF": 0} for src in SOURCES}, "lines": []}
    both = set(py) & set(rs)
    for uid in sorted(set(py) | set(rs)):
        a, b = py.get(uid), rs.get(uid)
        src = (a or b).get("source") or str(uid).split(":")[0]
        if uid not in both:
            if relevant(a or b):
                emit(rep, "DIFF", src, f"{uid} {'python' if a else 'rust'} only")
            continue
        if not relevant(a) and not relevant(b):
            continue
        notes = compare_one(uid, a, b)
        if notes:
            for note in notes:
                emit(rep, "DIFF", src, note)
        else:
            emit(rep, "PASS", src, f"{uid} agents={len(agents(a))} continued_in={continued(a) or '-'}")
    order = [src for src in SOURCES if src in rep["by_source"]]
    order += [src for src in rep["by_source"] if src not in SOURCES]
    for src in order:
        counts = rep["by_source"][src]
        print(f"{src}: {counts['PASS']} PASS, {counts['DIFF']} DIFF", flush=True)
    summary = f"SUMMARY {rep['counts']['PASS']} PASS, {rep['counts']['DIFF']} DIFF"
    print(summary, flush=True)
    if args.json_out:
        payload = {"rust": args.rust, "url": url, "python_source": str(py_src),
                   "roots": {key: str(path) for key, path in roots.items()},
                   "lines": rep["lines"], "counts": rep["counts"],
                   "by_source": rep["by_source"], "summary": summary}
        args.json_out.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
    raise SystemExit(1 if rep["counts"]["DIFF"] else 0)


if __name__ == "__main__":
    main()
