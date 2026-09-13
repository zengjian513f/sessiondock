#!/usr/bin/env python3
"""Find legacy-web api/ calls that 501/miss on Rust or are gated incorrectly."""
# run_validation: skip
from __future__ import annotations

import argparse, json, re, sys
from route_ledger import LEGACY, ROOT, ROUTER, UID_EXPR, api_path, loose_match, rust_inventory, skip_expr

STATE, LIB = ROOT / "crates/sessiondock/src/state.rs", ROOT / "crates/sessiondock/src/lib.rs"
FUNC_RE = re.compile(r"^\s*(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(")
ALLOWS_RE = re.compile(r"SessionDockCapabilities\.allows\(\s*['\"]([A-Za-z_]\w*)['\"]\s*\)")
CFG_TRUE_RE = re.compile(r"SessionDockCapabilities\.config\.([A-Za-z_]\w*)\s*===\s*true")
BACKEND_RE = re.compile(r"SessionDockCapabilities\.config\.backend\s*(?:===|!==|==|!=)\s*['\"]rust['\"]")
HELPER_DEF_RE = re.compile(r"function\s+(\w+Enabled)\s*\(")
CAP_BODY_RE = re.compile(r"pub fn capabilities\(\) -> Value \{(.+?)^\}", re.M | re.S)
ASSIGN_RE = re.compile(r'capabilities\["([a-z_]+)"\]\s*=')


def parse_funcs(text):
    lines, depth, stack, spans, pending = text.splitlines(), 0, [], [], None
    for i, raw in enumerate(lines, 1):
        code = raw.split("//", 1)[0]
        if (m := FUNC_RE.match(raw)):
            pending = (m.group(1), i, depth)
        opens, closes = code.count("{"), code.count("}")
        if pending and opens:
            stack.append(pending); pending = None
        depth += opens - closes
        while stack and depth <= stack[-1][2]:
            name, start, _ = stack.pop(); spans.append((name, start, i))
    spans.extend((n, s, len(lines)) for n, s, _ in stack)
    return spans


def enclosing(spans, line):
    hit = [s for s in spans if s[1] <= line <= s[2]]
    return max(hit, key=lambda s: s[1]) if hit else ("<top>", 0, 0)


def region(lines, spans, name, line):
    if name != "<top>":
        for n, start, end in spans:
            if n == name and start <= line <= end:
                return "\n".join(lines[start - 1:end])
        return ""
    prev = max((s[2] for s in spans if s[2] < line), default=0)
    nxt = min((s[1] for s in spans if s[1] > line), default=len(lines) + 1)
    return "\n".join(lines[prev:nxt - 1])


def extract_calls(text):
    out, index, n = [], 0, len(text)
    while index < n:
        quote = text[index]
        if quote in "'\"`" and text.startswith("api/", index + 1):
            line = text.count("\n", 0, index) + 1
            index += 1
            buf = []
            while index < n and text[index] not in {quote, "?"}:
                if text.startswith(UID_EXPR, index):
                    buf.append("{uid}"); index += len(UID_EXPR); continue
                if text.startswith("${", index):
                    buf.append("{param}"); index = skip_expr(text, index + 1); continue
                buf.append(text[index]); index += 1
            path = api_path("".join(buf))
            if path not in {"", "/api"}:
                out.append((line, path))
        index += 1
    return out


def rust_status(path, inventory):
    if path in inventory:
        return inventory[path]
    return next((s for k, s in inventory.items() if loose_match(path, k)), "missing")


def declared_caps():
    state = STATE.read_text(encoding="utf-8") if STATE.is_file() else ""
    lib = LIB.read_text(encoding="utf-8") if LIB.is_file() else ""
    blob = (m.group(1) if (m := CAP_BODY_RE.search(state)) else state)
    declared = set(re.findall(r'"([A-Za-z_]\w*)"\s*:', blob))
    falses = set(re.findall(r'"([A-Za-z_]\w*)"\s*:\s*false', blob))
    trues = set(re.findall(r'"([A-Za-z_]\w*)"\s*:\s*true', blob))
    for key in ASSIGN_RE.findall(lib):
        declared.add(key)
        if key not in trues:
            falses.add(key)
    return declared, falses


def helpers_in(files):
    found = {}
    for rec in files.values():
        for m in HELPER_DEF_RE.finditer(rec["text"]):
            name, line = m.group(1), rec["text"].count("\n", 0, m.start()) + 1
            body = region(rec["lines"], rec["spans"], name, line)
            flags = set(ALLOWS_RE.findall(body)) | set(CFG_TRUE_RE.findall(body))
            if flags or BACKEND_RE.search(body):
                found[name] = flags
    return found


def scan_gates(text, helpers):
    flags = set(ALLOWS_RE.findall(text)) | set(CFG_TRUE_RE.findall(text))
    used = [name for name in helpers if re.search(rf"\b{name}\s*\(", text)]
    for name in used:
        flags |= helpers[name]
    return flags, bool(BACKEND_RE.search(text)), used


def callers_of(name, file_name, start, files):
    if name == "<top>":
        return []
    pat, bodies = re.compile(rf"\b{re.escape(name)}\s*\("), []
    for rec in files.values():
        for m in pat.finditer(rec["text"]):
            line = rec["text"].count("\n", 0, m.start()) + 1
            if rec["name"] == file_name and line == start:
                continue
            enc, _, _ = enclosing(rec["spans"], line)
            if enc != name:
                bodies.append(region(rec["lines"], rec["spans"], enc, line))
    return bodies


def build():
    inventory, (declared, falses), files = rust_inventory(ROUTER.read_text(encoding="utf-8")), declared_caps(), {}
    for source in sorted(LEGACY.glob("*.js")):
        text = source.read_text(encoding="utf-8")
        files[source.name] = {"name": source.name, "text": text, "lines": text.splitlines(),
                              "spans": parse_funcs(text)}
    helpers, findings = helpers_in(files), {"A": [], "B": [], "C": []}
    for rec in files.values():
        for line, path in extract_calls(rec["text"]):
            fn, start, _ = enclosing(rec["spans"], line)
            begin = start - 1 if fn != "<top>" else max((s[2] for s in rec["spans"] if s[2] < line), default=0)
            prefix = "\n".join(rec["lines"][begin:line])
            flags, backend, used = scan_gates(prefix, helpers)
            near = scan_gates("\n".join(callers_of(fn, rec["name"], start, files)), helpers)
            gated = bool(flags or backend or used or near[0] or near[1] or near[2])
            status = rust_status(path, inventory)
            labels = []
            if flags - declared:
                labels.append("B")
            if status in {"501", "missing"} and not gated:
                labels.append("A")
            if status == "implemented" and flags & falses:
                labels.append("C")
            if not labels:
                continue
            allows = set(ALLOWS_RE.findall(prefix))
            gate = ", ".join([f"allows({f})" if f in allows else f"config.{f}" for f in sorted(flags)]
                             + (["backend"] if backend else []) + [f"{h}()" for h in used]) or "-"
            row = {"file": f"{rec['name']}:{line}", "function": fn, "route": path,
                   "rust": status, "gate": gate, "finding": ", ".join(labels)}
            for label in labels:
                findings[label].append(row)
    return {"findings": findings}


def render(data):
    lines = ["| File:line | Function | Route | Rust | Gate | Finding |",
             "| --- | --- | --- | --- | --- | --- |"]
    seen = set()
    for label in ("A", "B", "C"):
        for row in data["findings"][label]:
            key = (row["file"], row["finding"])
            if key in seen:
                continue
            seen.add(key)
            lines.append(f"| {row['file']} | {row['function']} | {row['route']} | "
                         f"{row['rust']} | {row['gate']} | {row['finding']} |")
    if len(lines) == 2:
        lines.append("| - | - | - | - | - | - |")
    c = {k: len(v) for k, v in data["findings"].items()}
    lines += ["", f"A (ungated 501/missing): {c['A']}; B (undeclared gate): {c['B']}; "
              f"C (implemented, false-default gate): {c['C']}"]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Legacy api/ call gating vs Rust routes.")
    parser.add_argument("--json", action="store_true", help="print the same report as JSON")
    args = parser.parse_args()
    try:
        data = build()
        if args.json:
            json.dump(data, sys.stdout, ensure_ascii=False, indent=2); sys.stdout.write("\n")
        else:
            print(render(data))
    except Exception as exc:  # noqa: BLE001 — report must still exit 0
        print(exc, file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
