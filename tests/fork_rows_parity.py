#!/usr/bin/env python3
"""Python-oracle parity of GET /api/sessions rows for batch-35 Codex fork shapes.

Compares adapters["codex"].list_sessions() with the isolated Rust list. Asserts
R1 first session_meta is identity (later metas skipped); R2 history_base null +
forked_from_id is self-contained, while history_base.thread_id R is inherited
even when forked_from_id is Q; R3 root_sid/fork_depth/created/title/size follow
the forked_from_id chain (missing parent ends it; size = own + Σ min(cut or 0,
parent.size)); R4 orphan agents are absent from the list (only owner agent_items).
"""
from __future__ import annotations

import argparse, json, sys, tempfile
from datetime import datetime, timezone
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (BINARY as DEBUG, REPO, Corpus, codex_message,
                            codex_row, encoded, get_json, isolated_server)
from provider_parity import adapter_module, load_adapters
from python_oracle import discover_source

RELEASE = REPO / "target/release" / DEBUG.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG
FIELDS = ("uid", "sid", "title", "cwd", "created", "size", "forked_from_id",
          "root_sid", "fork_depth", "agent_items", "supported")
ORPHANS = ("codex-orphan-agent",)
CWD = "/synthetic/fork-rows"


def fail(area, why):
    raise SystemExit(f"FAIL {area}: {why}")


def stamp(row, ts):
    row["timestamp"] = ts
    return row


def meta(sid, ts, **extra):
    payload = {
        "id": sid, "session_id": extra.pop("session_id", sid), "timestamp": ts,
        "cwd": extra.pop("cwd", CWD), "thread_source": extra.pop("thread_source", "user"),
        "forked_from_id": extra.pop("forked_from_id", None),
        "history_base": extra.pop("history_base", None), **extra}
    return stamp(codex_row("session_meta", payload), ts)


def turn(user, assistant, tid, ts, start=1):
    return [stamp(codex_row("event_msg", {"type": "task_started", "turn_id": tid}, start), ts),
            stamp(codex_message("user", user, start + 1), ts),
            stamp(codex_message("assistant", assistant, start + 2), ts),
            stamp(codex_row("event_msg", {"type": "task_complete", "turn_id": tid}, start + 3), ts)]


def spawn(parent, path="/root/x", nick="Name"):
    return {"subagent": {"thread_spawn": {
        "parent_thread_id": parent, "depth": 1, "agent_path": path,
        "agent_nickname": nick, "agent_role": None}}}


def put(corpus, sid, rows, parent=None):
    return corpus.put(sid, "codex", rows, [], parent=parent)


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    # (a) legacy 3-level self-contained fork chain; later session_meta skipped (R1).
    put(corpus, "codex-legacy-l0", [
        meta("codex-legacy-l0", "2026-09-11T10:00:00Z"),
        *turn("Legacy root question", "Legacy root answer", "t0", "2026-09-11T10:00:01Z"),
        meta("codex-legacy-l0", "2026-09-11T10:00:02Z", cwd="/synthetic/wrong-duplicate"),
        meta("codex-legacy-l0", "2026-09-11T10:00:03Z", cwd="/synthetic/wrong-duplicate")])
    put(corpus, "codex-legacy-l1", [
        meta("codex-legacy-l1", "2026-09-11T11:00:00Z", forked_from_id="codex-legacy-l0"),
        *turn("Legacy L1 question", "Legacy L1 answer", "t1", "2026-09-11T11:00:01Z")])
    put(corpus, "codex-legacy-l2", [
        meta("codex-legacy-l2", "2026-09-11T12:00:00Z", forked_from_id="codex-legacy-l1"),
        *turn("Legacy L2 question", "Legacy L2 answer", "t2", "2026-09-11T12:00:01Z")])
    # (b) legacy fork whose parent file is absent: chain ends, still listed (R3).
    put(corpus, "codex-legacy-missing-parent", [
        meta("codex-legacy-missing-parent", "2026-09-11T13:00:00Z",
             forked_from_id="codex-nowhere"),
        *turn("Missing parent question", "Missing parent answer", "tm", "2026-09-11T13:00:01Z")])
    # (f) plain thread; owner of (c).
    put(corpus, "codex-plain", [
        meta("codex-plain", "2026-09-11T09:00:00Z"),
        *turn("Plain thread question", "Plain thread answer", "tp", "2026-09-11T09:00:01Z")])
    # (c) subagent with parent present (copied parent session_id); list via agent_items.
    put(corpus, "codex-agent", [
        meta("codex-agent", "2026-09-11T09:30:00Z", session_id="codex-plain",
             cwd=CWD + "/agent", forked_from_id="codex-plain", thread_source="subagent",
             parent_thread_id="codex-plain", source=spawn("codex-plain")),
        *turn("Agent question", "Agent answer", "ta", "2026-09-11T09:30:01Z")],
        parent="codex-plain")
    # (d) subagent whose parent_thread_id is not indexed: absent from both lists (R4).
    put(corpus, "codex-orphan-agent", [
        meta("codex-orphan-agent", "2026-09-11T09:40:00Z", session_id="codex-missing",
             forked_from_id="codex-missing", thread_source="subagent",
             parent_thread_id="codex-missing", source=spawn("codex-missing", "/root/orphan", "Orphan")),
        *turn("Orphan agent question", "Orphan agent answer", "to", "2026-09-11T09:40:01Z")])
    # (e) rewind-past-fork A/Q/R: forked_from_id=Q, history_base.thread_id=A, line-boundary cut.
    a_prefix = [meta("codex-a", "2026-09-11T08:00:00Z"),
                *turn("Alpha question one", "Alpha answer one", "a1", "2026-09-11T08:00:01Z")]
    cut = sum(len(encoded(row)) for row in a_prefix)
    put(corpus, "codex-a", a_prefix + turn(
        "Alpha question two", "Alpha answer two", "a2", "2026-09-11T08:00:02Z", start=5))
    base_a = {"thread_id": "codex-a", "end_byte_offset": cut, "end_ordinal_exclusive": 5}
    put(corpus, "codex-q", [
        meta("codex-q", "2026-09-11T08:10:00Z", forked_from_id="codex-a", history_base=base_a),
        *turn("Quebec fork question", "Quebec fork answer", "q1", "2026-09-11T08:10:01Z")])
    put(corpus, "codex-r", [
        meta("codex-r", "2026-09-11T08:20:00Z", forked_from_id="codex-q", history_base=base_a),
        *turn("Romeo rewind question", "Romeo rewind answer", "r1", "2026-09-11T08:20:01Z")])
    return corpus


def bind(python_source, root):
    inst = load_adapters(python_source, fixture_root=root)
    cls = adapter_module(inst).CodexAdapter
    inst["codex"].list_sessions = cls.list_sessions.__get__(inst["codex"])
    inst["codex"].scan_sessions = cls.scan_sessions.__get__(inst["codex"])
    return inst


def parse_ts(v):
    try:
        return datetime.fromisoformat(str(v).replace("Z", "+00:00")).astimezone(timezone.utc)
    except (TypeError, ValueError):
        return None


def value(row, key):
    if key == "agent_items":
        return sorted(str(x.get("id")) for x in (row.get("agent_items") or []) if x.get("id"))
    if key == "supported":
        return False if row.get("supported") is False else True
    v = row.get(key)
    return None if v in (None, "") else v


def clip(v, n=96):
    text = v if isinstance(v, str) else json.dumps(v, ensure_ascii=False, default=str)
    return text if len(text) <= n else text[:n] + "…"


def same_instant(a, b):
    left, right = parse_ts(a), parse_ts(b)
    return bool(left and right and left == right)


def compare(py, rs):
    diffs = 0
    for sid in sorted(set(py) | set(rs)):
        a, b = py.get(sid), rs.get(sid)
        if not a or not b:
            print(f"DIFF {sid} missing: python={clip(a)} rust={clip(b)}", flush=True)
            diffs += 1
            continue
        notes = []
        for field in FIELDS:
            lv, rv = value(a, field), value(b, field)
            if lv == rv or (field == "created" and same_instant(a.get(field), b.get(field))):
                continue
            notes.append((field, a.get(field) if field == "created" else lv,
                          b.get(field) if field == "created" else rv))
        if notes:
            for field, lv, rv in notes:
                print(f"DIFF {sid} {field}: python={clip(lv)} rust={clip(rv)}", flush=True)
            diffs += 1
        else:
            print(f"PASS {sid} {' '.join(FIELDS)}", flush=True)
        lu, ru = a.get("updated"), b.get("updated")
        if lu != ru and not same_instant(lu, ru):
            print(f"DELTA {sid} updated: python={clip(lu)} rust={clip(ru)}", flush=True)
    return diffs


def run(corpus, python_source, binary):
    py = {row["sid"]: row for row in bind(python_source, corpus.root)["codex"].list_sessions() or []}
    with isolated_server(corpus, binary) as (base, opener):
        listed = get_json(opener, base, "/api/sessions?force=1")
    if not isinstance(listed.get("sessions"), list):
        fail("shape", "GET /api/sessions?force=1 needs a sessions list")
    rs = {row["sid"]: row for row in listed["sessions"] if row.get("source") == "codex"}
    leaked = [sid for sid in ORPHANS if sid in py or sid in rs]
    if leaked:
        fail("orphans", f"{leaked} listed python={[s for s in leaked if s in py]} rust={[s for s in leaked if s in rs]}")
    print("PASS orphans absent", flush=True)
    if compare(py, rs):
        raise SystemExit(1)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR")
    parser.add_argument("--python-source", type=Path, default=discover_source(REPO))
    args = parser.parse_args(argv)
    if args.fixtures_only is not None:
        root = args.fixtures_only.expanduser().resolve()
        root.mkdir(parents=True, exist_ok=True)
        for path in sorted(build(root).paths.values()):
            print(path)
        return
    python_source = args.python_source.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-fork-rows-") as tmp:
        run(build(Path(tmp)), python_source, args.binary)


if __name__ == "__main__":
    main()
