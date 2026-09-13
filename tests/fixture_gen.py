#!/usr/bin/env python3
"""Write a synthetic native corpus for manual testing of the Rust server.

Creates OUTDIR/{claude,codex,grok} with deterministic user/assistant records,
optional embedded PNG/GREEN images, Codex fixed-prefix forks and Claude
sidechain subagents. By default the written Claude/Codex files also carry the
record and attachment kinds current CLI versions write (Claude Code 2.1.x
mode/permission-mode/bridge-session/agent-name/atis-latch/file-history-delta/
cost-state and an attachment chain between each user record and its reply;
Codex token_usage_record/inter_agent_communication_metadata/event_msg
item_completed/response_item agent_message) so the read model's skip policy
(batch 33) is exercised; --plain omits them. --head-tail-edges also writes five
seeded list-summary boundary sessions (claude-edge-tail-title, claude-edge-big-head,
claude-edge-tail-cwd, codex-edge-big-line, codex-edge-fork). --batch35-shapes
also writes the batch-35 real-root shapes from history_parity.write_batch35_shapes
(legacy self-contained Codex forks with copied session_meta records, a rewind
past the parent's fork point, copied-meta subagents, orphan agents, a torn
Claude line, a missing last-prompt leaf and a parent cycle); independent of
--plain. No CLI, no network.
--print-env emits loopback-only SESSIONDOCK_* export lines for target/release/sessiondock.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import random
import sys
from pathlib import Path

TESTS = Path(__file__).resolve().parent
REPO = TESTS.parent
sys.path.insert(0, str(TESTS))
from history_parity import (CLAUDE_ATTACHMENT_KINDS, Corpus, claude_attachment_chain, claude_control_rows, claude_row,
                            claude_turn_tail_rows, codex_message, codex_row, codex_telemetry_rows, encoded, write_batch35_shapes)
from media_browser import GREEN, PNG, image

SOURCES = ("claude", "codex", "grok")
BIND = "127.0.0.1:8741"
STEMS = ("alpha", "bravo", "café", "名录", "delta", "echo", "foxtrot")


def die(message, code=1):
    print(message, file=sys.stderr)
    raise SystemExit(code)


def nonneg(value):
    number = int(value)
    if number < 0:
        raise argparse.ArgumentTypeError("must be >= 0")
    return number


def parse_sources(text):
    seen, ordered = set(), []
    for part in text.split(","):
        name = part.strip().lower()
        if not name:
            continue
        if name not in SOURCES:
            die(f"unknown source {name!r}; expected comma-separated {','.join(SOURCES)}")
        if name not in seen:
            seen.add(name)
            ordered.append(name)
    if not ordered:
        die("at least one source is required")
    return tuple(ordered)


def sentence(rng, sid, index):
    stem = rng.choice(STEMS)
    kind = index % 4
    if kind == 1:
        return f"{sid} #{index} {stem}\nsecond line about {stem}\nthird line."
    if kind == 2:
        return f"```python\nprint({index!r}, {stem!r})\n```"
    if kind == 3:
        return f"**{sid}** record `{index}` — _{stem}_ 名"
    return f"Synthetic {sid} record {index} about {stem}."


def spread(count, n):
    n = min(max(n, 0), count)
    if n == 0:
        return []
    if n == 1:
        return [0]
    return [i * (count - 1) // (n - 1) for i in range(n)]


def image_slots(count, n_images):
    return {index: (PNG if slot % 2 == 0 else GREEN)
            for slot, index in enumerate(spread(count, n_images))}


def meta(sid, **extra):
    return codex_row("session_meta", {"id": sid, "session_id": sid,
        "timestamp": "2026-09-11T09:00:00Z", "cwd": "/synthetic/fixture", **extra})


def make_row(source, sid, index, text, data):
    role = "user" if index % 2 == 0 else "assistant"
    if source == "claude":
        parent = f"row-{index - 1:06d}" if index else None
        content = [{"type": "text", "text": text}, image("claude", data)] if data else text
        return claude_row(sid, role, f"row-{index:06d}", parent, content)
    if source == "codex":
        if data is None:
            return codex_message(role, text, index + 1)
        kind = "input_text" if role == "user" else "output_text"
        return codex_row("response_item", {"type": "message", "role": role,
            "content": [{"type": kind, "text": text}, image("codex", data)]}, index + 1)
    content = [{"type": "text", "text": text}, image("grok", data)] if data else text
    return {"type": role, "content": content, "prompt_index": index // 2,
            "timestamp": "2026-09-11T10:00:00Z"}


def messages(source, sid, count, n_images, rng):
    slots = image_slots(count, n_images)
    return [make_row(source, sid, index, sentence(rng, sid, index), slots.get(index))
            for index in range(count)]


def with_cli_kinds(source, sid, rows):
    """Interleave the current-CLI record kinds in their observed positions.

    Claude: control records first; after every user record a short attachment
    chain (uuid/parentUuid linked) plus an atis-latch, and the following
    assistant record re-parented onto the last attachment; after every
    assistant record the turn-tail records. Codex: telemetry/item records
    after every user record. The conversation itself is unchanged.
    """
    if source == "claude":
        out = claude_control_rows(sid)
        tip = None
        for index, row in enumerate(rows):
            if row["type"] == "assistant" and tip is not None:
                row = {**row, "parentUuid": tip}
            out.append(row)
            if row["type"] == "user":
                start = (index // 2) % len(CLAUDE_ATTACHMENT_KINDS)
                kinds = tuple(CLAUDE_ATTACHMENT_KINDS[(start + k) % len(CLAUDE_ATTACHMENT_KINDS)]
                              for k in range(3))
                chain, tip = claude_attachment_chain(sid, row["uuid"], f"{row['uuid']}-att", kinds=kinds)
                out += chain
                out.append({"type": "atis-latch", "atis": {"latched": True}, "sessionId": sid})
            else:
                out += claude_turn_tail_rows(sid, row["uuid"], row["parentUuid"])
                tip = None
        return out
    if source == "codex":
        out = []
        for row in rows:
            out.append(row)
            payload = row.get("payload") or {}
            if payload.get("type") == "message" and payload.get("role") == "user":
                out += codex_telemetry_rows(f"turn-{row.get('ordinal', 0)}")
        return out
    return rows


def stat(source, sid, path, root):
    data = path.read_bytes()
    lines = data.splitlines()
    return {"source": source, "sid": sid,
            "path": str(path.relative_to(root)),
            "records": len(lines), "bytes": len(data)}


def put_grok(root, sid, rows):
    folder = root / "grok/%2Fsynthetic%2Ffixture" / sid
    folder.mkdir(parents=True, exist_ok=True)
    history = folder / "chat_history.jsonl"
    history.write_bytes(b"".join(encoded(row) for row in rows))
    (folder / "summary.json").write_bytes(encoded({
        "info": {"id": sid, "cwd": "/synthetic/fixture"},
        "generated_title": f"Synthetic {sid}",
        "created_at": "2026-09-11T10:00:00Z",
        "updated_at": "2026-09-11T10:00:00Z"}))
    return history


def write_forks(corpus, sid, parent_rows, n_forks, root):
    rows = []
    if n_forks <= 0:
        return rows
    total = len(parent_rows)
    for k in range(n_forks):
        keep = max(1, (k + 1) * total // (n_forks + 1))
        offset = sum(len(encoded(row)) for row in parent_rows[:keep])
        fsid = f"{sid}-fork-{k:02d}"
        frows = [meta(fsid, forked_from_id=sid, history_mode="paginated",
                      history_base={"thread_id": sid, "end_byte_offset": offset}),
                 codex_message("user", f"Fork {k} of {sid} continues", 1),
                 codex_message("assistant", f"Fork {k} reply", 2)]
        path = corpus.put(fsid, "codex", frows, [])
        rows.append(stat("codex", fsid, path, root))
    return rows


def write_agents(corpus, sid, n_agents, rng, root):
    rows = []
    if n_agents <= 0:
        return rows
    parent = corpus.paths[sid]
    for k in range(n_agents):
        agent = f"{sid}-agent-{k:02d}"
        path = parent.with_suffix("") / "subagents" / f"agent-{agent}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"".join(encoded(row) for row in [
            claude_row(sid, "user", f"{agent}-u", None, sentence(rng, agent, 0),
                       isSidechain=True, agentId=agent),
            claude_row(sid, "assistant", f"{agent}-a", f"{agent}-u",
                       sentence(rng, agent, 1), isSidechain=True, agentId=agent)]))
        path.with_suffix(".meta.json").write_text(json.dumps(
            {"description": f"Synthetic {agent}", "agentType": "reviewer"}))
        rows.append(stat("claude", agent, path, root))
    return rows


def ensure_outdir(path, force):
    if path.exists():
        if not path.is_dir():
            die(f"{path} exists and is not a directory")
        if any(path.iterdir()) and not force:
            die(f"{path} is not empty; pass --force to write anyway")
    else:
        path.mkdir(parents=True)


def print_table(rows):
    headers = ("source", "sid", "path", "records", "bytes")
    cells = [headers] + [tuple(str(row[key]) for key in headers) for row in rows]
    widths = [max(len(row[i]) for row in cells) for i in range(len(headers))]
    for i, row in enumerate(cells):
        aligned = []
        for j, cell in enumerate(row):
            aligned.append(cell.rjust(widths[j]) if j >= 3 else cell.ljust(widths[j]))
        print("  ".join(aligned))
        if i == 0:
            print("  ".join("-" * w for w in widths))


def print_env(root):
    mapping = [
        ("SESSIONDOCK_CLAUDE_ROOT", root / "claude"),
        ("SESSIONDOCK_CODEX_ROOT", root / "codex"),
        ("SESSIONDOCK_GROK_ROOT", root / "grok"),
        ("SESSIONDOCK_BIND", BIND),
        ("SESSIONDOCK_WEB_DIR", REPO / "legacy-web"),
    ]
    for key, value in mapping:
        print(f"export {key}={value}")


def write_head_tail_edges(corpus, root, seed):
    out, sid, rows, size, n = [], "claude-edge-tail-title", [], 0, 0
    pad = f"{seed}:" + "T" * 2048
    while size < 700 * 1024:
        rows.append(claude_row(sid, "user" if n % 2 == 0 else "assistant", f"t{n}", None if n == 0 else f"t{n-1}", pad))
        size += len(encoded(rows[-1]))
        n += 1
    rows.append({"type": "custom-title", "customTitle": "Edge tail title", "sessionId": sid})
    out.append(stat("claude", sid, corpus.put(sid, "claude", rows, []), root))
    sid = "claude-edge-big-head"
    rows = [claude_row(sid, "user", "big", None, "H" * (96 * 1024 + 64))]
    for i in range(4):
        rows.append(claude_row(sid, "assistant" if i % 2 == 0 else "user", f"n{i}", rows[-1]["uuid"], f"{sid} #{i}"))
    out.append(stat("claude", sid, corpus.put(sid, "claude", rows, []), root))
    sid, rows = "claude-edge-tail-cwd", []
    for i in range(40):
        rows.append(claude_row(sid, "user" if i % 2 == 0 else "assistant", f"c{i}", None if i == 0 else f"c{i-1}", f"head {i}"))
        del rows[-1]["cwd"]
    for i, cwd in enumerate(("/edge/majority",) * 3 + ("/edge/minority",)):
        rows.append(claude_row(sid, "user", f"w{i}", rows[-1]["uuid"], f"cwd {i}", cwd=cwd))
    out.append(stat("claude", sid, corpus.put(sid, "claude", rows, []), root))
    sid = "codex-edge-big-line"
    out.append(stat("codex", sid, corpus.put(sid, "codex", [meta(sid), codex_row("response_item", {
        "type": "function_call_output", "call_id": "edge-big",
        "output": json.dumps({"blob": "B" * (3 * 1024 * 1024)}, separators=(",", ":"))}, 1)], []), root))
    sid = "codex-edge-fork"
    out.append(stat("codex", sid, corpus.put(sid, "codex", [meta(sid, forked_from_id="codex-000",
        history_mode="paginated", history_base={"thread_id": "codex-000",
            "end_byte_offset": len(encoded(meta("codex-000")))}),
        codex_message("user", "Edge fork of codex-000", 1),
        codex_message("assistant", "Edge fork reply", 2)], []), root))
    return out


def write_batch35(corpus, root):
    """The batch-35 shapes; hidden orphan agents are reported with source
    ``<source>(hidden)`` because no list row exists for them."""
    out = []
    for sid in write_batch35_shapes(corpus):
        path = corpus.paths[sid]
        source = "claude" if path.is_relative_to(root / "claude") else "codex"
        row = stat(source, sid, path, root)
        if sid in corpus.hidden:
            row["source"] = source + "(hidden)"
        out.append(row)
    return out


def generate(root, sources, n_sessions, n_records, n_images, n_forks, n_agents, seed, plain=False, edges=False, batch35=False):
    corpus = Corpus(root)
    for source in SOURCES:
        (root / source).mkdir(parents=True, exist_ok=True)
    summary = []
    for source in sources:
        for index in range(n_sessions):
            sid = f"{source}-{index:03d}"
            rng = random.Random(seed * 1_000_003 + index * 97 + SOURCES.index(source))
            rows = messages(source, sid, n_records, n_images, rng)
            if not plain:
                rows = with_cli_kinds(source, sid, rows)
            if source == "claude":
                path = corpus.put(sid, "claude", rows, [])
                summary.append(stat(source, sid, path, root))
                summary.extend(write_agents(corpus, sid, n_agents, rng, root))
            elif source == "codex":
                full = [meta(sid), *rows]
                path = corpus.put(sid, "codex", full, [])
                summary.append(stat(source, sid, path, root))
                summary.extend(write_forks(corpus, sid, full, n_forks, root))
            else:
                path = put_grok(root, sid, rows)
                corpus.paths[sid] = path
                summary.append(stat(source, sid, path, root))
    if edges:
        summary.extend(write_head_tail_edges(corpus, root, seed))
    if batch35:
        summary.extend(write_batch35(corpus, root))
    return summary


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("outdir", type=Path)
    parser.add_argument("--sessions", type=nonneg, default=3)
    parser.add_argument("--records", type=nonneg, default=200)
    parser.add_argument("--images", type=nonneg, default=0)
    parser.add_argument("--forks", type=nonneg, default=0)
    parser.add_argument("--agents", type=nonneg, default=0)
    parser.add_argument("--sources", default="claude,codex,grok",
                        help="comma-separated subset of claude,codex,grok")
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--plain", action="store_true",
                        help="omit the current-CLI record/attachment kinds (pre-batch-33 shape)")
    parser.add_argument("--head-tail-edges", action="store_true", help="also write five list-summary head/tail boundary sessions")
    parser.add_argument("--batch35-shapes", action="store_true",
                        help="also write the batch-35 real-root shapes (copied session_meta forks, rewind past fork point, orphan agents, torn/cyclic Claude lineage)")
    parser.add_argument("--print-env", action="store_true")
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args(argv)
    sources = parse_sources(args.sources)
    outdir = args.outdir
    ensure_outdir(outdir, args.force)
    root = outdir.resolve()
    rows = generate(root, sources, args.sessions, args.records, args.images,
                    args.forks, args.agents, args.seed, plain=args.plain, edges=args.head_tail_edges,
                    batch35=args.batch35_shapes)
    print_table(rows)
    if args.print_env:
        print()
        print_env(root)


if __name__ == "__main__":
    main()
