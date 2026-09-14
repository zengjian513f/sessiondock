#!/usr/bin/env python3
"""Compare the checked-in synthetic M1 fixtures against the Python adapters.

Optional development tool only; the Rust server never imports or runs Python.
Requires an explicitly selected Python source checkout and a loopback Rust
server configured with crates/sessiondock/tests/fixtures/{claude,codex,grok}.
No index API, home/session discovery, CLI, production fixture, or external URL
is accepted. Environment HTTP proxies and redirects are disabled.
For generated advanced histories and an automatically isolated Rust server,
run history_parity.py --python-source PATH instead.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import ipaddress
import json
from pathlib import Path
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlsplit, urlunsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, build_opener

from python_oracle import import_oracle_module, package_dir

sys.dont_write_bytecode = True

FIXTURES = Path(__file__).resolve().parents[1] / "crates/sessiondock/tests/fixtures"
CASES = {
    "claude": "claude/project-demo/synthetic-claude.jsonl",
    "codex": "codex/2026/09/11/rollout-synthetic-codex.jsonl",
    "grok": "grok/project-demo/synthetic-grok",
}
FIELDS = (
    "role", "text", "name", "args", "turn_id", "phase", "call_id", "error",
    "counted", "silent", "questions", "event_kind", "duration_ms", "state",
    "interrupted", "interrupt_reason", "reason", "exit_code", "duration_s",
)
MAX_RESPONSE_BYTES = 2 * 1024 * 1024


class NoRedirects(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise RuntimeError("redirect refused: only the explicit fixture server is in scope")


def local_base(raw: str) -> str:
    parsed = urlsplit(raw)
    if parsed.scheme != "http" or parsed.username or parsed.password:
        raise ValueError("--base-url must be an unauthenticated local http URL")
    if parsed.query or parsed.fragment or parsed.path not in {"", "/"}:
        raise ValueError("--base-url must name the local server root")
    host = parsed.hostname or ""
    # Resolve localhost ourselves; never depend on DNS or hosts-file redirection.
    if host == "localhost":
        host = "127.0.0.1"
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        raise ValueError("--base-url host must be localhost or a loopback IP") from None
    if not address.is_loopback or parsed.port is None:
        raise ValueError("--base-url requires a loopback IP and an explicit port")
    authority = f"[{host}]:{parsed.port}" if address.version == 6 else f"{host}:{parsed.port}"
    return urlunsplit(("http", authority, "", "", ""))


def fixture_paths() -> dict[str, Path]:
    root = FIXTURES.resolve(strict=True)
    paths = {}
    for source, relative in CASES.items():
        selected = root / relative
        if any(part.is_symlink() for part in [selected, *selected.parents]
               if part.is_relative_to(root)):
            raise RuntimeError("symlink fixtures are not accepted")
        path = selected.resolve(strict=True)
        if not path.is_relative_to(root):
            raise RuntimeError("fixture path escapes the checked-in fixture directory")
        data_path = path / "chat_history.jsonl" if source == "grok" else path
        records = [json.loads(line) for line in data_path.read_text(encoding="utf-8").splitlines() if line]
        if source == "claude":
            if records[0].get("sessionId") != "synthetic-claude":
                raise RuntimeError("expected the synthetic Claude fixture")
        elif source == "codex":
            metadata = records[0].get("payload", {})
            if metadata.get("id") != "synthetic-codex" or metadata.get("history_base") or metadata.get("forked_from_id"):
                raise RuntimeError("expected the independent synthetic Codex fixture")
        else:
            summary = json.loads((path / "summary.json").read_text(encoding="utf-8"))
            if summary.get("info", {}).get("id") != "synthetic-grok":
                raise RuntimeError("expected the synthetic Grok fixture")
        paths[source] = path
    return paths


def load_adapters(source: Path, *, fixture_root: Path = FIXTURES,
                  codex_paths: dict[str, Path] | None = None):
    source = source.resolve(strict=True)
    package_dir(source, ("adapters.py",))
    adapters = import_oracle_module(source, "adapters")
    # Import only defines these paths; before any adapter method can use one,
    # replace every discovery root with this tool's fixed synthetic fixture tree.
    fixture_root = fixture_root.resolve(strict=True)
    adapters.CLAUDE_ROOT = fixture_root / "claude"
    adapters.CODEX_ROOT = fixture_root / "codex"
    adapters.GROK_ROOT = fixture_root / "grok"
    adapters.CODEX_INDEX = fixture_root / "no-native-session-index-used-by-parity"

    def forbidden(*args, **kwargs):
        raise RuntimeError("fixture parity must not discover sessions or register filesystem media")

    adapters.media.register_path = forbidden
    instances = {
        "claude": adapters.ClaudeAdapter(),
        "codex": adapters.CodexAdapter(),
        "grok": adapters.GrokAdapter(),
    }
    for adapter in instances.values():
        adapter.list_sessions = forbidden
        adapter.scan_sessions = forbidden
    # Advanced generated histories may resolve only an explicit synthetic SID
    # allowlist. Never fall back to globbing a native home or another session.
    allowed = {}
    for sid, candidate in (codex_paths or {}).items():
        path = candidate.resolve(strict=True)
        if not path.is_relative_to(adapters.CODEX_ROOT.resolve(strict=True)):
            raise RuntimeError("inherited fixture path escapes the synthetic Codex root")
        allowed[str(sid)] = path

    def find_synthetic(sid):
        if sid not in allowed:
            raise RuntimeError("inherited history requested a SID outside the synthetic allowlist")
        return allowed[sid]

    instances["codex"]._find_session_path = find_synthetic if codex_paths is not None else forbidden
    instances["codex"]._name_event = lambda sid: None
    instances["codex"]._thread_names = lambda: {}
    return instances


def adapter_module(instances):
    """Return the dynamically named module backing loaded adapter instances."""
    return sys.modules[type(instances["claude"]).__module__]


def normalized(message: dict) -> dict:
    selected = {key: message[key] for key in FIELDS if message.get(key) is not None}
    # Absent true/false defaults carry the same legacy rendering semantics.
    for key, default in {"counted": True, "silent": False, "error": False}.items():
        if selected.get(key) == default:
            selected.pop(key, None)
    if message.get("ts"):
        stamp = datetime.fromisoformat(str(message["ts"]).replace("Z", "+00:00"))
        selected["ts"] = stamp.astimezone(timezone.utc).isoformat(timespec="milliseconds")
    if selected.get("role") == "tool":
        # JSON key order/indentation does not change the shown arguments.
        try:
            selected["text"] = json.loads(selected.get("text", ""))
        except (TypeError, ValueError):
            pass
    return selected


def api(opener, base: str, uid: str, query: str = "") -> dict:
    # UID is computed only from the three hardcoded fixture paths, never taken
    # from a server inventory or supplied as an arbitrary user argument.
    url = base + "/api/messages/" + quote(uid, safe=":") + ("?" + query if query else "")
    with opener.open(url, timeout=10) as response:
        raw = response.read(MAX_RESPONSE_BYTES + 1)
        if len(raw) > MAX_RESPONSE_BYTES:
            raise RuntimeError("fixture response exceeds the allowed size")
        if response.status != 200:
            raise RuntimeError(f"fixture endpoint returned HTTP {response.status}")
        value = json.loads(raw)
    if not isinstance(value, dict) or value.get("meta", {}).get("uid") != uid:
        raise RuntimeError("server did not return the exact requested synthetic fixture")
    return value


def compare(source: str, path: Path, adapter, response: dict, *, read_kwargs=None) -> list[str]:
    native, end = adapter.read(str(path), **(read_kwargs or {}))
    messages = [row for row in native if row.get("role") != "status"]
    statuses = [row for row in native if row.get("role") == "status"]
    expected = [normalized(row) for row in messages]
    actual = [normalized(row) for row in response.get("messages", [])]
    issues = []
    if len(expected) != len(actual):
        issues.append(f"{source}: message length Python={len(expected)} Rust={len(actual)}")
    for index, (left, right) in enumerate(zip(expected, actual)):
        if left != right:
            keys = sorted(key for key in left.keys() | right.keys() if left.get(key) != right.get(key))
            issues.append(f"{source}: message[{index}] fields differ: {', '.join(keys)}")
    expected_activity = normalized(statuses[-1]) if statuses else None
    actual_activity = normalized(response["activity"]) if response.get("activity") else None
    if expected_activity != actual_activity:
        issues.append(f"{source}: latest native activity differs")
    if end != response.get("end"):
        issues.append(f"{source}: EOF byte cursor Python={end} Rust={response.get('end')}")
    counted = sum(row.get("counted") is not False for row in messages)
    if counted != response.get("message_total"):
        issues.append(f"{source}: counted total Python={counted} Rust={response.get('message_total')}")
    summaries = sum(bool(row.get("summary")) and not current.get("summary")
                    for row, current in zip(messages, response.get("messages", [])))
    print(f"{source}: {len(messages)} semantic messages, {counted} counted, {len(statuses)} native status records")
    if summaries:
        print(f"  presentation-only gap: {summaries} tool summaries absent (raw tool arguments remain visible)")
    return issues


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python-source", type=Path, required=True)
    parser.add_argument("--base-url", required=True)
    args = parser.parse_args()
    base = local_base(args.base_url)
    paths = fixture_paths()
    adapters = load_adapters(args.python_source)
    opener = build_opener(ProxyHandler({}), NoRedirects())
    issues = []
    for source, path in paths.items():
        uid = source + ":" + hashlib.sha1(str(path).encode()).hexdigest()[:16]
        response = api(opener, base, uid)
        issues.extend(compare(source, path, adapters[source], response))
    for issue in issues:
        print("FAIL:", issue)
    if issues:
        return 1
    print("PASS: three synthetic fixtures match the Python semantic-message/activity/count/cursor subset.")
    print("Not full provider parity: advanced histories, media, presentation summaries, and process state are outside this check.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except HTTPError as error:
        raise SystemExit(f"fixture request failed with HTTP {error.code}; verify the isolated fixture server") from None
    except (OSError, URLError, ValueError, RuntimeError) as error:
        raise SystemExit(str(error)) from None
