#!/usr/bin/env python3
"""Python-oracle parity of GET /api/messages for Claude torn lines and broken lineage (R5).

An invalid JSONL line is skipped (`跳过无效的JSONL 记录 ×N`); a parentUuid
chain that reaches a missing uuid keeps the reachable timeline (`Claude 祖先链在
<uuid> 处中断，之前的记录不在当前时间线`); a cycle truncates (`Claude 祖先链存在
循环，已在 <uuid> 处截断`); a last-prompt leafUuid with no record warns (`Claude
声明的叶子 <uuid> 不在记录中`). `supported` stays true in all three. Duplicate JSON
keys are a declared DELTA (Python last-key-wins, Rust skips the line).
"""
from __future__ import annotations

import argparse, json, sys, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, encoded, get_json, isolated_server)
from provider_parity import load_adapters  # noqa: E402
from python_oracle import discover_source  # noqa: E402

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
SIDS = ("TORN", "CYCLE", "LEAF", "DUPKEY", "GOOD")
SKIP_JSONL = "跳过无效的JSONL 记录 ×1"


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}" + (f"; {text[:240]}" if text else ""))


def passed(area):
    print(f"PASS {area}", flush=True)


def pairs(messages):
    return [(m.get("role"), m.get("text")) for m in messages if m.get("role") in ("user", "assistant")]


def python_pairs(adapter, path):
    native, _end = adapter.read(str(path))
    return pairs(row for row in native if row.get("role") != "status")


def fetch(opener, base, route, area):
    try:
        return get_json(opener, base, route)
    except HTTPError as err:
        fail(area, f"HTTP {err.code}", err.read(1024))


def put_bytes(corpus, sid, chunks):
    path = corpus.root / "claude/project-history" / f"{sid}.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(chunks))
    corpus.paths[sid] = path
    return path


def dup_line(row):
    raw = encoded(row).rstrip(b"\n")
    return raw[:-1] + b',"uuid":"' + str(row["uuid"]).encode() + b'"}\n'


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    sid, lost = "TORN", "lost"
    lost_line = encoded(claude_row(sid, "user", lost, "a1", "torn lost"))
    put_bytes(corpus, sid, [
        encoded(claude_row(sid, "user", "u1", None, "torn question one")),
        encoded(claude_row(sid, "assistant", "a1", "u1", "torn answer one")),
        b"\x00" * 4096 + lost_line[max(1, len(lost_line) // 2):],
        encoded(claude_row(sid, "user", "u2", lost, "torn question two")),
        encoded(claude_row(sid, "assistant", "a2", "u2", "torn answer two"))])
    corpus.put("CYCLE", "claude", [
        claude_row("CYCLE", "user", "x", "y", "cycle x"),
        claude_row("CYCLE", "assistant", "y", "x", "cycle y")], [])
    corpus.put("LEAF", "claude", [
        claude_row("LEAF", "user", "u1", None, "leaf question"),
        claude_row("LEAF", "assistant", "a1", "u1", "leaf answer"),
        {"type": "last-prompt", "leafUuid": "ghost"}], [])
    put_bytes(corpus, "DUPKEY", [
        encoded(claude_row("DUPKEY", "user", "u0", None, "dup prefix")),
        encoded(claude_row("DUPKEY", "assistant", "a0", "u0", "dup prefix answer")),
        dup_line(claude_row("DUPKEY", "user", "d1", "a0", "dup question"))])
    corpus.put("GOOD", "claude", [
        claude_row("GOOD", "user", "g-u", None, "good question"),
        claude_row("GOOD", "assistant", "g-a", "g-u", "good answer")], [])
    return corpus


def run(opener, base, corpus, adapter):
    listed = fetch(opener, base, "/api/sessions?force=1", "list")
    rows = {row["sid"]: row for row in listed.get("sessions") or []}
    if set(rows) != set(SIDS):
        fail("list", f"listed {sorted(rows)} want {list(SIDS)}", json.dumps(listed).encode())
    passed("list topology")
    for sid, row in rows.items():
        if row.get("supported") is not True:
            fail("supported", f"{sid} supported={row.get('supported')} warnings={row.get('migration_warnings')}",
                 json.dumps(row, ensure_ascii=False).encode())
    passed("supported")
    # A supported list row carries no migration_warnings; the
    # skipped-line note is on the detail meta.
    if "migration_warnings" in rows["TORN"]:
        fail("TORN-warning", "supported list row must not carry migration_warnings",
             json.dumps(rows["TORN"], ensure_ascii=False).encode())
    torn = fetch(opener, base, "/api/messages/" + quote(corpus.uid("TORN"), safe=":"), "TORN-warning")
    warnings = (torn.get("meta") or {}).get("migration_warnings") or []
    if SKIP_JSONL not in warnings:
        fail("TORN-warning", f"detail meta missing {SKIP_JSONL!r}: {warnings}",
             json.dumps(torn.get("meta"), ensure_ascii=False).encode())
    passed("TORN-warning")
    for sid in SIDS:
        uid = corpus.uid(sid)
        body = fetch(opener, base, "/api/messages/" + quote(uid, safe=":"), sid)
        rust = pairs(body.get("messages") or [])
        py = python_pairs(adapter, corpus.paths[sid])
        if sid == "DUPKEY":
            print(f"DELTA DUPKEY Python last-key-wins, Rust skips the line: python={py} rust={rust}",
                  flush=True)
            continue
        if rust != py:
            fail(sid, f"user/assistant (role, text) python={py} rust={rust}")
        passed(sid)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR",
                        help="write the synthetic corpus into DIR, print file paths, exit 0")
    parser.add_argument("--python-source", type=Path, default=discover_source(REPO))
    args = parser.parse_args()
    if args.fixtures_only is not None:
        args.fixtures_only.mkdir(parents=True, exist_ok=True)
        corpus = build(args.fixtures_only)
        for sid in SIDS:
            print(corpus.paths[sid])
        return
    python_source = args.python_source.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-claude-lineage-") as tmp:
        corpus = build(Path(tmp))
        adapters = load_adapters(python_source, fixture_root=corpus.root)
        with isolated_server(corpus, args.binary) as (base, opener):
            run(opener, base, corpus, adapters["claude"])


if __name__ == "__main__":
    main()
