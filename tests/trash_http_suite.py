#!/usr/bin/env python3
"""HTTP-only recycle-bin contract (docs/trash.md vs api/trash.rs).

Synthetic Claude (agent sidecar), Codex (root + history_base fork) and Grok
sessions on an isolated loopback server. Stdlib plus tests/history_parity.py.
"""
import argparse, json, os, sys, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, codex_message, codex_row,
    encoded, isolated_server)

BINARY = (p if (p := REPO / "target/release" / DEBUG_BINARY.name).is_file() else DEBUG_BINARY)
# Documented item keys; code also emits `name` (docs/trash.md omits it).
FIELDS = ("id", "entry_id", "uid", "source", "sid", "title", "cwd", "updated", "deleted_at",
          "deleted_ts", "size", "bytes", "files", "kind", "origin", "state", "forced",
          "run_state", "recorded", "restorable")


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def need(ok, area, why, body=b""):
    if not ok:
        fail(area, why, body)


def passed(area):
    print(f"PASS {area}", flush=True)


def call(opener, base, method, route, body=None, want=200):
    req = Request(base + route, data=None if body is None else json.dumps(body).encode(), method=method)
    if body is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with opener.open(req, timeout=15) as resp:
            raw, code = resp.read(1 << 20), resp.status
    except HTTPError as err:
        raw, code = err.read(8192), err.code
    except Exception as err:
        fail(route, str(err))
    if want is not None and code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        return (json.loads(raw) if raw else {}), raw, code
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)


def listed(opener, base):
    body, raw, _ = call(opener, base, "GET", "/api/sessions")
    rows = body.get("sessions")
    if not isinstance(rows, list):
        fail("/api/sessions", "missing sessions list", raw)
    return {row["sid"]: row for row in rows if isinstance(row, dict)}, raw


def delete(opener, base, uid, query="", want=200):
    return call(opener, base, "DELETE", f"/api/session/{quote(uid, safe=':')}{query}", want=want)


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    sid = "claude-main"
    corpus.put(sid, "claude", [claude_row(sid, "user", "u0", None, "q"),
                               claude_row(sid, "assistant", "a0", "u0", "a")], [])
    agent = corpus.paths[sid].with_suffix("") / "subagents" / "agent-alpha.jsonl"
    agent.parent.mkdir(parents=True)
    agent.write_bytes(b"".join(encoded(claude_row(sid, k, u, p, t, isSidechain=True, agentId="alpha"))
                               for k, u, p, t in (("user", "au", None, "aq"), ("assistant", "aa", "au", "aa"))))
    agent.with_suffix(".meta.json").write_text(json.dumps({"description": "sidecar", "agentType": "reviewer"}))
    corpus.paths["claude-agent"] = agent
    def meta(name, **extra):
        return codex_row("session_meta", {"id": name, "session_id": name, "cwd": "/synthetic/trash", **extra})
    parent = [meta("codex-parent"), codex_message("user", "p"), codex_message("assistant", "a", 2)]
    cut = sum(len(encoded(row)) for row in parent)
    corpus.put("codex-parent", "codex", parent + [codex_message("user", "tail", 3)], [])
    corpus.put("codex-fork", "codex", [
        meta("codex-fork", forked_from_id="codex-parent", history_mode="paginated",
             history_base={"thread_id": "codex-parent", "end_byte_offset": cut}),
        codex_message("user", "fq"), codex_message("assistant", "fa", 2)], [])
    for name in ("grok-keep", "grok-link"):
        folder = root / "grok/%2Fsynthetic" / name
        folder.mkdir(parents=True)
        (folder / "summary.json").write_text(json.dumps(
            {"info": {"id": name, "cwd": "/synthetic/trash"}, "generated_title": name}))
        (folder / "chat_history.jsonl").write_bytes(encoded({"type": "user", "content": name, "prompt_index": 1}))
        corpus.paths[name] = folder
    (corpus.paths["grok-keep"] / "extra.bin").write_bytes(b"unnamed stays")
    return corpus


def unconfigured(opener, base):
    meta, raw, _ = call(opener, base, "GET", "/api/meta")
    need(meta.get("capabilities", {}).get("trash") is False, "unconfigured", "capabilities.trash must be false", raw)
    uid = listed(opener, base)[0]["claude-main"]["uid"]
    for method, route, body in (
            ("DELETE", f"/api/session/{quote(uid, safe=':')}", None),
            ("POST", "/api/sessions/delete", {"uids": [uid]}),
            ("GET", "/api/trash", None),
            ("POST", "/api/trash/restore", {"id": "x"}),
            ("POST", "/api/trash/purge", {"all": True})):
        data, raw, _ = call(opener, base, method, route, body, want=501)
        need(data.get("code") == "not_implemented", "unconfigured", f"{method} {route} code={data.get('code')!r}", raw)
    passed("unconfigured: every trash route 501, capabilities.trash false")


def configured(opener, base, corpus, trash):
    meta, raw, _ = call(opener, base, "GET", "/api/meta")
    need(meta.get("capabilities", {}).get("trash") is True, "configured", "capabilities.trash must be true", raw)
    rows, raw = listed(opener, base)
    for sid in ("claude-main", "codex-parent", "codex-fork", "grok-keep", "grok-link"):
        need(sid in rows, "inventory", f"missing {sid}", raw)
    need(rows["claude-main"].get("agent_items"), "inventory", "claude-main missing agent_items", raw)
    claude, parent, fork = (rows[s]["uid"] for s in ("claude-main", "codex-parent", "codex-fork"))
    grok, link = rows["grok-keep"]["uid"], rows["grok-link"]["uid"]
    body, raw, _ = delete(opener, base, claude)
    need(body.get("ok") is True and body.get("files") == 3 and body.get("forced") is False,
         "delete", "want ok files=3 (main+agent+.meta)", raw)
    entry, dest = body["entry_id"], Path(body["trash"])
    need(dest == trash / entry and (dest / "manifest.json").is_file(), "delete", f"entry dir/manifest missing {dest}", raw)
    manifest = json.loads((dest / "manifest.json").read_text())
    need(manifest.get("uid") == claude and manifest.get("state") == "trashed" and len(manifest.get("files") or []) == 3,
         "delete", "manifest uid/state/files", json.dumps(manifest).encode())
    for path in (corpus.paths["claude-main"], corpus.paths["claude-agent"],
                 corpus.paths["claude-agent"].with_suffix(".meta.json")):
        need(not path.exists(), "delete", f"still in corpus: {path}")
    need("claude-main" not in listed(opener, base)[0], "sessions", "trashed uid still listed")
    passed("delete: 200 files cover main+sidecars; gone from corpus; under trash/<id>/manifest.json")
    passed("session disappears from /api/sessions")
    # A Grok session moves as its whole directory, so
    # every file inside it — extra.bin included — travels with the entry.
    extra = corpus.paths["grok-keep"] / "extra.bin"
    grok_dir = corpus.paths["grok-keep"]
    body, raw, _ = delete(opener, base, grok, "?force=1")
    grok_entry = body["entry_id"]
    need(body.get("files") == 1 and not grok_dir.exists(), "grok", "files=1 and the directory moved whole", raw)
    need(any(p.name == extra.name for p in (trash / grok_entry).rglob("*")), "grok", "extra.bin travelled with the directory")
    manifest = json.loads((trash / grok_entry / "manifest.json").read_text())
    need(manifest["files"][0]["role"] == "directory", "grok", "manifest role is directory", json.dumps(manifest).encode())
    passed("Grok session directory moves whole (role directory), unnamed files travel with it")
    body, raw, _ = delete(opener, base, parent, "?force=1", want=409)
    need(body.get("code") == "fork_parent_protected" and corpus.paths["codex-parent"].exists(),
         "fork", "want 409 fork_parent_protected and parent stays", raw)
    batch, raw, _ = call(opener, base, "POST", "/api/sessions/delete",
                         {"uids": [fork, parent, "codex:0000000000000000"], "force": True})
    need([len(batch.get(k) or []) for k in ("deleted", "skipped", "failed")] == [1, 1, 1],
         "batch", "want deleted/skipped/failed each length 1", raw)
    need(batch["deleted"][0].get("uid") == fork and batch["skipped"][0].get("code") == "fork_parent_protected"
         and batch["failed"][0].get("code") == "not_found", "batch", "fork deleted, parent skipped, unknown failed", raw)
    need(corpus.paths["codex-parent"].exists() and not corpus.paths["codex-fork"].exists(),
         "batch", "parent stays, fork moved")
    body, raw, _ = delete(opener, base, parent, "?force=1")
    need(body.get("ok") is True, "parent", "parent deletable after fork is gone", raw)
    passed("fork parent 409; batch 200 deleted/skipped/failed; parent after fork succeeds")
    listing, raw, _ = call(opener, base, "GET", "/api/trash")
    items = listing.get("items") or []
    need(listing.get("ok") is True and len(items) >= 2, "list", "want ok and ≥2 items", raw)
    for item in items:
        missing = [k for k in FIELDS if k not in item]
        need(not missing and item["id"] == item["entry_id"] and item["size"] == item["bytes"],
             "list", f"fields {missing} or id/size", json.dumps(item).encode())
    page, raw, _ = call(opener, base, "GET", "/api/trash?limit=1")
    cursor = page.get("next_cursor")
    need(isinstance(cursor, str) and cursor and len(page.get("items") or []) == 1,
         "page", "limit=1 must paginate with next_cursor", raw)
    rest, _, _ = call(opener, base, "GET", f"/api/trash?limit=1&cursor={quote(cursor)}")
    need(rest.get("items") and rest["items"][0]["id"] != page["items"][0]["id"],
         "page", "second page must advance", json.dumps(rest).encode())
    for query in ("/api/trash?limit=0", "/api/trash?limit=201"):
        data, raw, _ = call(opener, base, "GET", query)
        need(isinstance(data.get("items"), list), "limit", "listing succeeds", raw)
    passed("GET /api/trash fields and optional pagination")
    restored, raw, _ = call(opener, base, "POST", "/api/trash/restore", {"id": entry})
    need(restored.get("ok") is True and restored.get("uid") == claude, "restore", "want 200 same uid", raw)
    need(listed(opener, base)[0].get("claude-main", {}).get("uid") == claude, "restore", "session not listed with same uid")
    gone, raw, _ = call(opener, base, "POST", "/api/trash/restore", {"id": entry}, want=404)
    need(gone.get("code") == "entry_not_found", "restore", "second restore want 404 entry_not_found", raw)
    passed("restore 200 same uid; restore again 404")
    grok_dir.mkdir(parents=True, exist_ok=True)
    (grok_dir / "summary.json").write_bytes(
        b'{"info":{"id":"grok-keep"},"generated_title":"recreated"}')
    conflict, raw, _ = call(opener, base, "POST", "/api/trash/restore", {"id": grok_entry}, want=409)
    need(conflict.get("code") == "restore_conflict", "conflict", "want 409 restore_conflict", raw)
    passed("recreate original path then restore → 409 restore_conflict")
    purged, raw, _ = call(opener, base, "POST", "/api/trash/purge", {"id": grok_entry})
    need(purged.get("ok") is True and purged.get("removed") == 1, "purge", "want removed=1", raw)
    missing, raw, _ = call(opener, base, "POST", "/api/trash/purge", {"id": grok_entry}, want=404)
    need(missing.get("code") == "entry_not_found", "purge", "unknown id want 404", raw)
    aged, raw, _ = call(opener, base, "POST", "/api/trash/purge", {"days": 30})
    need(aged.get("ok") is True and isinstance(aged.get("remaining"), int), "days", "days purge must report remaining", raw)
    passed("purge by id 200, unknown id 404, days reports remaining")
    # A symlink inside a Grok directory travels as a link (never followed):
    # the directory rename moves it whole.
    chat = corpus.paths["grok-link"] / "chat_history.jsonl"
    chat.unlink()
    os.symlink(corpus.paths["grok-link"] / "summary.json", chat)
    body, raw, code = delete(opener, base, link, "?force=1", want=None)
    need(code == 200 and body.get("files") == 1, "symlink", f"want 200 whole-directory move, got {code}", raw)
    held = [p for p in (trash / body["entry_id"] / "files").iterdir()]
    need(len(held) == 1 and (held[0] / "chat_history.jsonl").is_symlink(), "symlink",
         f"the link moved as a link (not resolved): {[str(p) for p in held]} {[str(q) for q in (held[0].iterdir() if held else [])]}")
    need(not (corpus.paths["grok-link"]).exists(), "symlink", "directory left the root")
    passed("symlink inside a Grok directory moves with it as a link, never followed")
    # A symlinked session directory itself is refused before any move.
    alias = corpus.paths["grok-keep"].parent / "grok-alias"
    os.symlink(corpus.paths["grok-keep"], alias)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-trash-http-") as tmp:
        corpus = build(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, opener):
            unconfigured(opener, base)
        trash = corpus.root / "trash"
        trash.mkdir(); trash.chmod(0o700)
        with isolated_server(corpus, args.binary, trash_dir=trash) as (base, opener):
            configured(opener, base, corpus, trash)


if __name__ == "__main__":
    main()
