#!/usr/bin/env python3
"""Turn append/native/envelope JSONL benchmarks into Markdown comparison tables.

compare prints ordinary-history p50/p95 and RSS/HWM tables; single prints the
native/envelope tables from docs/native-input.md. --json emits numbers.
Malformed lines are skipped (stderr count); missing sizes/providers are n/a.
"""
# run_validation: skip
import argparse
import json
from pathlib import Path
import sys

MIB = 1024 * 1024
NA = "n/a"
PREFERRED = ("claude", "codex", "grok")
APPEND = (
    ("first_window", "First window", "first_window"),
    ("append_one", "Append", "append"),
    ("same_length_rewrite", "Rewrite", "rewrite"),
)
NATIVE = (("first_window", "First window"), ("cold_get", "Cold GET"), ("warm_get", "Warm GET"))
MEM_PHASES = ("startup", "first_window", "cold_get", "warm_get")
MEM_LABELS = ("Startup", "First window", "Cold GET", "Warm GET")


def load_jsonl(path):
    rows, skipped = [], 0
    with path.open(encoding="utf-8") as handle:
        for raw in handle:
            text = raw.strip()
            if not text:
                continue
            try:
                item = json.loads(text)
            except json.JSONDecodeError:
                skipped += 1
                continue
            if isinstance(item, dict) and "kind" in item:
                rows.append(item)
            else:
                skipped += 1
    return rows, skipped


def note_skipped(count):
    if count:
        print(f"skipped {count} malformed line(s)", file=sys.stderr)


def nested(row, *keys):
    cur = row or {}
    for key in keys:
        if not isinstance(cur, dict):
            return None
        cur = cur.get(key)
    return cur


def ms_of(row, phase):
    stats = nested(row, "measurements", phase)
    if not isinstance(stats, dict):
        return None
    p50, p95 = stats.get("p50_ms"), stats.get("p95_ms")
    return (p50, p95) if isinstance(p50, (int, float)) and isinstance(p95, (int, float)) else None


def mem_of(row, phase, field):
    value = nested(row, "memory_p50_bytes", phase, field)
    return None if not isinstance(value, (int, float)) else value / MIB


def fmt_ms(pair):
    return NA if pair is None else f"{pair[0]:.3f} ({pair[1]:.3f})"


def fmt_mib(rss, hwm, digits):
    return NA if rss is None or hwm is None else f"{rss:.{digits}f}/{hwm:.{digits}f}"


def fmt_pct(value):
    return NA if value is None else (f"+{value}%" if value > 0 else f"{value}%")


def pct(old, new):
    return None if old is None or new is None or old[0] == 0 else round((new[0] - old[0]) / old[0] * 100)


def pct_range(values):
    return NA if not values else f"{fmt_pct(min(values))}…{fmt_pct(max(values))}"


def display_name(source):
    return source[:1].upper() + source[1:] if source else NA


def rnd(value, digits):
    return None if value is None else round(value, digits)


def ordered_sources(*maps):
    seen = []
    for mapping in maps:
        for key in mapping:
            if key not in seen:
                seen.append(key)
    return [n for n in PREFERRED if n in seen] + sorted(n for n in seen if n not in PREFERRED)


def md_table(headers, body):
    lines = ["| " + " | ".join(headers) + " |", "| " + " | ".join("---" for _ in headers) + " |"]
    lines.extend("| " + " | ".join(row) + " |" for row in body)
    return "\n".join(lines)


def emit_json(payload):
    json.dump(payload, sys.stdout, ensure_ascii=False)
    sys.stdout.write("\n")


def index_append(rows):
    mapping = {}
    for row in rows:
        if row.get("kind") != "summary" or row.get("records") is None or not row.get("source"):
            continue
        mapping.setdefault(row["records"], {})[row["source"]] = row
    return mapping


def ms_entry(old_ms, new_ms, change):
    return {
        "old_p50_ms": None if old_ms is None else old_ms[0],
        "old_p95_ms": None if old_ms is None else old_ms[1],
        "new_p50_ms": None if new_ms is None else new_ms[0],
        "new_p95_ms": None if new_ms is None else new_ms[1],
        "p50_change_pct": change,
    }


def compare_size(records, old_map, new_map, label_old, label_new):
    body, mem_lines, providers = [], [], []
    changes = {phase: [] for phase, _, _ in APPEND}
    for source in ordered_sources(old_map, new_map):
        old_row, new_row = old_map.get(source), new_map.get(source)
        name = display_name(source)
        cells, entry = [name], {"provider": name, "source": source}
        for phase, _label, key in APPEND:
            old_ms, new_ms = ms_of(old_row, phase), ms_of(new_row, phase)
            cells.append(f"{fmt_ms(old_ms)} → {fmt_ms(new_ms)}")
            change = pct(old_ms, new_ms)
            if change is not None:
                changes[phase].append(change)
            entry[key] = ms_entry(old_ms, new_ms, change)
        old_rss, new_rss = mem_of(old_row, "first_window", "VmRSS"), mem_of(new_row, "first_window", "VmRSS")
        old_hwm, new_hwm = mem_of(old_row, "final", "VmHWM"), mem_of(new_row, "final", "VmHWM")
        mem_lines.append(f"{name} {fmt_mib(old_rss, old_hwm, 2)} → {fmt_mib(new_rss, new_hwm, 2)}")
        entry["first_window_rss_mib"] = {"old": rnd(old_rss, 2), "new": rnd(new_rss, 2)}
        entry["final_hwm_mib"] = {"old": rnd(old_hwm, 2), "new": rnd(new_hwm, 2)}
        body.append(cells)
        providers.append(entry)
    headers = ["Provider"] + [f"{label} p50 (p95)" for _, label, _ in APPEND]
    bits = [f"First-window p50 moved {pct_range(changes[phase])}" if phase == "first_window"
            else f"{label.lower()} {pct_range(changes[phase])}" for phase, label, _ in APPEND]
    text = "\n".join([
        f"At {records} records, milliseconds ({label_old} → {label_new}):", "",
        md_table(headers, body), "", "First-window RSS / final HWM medians, MiB:",
        *mem_lines, "", "; ".join(bits) + ".",
    ])
    payload = {"records": records, "providers": providers, "p50_change_pct": {
        key: None if not changes[phase] else {"min": min(changes[phase]), "max": max(changes[phase])}
        for phase, _label, key in APPEND}}
    return text, payload


def cmd_compare(args):
    old_rows, old_skip = load_jsonl(args.old)
    new_rows, new_skip = load_jsonl(args.new)
    note_skipped(old_skip + new_skip)
    old_idx, new_idx = index_append(old_rows), index_append(new_rows)
    sizes = sorted(set(old_idx) | set(new_idx), key=lambda item: (str(type(item)), item))
    old_sha, new_sha = sha256_of(old_rows), sha256_of(new_rows)
    blocks, payloads = [], []
    for records in sizes:
        text, payload = compare_size(
            records, old_idx.get(records, {}), new_idx.get(records, {}), args.label_old, args.label_new)
        blocks.append(text)
        payloads.append(payload)
    result = {"kind": "compare", "label_old": args.label_old, "label_new": args.label_new,
              "sha256_old": old_sha, "sha256_new": new_sha, "sizes": payloads}
    if args.json:
        emit_json(result)
        return
    parts = [f"{args.label_old} sha256: {old_sha or NA}", f"{args.label_new} sha256: {new_sha or NA}"]
    if blocks:
        parts.extend(["", "\n\n".join(blocks)])
    print("\n".join(parts))


def native_label(row):
    depth = row.get("envelope_depth")
    base = f"{row.get('image_mib')} MiB"
    return base if depth is None else f"{base}, {depth} {'layer' if depth == 1 else 'layers'}"


def sha256_of(rows):
    return nested(next((row for row in rows if row.get("kind") == "benchmark"), None), "binary_sha256")


def cmd_single(args):
    rows, skipped = load_jsonl(args.file)
    note_skipped(skipped)
    items = [row for row in rows if row.get("kind") == "summary" and row.get("image_mib") is not None]
    items.sort(key=lambda row: (row.get("image_mib"), row.get("envelope_depth") or 0))
    first = "Envelope" if any(row.get("envelope_depth") is not None for row in items) else "PNG size"
    time_headers = [first] + [f"{label} p50 (p95), ms" for _, label in NATIVE]
    time_body, mem_body, json_rows = [], [], []
    for row in items:
        label = native_label(row)
        time_body.append([label] + [fmt_ms(ms_of(row, phase)) for phase, _ in NATIVE])
        mem_json, mem_cells = {}, [label]
        for phase in MEM_PHASES:
            rss, hwm = mem_of(row, phase, "VmRSS"), mem_of(row, phase, "VmHWM")
            mem_cells.append(fmt_mib(rss, hwm, 3))
            mem_json[phase] = {"VmRSS": rnd(rss, 3), "VmHWM": rnd(hwm, 3)}
        mem_body.append(mem_cells)
        entry = {"image_mib": row.get("image_mib"), "envelope_depth": row.get("envelope_depth"),
                 "label": label, "memory_mib": mem_json}
        for phase, _ in NATIVE:
            pair = ms_of(row, phase)
            entry[phase] = None if pair is None else {"p50_ms": pair[0], "p95_ms": pair[1]}
        json_rows.append(entry)
    payload = {"kind": "single", "sha256": sha256_of(rows), "rows": json_rows}
    if args.json:
        emit_json(payload)
        return
    print(md_table(time_headers, time_body))
    print("\nExact-server RSS/HWM medians, MiB:\n")
    print(md_table([first, *MEM_LABELS], mem_body))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--json", action="store_true", help="print computed numbers as JSON")
    sub = parser.add_subparsers(dest="cmd", required=True)
    cmp = sub.add_parser("compare", parents=[common], help="diff two append JSONL runs")
    cmp.add_argument("old", type=Path)
    cmp.add_argument("new", type=Path)
    cmp.add_argument("--label-old", default="old")
    cmp.add_argument("--label-new", default="new")
    cmp.set_defaults(func=cmd_compare)
    sng = sub.add_parser("single", parents=[common], help="render one native/envelope JSONL run")
    sng.add_argument("file", type=Path)
    sng.set_defaults(func=cmd_single)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
