#!/usr/bin/env python3
"""Real legacy page against the bounded browser-audit intake.

With SESSIONDOCK_AUDIT_DIR configured the untouched legacy page posts its
receipts (fetch and pagehide sendBeacon) and the private JSONL receives bounded,
metadata-only records. Without the directory the capability is false, the page
sends nothing and the route stays 501. Synthetic corpus only; no CLI or model.
"""
import argparse
import json
import os
from pathlib import Path
import stat
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, build_corpus, get_json, isolated_server

MAX_LINE_BYTES = 16 * 1024
AUDIT_ROUTE = "/api/audit/browser"


def audit_files(directory: Path):
    return sorted(path for path in directory.iterdir()
                  if path.name.startswith("browser-") and path.name.endswith(".jsonl"))


def audit_lines(directory: Path):
    rows = []
    for path in audit_files(directory):
        for raw in path.read_bytes().split(b"\n"):
            if not raw:
                continue
            assert len(raw) <= MAX_LINE_BYTES, len(raw)
            rows.append(json.loads(raw))
    return rows


def wait_for_events(page, directory: Path, expected: set[str], timeout: float = 15.0):
    """Polls the private JSONL while pumping Playwright: with request routing
    active, the browser's requests only proceed while the client is inside a
    Playwright call, so a bare time.sleep would stall the page's own posts."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        seen = {row["event"] for row in audit_lines(directory)}
        if expected <= seen:
            return seen
        page.wait_for_timeout(100)
    raise AssertionError(f"audit records missing {expected - seen}; saw {sorted(seen)}")


def wait_for_quiescence(page, opener, base, timeout: float = 10.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        audit = get_json(opener, base, "/api/health")["audit"]
        if audit["queued_batches"] == 0 and audit["written_events"] == audit["accepted_events"]:
            return audit
        page.wait_for_timeout(100)
    raise AssertionError(f"audit writer did not drain: {audit}")


def post_status(opener, base, body: bytes) -> int:
    request = Request(base + AUDIT_ROUTE, data=body, method="POST",
                      headers={"Content-Type": "application/json"})
    try:
        with opener.open(request, timeout=10) as response:
            return response.status
    except HTTPError as error:
        return error.code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-audit-browser-") as temporary:
        corpus = build_corpus(Path(temporary))
        native_before = {path: path.read_bytes() for path in corpus.paths.values()}
        audit = corpus.root / "audit"
        audit.mkdir(mode=0o700)
        forbidden = ["Claude selected answer", "/synthetic/history", str(corpus.root), '"content"']
        with sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                with isolated_server(corpus, args.binary, audit_dir=audit) as (base, opener):
                    assert get_json(opener, base, "/api/meta")["capabilities"]["audit"] is True
                    assert get_json(opener, base, "/api/health")["audit"]["enabled"] is True
                    assert not audit_files(audit), "an idle service must not create files"
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    # Intercept only foreign URLs: routing loopback requests stalls the page's keepalive audit fetch.
                    context.route(lambda url: not url.startswith(base + "/"), lambda route: route.abort())
                    page = context.new_page()
                    posts = []
                    errors = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.on("response", lambda response: posts.append(
                        (response.status, response.request.header_value("content-type") or "",
                         response.request.method))
                        if response.url == base + AUDIT_ROUTE else None)
                    page.goto(base, wait_until="networkidle")
                    expect(page.locator("#backend-notice")).to_be_hidden()  # no standing banner
                    page_id = page.evaluate("window.__sessiondockPageId")
                    assert page_id and len(page_id) <= 128, page_id
                    uid = corpus.uid("claude-branch")
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Claude selected answer")
                    page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                    seen = wait_for_events(page, audit, {"browser.page.loaded", "browser.session.opened",
                                                   "browser.http.response.parsed", "browser.dom.snapshot",
                                                   "browser.sse.opened"})
                    # A ≥1 s main-thread frame is attributed by the browser and beaconed at once.
                    assert page.evaluate("PerformanceObserver.supportedEntryTypes.includes('long-animation-frame')")
                    page.evaluate("setTimeout(() => { const t = performance.now(); while (performance.now() - t < 1300) {} }, 0)")
                    deadline = time.monotonic() + 15
                    while not any(row["event"] == "browser.main_thread.long_frame"
                                  and any("setTimeout" in script["invoker"] for script in row["data"]["scripts"])
                                  for row in audit_lines(audit)):
                        assert time.monotonic() < deadline, "the synthetic long frame was never reported"
                        page.wait_for_timeout(100)
                    seen.add("browser.main_thread.long_frame")
                    # Navigating away fires pagehide: the page flushes through sendBeacon.
                    page.goto(base + "/?second=1", wait_until="networkidle")
                    seen |= wait_for_events(page, audit, {"browser.page.hidden"})
                    assert posts, "the page never posted"
                    assert all(status == 202 and method == "POST" for status, _, method in posts), posts
                    assert all(content_type.startswith("application/json") for _, content_type, _ in posts), posts
                    assert not errors, errors
                    health = wait_for_quiescence(page, opener, base)
                    rows = audit_lines(audit)
                    assert health["accepted_events"] == len(rows) == health["written_events"], (health, len(rows))
                    assert health["dropped_events"] == health["dropped_batches"] == 0, health
                    assert health["rejected_requests"] == 0, health
                    assert health["write_errors"] == 0, health
                    assert health["written_bytes"] == health["retained_bytes"] > 0, health
                    files = audit_files(audit)
                    assert len(files) == 1 and files[0].name.startswith("browser-20"), files
                    if os.name != "nt":
                        assert stat.S_IMODE(files[0].stat().st_mode) == 0o600, oct(files[0].stat().st_mode)
                    raw = files[0].read_text(encoding="utf-8")
                    for marker in forbidden:
                        assert marker not in raw, marker
                    by_page = [row for row in rows if row["page_id"] == page_id]
                    assert by_page, page_id
                    sequences = [row["seq"] for row in rows]
                    assert sequences == sorted(sequences) and len(set(sequences)) == len(sequences)
                    for row in rows:
                        assert row["event"].startswith("browser."), row
                        assert row["client"] == "127.0.0.1", row
                        assert row["received_at"].endswith("Z"), row
                        assert isinstance(row["data"], dict), row
                        assert "content" not in row, row
                        assert row["severity"] in {"info", "warning", "error"}, row
                    snapshots = [row for row in by_page if row["event"] == "browser.dom.snapshot"]
                    assert snapshots and all({"reason", "dom_messages", "selected"} <= set(row["data"]) for row in snapshots), snapshots[:1]
                    assert all("composer" not in row["data"] and "messages" not in row["data"] for row in snapshots)
                    parsed = [row for row in by_page if row["event"] == "browser.http.response.parsed"]
                    assert parsed and all(row["uid"] == uid and row["source"] == "claude" and row["trace_id"] for row in parsed), parsed[:1]
                    loaded = next(row for row in by_page if row["event"] == "browser.page.loaded")
                    assert loaded["build"] and loaded["data"]["user_agent"], loaded
                    hidden = [row for row in by_page if row["event"] == "browser.page.hidden"]
                    assert hidden and hidden[-1]["data"]["reason"] == "pagehide", hidden
                    frames = [row for row in by_page if row["event"] == "browser.main_thread.long_frame"
                              and any("setTimeout" in script["invoker"] for script in row["data"]["scripts"])]
                    assert frames and frames[-1]["severity"] == "warning" and frames[-1]["uid"] == uid, frames[:1]
                    frame = frames[-1]["data"]
                    assert frame["duration_ms"] >= 1000 and frame["dom_nodes"] > 0, frame
                    assert frame["heap_mb"] is None or len(frame["heap_mb"]) == 3, frame
                    script = next(script for script in frame["scripts"] if "setTimeout" in script["invoker"])
                    assert script["duration_ms"] >= 1000 and script["invoker_type"], script
                    context.close()
                # Graceful shutdown (SIGTERM above) must leave every segment parseable.
                after_shutdown = audit_lines(audit)
                assert len(after_shutdown) >= len(rows)
                sizes_before = {path.name: path.stat().st_size for path in audit_files(audit)}

                with isolated_server(corpus, args.binary) as (base, opener):
                    assert get_json(opener, base, "/api/meta")["capabilities"]["audit"] is False
                    assert get_json(opener, base, "/api/health")["audit"] == {
                        "enabled": False, "accepted_events": 0, "accepted_batches": 0,
                        "rejected_events": 0, "rejected_requests": 0,
                        "dropped_events": 0, "dropped_batches": 0, "written_events": 0,
                        "written_bytes": 0, "write_errors": 0, "retained_bytes": 0,
                        "queued_batches": 0, "queued_bytes": 0}
                    assert post_status(opener, base, b'{"events": []}') == 501
                    context = browser.new_context(service_workers="block")
                    # Intercept only foreign URLs: routing loopback requests stalls the page's keepalive audit fetch.
                    context.route(lambda url: not url.startswith(base + "/"), lambda route: route.abort())
                    page = context.new_page()
                    requests = []
                    page.on("request", lambda request: requests.append(request.url))
                    page.goto(base, wait_until="networkidle")
                    page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Claude selected answer")
                    page.wait_for_timeout(2500)  # past the 750 ms flush and the 1.5 s retry timer
                    page.goto(base + "/?second=1", wait_until="networkidle")
                    page.wait_for_timeout(1000)
                    assert not any(AUDIT_ROUTE in url for url in requests), [url for url in requests if "audit" in url]
                    context.close()
                assert {path.name: path.stat().st_size for path in audit_files(audit)} == sizes_before
                assert all(path.read_bytes() == before for path, before in native_before.items())
                print(f"PASS audit browser: {len(rows)} bounded records ({len(seen)} event kinds) via fetch and pagehide beacon, "
                      "metadata only, health counters consistent, capability off posts nothing and stays 501")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
