#!/usr/bin/env python3
"""Compare two run_validation.py --json result files.

Markdown (or --json) report of newly failing/timing out, newly passing,
missing/added suites, status changes, and elapsed deltas. Exit 1 on
regression (PASS→FAIL/TIMEOUT or newly FAIL/TIMEOUT).
"""
# run_validation: skip
import argparse
import json
import sys
from pathlib import Path

BAD = {"FAIL", "TIMEOUT"}
COUNT_KEYS = ("PASS", "FAIL", "TIMEOUT", "SKIP")


def load(path):
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    return data, {row["name"]: row for row in data.get("results") or []}


def pct(old, new):
    if old == 0:
        return 0.0 if new == 0 else None
    return (new - old) / old * 100.0


def abs_pct(row):
    return float("inf") if row["pct"] is None else abs(row["pct"])


def fmt_pct(value):
    return "n/a" if value is None else f"{value:+.1f}%"


def counts_text(counts):
    counts = counts or {}
    parts = [f"{k}={counts.get(k, 0)}" for k in COUNT_KEYS]
    extra = [f"{k}={counts[k]}" for k in counts if k not in COUNT_KEYS]
    return " ".join(parts + extra)


def section(title, items):
    body = [f"- {item}" for item in items] or ["_none_"]
    return [f"## {title}", "", *body, ""]


def named(rows, names):
    return [{"name": n, "kind": rows[n].get("kind"), "status": rows[n].get("status")} for n in names]


def event(name, old_st, new_st, reason=None):
    return {"name": name, "old_status": old_st, "new_status": new_st, "reason": reason}


def change_line(row):
    extra = f" ({row['reason']})" if row.get("reason") else ""
    return f"{row['name']}: {row['old_status'] or 'added'} → {row['new_status']}{extra}"


def run_header(label, path, data):
    return (f"- {label}: `{path}` started={data.get('started')} "
            f"finished={data.get('finished')} exit_code={data.get('exit_code')} "
            f"{counts_text(data.get('counts'))}")


def table(headers, rows, empty):
    if not rows:
        return [empty]
    lines = ["| " + " | ".join(headers) + " |", "| " + " | ".join("---" for _ in headers) + " |"]
    return lines + ["| " + " | ".join(row) + " |" for row in rows]


def side_meta(path, data):
    return {"path": str(path), "started": data.get("started"), "finished": data.get("finished"),
            "exit_code": data.get("exit_code"), "counts": data.get("counts") or {}}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("old", type=Path)
    parser.add_argument("new", type=Path)
    parser.add_argument("--threshold", type=float, default=25.0)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    old_data, old_rows = load(args.old)
    new_data, new_rows = load(args.new)
    old_names, new_names = set(old_rows), set(new_rows)
    missing, added = sorted(old_names - new_names), sorted(new_names - old_names)
    failing, passing, changes, elapsed = [], [], [], []
    for name in sorted(old_names & new_names):
        old, new = old_rows[name], new_rows[name]
        os, ns = old.get("status"), new.get("status")
        if os != ns:
            changes.append(event(name, os, ns))
        if ns in BAD and os not in BAD:
            failing.append(event(name, os, ns, new.get("reason")))
        if ns == "PASS" and os != "PASS":
            passing.append(event(name, os, ns))
        old_s, new_s = float(old.get("elapsed_s") or 0), float(new.get("elapsed_s") or 0)
        elapsed.append({"name": name, "old_s": old_s, "new_s": new_s, "pct": pct(old_s, new_s)})
    for name in added:
        new, ns = new_rows[name], new_rows[name].get("status")
        if ns in BAD:
            failing.append(event(name, None, ns, new.get("reason")))
        if ns == "PASS":
            passing.append(event(name, None, ns))

    elapsed.sort(key=abs_pct, reverse=True)
    notable = [row for row in elapsed if abs_pct(row) >= args.threshold]
    tot_old = sum(float(row.get("elapsed_s") or 0) for row in old_rows.values())
    tot_new = sum(float(row.get("elapsed_s") or 0) for row in new_rows.values())
    totals = {"old_s": tot_old, "new_s": tot_new, "pct": pct(tot_old, tot_new)}
    payload = {
        "old": side_meta(args.old, old_data), "new": side_meta(args.new, new_data),
        "newly_failing": failing, "newly_passing": passing,
        "missing": named(old_rows, missing), "added": named(new_rows, added),
        "status_changes": changes, "elapsed": notable, "elapsed_totals": totals,
        "threshold": args.threshold, "regressed": bool(failing),
    }
    if args.json:
        json.dump(payload, sys.stdout, indent=2)
        print()
        return 1 if failing else 0

    lines = ["# Validation run comparison", "",
             run_header("Old", args.old, old_data), run_header("New", args.new, new_data), ""]
    lines += section("Newly failing / timing out", [change_line(r) for r in failing])
    lines += section("Newly passing", [change_line(r) for r in passing])
    lines += section("Missing suites", [f"{r['name']} ({r['kind']}, {r['status']})" for r in payload["missing"]])
    lines += section("Added suites", [f"{r['name']} ({r['kind']}, {r['status']})" for r in payload["added"]])
    lines += ["## Status changes", ""] + table(
        ["Suite", "Old", "New"],
        [[r["name"], str(r["old_status"]), str(r["new_status"])] for r in changes], "_none_")
    lines += ["", f"## Elapsed (notable ≥ {args.threshold:g}%)", ""] + table(
        ["Suite", "Old", "New", "Change"],
        [[r["name"], f"{r['old_s']:.3f}s", f"{r['new_s']:.3f}s", fmt_pct(r["pct"])] for r in notable],
        "_none above threshold_")
    lines += ["", f"Totals: {tot_old:.3f}s → {tot_new:.3f}s ({fmt_pct(totals['pct'])})", ""]
    print("\n".join(lines))
    return 1 if failing else 0


if __name__ == "__main__":
    sys.exit(main())
