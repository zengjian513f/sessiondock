#!/usr/bin/env python3
"""Recover lost history grants through real servers and Chromium gap clicks."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlsplit

from browser_runtime import js
from playwright.sync_api import expect, sync_playwright
from history_pages_browser import build, row
from history_parity import BINARY, encoded, get_json, isolated_server
from media_browser import uid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-page-resume-") as temporary:
        corpus = build(Path(temporary))
        before = {path: path.read_bytes() for path in corpus.root.rglob("*.jsonl")}
        env = {"SESSIONDOCK_HISTORY_PAGE_EVENTS": "200"}
        with isolated_server(corpus, args.binary, extra_env=env) as (base, opener), \
                isolated_server(corpus, args.binary, extra_env=env) as (fresh, fresh_opener), \
                sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                page = context.new_page()
                errors, requests, held, recovered = [], [], [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("request", lambda request: requests.append(request.url))
                mode = {"hold": True, "fresh": True}

                def route(request):
                    query = parse_qs(urlsplit(request.request.url).query)
                    destination = fresh if mode["fresh"] else base
                    mode["fresh"] = not mode["fresh"]
                    path = urlsplit(request.request.url).path
                    # The two real processes alternate: neither holds the token
                    # just issued by the other. Prove the ordinary lookup fails.
                    bare = {key: values[0] for key, values in query.items() if key not in ("resume", "next")}
                    missing = request.fetch(url=destination + path + "?" + urlencode(bare))
                    assert missing.status == 404, missing.status
                    response = request.fetch(url=destination + path + "?" + urlsplit(request.request.url).query)
                    assert response.status == 200, response.text()
                    recovered.append(response.json()["page"])
                    if mode["hold"]:
                        mode["hold"] = False
                        held.append((request, response))
                    else:
                        request.fulfill(response=response)

                page.route("**/api/messages/*/page?*", route)
                page.goto(base, wait_until="networkidle")
                page.evaluate(js('HISTORY_PAGE_CHAIN=false', 'runtime.core.history.timing.chain=false'))
                selected = uid(corpus, "codex-pages")
                page.locator(f'#side .item[data-uid="{selected}"]').click()
                expect(page.locator("#msgs")).to_contain_text("PAGE ROW 1399")
                page.wait_for_function(js('_es && _es.readyState===EventSource.OPEN', 'runtime.core.sync.watching && runtime.core.sync.watching.readyState===EventSource.OPEN'))
                page.evaluate(js('window.__originalWatch=_es', 'window.__originalWatch=runtime.core.sync.watching'))

                def snapshot():
                    return page.evaluate(js(r"""(() => {const e=cache.get(viewKey(S.sel,S.agent));return {
                      text:e.msgs.map(m=>m.text),partial:e.partial,end:e.end,anchor:e.anchor,
                      version:e.version,cursor:S.cursors.get(viewKey(S.sel,S.agent))};})()""", r"""(() => {const e=runtime.core.cache.cache.get((runtime.core.state.selection.agent ? runtime.core.state.selection.sel + "::" + runtime.core.state.selection.agent : runtime.core.state.selection.sel));return {
                      text:e.msgs.map(m=>m.text),partial:e.partial,end:e.end,anchor:e.anchor,
                      version:e.version,cursor:runtime.core.state.unread.cursors.get((runtime.core.state.selection.agent ? runtime.core.state.selection.sel + "::" + runtime.core.state.selection.agent : runtime.core.state.selection.sel))};})()"""))

                def click():
                    page.locator(".history-gap-load").click()

                def settled():
                    page.wait_for_function(js("historyPageRequests.size===0 && document.querySelector('#msgs .msg')", "runtime.core.history.historyPageRequests.size===0 && document.querySelector('#msgs .msg')"))

                initial = snapshot()
                assert initial["partial"]["resume"]["uid"] == selected
                click()
                page.wait_for_function(js('historyPageRequests.size===1', 'runtime.core.history.historyPageRequests.size===1'))
                path = corpus.paths["codex-pages"]
                added = encoded(row("LIVE APPEND WHILE GRANT RECOVERS"))
                with path.open("ab") as stream:
                    stream.write(added)
                before[path] += added
                page.wait_for_function(js("cache.get(viewKey(S.sel,S.agent)).msgs.at(-1).text==='LIVE APPEND WHILE GRANT RECOVERS'", 'runtime.core.cache.cache.get((runtime.core.state.selection.agent ? runtime.core.state.selection.sel + "::" + runtime.core.state.selection.agent : runtime.core.state.selection.sel)).msgs.at(-1).text===\'LIVE APPEND WHILE GRANT RECOVERS\''))
                live = snapshot()
                request, response = held.pop()
                request.fulfill(response=response)
                settled()
                first = snapshot()
                assert first["partial"]["head"] == initial["partial"]["head"] + 200
                assert first["partial"]["resume"] == initial["partial"]["resume"]
                assert first["cursor"] == live["cursor"] and first["end"] == live["end"]
                while snapshot()["partial"]:
                    click()
                    settled()
                final = snapshot()
                assert final["text"] == [*[f"PAGE ROW {i:04d}" for i in range(1400)], "LIVE APPEND WHILE GRANT RECOVERS"]
                assert final["cursor"] == live["cursor"] and final["anchor"] == live["anchor"]
                assert page.evaluate(js('_es===window.__originalWatch', 'runtime.core.sync.watching===window.__originalWatch'))
                expect(page.locator(".history-page-error")).to_have_count(0)
                print(f"PASS lost grants: {len(recovered)} bounded pages, existing pages/live tail preserved", flush=True)

                # Recovery is still an authenticated, selected history read.
                # Wrong scope, prefix, event count and range never return rows.
                resume = initial["partial"]["resume"]

                def rejected(descriptor, code, *, agent="", next_index=None):
                    query = {"cursor": "0" * 32, "resume": json.dumps(descriptor),
                             "next": initial["partial"]["head"] if next_index is None else next_index}
                    if agent:
                        query["agent"] = agent
                    try:
                        get_json(fresh_opener, fresh, f"/api/messages/{selected}/page?{urlencode(query)}")
                    except HTTPError as error:
                        assert error.code == code, error.read().decode()
                    else:
                        raise AssertionError("invalid recovery returned history")

                rejected({**resume, "uid": uid(corpus, "codex-other")}, 403)
                rejected(resume, 403, agent="codex-page-agent")
                rejected({**resume, "anchor": "invalid-checkpoint"}, 409)
                rejected({**resume, "total": resume["total"] + 1}, 409)
                rejected(resume, 409, next_index=resume["stop"])
                # The actual original prefix changes; its old resume must fail.
                changed = path.read_bytes().replace(b"PAGE ROW 0000", b"EDIT ROW 0000")
                path.write_bytes(changed)
                before[path] = changed
                rejected(resume, 409)
                print("PASS recovery scope/checkpoint/range and actual prefix rewrite rejection", flush=True)

                page.locator("#a-view-switch").click()
                page.locator('#session-view-menu button[data-agent="codex-page-agent"]').click()
                expect(page.locator("#msgs")).to_contain_text("AGENT ROW 0749")
                agent = snapshot()
                assert agent["partial"]["resume"]["agent"] == "codex-page-agent"
                mode["fresh"] = True
                while snapshot()["partial"]:
                    click()
                    settled()
                assert snapshot()["text"] == [f"AGENT ROW {i:04d}" for i in range(750)]
                assert snapshot()["cursor"] == agent["cursor"]
                print("PASS exact subagent recovery through browser view selection", flush=True)
                assert not errors, errors
                assert all("/page?" in url or "window=1" in url or "start=" in url
                           for url in requests if "/api/messages/" in url), "unbounded read"
                assert all(path.read_bytes() == data for path, data in before.items())
            finally:
                browser.close()


if __name__ == "__main__":
    main()
