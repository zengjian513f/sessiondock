#!/usr/bin/env python3
"""Pretty-print isolated lifecycle and delivery ledgers without exposing secrets.

Reads only explicit --lifecycle/--delivery directories. Never discovers,
writes, or prints token/secret/credential/endpoint/socket/argv/env values.
SID/UID are masked to a source:hash prefix such as `codex:0123…`.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import stat
import sys
from pathlib import Path

SENSITIVE = ("token", "secret", "credential", "endpoint", "socket", "argv", "env")
IDENTITY = {"sid", "uid", "session_id"}
KINDS = {
    "lifecycle": ("lifecycle-ledger.json", ".lifecycle.lock", ".lifecycle-tmp-",
                  {1, 2, 3, 4}, "agenthub-lifecycle",
                  ("record_id", "state", "source", "launch", "sid/uid",
                   "instance", "created", "updated", "binding")),
    "delivery": ("delivery-ledger.json", ".delivery.lock", ".delivery-tmp-",
                 {1}, "agenthub-delivery",
                 ("request_id", "provider", "state", "revision", "epoch")),
}

def die(message, code=1):
    print(message, file=sys.stderr)
    raise SystemExit(code)
def obj(value):
    return value if isinstance(value, dict) else {}
def regular(path):
    try:
        return stat.S_ISREG(path.lstat().st_mode)
    except OSError:
        return False
def is_temp(name, prefix):
    rest = name[len(prefix):] if name.startswith(prefix) else ""
    return len(rest) == 32 and all(c in "0123456789abcdefABCDEF" for c in rest)
def collect_keys(value, found):
    if isinstance(value, dict):
        for key, child in value.items():
            found.add(key)
            collect_keys(child, found)
    elif isinstance(value, list):
        for child in value:
            collect_keys(child, found)
def mask_id(value, source=None):
    if not isinstance(value, str) or not value:
        return "-"
    if ":" in value:
        prefix, rest = value.split(":", 1)
        return f"{prefix}:{rest[:4]}…"
    return f"{source or 'id'}:{value[:4]}…"
def clip8(value):
    if not isinstance(value, str) or not value:
        return "-"
    return value[:8] + "…" if len(value) > 8 else value
def redact(value):
    if isinstance(value, dict):
        out = {}
        for key, child in value.items():
            lower = key.lower()
            if any(part in lower for part in SENSITIVE):
                out[key] = "<redacted>"
            elif key in IDENTITY and isinstance(child, str):
                out[key] = mask_id(child)
            elif key == "instance_id" and isinstance(child, str):
                out[key] = clip8(child)
            else:
                out[key] = redact(child)
        return out
    return [redact(child) for child in value] if isinstance(value, list) else value
def first(record, *names):
    for name in names:
        if record.get(name) is not None:
            return record[name]
    return "-"
def declared(record):
    spec = obj(record.get("spec"))
    launch, binding = obj(spec.get("launch")), obj(obj(record.get("binding")).get("spec"))
    source = spec.get("source")
    sid = launch.get("sid") or record.get("session_id") or binding.get("sid")
    uid = launch.get("uid") or binding.get("uid")
    parts = ([mask_id(sid, source)] if sid else []) + ([mask_id(uid, source)] if uid else [])
    return "/".join(parts) if parts else "-"
def lifecycle_rows(doc):
    rows = []
    for key, raw in obj(doc.get("records")).items():
        rec, spec, bind = obj(raw), obj(obj(raw).get("spec")), obj(raw).get("binding")
        rows.append((rec.get("record_id") or key, rec.get("state") or "-",
                     spec.get("source") or "-", obj(spec.get("launch")).get("kind") or "-",
                     declared(rec), clip8(rec.get("instance_id")),
                     first(rec, "created", "created_ms", "created_at"),
                     first(rec, "updated", "updated_ms", "updated_at"),
                     obj(bind).get("state", "-") if bind else "-"))
    return rows
def delivery_rows(doc):
    rows = []
    for provider in ("claude", "codex"):
        snap = obj(doc.get(provider))
        version = obj(snap.get("version"))
        epoch, revision = version.get("epoch", "-"), version.get("revision", "-")
        for key, raw in obj(snap.get("receipts")).items():
            rec, request = obj(raw), obj(obj(raw).get("request"))
            req_id = request.get("request_id") or request.get("id") or key
            rows.append((req_id, provider, rec.get("state") or "-",
                         rec.get("revision", revision), epoch))
    return rows
def print_table(title, headers, rows):
    print(title)
    if not rows:
        print("0 records")
        return
    widths, rendered = [len(h) for h in headers], []
    for row in rows:
        cells = ["-" if cell is None else str(cell) for cell in row]
        rendered.append(cells)
        for index, cell in enumerate(cells):
            widths[index] = max(widths[index], len(cell))
    fmt = "  ".join(f"{{:<{width}}}" for width in widths)
    print(fmt.format(*headers))
    for cells in rendered:
        print(fmt.format(*cells))
def load_ledger(directory, filename, lock, prefix):
    directory = Path(directory)
    if directory.is_symlink() or not directory.is_dir():
        die(f"not a real directory: {directory}")
    try:
        names = [entry.name for entry in directory.iterdir()]
    except OSError as err:
        die(f"cannot list {directory}: {err}")
    path = None
    for name in names:
        if name == lock or is_temp(name, prefix):
            continue
        if name == filename and regular(directory / name):
            path = directory / name
    if path is None:
        die(f"missing regular file {filename} in {directory}")
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeDecodeError) as err:
        die(f"cannot parse {path}: {err}")
    if not isinstance(payload, dict):
        die(f"{path} is not a JSON object")
    return path, payload
def dump(kind, directory, as_json, raw_keys, bucket):
    filename, lock, prefix, schemas, expected, headers = KINDS[kind]
    path, doc = load_ledger(directory, filename, lock, prefix)
    keys = set()
    collect_keys(doc, keys)
    schema, fmt = doc.get("schema"), doc.get("format")
    unknown = fmt != expected or schema not in schemas
    if unknown:
        print(f"warning: unknown {kind} schema version {schema!r} format={fmt!r}",
              file=sys.stderr)
    if raw_keys or unknown:
        ordered = sorted(keys)
        if as_json:
            bucket[kind] = ordered
        else:
            print(f"{kind} {path.name} schema={schema} keys:")
            print("\n".join(ordered) if ordered else "(no keys)")
        return
    if as_json:
        bucket[kind] = redact(doc)
        return
    extra = f"revision={doc.get('revision')}" if kind == "lifecycle" else (
        f"claude_rev={obj(obj(doc.get('claude')).get('version')).get('revision')}  "
        f"codex_rev={obj(obj(doc.get('codex')).get('version')).get('revision')}")
    rows_fn = lifecycle_rows if kind == "lifecycle" else delivery_rows
    print_table(f"{kind} {path.name}  schema={schema}  {extra}", headers, rows_fn(doc))
def main():
    parser = argparse.ArgumentParser(
        description="Pretty-print isolated development ledgers without secrets.")
    parser.add_argument("--lifecycle", metavar="DIR", help="lifecycle ledger directory")
    parser.add_argument("--delivery", metavar="DIR", help="delivery ledger directory")
    parser.add_argument("--json", dest="as_json", action="store_true",
                        help="print sanitized records as JSON")
    parser.add_argument("--raw-keys", action="store_true", help="list key names without values")
    args = parser.parse_args()
    if not args.lifecycle and not args.delivery:
        parser.error("specify --lifecycle DIR and/or --delivery DIR")
    bucket = {}
    if args.lifecycle:
        dump("lifecycle", args.lifecycle, args.as_json, args.raw_keys, bucket)
    if args.delivery:
        dump("delivery", args.delivery, args.as_json, args.raw_keys, bucket)
    if args.as_json:
        json.dump(bucket, sys.stdout, ensure_ascii=False, indent=2)
        sys.stdout.write("\n")

if __name__ == "__main__":
    main()
