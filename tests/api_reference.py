#!/usr/bin/env python3
"""Generate an HTTP API reference from the Rust router, handlers, and capabilities."""
# run_validation: skip
import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
API = ROOT / "crates/sessiondock/src/api"
STATE = ROOT / "crates/sessiondock/src/state.rs"
ROUTE_RE = re.compile(
    r'\.route\(\s*"([^"]+)"\s*,\s*(get|post|any)\s*\(\s*(?:(\w+)::)?(\w+)', re.S)
LIST_RE = re.compile(r"for path in \[([^\]]+)\]")
CAP_KV_RE = re.compile(r'"([^"]+)"\s*:\s*(true|false|"[^"]*"|-?\d+(?:\.\d+)?)')
EXTRACT_RE = re.compile(r"\b(Query|Json)<(\w+)>")
STRUCT_RE = re.compile(
    r"((?:#\[[^\]]+\]\s*)*)(?:pub(?:\([^)]+\))?\s+)?struct\s+(\w+)\s*\{", re.S)
FIELD_RE = re.compile(
    r"(?P<pre>(?:#\[[^\]]+\]\s*|///[^\n]*\n\s*|//[^\n]*\n\s*)*)"
    r"(?:pub(?:\([^)]+\))?\s+)?(?P<name>r#\w+|\w+)\s*:\s*"
    r"(?P<ty>[^=,\n{]+?)\s*(?:,|(?=\s*\}))")


def warn(message):
    print(f"warning: {message}", file=sys.stderr)


def rel(path):
    return path.relative_to(ROOT).as_posix() if path.is_relative_to(ROOT) else str(path)


def api_path(path):
    path = "/" + path.strip().lstrip("/")
    return path if path.startswith("/api") else "/api" + path


def close(text, i, opn="(", cls=")"):
    depth, n = 0, len(text)
    while i < n:
        char = text[i]
        if char in "\"'":
            quote, i = char, i + 1
            while i < n and text[i] != quote:
                i += 2 if text[i] == "\\" else 1
            i += 1
            continue
        if char == "/" and i + 1 < n and text[i + 1] == "/":
            i = text.find("\n", i)
            if i < 0:
                return -1
            i += 1
            continue
        depth += (char == opn) - (char == cls)
        if char == cls and depth == 0:
            return i
        i += 1
    return -1


def first_sentence(doc):
    text = " ".join(doc.split())
    idx = next((i for i, c in enumerate(text) if c == "." and (i + 1 == len(text) or text[i + 1].isspace())), None)
    return text[: idx + 1] if idx is not None else text


def read_text(path):
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        warn(f"cannot read {rel(path)}: {exc}")
        return ""


def parse_capabilities(text):
    start = text.find("pub fn capabilities(")
    brace = text.find("{", start) if start >= 0 else -1
    end = close(text, brace, "{", "}") if brace >= 0 else -1
    if end < 0:
        warn(f"capabilities() not found in {rel(STATE)}")
        return {}
    out = {}
    for key, raw in CAP_KV_RE.findall(text[brace:end]):
        if raw in ("true", "false"):
            out[key] = raw == "true"
        else:
            out[key] = raw[1:-1] if raw[:1] == '"' else (float(raw) if "." in raw else int(raw))
    return out


def parse_routes(text):
    routes = []
    for path, method, module, handler in ROUTE_RE.findall(text):
        if handler != "not_implemented":
            routes.append({
                "method": method.upper(), "route": api_path(path),
                "module": module or "mod", "handler": handler,
                "params": [], "notes": "", "not_implemented": False,
            })
    block = LIST_RE.search(text)
    if not block:
        warn(f"501 path list not found in {rel(API / 'mod.rs')}")
        return routes
    for path in re.findall(r'"([^"]+)"', block.group(1)):
        routes.append({
            "method": "ANY", "route": api_path(path), "module": "not_implemented",
            "handler": "not_implemented", "params": [],
            "notes": "not implemented (501)", "not_implemented": True,
        })
    return routes


def parse_structs(text):
    found = {}
    for match in STRUCT_RE.finditer(text):
        attrs, name = match.group(1), match.group(2)
        if "Deserialize" not in attrs:
            continue
        end = close(text, match.end() - 1, "{", "}")
        if end < 0:
            warn(f"unclosed struct {name}")
            continue
        default = bool(re.search(r"#\[serde\([^)]*\bdefault\b", attrs))
        fields = []
        for field in FIELD_RE.finditer(text[match.end():end]):
            raw = field.group("name")
            if raw in ("fn", "impl", "struct", "enum", "use", "mod"):
                continue
            ty = " ".join(field.group("ty").split())
            optional = default or ty.replace(" ", "").startswith("Option<") or bool(
                re.search(r"#\[serde\([^)]*\bdefault\b", field.group("pre")))
            fields.append({
                "name": raw[2:] if raw.startswith("r#") else raw,
                "type": ty, "optional": optional,
            })
        found[name] = {
            "name": name, "deny_unknown_fields": "deny_unknown_fields" in attrs, "fields": fields,
        }
    return found


def parse_handler(text, handler, source):
    pattern = re.compile(
        rf"(?P<doc>(?:[ \t]*///[^\n]*\n)*)[ \t]*(?:pub\s+)?async\s+fn\s+{re.escape(handler)}\s*\("
    )
    match = pattern.search(text)
    if not match:
        warn(f"{handler} not found in {rel(source)}")
        return "", []
    doc = " ".join(line.strip()[3:].strip() for line in match.group("doc").splitlines())
    end = close(text, match.end() - 1)
    if end < 0:
        warn(f"unclosed signature for {handler} in {rel(source)}")
        return doc, []
    names = list(dict.fromkeys(n for _, n in EXTRACT_RE.findall(text[match.end() - 1:end + 1])))
    return doc, names


def extract():
    router = read_text(API / "mod.rs")
    routes = parse_routes(router) if router else []
    cache, structs = {}, {}
    for route in routes:
        if route["not_implemented"]:
            continue
        module, handler = route["module"], route["handler"]
        source = API / ("mod.rs" if module == "mod" else f"{module}.rs")
        try:
            if str(source) not in cache:
                text = read_text(source)
                cache[str(source)] = (text, parse_structs(text))
            text, parsed = cache[str(source)]
            doc, names = parse_handler(text, handler, source)
        except Exception as exc:
            warn(f"{module}::{handler}: {exc}")
            doc, names, parsed = "", [], {}
        route["notes"], route["params"] = first_sentence(doc), names
        for name in names:
            info = parsed.get(name)
            if info is None:
                warn(f"{name} (Deserialize) not found in {rel(source)}")
            else:
                structs[f"{module}::{name}"] = {**info, "module": module}
    return {"capabilities": parse_capabilities(read_text(STATE)), "routes": routes, "structs": structs}


def table(headers, rows):
    head = ["| " + " | ".join(headers) + " |", "| " + " | ".join("---" for _ in headers) + " |"]
    return head + ["| " + " | ".join(row) + " |" for row in rows]


def render(model):
    cap = [
        [f"`{key}`", (str(value).lower() if isinstance(value, bool) else str(value)).replace("|", "\\|")]
        for key, value in model["capabilities"].items()
    ]
    lines = [
        "# API reference", "",
        "Generated from `crates/sessiondock/src/api` and `crates/sessiondock/src/state.rs`.",
        "", "## Capabilities", "",
    ] + table(["Flag", "Value"], cap)
    by_mod = {}
    for route in model["routes"]:
        by_mod.setdefault(route["module"], []).append(route)
    for module, group in by_mod.items():
        rows, used = [], []
        for route in group:
            params = ", ".join(f"`{name}`" for name in route["params"]) or "—"
            rows.append([route["method"], f"`{route['route']}`", params, route["notes"].replace("|", "\\|")])
            used.extend(name for name in route["params"] if name not in used)
        lines += ["", f"## {module}", ""] + table(["Method", "Route", "Params", "Notes"], rows)
        for name in used:
            info = model["structs"].get(f"{module}::{name}")
            if not info:
                continue
            extra = " (`deny_unknown_fields`)" if info.get("deny_unknown_fields") else ""
            fields = [
                [f"`{item['name']}`", f"`{item['type']}`", "optional" if item["optional"] else "required"]
                for item in info["fields"]
            ]
            lines += ["", f"### Params: {name}{extra}", ""] + table(["Field", "Type", "Required"], fields)
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="print the extracted model")
    args = parser.parse_args()
    model = extract()
    if args.json:
        json.dump(model, sys.stdout, ensure_ascii=False, indent=2)
        sys.stdout.write("\n")
        return
    sys.stdout.write(render(model))


if __name__ == "__main__":
    main()
