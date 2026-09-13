#!/usr/bin/env python3
"""One-shot M8 converter: Python session-meta.json → Rust session-metadata.json.

With --python-debug-runs (Python's debug-runs.json, same directory by default)
the debug-run registry is copied beside it as debug-runs.json: the Rust read
model consults it in Python's format (docs/read-model.md "debug_run"), so the
registered monkey/test sessions stay hidden after the cutover."""
# run_validation: skip
import argparse, json, os, re, socket, stat, subprocess, sys, tempfile, time
from datetime import datetime
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, build_opener

REPO = Path(__file__).resolve().parents[1]
UID_RE = re.compile(r"^[A-Za-z0-9:._-]{1,256}$")
MAX_ROWS, MAX_BYTES, REASON_MAX, TIP_MAX = 10_000, 4 * 1024 * 1024, 2048, 256
EPOCH_MAX = 253_402_300_799.0
DEFAULT_REASON, INFERRED_REASON = "网页发送 Escape", "终端已结束或中断"
KNOWN = {"starred", "starred_at", "fork_parent_visible", "activity_stopped_at",
         "activity_stop_reason", "activity_stop_state", "activity_stop_inferred",
         "timeline_tip", "timeline_stale_end", "timeline_rewind", "spawned_by"}
ORDER = ("starred", "starred_at", "fork_parent_visible", "stopped", "rewind_pending", "timeline", "spawned_by")
SPAWN_SOURCE_MAX, SPAWN_SID_MAX = 32, 256
HOME_SHARE = Path.home() / ".local" / "share" / "sessiondock"


def die(msg, code=1):
    print(msg, flush=True); raise SystemExit(code)

def epoch(value):
    try:
        if isinstance(value, bool):
            return None
        at = float(value) if isinstance(value, (int, float)) else datetime.fromisoformat(str(value).replace("Z", "+00:00")).timestamp()
        return at if 0.0 <= at <= EPOCH_MAX else None
    except (TypeError, ValueError, OverflowError, OSError):
        return None

def as_int(value):
    try:
        return int(value or 0)
    except (TypeError, ValueError):
        return -1

def clip_reason(text):
    raw, cut = text.encode(), text.encode()[:REASON_MAX]
    while True:
        try:
            return cut.decode(), len(raw) > REASON_MAX
        except UnicodeDecodeError:
            cut = cut[:-1]

def convert_row(uid, src, keep, warns):
    if not isinstance(src, dict):
        return None, "not an object"
    out = {}
    warns += [(uid, f"unknown field {key}") for key in src if key not in KNOWN]
    if src.get("starred") is True:
        raw_at = src.get("starred_at")
        at = epoch(raw_at) if isinstance(raw_at, (int, float)) and not isinstance(raw_at, bool) else None
        out["starred"] = True
        if at is not None:
            out["starred_at"] = at
        else:
            warns.append((uid, "invalid starred_at omitted"))
    if src.get("fork_parent_visible") is True:
        out["fork_parent_visible"] = True
    # Python live.spawn_parents wrote {source, sid} once; the spawner row may no
    # longer exist, the relation is kept as-is (Rust decorates from the key alone).
    spawned = src.get("spawned_by")
    if spawned is not None:
        source = str(spawned.get("source") or "").strip() if isinstance(spawned, dict) else ""
        sid = str(spawned.get("sid") or "").strip() if isinstance(spawned, dict) else ""
        if (not source or not sid or len(source.encode()) > SPAWN_SOURCE_MAX or len(sid.encode()) > SPAWN_SID_MAX
                or any(ch.isspace() or ord(ch) < 32 for ch in source + sid)):
            warns.append((uid, "invalid spawned_by"))
        else:
            out["spawned_by"] = {"source": source, "sid": sid}
    if src.get("activity_stopped_at") is not None:
        at, state = epoch(src.get("activity_stopped_at")), src.get("activity_stop_state") or "aborted"
        if at is None:
            warns.append((uid, "unparseable activity_stopped_at"))
        elif state not in ("idle", "aborted"):
            warns.append((uid, "invalid activity_stop_state"))
        else:
            original = str(src.get("activity_stop_reason") or DEFAULT_REASON)
            reason, truncated = clip_reason(original)
            if truncated:
                warns.append((uid, "activity_stop_reason truncated"))
            out["stopped"] = {"at": at, "reason": reason, "state": state,
                              "inferred": bool(src.get("activity_stop_inferred")) or original == INFERRED_REASON}
    tip = src.get("timeline_tip")
    if isinstance(tip, str) and tip:
        stale = as_int(src.get("timeline_stale_end"))
        if len(tip.encode()) > TIP_MAX or stale < 0:
            warns.append((uid, "timeline_tip too long" if len(tip.encode()) > TIP_MAX else "invalid timeline_stale_end"))
        else:
            out["timeline"] = {"tip": tip, "stale_end": stale}
    pending = src.get("timeline_rewind")
    if pending is None:
        return (out or None), None
    if not keep or not isinstance(pending, dict):
        warns.append((uid, "dropped pending rewind" if not keep else "invalid timeline_rewind"))
        return (out or None), None
    from_tip, started, stale = str(pending.get("from_tip") or "").strip(), epoch(pending.get("started_at")), as_int(pending.get("stale_end"))
    if not from_tip or len(from_tip.encode()) > TIP_MAX or stale < 0 or started is None:
        warns.append((uid, "invalid timeline_rewind"))
    else:
        out["rewind_pending"] = {"from_tip": from_tip, "stale_end": stale, "started_at": started}
    return (out or None), None

def lmode(path):
    try:
        return os.lstat(path).st_mode
    except OSError:
        return 0

def check_safety(meta, out):
    if not stat.S_ISREG(lmode(meta)):
        die("--python-meta is not a regular file")
    mode = lmode(out)
    if stat.S_ISLNK(mode) or not stat.S_ISDIR(mode):
        die("out-dir is a symlink" if stat.S_ISLNK(mode) else "--out-dir is not an existing directory")
    meta_dir, dest, share = meta.resolve().parent, out.resolve(), HOME_SHARE.resolve()
    if dest.is_relative_to(meta_dir):
        die("out-dir is the python-meta directory or inside it")
    if dest.is_relative_to(share):
        die("out-dir is $HOME/.local/share/sessiondock or inside it")
    if (dest / "session-metadata.json").exists() or (dest / ".metadata.lock").exists():
        die("out-dir already contains session-metadata.json or .metadata.lock; remove it or choose another directory")
    if (dest / "debug-runs.json").exists():
        die("out-dir already contains debug-runs.json; remove it or choose another directory")
    mode = dest.stat().st_mode
    if mode & 0o022 or (mode & 0o300) != 0o300:
        die(f"out-dir permissions unsafe; chmod 700 {dest}")
    return dest

def write_atomic(out, data, name="session-metadata.json"):
    tmp, dest = out / f".metadata-tmp-import-{os.getpid()}", out / name
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        view = memoryview(data)
        while view:
            view = view[os.write(fd, view):]
        os.fsync(fd); os.fchmod(fd, 0o600)
    except Exception:
        os.close(fd)
        try: os.unlink(tmp)
        except OSError: pass
        raise
    os.close(fd); os.replace(tmp, dest); os.chmod(dest, 0o600)
    return dest.resolve()

RUN_ID_RE = re.compile(r"^[A-Za-z0-9_-]{1,64}$")
DEBUG_RUNS_MAX = 64 * 1024 * 1024


def convert_debug_runs(path, warns):
    """Python `debug_runs._read`: `{"version":1,"runs":{...}}`, anything else empty.

    Runs are copied verbatim (root, created, sessions[{source,cwd,sid,uid,name}]);
    a run whose id Python would refuse, or that is not an object, is dropped with
    a warning — the Rust reader would ignore it too, this only makes it visible.
    """
    if not stat.S_ISREG(lmode(path)):
        die("--python-debug-runs is not a regular file")
    try:
        raw = json.loads(path.read_text())
    except (OSError, ValueError) as err:
        die(f"cannot read python-debug-runs: {err}")
    runs = raw.get("runs") if isinstance(raw, dict) else None
    if not isinstance(runs, dict):
        die('python-debug-runs must be {"version": 1, "runs": {...}}')
    out, sessions = {}, 0
    for run_id, run in runs.items():
        if not isinstance(run_id, str) or not RUN_ID_RE.fullmatch(run_id):
            warns.append((str(run_id), "invalid debug_run id dropped")); continue
        if not isinstance(run, dict):
            warns.append((run_id, "debug run is not an object; dropped")); continue
        rows = [dict(row) for row in (run.get("sessions") or []) if isinstance(row, dict)]
        root = str(run.get("root") or "")
        if not root or root != os.path.normpath(root) or not os.path.isabs(root):
            warns.append((run_id, "debug run root is not a normalized absolute path (cwd matching disabled)"))
        out[run_id] = {"root": root, "created": run.get("created"),
                       "sessions": [{k: str(row.get(k) or "") for k in ("source", "cwd", "sid", "uid", "name")}
                                    for row in rows]}
        sessions += len(rows)
    return {"version": 1, "runs": out}, sessions


def verify(binary, out, roots, imported):
    binary = binary.resolve(strict=True)
    env = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0)); port = sock.getsockname()[1]
    env.update(SESSIONDOCK_BIND=f"127.0.0.1:{port}", SESSIONDOCK_WEB_DIR=str(REPO / "legacy-web"),
               SESSIONDOCK_STATE_DIR=str(out.resolve()))
    for src, path in roots.items():
        env["SESSIONDOCK_" + src.upper() + "_ROOT"] = str(path.resolve(strict=True))
    opener, base = build_opener(ProxyHandler({})), f"http://127.0.0.1:{port}"
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen([str(binary)], cwd=REPO, env=env, stdout=log, stderr=log)
        def tail():
            log.seek(0); return log.read()[-4000:].decode("utf-8", "replace")
        try:
            code = None
            for _ in range(150):
                if proc.poll() is not None:
                    die("VERIFY FAIL early exit " + tail())
                try:
                    with opener.open(base + "/api/health", timeout=1) as resp:
                        code = resp.status; resp.read(256); break
                except HTTPError as err:
                    code = err.code; break
                except (OSError, URLError):
                    time.sleep(0.05)
            else:
                die("VERIFY FAIL health timeout " + tail())
            if code != 200:
                die(f"VERIFY FAIL health {code} " + tail())
            print("VERIFY health 200", flush=True)
            if not roots:
                return
            try:
                with opener.open(base + "/api/sessions?force=1", timeout=10) as resp:
                    status, raw = resp.status, resp.read(2 * 1024 * 1024 + 1)
            except (HTTPError, OSError, URLError) as err:
                status, raw = getattr(err, "code", None), err.read(8192) if isinstance(err, HTTPError) else str(err).encode()
            listed = json.loads(raw) if raw and status == 200 else {}
            rows = listed.get("sessions") if isinstance(listed, dict) else None
            if status != 200 or not isinstance(rows, list):
                die(f"VERIFY FAIL /api/sessions {status} " + tail())
            by_uid = {row.get("uid"): row for row in rows if isinstance(row, dict)}
            for uid, row in imported.items():
                if uid not in by_uid:
                    continue
                got, details = by_uid[uid], []
                want_s, got_s = row.get("starred") is True, got.get("starred") is True
                if want_s != got_s:
                    details.append(f"starred {got_s!r}!={want_s!r}")
                if want_s:
                    w, g = row.get("starred_at"), got.get("starred_at")
                    # serde_json's default float parser is best-effort (no float_roundtrip
                    # feature): a 1-ULP difference on an epoch is not a lost star.
                    if g != w and (g is None or w is None or abs(float(g) - float(w)) > 1e-3):
                        details.append(f"starred_at {g!r}!={w!r}")
                if got.get("fork_parent") is True or "fork_parent_visible" in got:
                    if (row.get("fork_parent_visible") is True) != (got.get("fork_parent_visible") is True):
                        details.append("fork_parent_visible")
                if row.get("spawned_by") != got.get("spawned_by"):
                    details.append(f"spawned_by {got.get('spawned_by')!r}!={row.get('spawned_by')!r}")
                if details:
                    die(f"VERIFY {uid} mismatch {', '.join(details)}")
                print(f"VERIFY {uid} ok", flush=True)
        finally:
            if proc.poll() is None:
                proc.terminate()
                try: proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill(); proc.wait(timeout=5)

def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--python-meta", type=Path, required=True)
    p.add_argument("--out-dir", type=Path, required=True)
    p.add_argument("--python-debug-runs", type=Path,
                   help="Python debug-runs.json to copy beside the metadata (default: next to --python-meta when present)")
    p.add_argument("--no-debug-runs", action="store_true", help="do not carry the debug-run registry over")
    p.add_argument("--keep-pending", action="store_true")
    p.add_argument("--json", type=Path, dest="json_out")
    p.add_argument("--verify", action="store_true"); p.add_argument("--binary", type=Path)
    for src in ("claude", "codex", "grok"):
        p.add_argument(f"--{src}-root", type=Path)
    args = p.parse_args(argv)
    if args.verify and not args.binary:
        die("--verify requires --binary", 2)
    dest_dir = check_safety(args.python_meta, args.out_dir)
    try:
        data = json.loads(args.python_meta.read_text())
    except (OSError, ValueError) as err:
        die(f"cannot read python-meta: {err}")
    if not isinstance(data, dict) or data.get("version") != 1 or not isinstance(data.get("sessions"), dict):
        die('python-meta must be {"version": 1, "sessions": {...}}')
    imports, warns, skips, rows = [], [], [], {}
    for uid, src in data["sessions"].items():
        if not isinstance(uid, str) or not UID_RE.fullmatch(uid):
            skips.append((uid, "invalid uid")); continue
        converted, reason = convert_row(uid, src, args.keep_pending, warns)
        if reason:
            skips.append((uid, reason)); continue
        if converted:
            len(rows) < MAX_ROWS or die("more than 10000 emitted rows")
            rows[uid] = converted
            imports.append((uid, ",".join(k for k in ORDER if k in converted)))
    blob = (json.dumps({"schema_version": 1, "revision": 1, "sessions": rows},
                       ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode()
    if len(blob) > MAX_BYTES:
        die("output document larger than 4 MiB")
    dest = write_atomic(dest_dir, blob)
    debug_lines = []
    debug_src = args.python_debug_runs
    if debug_src is None and not args.no_debug_runs:
        candidate = args.python_meta.resolve().parent / "debug-runs.json"
        debug_src = candidate if candidate.is_file() else None
    if debug_src is not None and not args.no_debug_runs:
        registry, count = convert_debug_runs(debug_src, warns)
        debug_blob = (json.dumps(registry, ensure_ascii=False, indent=2) + "\n").encode()
        if len(debug_blob) > DEBUG_RUNS_MAX:
            die("debug-runs document larger than 64 MiB")
        debug_dest = write_atomic(dest_dir, debug_blob, "debug-runs.json")
        debug_lines = [f"DEBUG_RUNS runs={len(registry['runs'])} sessions={count} bytes={len(debug_blob)} out={debug_dest}"]
    lines = ([f"IMPORT {u} {n}" for u, n in imports]
             + [f"WARN {u} {t}" for u, t in warns]
             + [f"SKIP {u} {r}" for u, r in skips]
             + debug_lines
             + [f"SUMMARY imported={len(imports)} skipped={len(skips)} warnings={len(warns)} bytes={len(blob)} out={dest}"])
    print("\n".join(lines), flush=True)
    if args.json_out:
        args.json_out.write_text(json.dumps({"imported": len(imports), "skipped": len(skips),
            "warnings": len(warns), "bytes": len(blob), "out": str(dest), "lines": lines},
            ensure_ascii=False, indent=2) + "\n")
    if not args.verify:
        return
    if not args.binary.is_file():
        die(f"binary not found: {args.binary}")
    roots = {s: getattr(args, s + "_root") for s in ("claude", "codex", "grok") if getattr(args, s + "_root")}
    bad = [str(path) for path in roots.values() if not path.is_dir()]
    if bad:
        die("not a directory: " + ", ".join(bad))
    if args.json_out and any(args.json_out.resolve().is_relative_to(path.resolve()) for path in roots.values()):
        die("--json must not be under a native root")
    verify(args.binary, dest_dir, roots, rows)

if __name__ == "__main__":
    main()
