#!/usr/bin/env python3
"""Python-oracle parity corpus for the hub wire namespace (`federation.public_payload`).

Loads `<python-source>/sessiondock/federation.py` in-process (no package import, no
server) and runs a fixed payload corpus through `public_payload`, recording the
scoped payload — or the `ValueError` text — per case. The corpus covers every
rewritten key (uid/from_uid/to_uid/continued_in, media `src`, `epoch`), the
opaque subtrees (data/content/input/arguments/raw/resolved), and the per-path
row decoration (sessions/search rows, live keys, terminal names, term/create
session, bug-report worker, trash ids, messages/watch meta).

The Rust side is `crates/sessiondock/tests/hub_namespace.rs`, which replays
`tests/fixtures/hub_namespace_cases.json` against `hub::namespace` and reports
DIFF. This script keeps that fixture honest: without `--write` it regenerates
the expectations from the oracle and fails when the committed fixture differs
(the Python source moved, or the corpus changed without a rewrite).

    python3 tests/hub_namespace_parity.py --python-source ../sessiondock [--write]
"""
from __future__ import annotations

import argparse
import copy
import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests/fixtures/hub_namespace_cases.json"
NID = "a" * 32
NODE = {"id": NID, "name": "NodeA"}
NODE_CN = {"id": "b" * 32, "name": "机器乙"}
MEDIA = "/api/media/" + "d" * 32


def load_oracle(source: Path):
    path = source / "sessiondock/federation.py"
    if not path.is_file():
        raise SystemExit(f"FAIL oracle: {path} not found")
    spec = importlib.util.spec_from_file_location("sessiondock_federation_oracle", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def corpus():
    """(name, path, node, input) — deterministic order, plain JSON values only."""
    row = {"uid": "claude:same-file-hash", "sid": "same-native-id", "source": "claude",
           "title": "NodeA session", "cwd": "/same/project", "size": 20,
           "created": "2026-09-01T00:00:00Z", "updated": "2026-09-07T00:00:00Z", "agents": 0}
    cases = [
        ("sessions_rows_are_scoped_and_stamped", "/api/sessions", NODE,
         {"sessions": [row, {**row, "uid": "codex:other", "continued_in": "codex:next"}],
          "sig": "fixture-sig", "built_at": 0}),
        ("sessions_from_and_to_uid", "/api/sessions", NODE,
         {"sessions": [{"uid": "claude:a", "from_uid": "claude:b", "to_uid": "grok:c", "title": "t"}]}),
        ("sessions_empty_references_stay_empty", "/api/sessions", NODE,
         {"sessions": [{"uid": "", "continued_in": "", "from_uid": None, "to_uid": 7}]}),
        ("sessions_results_fallback_key", "/api/sessions", NODE,
         {"results": [{"uid": "claude:a"}]}),
        ("sessions_key_wins_over_results", "/api/sessions", NODE,
         {"sessions": [{"uid": "claude:a"}], "results": [{"uid": "claude:b"}]}),
        ("sessions_row_node_stamp_overwrites_existing", "/api/sessions", NODE,
         {"sessions": [{"uid": "claude:a", "node_id": "z" * 32, "node_name": "old"}]}),
        ("sessions_unchanged_probe_has_no_rows", "/api/sessions", NODE,
         {"unchanged": True, "sig": "fixture-sig"}),
        ("sessions_non_object_rows_are_skipped_by_walk", "/api/sessions", NODE,
         {"sessions": [{"uid": "claude:a"}], "pending": [{"uid": "claude:p"}]}),
        ("search_results_rows", "/api/search", NODE,
         {"results": [{**row, "hits": 1, "snippet": "needle /api/media/x uid"}],
          "total_pool": 1, "truncated": False, "scanned": 1}),
        ("search_sessions_key_wins", "/api/search", NODE,
         {"sessions": [{"uid": "claude:a"}], "results": [{"uid": "claude:b"}]}),
        ("search_matches_chunk", "/api/search", NODE_CN,
         {"results": [{"uid": "claude:x", "title": "中文 標題 🚀"}]}),
        ("live_keys_scoped", "/api/live", NODE,
         {"uids": ["claude:a", "codex:b"], "tmux_uids": ["claude:c"],
          "started_at": {"claude:a": 1.5, "codex:b": 2}}),
        ("live_missing_keys_are_created", "/api/live", NODE, {}),
        ("live_empty_uid_and_extra_keys", "/api/live", NODE,
         {"uids": [""], "tmux_uids": [], "started_at": {}, "extra": {"uid": "claude:e"}}),
        ("term_list_names_and_pending", "/api/term/list", NODE,
         {"enabled": True, "unavailable_reason": "", "sources": {"claude": True, "codex": True},
          "home": "/home/u/NodeA", "backend": "tmux",
          "backends": [{"name": "tmux", "label": "tmux", "available": True, "current": True,
                        "unavailable_reason": ""}],
          "sessions": [{"name": "same-terminal", "uid": "claude:x", "source": "claude", "cwd": "/w"}],
          "pending": [{"name": "pending-one", "source": "codex", "sid": "s", "token": "lease"}]}),
        ("term_list_empty_and_missing_names", "/api/term/list", NODE,
         {"sessions": [{"name": ""}, {"uid": "claude:x"}], "pending": [{"name": None}]}),
        ("term_list_stale_rows", "/api/term/list", NODE_CN,
         {"enabled": False, "sources": {}, "sessions": [{"name": "t", "stale": True, "last_seen": 1.0}],
          "pending": []}),
        ("term_create_with_session", "/api/term/create", NODE,
         {"name": "same-terminal", "source": "claude", "sid": "new-sid", "cwd": "/same",
          "token": "fixture-lease", "started": 1.0,
          "session": {"uid": "claude:new", "title": "new", "name": "not-a-terminal"}}),
        ("term_create_without_session", "/api/term/create", NODE,
         {"name": "same-terminal", "session": None}),
        ("term_create_empty_session_object", "/api/term/create", NODE,
         {"name": "", "session": {}}),
        ("term_takeover", "/api/term/takeover", NODE,
         {"name": "taken", "session": {"uid": "codex:t"}}),
        ("term_new_status", "/api/term/new-status", NODE,
         {"waiting": True, "running": True, "name": "status-term"}),
        ("bug_report_worker", "/api/bug-report", NODE,
         {"ok": True, "report_id": "r1", "worker": {"name": "bug-worker", "uid": "claude:w"}}),
        ("bug_report_without_worker", "/api/bug-report", NODE,
         {"ok": True, "worker": None, "name": "not-scoped"}),
        ("trash_items", "/api/trash", NODE,
         {"items": [{"id": "claude/same-trash", "uid": "claude:same", "source": "claude",
                     "title": "deleted", "size": 10, "restorable": True},
                    {"id": "", "uid": "codex:x"}, {"uid": "grok:y"}], "size": 10}),
        ("trash_ids_only_scoped_under_trash_path", "/api/trash/restore", NODE,
         {"ok": True, "id": "claude/same-trash", "uid": "claude:same"}),
        ("messages_meta_and_media", "/api/messages/claude:same-file-hash", NODE,
         {"meta": {**row}, "version": {"head": "h", "size": 2, "mtime": 1},
          "messages": [{"id": "native-message", "role": "assistant", "text": "/api/media/keep-in-text",
                        "input": {"uid": "arbitrary user input", "src": MEDIA},
                        "media": [{"src": MEDIA, "mime": "image/png"}, {"src": "/api/other/x"}]}],
          "partial": None}),
        ("messages_without_meta", "/api/messages/claude:x", NODE,
         {"messages": [{"uid": "claude:m"}], "meta": None}),
        ("watch_meta", "/api/watch", NODE,
         {"meta": {"uid": "codex:abc", "cwd": "/same"}, "messages": [{"content": [{"uid": "claude:z"}]}]}),
        ("resolve_files_reference_names_untouched", "/api/session/resolve-files", NODE,
         {"resolved": {"uid": "/project/uid", "epoch": "/project/epoch"},
          "targets": [{"ref": "uid", "path": "/project/uid", "kind": "file"}], "node_id": NID}),
        ("media_src_only_with_media_prefix", "/api/session/attachment", NODE,
         {"media": {"src": MEDIA}, "list": [{"src": "/api/media/"}, {"src": "api/media/x"},
                                            {"src": "/api/mediax"}, {"src": 5}, {"src": ""}]}),
        ("epoch_scoped_outside_opaque", "/api/session/outbox", NODE,
         {"outbox": [{"uid": "claude:o", "epoch": "e1"}],
          "outbox_version": {"epoch": "same-epoch", "revision": 0},
          "empty": {"epoch": ""}, "number": {"epoch": 3}, "nested": [[{"epoch": "deep"}]]}),
        ("opaque_subtrees_verbatim", "/api/sessions", NODE,
         {key: {"uid": "claude:hidden", "src": MEDIA, "epoch": "hidden", "deeper": [{"uid": "claude:h2"}]}
          for key in ("data", "content", "input", "arguments", "raw", "resolved")}),
        ("opaque_list_verbatim", "/api/session/send", NODE,
         {"content": [{"uid": "claude:h"}], "arguments": ["claude:h", {"src": MEDIA}],
          "sibling": {"uid": "claude:visible"}}),
        ("uid_non_string_values_recurse", "/api/session/star", NODE,
         {"uid": 5, "from_uid": None, "to_uid": True, "continued_in": ["claude:in-list"],
          "wrapped": {"uid": {"uid": "claude:inner"}}}),
        ("uid_list_of_rows_under_reference_key", "/api/x", NODE,
         {"uid": [{"uid": "claude:a"}, {"from_uid": "codex:b"}, "claude:plain"]}),
        ("deep_nesting_lists_of_lists", "/api/x", NODE,
         {"a": [[[{"uid": "claude:deep", "media": [[{"src": MEDIA}]]}]], "claude:text"]}),
        ("top_level_list_is_walked_not_decorated", "/api/sessions", NODE,
         [{"uid": "claude:a", "src": MEDIA}, "claude:b", 3]),
        ("top_level_string_passes_through", "/api/sessions", NODE, "claude:a"),
        ("top_level_null_passes_through", "/api/live", NODE, None),
        ("top_level_number_passes_through", "/api/trash", NODE, 3.5),
        ("uid_tail_keeps_colons_and_tildes", "/api/sessions", NODE,
         {"sessions": [{"uid": "claude:a:b~c", "continued_in": "codex:x~y:z"}]}),
        ("delete_receipt", "/api/sessions/delete", NODE,
         {"ok": True, "deleted": [{"uid": "claude:a"}, {"uid": "codex:b"}],
          "errors": [{"uid": "grok:c", "error": "busy"}]}),
        ("fork_visibility_receipt", "/api/sessions/fork-visibility", NODE_CN,
         {"ok": True, "updated": [{"uid": "codex:parent", "fork_parent_visible": True}], "errors": []}),
        ("unicode_node_name_and_text", "/api/sessions", NODE_CN,
         {"sessions": [{"uid": "claude:中", "title": "标题 ✓  ", "cwd": "/家"}]}),
        # A non-string terminal name is out of corpus: Python would str() it into
        # the scoped name, Rust leaves it alone; real nodes only send strings.
        ("keys_named_like_references_but_not_strings", "/api/term/list", NODE,
         {"sessions": [{"name": None, "uid": None, "from_uid": 0}], "pending": [{"name": False}]}),
        # Python raises ValueError; Rust `try_public_payload` must refuse the same payloads.
        ("error_uid_without_source", "/api/sessions", NODE, {"sessions": [{"uid": "nosource"}]}),
        ("error_nested_from_uid_without_source", "/api/watch", NODE,
         {"messages": [{"blocks": [{"from_uid": "bad"}]}]}),
        ("error_live_uid_without_source", "/api/live", NODE, {"uids": ["claude:a", "bad"]}),
        ("error_live_started_at_without_source", "/api/live", NODE, {"started_at": {"bad": 1}}),
        ("error_continued_in_without_source", "/api/sessions", NODE,
         {"sessions": [{"uid": "claude:ok", "continued_in": "bad"}]}),
    ]
    return cases


def build(oracle):
    out = []
    for name, path, node, data in corpus():
        case = {"name": name, "path": path, "node": dict(node), "input": copy.deepcopy(data)}
        try:
            case["expected"] = oracle.public_payload(copy.deepcopy(data), node, path)
        except ValueError as error:
            case["error"] = str(error)
        out.append(case)
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--python-source", default=str(ROOT.parent / "sessiondock"),
                    help="directory containing sessiondock/federation.py (the oracle)")
    ap.add_argument("--fixture", type=Path, default=FIXTURE)
    ap.add_argument("--write", action="store_true", help="rewrite the fixture from the oracle")
    args = ap.parse_args()
    oracle = load_oracle(Path(args.python_source).resolve())
    cases = build(oracle)
    errors = sum(1 for case in cases if "error" in case)
    text = json.dumps(cases, ensure_ascii=False, indent=1) + "\n"
    if args.write:
        args.fixture.parent.mkdir(parents=True, exist_ok=True)
        args.fixture.write_text(text, encoding="utf-8")
        print(f"wrote {args.fixture.relative_to(ROOT)}: {len(cases)} cases ({errors} raise)")
        return 0
    if not args.fixture.is_file():
        print(f"FAIL fixture: {args.fixture} missing; run with --write")
        return 1
    committed = json.loads(args.fixture.read_text(encoding="utf-8"))
    by_name = {case["name"]: case for case in committed}
    diff = []
    for case in cases:
        other = by_name.get(case["name"])
        if other != case:
            diff.append(case["name"])
    for name in by_name:
        if name not in {case["name"] for case in cases}:
            diff.append(name)
    print(f"hub_namespace_parity: {len(cases)} cases ({errors} raise), DIFF {len(diff)}")
    for name in diff:
        print(f"  DIFF {name}")
    if diff:
        print("fixture is out of date with the oracle; rerun with --write and re-run cargo test --test hub_namespace")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
