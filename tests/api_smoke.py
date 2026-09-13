#!/usr/bin/env python3
"""Five-second loopback HTTP smoke test of implemented read routes.

Starts one isolated Rust server against history_parity's synthetic corpus and
checks each listed route's status and top-level JSON. No Chromium, no
redirects, no production data. urllib and http.client only.
"""
from __future__ import annotations

import sys

import argparse
import http.client
import json
import tempfile
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode, urlsplit
from urllib.request import Request

from history_parity import BINARY, Corpus, build_corpus, get_json, isolated_server

MSG_KEYS = ("meta", "version", "end", "anchor", "messages", "message_total")
BODY_CAP = 2 * 1024 * 1024


def fail(route, why, body=b""):
    if isinstance(body, (bytes, bytearray)):
        text = body.decode("utf-8", "replace")
    else:
        text = str(body)
    raise SystemExit(f"FAIL {route}: {why}; {text[:240]}")


def passed(route):
    print(f"PASS {route}", flush=True)


def ok(opener, base, route):
    try:
        return get_json(opener, base, route)
    except HTTPError as err:
        fail(route, f"HTTP {err.code}", err.read(4096))
    except (URLError, TimeoutError, OSError, json.JSONDecodeError, AssertionError) as err:
        fail(route, str(err))


def expect(opener, base, route, status, method="GET", data=None):
    request = Request(base + route, data=data, method=method)
    if data is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with opener.open(request, timeout=10) as resp:
            raw, code = resp.read(BODY_CAP + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(BODY_CAP + 1), err.code
    except (URLError, TimeoutError, OSError) as err:
        fail(route, str(err))
    if isinstance(status, int):
        wanted = code == status
    else:
        wanted = code in status
    if not wanted:
        fail(route, f"HTTP {code} (want {status})", raw)
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        payload = None
    return code, payload, raw


def error_json(route, payload, raw):
    if not isinstance(payload, dict) or not payload.get("error") or not payload.get("code"):
        fail(route, "expected JSON error/code", raw)


def first_sse(base, route):
    parsed = urlsplit(base)
    conn = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=3)
    try:
        conn.request("GET", route, headers={"Accept": "text/event-stream"})
        resp = conn.getresponse()
        buf = b""
        # read(n>1) waits for a full amt across chunked SSE; the first event
        # is often smaller than one read() and the stream stays open.
        while b"\n\n" not in buf and b"\r\n\r\n" not in buf:
            chunk = resp.read(1)
            if not chunk:
                break
            buf += chunk
            if len(buf) > BODY_CAP:
                fail(route, "SSE event too large", buf[:240])
        if resp.status != 200:
            fail(route, f"HTTP {resp.status}", buf)
        if b"\n\n" not in buf and b"\r\n\r\n" not in buf:
            fail(route, "no complete SSE event within 3s", buf)
        return buf
    except (TimeoutError, OSError, http.client.HTTPException) as err:
        fail(route, f"watch socket: {err}")
    finally:
        conn.close()


def parse_sse(route, buf):
    lines = []
    for line in buf.decode("utf-8", "replace").splitlines():
        if line.startswith("data:"):
            lines.append(line[5:].lstrip())
    if not lines:
        fail(route, "SSE event had no data:", buf)
    try:
        payload = json.loads("\n".join(lines))
    except json.JSONDecodeError:
        fail(route, "SSE data is not JSON", buf)
    if not isinstance(payload, dict):
        fail(route, "SSE data is not an object", buf)
    return payload


def run(base, opener):
    route = "/api/health"
    health = ok(opener, base, route)
    assert health.get("status") == "ok", f"{route} status={health.get('status')!r} {health}"
    assert health.get("stage") == "read_only", f"{route} stage={health.get('stage')!r} {health}"
    assert health.get("service") == "sessiondock", f"{route} {health}"
    passed(route)

    route = "/api/meta"
    meta = ok(opener, base, route)
    caps = meta.get("capabilities")
    assert isinstance(caps, dict) and caps.get("backend") == "rust", f"{route} {meta}"
    passed(route)

    route = "/api/nodes"
    nodes = ok(opener, base, route)
    assert nodes.get("mode") == "local" and isinstance(nodes.get("nodes"), list), f"{route} {nodes}"
    passed(route)

    route = "/api/sessions"
    listed = ok(opener, base, route)
    sessions, sig = listed.get("sessions"), listed.get("sig")
    assert isinstance(sessions, list) and sig, f"{route} {listed}"
    again = ok(opener, base, f"{route}?sig={quote(str(sig))}")
    assert again.get("unchanged") is True and again.get("sig") == sig, f"{route}?sig= {again}"
    passed(route)

    picked = {}
    for row in sessions:
        src, uid = row.get("source"), row.get("uid")
        if row.get("supported") is False or not src or not uid or src in picked:
            continue
        picked[src] = uid
    assert picked, f"/api/sessions had no supported row {sessions!r}"
    sample_uid = next(iter(picked.values()))
    for src, uid in picked.items():
        path = f"/api/messages/{quote(uid, safe=':')}?window=1"
        body = ok(opener, base, path)
        missing = [key for key in MSG_KEYS if key not in body]
        assert not missing, f"{path} missing {missing} {body}"
        assert isinstance(body["messages"], list) and isinstance(body["meta"], dict), path
        assert isinstance(body["version"], dict), path
        passed(f"/api/messages/{src}")

    encoded = quote(sample_uid, safe=":")
    for suffix in ("page", "media-page"):
        route = f"/api/messages/{encoded}/{suffix}?cursor=not-a-grant"
        _, payload, raw = expect(opener, base, route, 400)
        error_json(route, payload, raw)
        passed(f"/api/messages/{{uid}}/{suffix}")

    route = f"/api/session/input-history?uid={encoded}"
    hist = ok(opener, base, route)
    for key in ("history", "end", "version", "anchor"):
        assert key in hist, f"{route} missing {key} {hist}"
    assert isinstance(hist["history"], list), f"{route} {hist}"
    passed("/api/session/input-history")

    route = "/api/search?q=Claude"
    search = ok(opener, base, route)
    assert isinstance(search.get("results"), list), f"{route} {search}"
    passed("/api/search")

    snap = ok(opener, base, f"/api/messages/{encoded}?window=1")
    watch = "/api/watch?" + urlencode({
        "uid": sample_uid, "start": snap["end"],
        "head": snap["version"]["head"], "anchor": snap["anchor"],
    })
    event = parse_sse(watch, first_sse(base, watch))
    assert "messages" in event, f"{watch} first event {event}"
    passed("/api/watch")

    route = "/api/live"
    live = ok(opener, base, route)
    assert live.get("enabled") is sys.platform.startswith("linux") and live.get("known") is sys.platform.startswith("linux"), f"{route} {live}"
    passed(route)

    route = "/api/term/list"
    terms = ok(opener, base, route)
    assert terms.get("enabled") is False and isinstance(terms.get("sessions"), list), f"{route} {terms}"
    passed(route)

    route = f"/api/session/outbox?uid={encoded}"
    _, payload, raw = expect(opener, base, route, 501)
    error_json(route, payload, raw)
    passed("/api/session/outbox")

    # Send is a POST route (batch 31); unconfigured it is 501, and a GET is 405.
    route = "/api/session/send"
    _, payload, raw = expect(opener, base, route, 501, method="POST", data=b"{}")
    error_json(route, payload, raw)
    passed(route)

    route = "/api/__smoke_unknown__"
    _, payload, raw = expect(opener, base, route, 404)
    error_json(route, payload, raw)
    passed(route)

    route = "/api/health"
    code, _, raw = expect(opener, base, route, (405, 404), method="POST", data=b"{}")
    assert code != 200, f"POST {route} HTTP {code} {raw[:120]!r}"
    passed("POST /api/health")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-api-smoke-") as tmp:
        corpus: Corpus = build_corpus(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, opener):
            try:
                run(base, opener)
            except AssertionError as err:
                raise SystemExit(f"FAIL {err}") from None


if __name__ == "__main__":
    main()
