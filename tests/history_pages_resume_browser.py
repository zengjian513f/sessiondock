#!/usr/bin/env python3
"""Recover lost history grants through real servers and Chromium gap clicks."""
from __future__ import annotations

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlsplit


from playwright.sync_api import expect, sync_playwright
from history_pages_browser import build, row
from history_fixtures import BINARY, encoded, get_json, isolated_server
from media_browser import uid
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN


def run(binary, through_hub):
    with tempfile.TemporaryDirectory(prefix="sessiondock-page-resume-") as temporary:
        corpus = build(Path(temporary))
        before = {path: path.read_bytes() for path in corpus.root.rglob("*.jsonl")}
        env = {"SESSIONDOCK_HISTORY_PAGE_EVENTS": "200"}
        with ExitStack() as stack, sync_playwright() as playwright:
            nodes = []
            if through_hub:
                ids = corpus.root / "ids"
                ids.mkdir()
                (ids / "node-id").write_text("a" * 32 + "\n")
                nodes = [SimpleNamespace(name="fixture", nid="a" * 32, port=free_port(), token=TOKEN)
                         for _ in range(2)]
            servers = []
            for index in range(2):
                extra = node_env(corpus.root, nodes[index].port, "127.0.0.0/8") if through_hub else {}
                servers.append(stack.enter_context(isolated_server(corpus, binary, extra_env={**env, **extra})))
            (base, opener), (fresh, fresh_opener) = servers
            if through_hub:
                hubs = []
                for index, node in enumerate(nodes):
                    root = corpus.root / f"hub-{index}"
                    root.mkdir()
                    hub = Hub(binary.resolve().with_name("sessiondock-hub"), root, [node])
                    hub.start()
                    stack.callback(hub.stop)
                    hubs.append(f"http://127.0.0.1:{hub.port}")
                base, fresh = hubs
            selected_uid = lambda name: scoped(nodes[0].nid, uid(corpus, name)) if through_hub else uid(corpus, name)
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
                mode = {"hold": True, "fresh": True, "warm": through_hub}

                def route(request):
                    if mode["warm"]:
                        mode["warm"] = False
                        request.continue_()
                        return
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
                page.evaluate('HISTORY_PAGE_CHAIN=false')
                selected = selected_uid("codex-pages")
                page.locator(f'#side .item[data-uid="{selected}"]').click()
                expect(page.locator("#msgs")).to_contain_text("PAGE ROW 1399")
                page.wait_for_function('_es && _es.readyState===EventSource.OPEN')
                page.evaluate('window.__originalWatch=_es')

                def snapshot():
                    return page.evaluate(r"""(() => {const e=cache.get(viewKey(S.sel,S.agent));return {
                      text:e.msgs.map(m=>m.text),partial:e.partial,end:e.end,anchor:e.anchor,
                      version:e.version,cursor:S.cursors.get(viewKey(S.sel,S.agent))};})()""")

                def click():
                    page.locator(".history-gap-load").click()

                def settled():
                    page.wait_for_function("historyPageRequests.size===0 && document.querySelector('#msgs .msg')")

                initial = snapshot()
                assert initial["partial"]["resume"]["uid"] == selected
                if through_hub:
                    # Valid grants work first; then use the other real node
                    # process to simulate a restart/expiry losing the grant.
                    click()
                    settled()
                    warm = snapshot()
                    assert warm["partial"]["head"] == initial["partial"]["head"] + 200
                    assert warm["cursor"] == initial["cursor"]
                    initial = warm
                    print("PASS hub valid grant before recovery", flush=True)
                click()
                page.wait_for_function('historyPageRequests.size===1')
                path = corpus.paths["codex-pages"]
                added = encoded(row("LIVE APPEND WHILE GRANT RECOVERS"))
                with path.open("ab") as stream:
                    stream.write(added)
                before[path] += added
                page.wait_for_function("cache.get(viewKey(S.sel,S.agent)).msgs.at(-1).text==='LIVE APPEND WHILE GRANT RECOVERS'")
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
                assert page.evaluate('_es===window.__originalWatch')
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

                rejected({**resume, "uid": selected_uid("codex-other")}, 403)
                if through_hub:
                    rejected({**resume, "uid": scoped("b" * 32, uid(corpus, "codex-pages"))}, 400)
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
                print(f"PASS {'Hub' if through_hub else 'direct node'} history recovery", flush=True)
            finally:
                browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    for through_hub in (False, True):
        run(args.binary, through_hub)


if __name__ == "__main__":
    main()
