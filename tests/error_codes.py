#!/usr/bin/env python3
"""Catalog HTTP-layer errors from sessiondock constructors.

Prints Markdown; --json prints the model; --write writes docs/error-codes.md last.
"""
# run_validation: skip
import argparse, json, re, sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC, OUT = ROOT / "crates/sessiondock/src", ROOT / "docs/error-codes.md"
UNAVAIL = "Rust 后端尚未迁移此能力：{cap}。当前是只读开发阶段。"
FIXED = {"unsupported": (501, "file_mode_not_implemented",
                         "只读文件服务尚未实现此模式；不会伪造空任务或成功结果"),
         "changed": (409, "file_changed", "文件或父目录在读取期间已变化，请刷新后重试")}
STATUS = dict(BAD_REQUEST=400, FORBIDDEN=403, NOT_FOUND=404, CONFLICT=409, GONE=410,
              PAYLOAD_TOO_LARGE=413, URI_TOO_LONG=414, TOO_MANY_REQUESTS=429,
              INTERNAL_SERVER_ERROR=500, NOT_IMPLEMENTED=501, SERVICE_UNAVAILABLE=503)
PHRASE = {v: k.replace("_", " ").title() for k, v in STATUS.items()}
CALL_RE = re.compile(
    r"(ApiError|SessionError|FileError)::(new|unavailable|unsupported|changed)\s*\(")
SELF_RE, UNSUP_RE = re.compile(r"Self::new\s*\("), re.compile(r"\bunsupported\s*\(")
FN_RE = re.compile(r"(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(")
ROUTE_RE = re.compile(
    r'\.route\(\s*"([^"]+)"\s*,\s*(get|post|any)\s*\(\s*(?:(\w+)::)?(\w+)', re.S)
LIST_RE = re.compile(r"for path in \[([^\]]+)\]")
TRIPLE_RE = re.compile(
    r'\(\s*StatusCode::([A-Z_]+)\s*,\s*("(?:\\.|[^"\\])*")\s*,\s*("(?:\\.|[^"\\])*")\s*\)', re.S)
KIND_RE = re.compile(r"const\s+(\w+):\s*Kind\s*=\s*Kind\s*\{([^}]+)\}", re.S)
FIELD_RE, STR_RE = re.compile(r'(\w+)\s*:\s*("(?:\\.|[^"\\])*")'), re.compile(r'"(?:\\.|[^"\\])*"')
INTRO = (
    "# HTTP error codes\n\nThis file is produced by `tests/error_codes.py`. "
    "Handlers return JSON `{\"error\": \"<message>\", \"code\": \"<code>\"}`. "
    "Status **501** means the route or capability is declared not implemented "
    "in this migration stage. Session errors use `unsupported_history` at 501 "
    "and `session_error` otherwise. Regenerate:\n\n```sh\npython3 tests/error_codes.py --write\n```\n")

def close(text, i, opn="(", cls=")"):
    depth, n = 0, len(text)
    while i < n:
        c = text[i]
        if c in "\"'":
            q = c; i += 1
            while i < n and text[i] != q: i += 2 if text[i] == "\\" else 1
            i += 1; continue
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            i = text.find("\n", i); i = n if i < 0 else i + 1; continue
        depth += (c == opn) - (c == cls)
        if c == cls and depth == 0: return i
        i += 1
    return -1

def unquote(s): return s[1:-1].replace(r"\"", '"').replace(r"\\", "\\")

def impl_ty(text, pos):
    last = None
    for m in re.finditer(r"(?m)^\s*impl\b", text[:pos]): last = m.start()
    brace = text.find("{", last, pos) if last is not None else -1
    if brace < 0: return None
    head = re.sub(r"\s+", " ", text[last:brace])
    m = re.search(r"\bfor (\w+)$", head)
    return m.group(1) if m else (re.findall(r"\b([A-Z][A-Za-z0-9]*)\b", head) or [None])[-1]

def fn_meta(text, pos):
    found = list(FN_RE.finditer(text[:pos]))
    if not found: return "", ""
    brace = text.find("{", found[-1].end())
    end = close(text, brace, "{", "}") if brace >= 0 else -1
    return found[-1].group(1), text[found[-1].start():end + 1] if end >= 0 else ""

def msgs(inner, kinds):
    found = [unquote(s) for s in STR_RE.findall(inner)]
    if found: return found
    out = []
    for recv, field in re.findall(r"(\w+)\.(malformed|missing|foreign|expired)", inner):
        out.extend([kinds[recv][field]] if recv in kinds and kinds[recv].get(field)
                   else [k[field] for k in kinds.values() if k.get(field)])
    return out

def parse_routes(text):
    table = defaultdict(list)
    for path, method, module, handler in ROUTE_RE.findall(text):
        if handler != "not_implemented":
            table[(module or "mod", handler)].append(
                f"{method.upper()} /api{path}" if not path.startswith("/api") else f"{method.upper()} {path}")
    block = LIST_RE.search(text)
    if block:
        for path in re.findall(r'"([^"]+)"', block.group(1)):
            table[("mod", "not_implemented")].append(f"ANY /api{path}")
    table[("mod", "not_found")].append("ANY (fallback)")
    return table

def emit(rows, status, code, messages, rel, line, handler, table):
    if status is None or not code: return
    routes = list(table.get((Path(rel).stem, handler), [])) if rel.startswith("api/") else []
    rows.extend((status, code, m, rel, line, handler, routes) for m in (messages or [""]))

def scan_file(path, table, rows):
    rel, text = path.relative_to(SRC).as_posix(), path.read_text(encoding="utf-8")
    kinds = {n: {k: unquote(v) for k, v in FIELD_RE.findall(b)} for n, b in KIND_RE.findall(text)}
    hits = [(m.start(), m.group(1), m.group(2), m.end() - 1) for m in CALL_RE.finditer(text)]
    hits += [(m.start(), impl_ty(text, m.start()), "new", m.end() - 1) for m in SELF_RE.finditer(text)
             if impl_ty(text, m.start()) in ("ApiError", "FileError", "SessionError")]
    if "fn unsupported" in text:
        hits += [(m.start(), "SessionError", "helper", m.end() - 1) for m in UNSUP_RE.finditer(text)
                 if not (text[max(0, m.start() - 5):m.start()].endswith("fn ")
                         or text[max(0, m.start() - 5):m.start()].endswith("mark_"))]
    for pos, recv, meth, paren in hits:
        end = close(text, paren)
        if end < 0: continue
        inner, line = text[paren + 1:end], text[:pos].count("\n") + 1
        handler, body = fn_meta(text, pos)
        found = [unquote(s) for s in STR_RE.findall(inner)]
        if recv == "FileError" and meth in FIXED:
            emit(rows, *FIXED[meth][:2], [FIXED[meth][2]], rel, line, handler, table); continue
        if meth == "unavailable":
            emit(rows, 501, "not_implemented",
                 [UNAVAIL.format(cap=found[0] if found else "{capability}")],
                 rel, line, handler, table); continue
        if meth == "helper":
            emit(rows, 501, "unsupported_history", msgs(inner, kinds), rel, line, handler, table); continue
        m = re.match(r"\s*StatusCode::([A-Z_]+)", inner)
        st = STATUS.get(m.group(1)) if m else None
        if st is None:
            m = re.match(r"\s*(\d{3})\b", inner); st = int(m.group(1)) if m else None
        if recv == "SessionError":
            if st is not None:
                emit(rows, st, "unsupported_history" if st == 501 else "session_error",
                     msgs(inner, kinds), rel, line, handler, table)
            continue
        code = found[0] if found else None
        if st is None or code is None:
            for name, c, msg in TRIPLE_RE.findall(body):
                emit(rows, STATUS.get(name), unquote(c), [unquote(msg)], rel, line, handler, table)
            continue
        emit(rows, st, code, found[1:], rel, line, handler, table)

def collect():
    rows, table = [], parse_routes((SRC / "api/mod.rs").read_text(encoding="utf-8"))
    for path in sorted(p for p in SRC.rglob("*.rs") if p.is_file() and not (
            p.name.endswith("tests.rs") or "/tests/" in p.relative_to(SRC).as_posix())):
        scan_file(path, table, rows)
    grouped = {}
    for status, code, message, rel, line, handler, routes in rows:
        item = grouped.setdefault((status, code), {"status": status, "code": code,
                                                   "messages": [], "sites": []})
        if message and message not in item["messages"]:
            item["messages"].append(message)
        site = next((s for s in item["sites"] if s["file"] == rel and s["handler"] == handler), None)
        if site is None:
            item["sites"].append({"file": rel, "handler": handler, "lines": [line], "routes": routes})
        elif line not in site["lines"]: site["lines"].append(line)
    errors = [grouped[k] for k in sorted(grouped)]
    for item in errors:
        item["sites"].sort(key=lambda s: (s["file"], s["handler"]))
        for site in item["sites"]: site["lines"].sort()
    return errors

def where(site):
    link = f"[`{site['file']}`](../crates/sessiondock/src/{site['file']})"
    text = f"{link} `{site['handler']}` " + ", ".join(f"L{n}" for n in site["lines"])
    return text + (" → " + ", ".join(f"`{r}`" for r in site["routes"]) if site["routes"] else "")

def render(errors):
    parts, prev = [INTRO.rstrip(), "",
                   f"Scanned `{SRC.relative_to(ROOT).as_posix()}`: **{len(errors)}** "
                   f"(status, code) pairs.", ""], None
    for item in errors:
        if item["status"] != prev:
            parts += [f"## {item['status']} {PHRASE.get(item['status'], '')}".rstrip(), ""]
            prev = item["status"]
        notes = [m.replace("|", "\\|").replace("\n", " ") for m in item["messages"]] or ["(dynamic)"]
        body = ([f"- {notes[0]} — {where(item['sites'][0])}"] if len(notes) == 1 and len(item["sites"]) == 1
                else [f"- {m}" for m in notes] + [f"- {where(s)}" for s in item["sites"]])
        parts += [f"### `{item['code']}`", ""] + body + [""]
    return "\n".join(parts).rstrip() + "\n"

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--write", action="store_true")
    args, errors = parser.parse_args(argv), collect()
    markdown = render(errors)
    crate = "crates/sessiondock/src/"
    payload = {"shape": {"error": "<message>", "code": "<code>"}, "errors": [
        {"status": e["status"], "code": e["code"], "messages": e["messages"],
         "sites": [{"file": crate + s["file"], "handler": s["handler"],
                    "lines": s["lines"], "routes": s["routes"]} for s in e["sites"]]}
        for e in errors]}
    try:
        sys.stdout.write(json.dumps(payload, ensure_ascii=False, indent=2) + "\n"
                         if args.json else markdown)
    except BrokenPipeError: pass
    if args.write: OUT.write_text(markdown, encoding="utf-8")
    return 0

if __name__ == "__main__":
    try: raise SystemExit(main())
    except BrokenPipeError: raise SystemExit(0)
