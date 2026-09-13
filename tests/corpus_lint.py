#!/usr/bin/env python3
"""Validate a synthetic native corpus before pointing the Rust server at it.

Walk ROOT/{claude,codex,grok} (never follow symlinks). Report per session
file: JSON validity (first bad line), records, bytes, longest line, LF tail,
expected first record (Claude sessionId/type, Codex session_meta, Grok
summary/chat pair) and session id. Flag 16 MiB/file, 2 MiB/line, 50000
records, 64 MiB total, 1000 sessions, 20000 dir entries, and symlinks.
CLI: corpus_lint.py ROOT [--json] [--strict]
"""
# run_validation: skip
import argparse
import json
import os
import sys
from pathlib import Path

SOURCES = ("claude", "codex", "grok")
FILE_MAX, LINE_MAX, TOTAL_MAX = 16 * 1024 * 1024, 2 * 1024 * 1024, 64 * 1024 * 1024
RECORDS_MAX, SESSIONS_MAX, ENTRIES_MAX = 50_000, 1000, 20_000
HEADS = ("source", "sid", "path", "records", "bytes", "longest", "json", "term", "first")


def rel(root, path):
    return str(path.relative_to(root)) if path.is_relative_to(root) else str(path)


def note(bucket, kind, path, detail=""):
    bucket.append({"kind": kind, "path": path, "detail": detail})


def walk(base):
    stack = [base]
    while stack:
        try:
            kids = list(os.scandir(stack.pop()))
        except OSError:
            continue
        for kid in kids:
            path = Path(kid.path)
            yield path, kid
            try:
                if not kid.is_symlink() and kid.is_dir(follow_symlinks=False):
                    stack.append(path)
            except OSError:
                pass


def inspect_jsonl(path):
    data = path.read_bytes()
    size, term, rec, longest, bad, first = len(data), (not data or data.endswith(b"\n")), 0, 0, None, None
    parts = data.split(b"\n")[:-1] if term and data else data.split(b"\n")
    for n, raw in enumerate(parts, 1):
        body = raw[:-1] if raw.endswith(b"\r") else raw
        longest = max(longest, len(body))
        if not body.strip():
            continue
        rec += 1
        try:
            obj = json.loads(body)
        except (json.JSONDecodeError, UnicodeDecodeError):
            bad = n if bad is None else bad
            continue
        if first is None and isinstance(obj, dict):
            first = obj
    return rec, size, longest, bad, term, first


def finish(rows, warnings, violations, source, path, rec, size, longest, bad, term, obj,
           folder=None, summary=None, pair=False):
    sid, ok = None, False
    if source == "claude" and isinstance(obj, dict):
        sid, ok = obj.get("sessionId"), "sessionId" in obj and "type" in obj
    elif source == "codex" and isinstance(obj, dict):
        ok = obj.get("type") == "session_meta"
        payload = obj.get("payload") if ok else None
        sid = (payload.get("id") or payload.get("session_id")) if isinstance(payload, dict) else None
    elif source == "grok":
        ok = pair
        info = summary.get("info") if isinstance(summary, dict) else None
        sid = info.get("id") if isinstance(info, dict) else None
        sid = sid or (None if folder is None else folder.name)
    rows.append({"source": source, "sid": sid or "-", "path": path, "records": rec,
                 "bytes": size, "longest": longest, "json": bad, "term": term, "first": ok})
    if bad is not None:
        note(warnings, "json-invalid", path, f"line {bad}")
    if not term:
        note(warnings, "unterminated", path)
    if not ok:
        note(warnings, "first-record", path)
    if rec > RECORDS_MAX:
        note(violations, "records", path, f"{rec}>{RECORDS_MAX}")
    if longest > LINE_MAX:
        note(violations, "line-size", path, f"{longest}>{LINE_MAX}")


def print_table(rows):
    fmt = {"json": lambda v: "ok" if v is None else f"L{v}",
           "term": lambda v: "yes" if v else "no",
           "first": lambda v: "ok" if v else "no"}
    cells = [HEADS] + [tuple(fmt[k](r[k]) if k in fmt else str(r[k]) for k in HEADS) for r in rows]
    widths = [max(len(r[i]) for r in cells) for i in range(len(HEADS))]
    for i, row in enumerate(cells):
        print("  ".join(c.rjust(w) if j in (3, 4, 5) else c.ljust(w)
                        for j, (c, w) in enumerate(zip(row, widths))))
        if not i:
            print("  ".join("-" * w for w in widths))


def lint(root):
    warnings, violations, rows = [], [], []
    entries = total_bytes = 0
    grok, jsonl = {}, []
    for source in SOURCES:
        base = root / source
        if base.is_symlink():
            note(violations, "symlink", rel(root, base))
            entries += 1
            continue
        if not base.is_dir():
            continue
        for path, kid in walk(base):
            entries += 1
            loc = rel(root, path)
            if kid.is_symlink():
                note(violations, "symlink", loc)
                continue
            try:
                is_file = kid.is_file(follow_symlinks=False)
                size = kid.stat(follow_symlinks=False).st_size if is_file else 0
            except OSError as exc:
                note(warnings, "stat", loc, str(exc))
                continue
            if not is_file:
                continue
            total_bytes += size
            if size > FILE_MAX:
                note(violations, "file-size", loc, f"{size}>{FILE_MAX}")
            if source == "grok" and path.name in ("summary.json", "chat_history.jsonl"):
                grok.setdefault(path.parent, {})[path.name] = path
            elif path.suffix == ".jsonl":
                jsonl.append((source, path))
    for source, path in sorted(jsonl, key=lambda t: t[1].as_posix()):
        finish(rows, warnings, violations, source, rel(root, path), *inspect_jsonl(path))
    for folder, parts in sorted(grok.items(), key=lambda t: t[0].as_posix()):
        chat, sp = parts.get("chat_history.jsonl"), parts.get("summary.json")
        summary = err = None
        if sp is not None:
            try:
                loaded = json.loads(sp.read_bytes())
                summary = loaded if isinstance(loaded, dict) else None
            except (json.JSONDecodeError, UnicodeDecodeError, OSError) as exc:
                err = str(exc)
        if err:
            note(warnings, "json-invalid", rel(root, sp), err)
        packed = inspect_jsonl(chat) if chat else (0, sp.stat().st_size if sp else 0, 0, None, True, None)
        finish(rows, warnings, violations, "grok", rel(root, chat or sp or folder), *packed,
               folder=folder, summary=summary, pair=bool(sp and chat))
    for kind, hit, limit in (("total-bytes", total_bytes, TOTAL_MAX),
                             ("sessions", len(rows), SESSIONS_MAX),
                             ("entries", entries, ENTRIES_MAX)):
        if hit > limit:
            note(violations, kind, str(root), f"{hit}>{limit}")
    rows.sort(key=lambda r: (SOURCES.index(r["source"]), r["path"]))
    totals = {"files": len(rows), "sessions": len(rows),
              "records": sum(r["records"] for r in rows), "bytes": total_bytes,
              "entries": entries, "longest": max((r["longest"] for r in rows), default=0),
              "symlinks": sum(1 for v in violations if v["kind"] == "symlink")}
    return rows, totals, warnings, violations


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--strict", action="store_true")
    args = parser.parse_args(argv)
    if not args.root.is_dir():
        print(f"{args.root} is not a directory", file=sys.stderr)
        return 1
    rows, totals, warnings, violations = lint(args.root)
    if args.json:
        json.dump({"root": str(args.root), "files": rows, "totals": totals,
                   "warnings": warnings, "violations": violations},
                  sys.stdout, ensure_ascii=False)
        sys.stdout.write("\n")
    else:
        print_table(rows)
        print()
        print("  ".join(f"{k}={v}" for k, v in totals.items()))
        for title, items in (("violations", violations), ("warnings", warnings)):
            print(f"{title}: {len(items)}")
            for item in items:
                extra = f"  {item['detail']}" if item["detail"] else ""
                print(f"  {item['kind']}  {item['path']}{extra}")
    return 1 if violations or (args.strict and warnings) else 0


if __name__ == "__main__":
    raise SystemExit(main())
