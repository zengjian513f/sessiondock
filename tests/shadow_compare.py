#!/usr/bin/env python3
"""Operator-run read-only shadow comparison of CLI histories vs Python adapters.

Isolated loopback server; explicit native read roots only; never writes under them.
Requires --i-understand-this-reads-real-histories.
"""
# run_validation: skip
import argparse, json, os, socket, subprocess, sys, tempfile, time
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import ProxyHandler, build_opener

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from advanced_parity import classify_grok, classify_tools, python_read
from history_parity import REPO
from provider_parity import NoRedirects, load_adapters

SOURCES = ("claude", "codex", "grok")
PLAN = ("生产数据只读影子比对。 Isolated loopback "
        "server with only given read roots; compare sessions/messages vs Python adapters; "
        "never writes under the roots. Pass --i-understand-this-reads-real-histories to run.")


def die(msg, code=1):
    print(msg, file=sys.stderr, flush=True); raise SystemExit(code)

def trim(v):
    return v.rstrip() if isinstance(v, str) else v

def clip(v, n=96):
    t = v if isinstance(v, str) else json.dumps(v, ensure_ascii=False, default=str)
    return t if len(t) <= n else t[:n] + "…"

def multi_chunk(source, L, R):
    """docs/migration.md batch 36 (WP-G): a multi-part Codex/Grok tool output.
    Python `_tool_output` shows one chunk's `output` (the last chunk when the
    joined text ends with a chunk, otherwise the first envelope part); Rust
    shows the concatenation of every chunk in part order, so Python's text is
    the tail or the head of Rust's longer text."""
    if source not in ("codex", "grok") or L.get("role") != "tool_result" or R.get("role") != "tool_result":
        return False
    left, right = trim(L.get("text")) or "", trim(R.get("text")) or ""
    return len(right) > len(left) and (right.startswith(left) or right.endswith(left))

def classify(source, i, key, L, R):
    try:
        hit = (classify_grok if source == "grok" else classify_tools(source))(i, key, L, R)
        if hit:
            return hit
    except Exception:
        pass
    if key in ("text", "exit_code") and multi_chunk(source, L, R):
        return "delta"
    if (key == "exit_code" and L.get("exit_code") is None and R.get("exit_code") is not None
            or key == "text" and L.get("role") == "tool_result" and L.get("text") == "[图片]" and R.get("text") == ""
            # docs/media.md: Codex/Grok add no image placeholder lines to mixed or tool-output text.
            # Envelope output that is empty apart from an image: Python drops the image, Rust shows it.
            or key == "text" and source in ("codex", "grok") and L.get("role") == "tool_result"
            and L.get("text") == "" and R.get("text") == "[图片]" and R.get("media")
            or key == "text" and source in ("codex", "grok") and L.get("role") == R.get("role")
            and "[图片]" in str(L.get("text") or "")
            and trim("\n".join(x for x in str(L.get("text") or "").split("\n") if x.strip() != "[图片]")) == trim(R.get("text"))
            or key == "ts" and source == "grok"
            or key in ("changes", "changes_unavailable_reason") and "512 KiB" in str(R.get("changes_unavailable_reason") or "")):
        return "delta"
    return None

def stamp(paths):
    out = {}
    for path in paths:
        try:
            info = Path(path).stat(); out[str(path)] = (info.st_mtime_ns, info.st_size)
        except OSError:
            out[str(path)] = None
    return out

def pull(opener, url, timeout, cap):
    try:
        with opener.open(url, timeout=timeout) as resp:
            raw, code = resp.read(cap + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(min(65536, cap + 1)), err.code
    if len(raw) > cap:
        raise RuntimeError("oversized " + url)
    try:
        return code, (json.loads(raw) if raw else {})
    except json.JSONDecodeError:
        return code, {"raw": raw[:200].decode("utf-8", "replace")}

@contextmanager
def server(binary, roots):
    binary = binary.resolve(strict=True)
    env = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env.update(SESSIONDOCK_BIND=f"127.0.0.1:{port}", SESSIONDOCK_WEB_DIR=str(REPO / "legacy-web"))
    shown = {}
    for src, path in roots.items():
        key = "SESSIONDOCK_" + src.upper() + "_ROOT"
        env[key] = shown[key] = str(path.resolve(strict=True))
    opener, base = build_opener(ProxyHandler({}), NoRedirects()), f"http://127.0.0.1:{port}"
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen([str(binary)], cwd=REPO, env=env, stdout=log, stderr=log)
        try:
            for _ in range(150):
                if proc.poll() is not None:
                    die(f"isolated Rust server exited early ({proc.returncode})")
                try:
                    with opener.open(base + "/api/health", timeout=1) as resp:
                        resp.read(256); break
                except (OSError, URLError):
                    time.sleep(0.05)
            else:
                die("isolated Rust health check timed out")
            yield base, opener, shown
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill(); proc.wait(timeout=5)

def bind_adapters(python_source, roots):
    inst = load_adapters(python_source, fixture_root=python_source)
    mod = sys.modules["sessiondock.adapters"]
    for src, path in roots.items():
        setattr(mod, src.upper() + "_ROOT", path.resolve(strict=True))
    mod.media.register_path = lambda *a, **k: None
    for name, methods in (("claude", ("list_sessions",)), ("grok", ("list_sessions",)),
                          ("codex", ("list_sessions", "scan_sessions", "_find_session_path"))):
        cls = getattr(mod, name.title() + "Adapter")
        for method in methods:
            setattr(inst[name], method, getattr(cls, method).__get__(inst[name]))
    return inst

def emit(rep, kind, line):
    rep["counts"][kind] += 1
    text = f"{kind} {line}"
    rep["lines"].append(text); print(text, flush=True)
    return kind == "DIFF" and not rep["all"] and rep["counts"]["DIFF"] >= 50

def reason(row):
    warns = row.get("migration_warnings") or []
    return "" if row.get("supported") is not False else " supported:false " + ",".join(map(str, warns))

def compare_one(source, label, python, rust):
    messages, statuses, _end = python
    actual, diffs, deltas = rust.get("messages") or [], [], []
    if len(messages) != len(actual):
        diffs.append(f"count Python={len(messages)} Rust={len(actual)}")
    counted = sum(m.get("counted") is not False for m in messages)
    if counted != rust.get("message_total"):
        diffs.append(f"counted Python={counted} Rust={rust.get('message_total')}")
    for i, (L, R) in enumerate(zip(messages, actual)):
        if L.get("role") != R.get("role") or trim(L.get("text")) != trim(R.get("text")):
            hit = classify(source, i, "text", L, R)
            note = (f" [multi-chunk: Python {len(trim(L.get('text')) or '')} chars, Rust {len(trim(R.get('text')) or '')} chars]"
                    if multi_chunk(source, L, R) else "")
            (deltas if hit in {"delta", "known"} and L.get("role") == R.get("role") else diffs).append(
                f"message[{i}] {L.get('role')}/{R.get('role')} {clip(trim(L.get('text')))!r}->{clip(trim(R.get('text')))!r}{note}")
        if (L.get("counted") is not False) != (R.get("counted") is not False):
            diffs.append(f"message[{i}] counted")
        deltas.extend(k for k in ("exit_code", "ts", "changes", "changes_unavailable_reason")
                      if L.get(k) != R.get(k) and classify(source, i, k, L, R) in {"delta", "known"})
    pa, ra = (statuses[-1] if statuses else None) or {}, rust.get("activity") or {}
    if pa.get("role") != ra.get("role") or trim(pa.get("text")) != trim(ra.get("text")):
        diffs.append("activity")
    noted = f"{label} documented {sorted(set(deltas))}"
    if diffs:
        return [("DIFF", f"{label} {x}") for x in diffs[:8]] + ([("DELTA", noted)] if deltas else [])
    return [("DELTA", noted)] if deltas else [("PASS", f"{label} {len(actual)} messages, {counted} counted")]

def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__)
    for src in SOURCES:
        p.add_argument(f"--{src}-root", type=Path)
    p.add_argument("--python-source", type=Path, required=True)
    p.add_argument("--binary", type=Path, required=True)
    p.add_argument("--sample", type=int, default=5)
    p.add_argument("--uid", action="append", default=[], metavar="UID")
    p.add_argument("--windowed", action="store_true")
    p.add_argument("--timeout", type=int, default=30)
    p.add_argument("--max-bytes", type=int, default=64 * 1024 * 1024)
    p.add_argument("--json", type=Path, dest="json_out")
    p.add_argument("--all", action="store_true")
    p.add_argument("--i-understand-this-reads-real-histories", action="store_true", dest="consent")
    args = p.parse_args(argv)
    roots = {s: getattr(args, s + "_root") for s in SOURCES if getattr(args, s + "_root")}
    if not roots:
        die("need at least one of --claude-root / --codex-root / --grok-root")
    if not args.consent:
        print(PLAN, flush=True)
        print("\n".join(f"  SESSIONDOCK_{s.upper()}_ROOT={path}" for s, path in roots.items()), flush=True)
        raise SystemExit(2)
    bad = [str(path) for path in roots.values() if not path.is_dir()]
    if bad:
        die("not a directory: " + ", ".join(bad))
    uids = [x.strip() for item in args.uid for x in item.split(",") if x.strip()]
    py_src, binary = Path.cwd() / args.python_source, Path.cwd() / args.binary
    if args.json_out and any(args.json_out.resolve().is_relative_to(path.resolve()) for path in roots.values()):
        die("--json OUT must not be under a native root")
    adapters = bind_adapters(py_src, roots)
    py_rows = [row for src in roots for row in (adapters[src].list_sessions() or [])]
    py_by_uid = {row["uid"]: row for row in py_rows if row.get("uid")}
    before = stamp(row["path"] for row in py_rows if row.get("path"))
    rep = {"counts": {"PASS": 0, "DELTA": 0, "DIFF": 0}, "lines": [], "all": args.all}
    started = time.perf_counter()
    with server(binary, roots) as (base, opener, env):
        print("\n".join(f"ENV {k}={v}" for k, v in env.items()), flush=True)
        code, listed = pull(opener, base + "/api/sessions?force=1", args.timeout, args.max_bytes)
        if code != 200 or not isinstance(listed.get("sessions"), list):
            die(f"/api/sessions HTTP {code} {clip(listed)}")
        rust_rows = listed["sessions"]
        for src in roots:
            py_sids = {r["sid"] for r in py_rows if r.get("source") == src}
            rs = [r for r in rust_rows if r.get("source") == src]
            rs_sids = {r["sid"] for r in rs}
            for sid in sorted(rs_sids - py_sids):
                row = next(r for r in rs if r["sid"] == sid)
                kind = "DELTA" if row.get("supported") is False else "DIFF"
                if emit(rep, kind, f"inventory {src} rust extra sid={sid} uid={row.get('uid')}{reason(row)}"):
                    break
            for sid in sorted(py_sids - rs_sids):
                if emit(rep, "DIFF", f"inventory {src} python extra sid={sid}"):
                    break
            if py_sids == rs_sids:
                emit(rep, "PASS", f"inventory {src}: {len(py_sids)} sids")
        if uids:
            want = set(uids)
            chosen = [r for r in rust_rows if r.get("uid") in want]
            for uid in sorted(want - {r.get("uid") for r in chosen}):
                emit(rep, "DIFF", f"unknown --uid {uid}")
        else:
            chosen = []
            for src in SOURCES:
                group = [r for r in rust_rows if r.get("source") == src]
                group.sort(key=lambda r: r.get("updated") or "", reverse=True)
                chosen.extend(group[:args.sample])
        query = "?window=1" if args.windowed else ""
        for row in chosen:
            if not rep["all"] and rep["counts"]["DIFF"] >= 50:
                break
            uid, src, sid = row.get("uid"), row.get("source"), row.get("sid")
            label, py_row = f"{src}:{sid} {uid}", py_by_uid.get(uid)
            if row.get("supported") is False:
                emit(rep, "DELTA", f"{label} skip messages{reason(row)}")
                continue
            if not py_row or not py_row.get("path"):
                emit(rep, "DIFF", f"{label} python path missing")
                continue
            try:
                code, body = pull(opener, base + "/api/messages/" + quote(uid, safe=":") + query,
                                  args.timeout, args.max_bytes)
                python = python_read(adapters, src, py_row["path"]) if code == 200 else None
            except Exception as err:
                emit(rep, "DIFF", f"{label} {err}")
                continue
            if code != 200 or python is None:
                emit(rep, "DIFF", f"{label} HTTP {code} {clip(body)}")
                continue
            for kind, line in compare_one(src, label, python, body):
                if emit(rep, kind, line):
                    break
        after = stamp(row["path"] for row in py_rows if row.get("path"))
        emit(rep, "PASS" if after == before else "DIFF", "roots unchanged" if after == before else "native roots were written")
    elapsed = time.perf_counter() - started
    summary = (f"SUMMARY {rep['counts']['PASS']} PASS, {rep['counts']['DELTA']} DELTA, "
               f"{rep['counts']['DIFF']} DIFF, {elapsed:.1f}s")
    print(summary, flush=True)
    if args.json_out:
        payload = {"env": {k: str(v) for k, v in roots.items()}, "lines": rep["lines"],
                   "counts": rep["counts"], "elapsed_s": round(elapsed, 3), "summary": summary}
        args.json_out.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
    raise SystemExit(1 if rep["counts"]["DIFF"] else 0)

if __name__ == "__main__":
    main()
