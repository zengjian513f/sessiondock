#!/usr/bin/env python3
"""Authoritative Python-vs-Rust HTTP route gap list for replacing the Python backend."""
# run_validation: skip
from __future__ import annotations

import argparse, ast, json, re, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from route_ledger import ROUTER, api_path, loose_match, rust_inventory

PY = Path(__file__).resolve().parents[2] / "sessiondock" / "sessiondock"
SKIP = {"_json", "_send", "_audit_begin", "_audit_body", "_allowed", "_static", "read_body"}
PREFIX = {"/api/media": "{token}", "/api/messages": "{uid}", "/api/session": "{uid}"}
RUST_RE = re.compile(r'\.route\(\s*"([^"]+)"\s*,\s*(get|post|put|delete|patch|any)\s*\(')
STATUSES = ("implemented", "501 (declared)", "missing (not even declared)", "hub-only")


def read(path):
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return ""


def cstr(node):
    return node.value if isinstance(node, ast.Constant) and isinstance(node.value, str) else None


def calln(node):
    func = getattr(node, "func", None)
    if isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name) and func.value.id == "self":
        return func.attr
    return None


def handler(body):
    for stmt in body:
        if isinstance(stmt, ast.Try):
            name = handler(stmt.body)
            if name != "?":
                return name
        value = stmt.value if isinstance(stmt, (ast.Return, ast.Expr, ast.Assign)) else None
        name = calln(value)
        if name and name not in SKIP:
            return name
    return "?"


def norm(raw, kind):
    if kind == "start":
        if not raw.rstrip("/").split("/")[2:]:
            return None
        raw = raw.rstrip("/")
        raw = f"{raw}/{PREFIX.get(raw, '{param}')}"
    elif kind == "re":
        raw = re.sub(r"\(\[a-f0-9\]\{32\}\)", "{nid}", raw)
        raw = re.sub(r"\(/api/\.\*\)", "/api/{path}", raw)
        raw = re.sub(r"\([^)]+\)", "{param}", raw).replace("\\", "")
    return api_path(raw)


def is_path(node):
    return (isinstance(node, ast.Name) and node.id == "path") or (
        isinstance(node, ast.Attribute) and node.attr == "path")


def analyze(test, aliases):
    methods, paths, negated = set(), [], False

    def walk(node):
        nonlocal negated
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
            negated = True
            walk(node.operand)
        elif isinstance(node, ast.BoolOp):
            for value in node.values:
                walk(value)
        elif isinstance(node, ast.Name) and node.id in aliases:
            paths.append((aliases[node.id], "re"))
        elif isinstance(node, ast.Compare):
            op, right = node.ops[0], node.comparators[0]
            if isinstance(op, ast.Eq) and is_path(node.left):
                value = cstr(right)
                if value and value.startswith("/api"):
                    paths.append((value, "eq"))
            elif isinstance(op, ast.Eq) and isinstance(node.left, ast.Attribute) and node.left.attr == "command":
                methods.add((cstr(right) or "").upper())
            elif isinstance(op, ast.In) and is_path(node.left) and hasattr(right, "elts"):
                for item in right.elts:
                    value = cstr(item)
                    if value and value.startswith("/api"):
                        paths.append((value, "eq"))
        elif isinstance(node, ast.Call):
            name = node.func.attr if isinstance(node.func, ast.Attribute) else getattr(node.func, "id", "")
            arg = cstr(node.args[0]) if node.args else None
            if name == "startswith" and arg:
                paths.append((arg, "start"))
            elif name == "fullmatch" and arg:
                paths.append((arg, "re"))

    walk(test)
    return methods, paths, negated


def walk_stmts(stmts, method, aliases, rows):
    for index, stmt in enumerate(stmts):
        if isinstance(stmt, ast.Assign) and isinstance(stmt.value, ast.Call):
            func, targets = stmt.value.func, stmt.targets
            if isinstance(func, ast.Attribute) and func.attr == "fullmatch" and stmt.value.args:
                pat = cstr(stmt.value.args[0])
                if pat and isinstance(targets[0], ast.Name):
                    aliases[targets[0].id] = pat
        elif isinstance(stmt, ast.If):
            methods, paths, negated = analyze(stmt.test, aliases)
            used = next(iter(methods), method) if methods else method
            nested = []
            walk_stmts(stmt.body, used, aliases, nested)
            seen = {item[1] for item in nested}
            rows.extend(nested)
            name = handler(stmts[index + 1:] if negated else stmt.body)
            for raw, kind in paths:
                path = norm(raw, kind)
                if path and path not in seen:
                    rows.append((used or "*", path, name))
                    seen.add(path)
            walk_stmts(stmt.orelse, method, aliases, rows)
        elif isinstance(stmt, ast.Try):
            for block in (stmt.body, stmt.orelse, stmt.finalbody, *[h.body for h in stmt.handlers]):
                walk_stmts(block, method, aliases, rows)


def parse_py(text, class_name, names):
    rows = []
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return rows
    for node in tree.body:
        if not isinstance(node, ast.ClassDef) or node.name != class_name:
            continue
        for item in node.body:
            if isinstance(item, ast.FunctionDef) and item.name in names:
                walk_stmts(item.body, names[item.name], {}, rows)
    return rows


def hit(path, rust):
    if path in rust:
        return path
    return next((other for other in rust if loose_match(path, other)), None)


def build():
    node = parse_py(read(PY / "server.py"), "Handler",
                    {"do_GET": "GET", "do_POST": "POST", "do_DELETE": "DELETE", "_api_get": "GET"})
    hub = parse_py(read(PY / "hub.py"), "HubHandler", {"dispatch": "*"})
    seen, node_paths = {}, {path for _, path, _ in node}
    for method, path, name in node:
        seen[(method, path)] = (method, path, name, False)
    for method, path, name in hub:
        if path in node_paths or any(loose_match(path, other) for other in node_paths):
            continue
        seen.setdefault((method, path), (method, path, name, True))
    text = read(ROUTER)
    rust = rust_inventory(text)
    rmethod = {api_path(path): method.upper() for path, method in RUST_RE.findall(text)}
    routes = []
    for method, path, name, hub_only in sorted(seen.values(), key=lambda row: (row[0], row[1])):
        found = None if hub_only else hit(path, rust)
        if hub_only:
            status, notes = "hub-only", "hub.py only"
        elif found is None:
            status, notes = "missing (not even declared)", "no rust route"
        elif rust[found] == "501":
            status, notes = "501 (declared)", f"any() stub ({rmethod.get(found, 'ANY')})"
        else:
            status, notes = "implemented", "-"
        routes.append({"method": method, "python": path, "handler": name,
                       "rust": status, "notes": notes})
    rust_only = []
    for path, status in sorted(rust.items()):
        if any(path == row["python"] or loose_match(path, row["python"]) for row in routes):
            continue
        rust_only.append({"method": rmethod.get(path, "?"), "route": path,
                          "rust": "501 (declared)" if status == "501" else "implemented"})
    counts = {key: sum(row["rust"] == key for row in routes) for key in STATUSES}
    counts.update(python=len(routes), rust_only=len(rust_only))
    return {"routes": routes, "counts": counts, "rust_only": rust_only}


def render(data):
    lines = ["| Method | Python route | Handler | Rust status | Notes |",
             "| --- | --- | --- | --- | --- |"]
    for row in data["routes"]:
        lines.append(f"| {row['method']} | {row['python']} | {row['handler']} | {row['rust']} | {row['notes']} |")
    lines += ["", *[f"{key}: {value}" for key, value in data["counts"].items()], "", "Rust-only routes:"]
    lines += [f"- {row['method']} `{row['route']}` ({row['rust']})" for row in data["rust_only"]] or ["- none"]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Python vs Rust HTTP route gap list.")
    parser.add_argument("--json", action="store_true", help="print the same report as JSON")
    args = parser.parse_args()
    try:
        data = build()
        if args.json:
            json.dump(data, sys.stdout, ensure_ascii=False, indent=2)
            sys.stdout.write("\n")
        else:
            print(render(data))
    except Exception as exc:  # noqa: BLE001 — report must still exit 0
        print(exc, file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
