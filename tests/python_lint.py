#!/usr/bin/env python3
"""Static checks for tests/*.py: py_compile; unused imports (skip
__future__/__all__), unused locals (once, skip _*), unused module
functions (hint), bare except, unflushed print in isolated_server files.
Suites need --binary if they import isolated_server, and a PASS literal.
CLI: python_lint.py [--json] [--strict]
"""
# run_validation: skip
import argparse
import ast
import json
import py_compile
import re
import sys
from pathlib import Path

TESTS = Path(__file__).resolve().parent
SKIP = "# run_validation: skip"
KINDS = ("syntax", "unused-import", "unused-local", "unused-function",
         "bare-except", "print-flush", "suite-binary", "suite-pass")
FATAL = {"syntax", "bare-except"}


def note(out, kind, path, line, msg=""):
    out.append({"kind": kind, "file": f"tests/{path.name}", "line": line, "message": msg})


def unused_locals(fn):
    skip, used, assigned, nested = set(), set(), {}, set()
    for node in ast.walk(fn):
        if node is not fn and isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)):
            nested.update(id(item) for item in ast.walk(node))
    for node in ast.walk(fn):
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load):
            used.add(node.id)
        if id(node) in nested:
            continue
        if isinstance(node, (ast.Global, ast.Nonlocal)):
            skip.update(node.names)
        elif isinstance(node, ast.AugAssign) and isinstance(node.target, ast.Name):
            used.add(node.target.id)
        elif isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store):
            assigned.setdefault(node.id, []).append(node.lineno)
        elif isinstance(node, ast.ExceptHandler) and node.name:
            assigned.setdefault(node.name, []).append(node.lineno)
        elif isinstance(node, ast.arg):
            assigned.setdefault(node.arg, []).append(fn.lineno)
        elif isinstance(node, ast.NamedExpr) and isinstance(node.target, ast.Name):
            assigned.setdefault(node.target.id, []).append(node.lineno)
    return [
        (name, lines[0]) for name, lines in assigned.items()
        if len(lines) == 1 and not name.startswith("_")
        and name not in used and name not in skip and name not in ("self", "cls")
    ]


def analyze(path, text, tree, out):
    loaded, exported, aliases, froms, imps = set(), set(), {}, [], []
    isolated = has_bin = False
    for node in ast.walk(tree):
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load):
            loaded.add(node.id)
        elif isinstance(node, ast.Assign) and any(
            isinstance(t, ast.Name) and t.id == "__all__" for t in node.targets
        ):
            elts = node.value.elts if isinstance(node.value, (ast.List, ast.Tuple)) else []
            exported.update(e.value for e in elts if isinstance(e, ast.Constant) and isinstance(e.value, str))
        elif isinstance(node, ast.Import):
            for alias in node.names:
                bound = alias.asname or alias.name.split(".")[0]
                aliases[bound] = alias.name.split(".")[0]
                imps.append((bound, node.lineno))
        elif isinstance(node, ast.ImportFrom) and node.module != "__future__":
            mod = (node.module or "").split(".")[0]
            for alias in node.names:
                if alias.name == "*":
                    continue
                bound = alias.asname or alias.name
                froms.append((mod, alias.name))
                imps.append((bound, node.lineno))
                if alias.name == "isolated_server" or alias.asname == "isolated_server":
                    isolated = True
        elif isinstance(node, ast.ExceptHandler) and node.type is None:
            note(out, "bare-except", path, node.lineno, "except:")
        elif isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == "print":
            flushed = any(
                kw.arg == "flush" and isinstance(kw.value, ast.Constant) and kw.value.value
                for kw in node.keywords
            )
            if not flushed and "isolated_server" in text:
                note(out, "print-flush", path, node.lineno, "print without flush=True")
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            for name, line in unused_locals(node):
                note(out, "unused-local", path, line, name)
        elif isinstance(node, ast.Call) and getattr(node.func, "attr", None) == "add_argument":
            has_bin = has_bin or any(isinstance(a, ast.Constant) and a.value == "--binary" for a in node.args)
    for node in ast.walk(tree):
        if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name):
            mod = aliases.get(node.value.id)
            if mod:
                froms.append((mod, node.attr))
    for bound, line in imps:
        if bound not in loaded and bound not in exported:
            note(out, "unused-import", path, line, bound)
    suite = SKIP not in text.splitlines()[:40]
    if suite:
        if isolated and not has_bin:
            note(out, "suite-binary", path, 1, "imports isolated_server but no argparse --binary")
        if not any(isinstance(n, ast.Constant) and isinstance(n.value, str) and "PASS" in n.value for n in ast.walk(tree)):
            note(out, "suite-pass", path, 1, "no PASS literal")
    funcs = [(n.name, n.lineno) for n in tree.body if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))]
    return loaded | exported, froms, funcs


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--strict", action="store_true", help="exit 1 on any finding")
    args = parser.parse_args()
    findings, imported, defined, local = [], {}, [], {}
    for path in sorted(TESTS.glob("*.py")):
        text = path.read_text(encoding="utf-8")
        try:
            py_compile.compile(str(path), doraise=True)
            tree = ast.parse(text, filename=str(path))
        except (py_compile.PyCompileError, SyntaxError) as exc:
            msg = str(getattr(exc, "msg", None) or exc).strip()
            match = re.search(r"line (\d+)", str(exc))
            line = getattr(exc, "lineno", None) or (int(match.group(1)) if match else 1)
            note(findings, "syntax", path, line or 1, msg)
            continue
        used, froms, funcs = analyze(path, text, tree, findings)
        local[path] = used
        defined.extend((path, path.stem, name, line) for name, line in funcs)
        for mod, name in froms:
            imported.setdefault(mod, set()).add(name)
    for path, stem, name, line in defined:
        if name not in local.get(path, ()) and name not in imported.get(stem, ()):
            note(findings, "unused-function", path, line, f"{name} (hint)")
    if args.json:
        json.dump({"findings": findings}, sys.stdout, ensure_ascii=False)
        sys.stdout.write("\n")
    elif not findings:
        print("0 findings", flush=True)
    else:
        for kind in KINDS:
            rows = [i for i in findings if i["kind"] == kind]
            if not rows:
                continue
            print(kind, flush=True)
            for item in rows:
                extra = f" {item['message']}" if item["message"] else ""
                print(f"  {item['file']}:{item['line']}{extra}", flush=True)
    return 1 if any(f["kind"] in FATAL for f in findings) or (args.strict and findings) else 0


if __name__ == "__main__":
    raise SystemExit(main())
