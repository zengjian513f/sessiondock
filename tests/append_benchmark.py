#!/usr/bin/env python3
"""Free synthetic append benchmark and cursor correctness checks.

Each source/size/sample starts a fresh explicitly configured loopback server.
Timings include HTTP transfer and Python JSON decoding, not process startup or
fixture writes. They are observations, not performance pass/fail thresholds.
Optional --rss separately samples the exact child on Linux outside HTTP timings.
Run the SAME script with --binary pointing to each saved
release binary on an otherwise idle machine. No CLI/model/native home is used.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import json
import math
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.error import URLError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, build_opener

from history_parity import claude_row, codex_message, codex_row, encoded
from provider_parity import NoRedirects

REPO = Path(__file__).resolve().parents[1]
DEFAULT_BINARY = REPO / "target/release" / ("sessiondock.exe" if os.name == "nt" else "sessiondock")
MAX_HTTP_BYTES = 20 * 1024 * 1024
SOURCES = ("claude", "codex", "grok")


def fetch(opener, base, route):
    started = time.perf_counter()
    with opener.open(base + route, timeout=60) as response:
        status = response.status
        raw = response.read(MAX_HTTP_BYTES + 1)
        assert len(raw) <= MAX_HTTP_BYTES, "synthetic response exceeded limit"
        value = json.loads(raw)
    return value, {"ms": round((time.perf_counter() - started) * 1000, 3),
                   "http_status": status, "http_bytes": len(raw)}


@contextmanager
def server(root, binary):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    # Explicit roots, no inherited credentials, CLI configuration or proxies.
    environment = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8",
                   "SESSIONDOCK_BIND": f"127.0.0.1:{port}",
                   "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web")}
    if os.name == "nt" and "SystemRoot" in os.environ:
        environment["SystemRoot"] = os.environ["SystemRoot"]
    for source in SOURCES:
        environment[f"SESSIONDOCK_{source.upper()}_ROOT"] = str(root / source)
    opener = build_opener(ProxyHandler({}), NoRedirects())
    with tempfile.TemporaryFile(mode="w+b") as log:
        process = subprocess.Popen([str(binary)], cwd=root, env=environment,
                                   stdout=log, stderr=log)
        try:
            for _ in range(200):
                assert process.poll() is None, "isolated server exited before readiness"
                try:
                    fetch(opener, base, "/api/health")
                    break
                except (OSError, URLError):
                    time.sleep(.025)
            else:
                raise AssertionError("isolated server readiness timed out")
            yield base, opener, process
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()  # Only this exact isolated child.
                    process.wait(timeout=5)
                    raise AssertionError("isolated server failed graceful shutdown")


def record(source, sid, index, text):
    role = "user" if index % 2 == 0 else "assistant"
    if source == "claude":
        return claude_row(sid, role, f"row-{index:06d}",
                          f"row-{index - 1:06d}" if index else None, text)
    if source == "codex":
        return codex_message(role, text, index + 1)
    return {"type": role, "content": text, "prompt_index": index // 2,
            "timestamp": "2026-09-11T10:00:00Z"}


def fixture(root, source, count):
    for name in SOURCES:
        (root / name).mkdir()
    sid = "synthetic-append-benchmark"
    message_count = count - (source == "codex")
    texts = [f"row-{index:06d}-ORIGINAL synthetic payload " + "x" * 64
             for index in range(message_count)]
    rows = [record(source, sid, index, text) for index, text in enumerate(texts)]
    if source == "claude":
        identity_path = root / "claude/project" / f"{sid}.jsonl"
        path = identity_path
    elif source == "codex":
        identity_path = root / "codex/2026/09/11" / f"rollout-{sid}.jsonl"
        path = identity_path
        rows.insert(0, codex_row("session_meta", {"id": sid, "cwd": "/synthetic/append"}))
    else:
        identity_path = root / "grok/%2Fsynthetic%2Fappend" / sid
        path = identity_path / "chat_history.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    if source == "grok":
        (identity_path / "summary.json").write_bytes(encoded({
            "info": {"id": sid, "cwd": "/synthetic/append"},
            "generated_title": "Synthetic append benchmark",
            "created_at": "2026-09-11T10:00:00Z",
            "updated_at": "2026-09-11T10:00:00Z"}))
    original = b"".join(encoded(row) for row in rows)
    path.write_bytes(original)
    uid = source + ":" + hashlib.sha1(str(identity_path).encode()).hexdigest()[:16]
    return path, uid, sid, texts, original


def delta_query(snapshot):
    return urlencode({"start": snapshot["end"], "head": snapshot["version"]["head"],
                      "anchor": snapshot["anchor"], "append": "1", "window": "1"})


def observed(snapshot, timing, prior=None):
    timing.update({"reset": snapshot["reset"], "start": snapshot["start"],
                   "end": snapshot["end"], "messages": len(snapshot["messages"]),
                   "message_total": snapshot["message_total"],
                   "activity_changed": snapshot["activity_changed"]})
    if prior is not None:
        timing["head_changed"] = snapshot["version"]["head"] != prior["version"]["head"]
        timing["anchor_changed"] = snapshot["anchor"] != prior["anchor"]
    return timing


def check_window(snapshot, texts):
    expected = texts if len(texts) <= 600 else texts[:100] + texts[-500:]
    assert snapshot["reset"] is True
    assert [message["text"] for message in snapshot["messages"]] == expected
    assert snapshot["message_total"] == len(texts)
    assert snapshot["meta"]["supported"] is True
    assert snapshot["meta"]["cursor"]["end"] == snapshot["end"]


def server_memory(process):
    """Optional Linux evidence for this exact live child, never a process search."""
    if process.poll() is not None:
        raise AssertionError("isolated child exited before RSS observation")
    result = {}
    for line in Path(f"/proc/{process.pid}/status").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in ("VmRSS", "VmHWM"):
            count, unit = value.split()
            assert unit == "kB"
            result[key] = int(count) * 1024
    assert set(result) == {"VmRSS", "VmHWM"}
    return result


def run(binary, source, count, sample, rss=False):
    with tempfile.TemporaryDirectory(prefix="sessiondock-append-bench-") as temporary:
        root = Path(temporary).resolve()
        path, uid, sid, texts, original = fixture(root, source, count)
        with server(root, binary) as (base, opener, process):
            route = "/api/messages/" + uid
            # No list request first: this measurement includes cold inventory,
            # deserialization, semantic projection, view construction and HTTP.
            initial, initial_cost = fetch(opener, base, route + "?window=1")
            check_window(initial, texts)
            assert initial["end"] == len(original)
            initial_memory = server_memory(process) if rss else None
            idle, idle_cost = fetch(opener, base, route + "?" + delta_query(initial))
            assert not idle["reset"] and idle["messages"] == []
            assert idle["end"] == initial["end"] and idle["anchor"] == initial["anchor"]

            tail_text = "synthetic-single-appended-message"
            tail = encoded(record(source, sid, len(texts), tail_text))
            with path.open("ab") as stream:
                stream.write(tail)
            appended, append_cost = fetch(opener, base, route + "?" + delta_query(initial))
            assert not appended["reset"]
            assert [message["text"] for message in appended["messages"]] == [tail_text]
            assert appended["start"] == initial["end"]
            assert appended["end"] == len(original) + len(tail)
            assert appended["version"]["head"] == initial["version"]["head"]

            # Rewrite a visible tail message well beyond the 4 KiB head, without
            # replacing the inode or changing length. Unix restores mtime too;
            # ctime and full-prefix equality must not be confused with append.
            index = len(texts) - 5
            old = texts[index].encode()
            new_text = texts[index].replace("ORIGINAL", "MODIFIED")
            new = new_text.encode()
            before = path.read_bytes()
            offset = before.index(old)
            assert offset > 4096 and len(old) == len(new)
            saved = path.stat()
            with path.open("r+b") as stream:
                stream.seek(offset)
                stream.write(new)
            if os.name == "posix":
                os.utime(path, ns=(saved.st_atime_ns, saved.st_mtime_ns))
            assert path.stat().st_size == len(before)
            rewritten, rewrite_cost = fetch(opener, base, route + "?" + delta_query(appended))
            assert rewritten["reset"] is True and rewritten["messages"] == []
            assert rewritten["start"] == rewritten["end"] == appended["end"]
            assert rewritten["version"]["head"] == appended["version"]["head"]
            assert rewritten["anchor"] != appended["anchor"]

            texts[index] = new_text
            texts.append(tail_text)
            reloaded, reload_cost = fetch(opener, base, route + "?window=1")
            check_window(reloaded, texts)
            assert reloaded["anchor"] == rewritten["anchor"]
            final, final_cost = fetch(opener, base, route + "?" + delta_query(reloaded))
            assert not final["reset"] and final["messages"] == []
            assert final["anchor"] == reloaded["anchor"]
            final_memory = server_memory(process) if rss else None
            result = {"kind": "sample", "source": source, "records": count,
                      "sample": sample, "native_bytes": len(original),
                      "mtime_restored_on_rewrite": os.name == "posix",
                      "first_window": observed(initial, initial_cost),
                      "idle_delta": observed(idle, idle_cost, initial),
                      "append_one": observed(appended, append_cost, initial),
                      "same_length_rewrite": observed(rewritten, rewrite_cost, appended),
                      "reload_window": observed(reloaded, reload_cost),
                      "final_idle": observed(final, final_cost, reloaded),
                      "cursor_checks": "PASS"}
            if rss:
                result["memory"] = {"first_window": initial_memory, "final": final_memory}
            print(json.dumps(result, ensure_ascii=False), flush=True)
            return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--sizes", type=int, nargs="+", default=[1000, 5000, 10000])
    parser.add_argument("--sources", choices=SOURCES, nargs="+", default=list(SOURCES))
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--rss", action="store_true", help="Linux-only: exact child VmRSS/VmHWM outside HTTP timings")
    args = parser.parse_args()
    if args.rss and not Path("/proc/self/status").is_file():
        parser.error("--rss requires Linux procfs; no inferred cross-platform memory figures")
    if not 1 <= args.samples <= 20 or any(not 1000 <= size <= 10000 for size in args.sizes):
        parser.error("samples must be 1..20 and each size 1000..10000")
    binary = args.binary.resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("--binary must name an existing executable server binary")
    with binary.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    print(json.dumps({"kind": "benchmark", "binary_sha256": digest,
                      "binary_bytes": binary.stat().st_size, "samples": args.samples,
                      "timing": "loopback HTTP + JSON decode; no fixture writes/startup",
                      "cache": "fresh process, not a cold OS page cache",
                      "platform": os.name}), flush=True)
    for size in args.sizes:
        for source in args.sources:
            rows = [run(binary, source, size, sample + 1, args.rss) for sample in range(args.samples)]
            stats = {}
            for phase in ("first_window", "idle_delta", "append_one", "same_length_rewrite"):
                values = sorted(row[phase]["ms"] for row in rows)
                stats[phase] = {"p50_ms": values[math.ceil(len(values) * .5) - 1],
                                "p95_ms": values[math.ceil(len(values) * .95) - 1]}
            result = {"kind": "summary", "source": source, "records": size,
                      "samples": args.samples, "measurements": stats}
            if args.rss:
                result["memory_p50_bytes"] = {
                    phase: {field: sorted(row["memory"][phase][field] for row in rows)[math.ceil(len(rows) * .5) - 1]
                            for field in ("VmRSS", "VmHWM")}
                    for phase in ("first_window", "final")}
            print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
