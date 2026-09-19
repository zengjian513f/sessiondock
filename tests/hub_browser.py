#!/usr/bin/env python3
"""The hub page: `sessiondock-hub` in front of three fake nodes.

Machine filter chips, per-machine nesting under the hub namespace (identical native ids on
different machines never cross-nest), NDJSON search with visible progress and a per-machine
failure (chip color + title, no layout-shifting banner), a session opened through the proxy
with media and SSE, the settings page (untick, tick back, drag to reorder, keyboard reorder)
and the `sessiondock.hub.<path>.` storage prefix.
Fake nodes only (`tests/hub_fake_node.py`); no CLI, no session root.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import sync_playwright

from hub_http_suite import REPO, FakeNode, Hub

CHROMIUM = Path.home() / ".cache/ms-playwright/chromium-1234/chrome-linux64/chrome"
NID = {"a": "a" * 32, "b": "b" * 32, "c": "c" * 32}
ROWS_JS = """() => { const byUid = new Map(S.sessions.map(s => [s.uid, s]));
  return [...document.querySelectorAll('#side .item')].map(n => ({
    uid: n.dataset.uid || null, depth: +n.dataset.depth, caret: !!n.querySelector('.nest-caret'),
    node: byUid.get(n.dataset.uid)?.node_id || ''})); }"""


def scoped(nid, uid):
    source, tail = uid.split(":", 1)
    return f"{source}:{nid}~{tail}"


class Injector:
    """Adds one spawned child row per machine at the browser boundary: the fixture serves one
    session per node, and nesting is decided by the page from `spawned_by` + `node_id`."""

    def __init__(self):
        self.on = False

    def handle(self, route):
        if not self.on or "/api/sessions" not in route.request.url or route.request.method != "GET":
            return route.continue_()
        response = route.fetch()
        data = response.json() if response.ok else None
        if not isinstance(data, dict) or data.get("unchanged") or "sessions" not in data:
            return route.fulfill(response=response)
        children = []
        for row in data["sessions"]:
            if row.get("sid") != "same-native-id":
                continue
            children.append({**row, "uid": scoped(row["node_id"], "claude:child-of-same"), "sid": "child-sid",
                             "title": row["node_name"] + " child", "spawned_by": {"source": "claude", "sid": "same-native-id"}})
        data["sessions"].extend(children)
        data["sig"] = str(data.get("sig")) + "-nested"
        route.fulfill(status=response.status, headers={"content-type": "application/json"}, body=json.dumps(data))


def check_page(page, nodes, hub):
    a, b, vega = nodes
    page.wait_for_function('S.sessions.length === 3 && Nodes.list.length === 3 && !!Nodes.capabilities["' + NID["b"] + '"]')
    assert page.evaluate("SessionDockCapabilities.namespace") == "sessiondock.hub./."
    assert page.evaluate("STORAGE_PREFIX") == "sessiondock.hub./."
    assert page.evaluate("document.querySelector('meta[name=sessiondock-mode]').content") == "hub"
    assert "SessionDock" in page.title()
    side = page.locator("#side").inner_text()
    assert all(name in side for name in ("NodeA", "NodeB", "Vega")), side
    assert len(set(page.evaluate("S.sessions.map(s => s.uid)"))) == 3
    page.wait_for_function('document.querySelector("#session-total").textContent === "3"')
    # Machine filter: right-click keeps one machine, while an ordinary click adds another.
    page.get_by_role("button", name="NodeB 1", exact=True).click(button="right")
    page.wait_for_function('visible().length === 1')
    assert page.locator("#session-total").inner_text() == "1"
    page.get_by_role("button", name="NodeA 1", exact=True).click()
    page.wait_for_function('visible().length === 2')
    assert page.evaluate("JSON.parse(localStorage.getItem('sessiondock.hub./.nodesOff'))") == [NID["c"]]
    page.get_by_role("button", name="Vega 1", exact=True).click()
    page.wait_for_function('visible().length === 3')
    node_a = page.get_by_role("button", name="NodeA 1", exact=True)
    node_a.dispatch_event("pointerdown", {"pointerType": "touch", "pointerId": 40,
                                          "button": 0, "clientX": 20, "clientY": 20})
    page.wait_for_timeout(550)
    node_a.dispatch_event("pointerup", {"pointerType": "touch", "pointerId": 40,
                                        "button": 0, "clientX": 20, "clientY": 20})
    node_a.dispatch_event("click")
    assert page.evaluate("[...Nodes.off].sort()") == [NID["b"], NID["c"]]
    page.get_by_role("button", name="NodeB 1", exact=True).click()
    page.get_by_role("button", name="Vega 1", exact=True).click()
    page.wait_for_function('visible().length === 3')
    page.get_by_role("button", name="NodeB 1", exact=True).dblclick()
    page.wait_for_function('visible().length === 3')
    page.wait_for_function("""nid => document.querySelector(
      `#node-chips button[data-node="${nid}"]`)?.title ===
      '点击选择或取消；右键或长按只选这台机器'""", arg=NID["b"])
    assert page.get_by_role("button", name="NodeB 1", exact=True).get_attribute("title") == \
        "点击选择或取消；右键或长按只选这台机器"

    # Agent Type has the same exclusive gesture. Give the three synthetic rows distinct types
    # locally, then verify both desktop right-click and touch long-press (including the browser's
    # trailing click, which must not undo the exclusive choice).
    page.evaluate("""() => {
      const types = ['claude', 'codex', 'grok'];
      S.sessions.forEach((row, index) => { row.source = types[index]; });
      renderChips(); renderSide();
    }""")
    page.locator('#chips button[data-source="codex"]').click(button="right")
    assert page.evaluate("[...S.off].sort()") == ["claude", "grok", "shell"]
    grok = page.locator('#chips button[data-source="grok"]')
    grok.dispatch_event("pointerdown", {"pointerType": "touch", "pointerId": 41,
                                        "button": 0, "clientX": 20, "clientY": 20})
    page.wait_for_timeout(550)
    grok.dispatch_event("pointerup", {"pointerType": "touch", "pointerId": 41,
                                      "button": 0, "clientX": 20, "clientY": 20})
    grok.dispatch_event("click")
    assert page.evaluate("[...S.off].sort()") == ["claude", "codex", "shell"]
    assert page.evaluate("JSON.parse(localStorage.getItem('sessiondock.hub./.off')).sort()") == ["claude", "codex", "shell"]
    page.evaluate("""() => {
      S.off.clear(); store.set('off', []); loadSessions(true);
    }""")
    page.wait_for_function("S.sessions.length === 3 && S.sessions.every(row => row.source === 'claude')")
    # Search streams NDJSON progress; a slow machine keeps the bar visible.
    page.locator("#q").fill("needle")
    page.locator("#q").press("Enter")
    page.wait_for_function("S.results?.length === 3")
    assert {r["node_name"] for r in page.evaluate("S.results")} == {"NodeA", "NodeB", "Vega"}
    b.set(search_steps=6, search_delay=0.5)
    seq = page.locator("#stat").get_attribute("data-seq")
    page.locator("#q").press("Enter")
    page.wait_for_function('document.querySelector("#search-progress b")?.textContent.includes(" / ")')
    assert page.locator("#search-progress").is_visible()
    page.wait_for_function("(seq) => document.querySelector('#stat').dataset.seq !== seq", arg=seq, timeout=20000)
    assert page.evaluate("S.results.length") == 3
    b.pop("search_steps", "search_delay")
    b.set(search_error=True)
    seq = page.locator("#stat").get_attribute("data-seq")
    page.locator("#q").press("Enter")
    page.wait_for_function("(seq) => document.querySelector('#stat').dataset.seq !== seq", arg=seq)
    assert page.locator("#node-notice").is_hidden()
    assert page.evaluate('Nodes.list.find(n => n.name === "NodeB").online')
    chip = page.locator(f'#node-chips button[data-node="{NID["b"]}"]')
    page.wait_for_function("""nid => {
      const b = document.querySelector(`#node-chips button[data-node="${nid}"]`);
      return b?.classList.contains('node-issue') && (b.title || '').includes('全文搜索失败');
    }""", arg=NID["b"])
    chip.hover()
    assert "全文搜索失败" in (chip.get_attribute("title") or "")
    b.pop("search_error")
    page.evaluate("cancelSearch(true)")
    page.wait_for_function("""nid => {
      const b = document.querySelector(`#node-chips button[data-node="${nid}"]`);
      return b && !b.classList.contains('node-issue');
    }""", arg=NID["b"])
    check_node_chip_issue(page)
    # Open the same native id on two machines through the proxy: media and SSE.
    for node, char in ((a, "a"), (b, "b")):
        uid = scoped(NID[char], "claude:same-file-hash")
        page.evaluate("(uid) => openSession(uid)", uid)
        page.wait_for_function('(name) => document.querySelector("#msgs")?.textContent.includes("reply " + name)', arg=node.name)
        page.wait_for_function('Array.from(document.querySelectorAll("#msgs img")).some(i => i.complete && i.naturalWidth > 0)')
        node.set(messages=node.state()["messages"] + [{"role": "assistant", "text": node.name + " streamed update", "ts": "2026-09-07T00:01:00Z"}])
        page.wait_for_function('(name) => document.querySelector("#msgs")?.textContent.includes(name + " streamed update")', arg=node.name)
        assert page.evaluate("S.sel") == uid
        assert node.name in page.locator("#detail .dhead").inner_text()
    page.evaluate("closeWatch()")


def check_node_chip_issue(page):
    """Live/term failures gray the machine chip and put the reason in title; #node-notice stays off."""
    result = page.evaluate("""async () => {
      const notice = document.querySelector('#node-notice');
      const nid = Nodes.list.find(n => n.name === 'NodeB').id;
      const chip = document.querySelector(`#node-chips button[data-node="${nid}"]`);
      const fail = {nodes: Nodes.list, errors: [{node_id: nid, name: 'NodeB', error: 'timeout'}]};
      const ok = {nodes: Nodes.list, errors: []};
      applyNodeState(fail, 'live');
      applyNodeState(fail, 'term');
      const shown = {
        noticeHidden: notice.hidden, noticeText: notice.textContent,
        issue: chip.classList.contains('node-issue'), title: chip.title,
      };
      applyNodeState(ok, 'live');
      applyNodeState(ok, 'term');
      const recovered = {noticeHidden: notice.hidden, issue: chip.classList.contains('node-issue')};
      await Promise.resolve();
      return {shown, recovered, title: chip.title};
    }""")
    shown, recovered = result["shown"], result["recovered"]
    assert shown["noticeHidden"] and not shown["noticeText"], shown
    assert shown["issue"], shown
    assert "运行状态" in shown["title"] and "终端列表" in shown["title"], shown
    assert recovered["noticeHidden"], recovered
    assert not recovered["issue"], recovered
    chip = page.locator(f'#node-chips button[data-node="{NID["b"]}"]')
    page.wait_for_function("""nid => document.querySelector(
      `#node-chips button[data-node="${nid}"]`)?.title ===
      '点击选择或取消；右键或长按只选这台机器'""", arg=NID["b"])
    chip.hover()
    assert chip.get_attribute("title") == "点击选择或取消；右键或长按只选这台机器"


def check_nesting(page, injector):
    injector.on = True
    page.evaluate("loadSessions(true)")
    page.wait_for_function("S.sessions.length === 6 && S.sessions.some(s => s.spawned_by)")
    toggle = page.locator("#nest-toggle")
    assert toggle.is_visible()
    toggle.click()
    page.wait_for_function("S.nest === true")
    assert page.evaluate("JSON.parse(localStorage.getItem('sessiondock.hub./.nest'))") is True
    rows = page.evaluate(ROWS_JS)
    children = [r for r in rows if r["uid"] and "child-of-same" in r["uid"]]
    assert len(children) == 3 and all(r["depth"] == 1 for r in children), rows
    roots = [r for r in rows if r["depth"] == 0]
    assert all(r["caret"] for r in roots), rows
    # Each child sits directly under the root of its own machine, never under another machine's
    # session with the same native id.
    for index, row in enumerate(rows):
        if row["depth"] == 1:
            parent = rows[index - 1]
            assert parent["depth"] == 0 and parent["node"] == row["node"], (parent, row)
    toggle.click()
    page.wait_for_function("S.nest === false")
    injector.on = False
    page.evaluate("loadSessions(true)")
    page.wait_for_function("S.sessions.length === 3")


def check_settings(page, nodes, hub):
    vega = nodes[2]
    if not page.locator("#settings").is_visible():
        page.locator("#header-more-btn").click()
    page.click("#settings")
    page.click(".settings-tab[data-tab='machines']")
    page.wait_for_selector("#settings-machines:not([hidden])")
    rows = page.locator("#machine-rows .machine-row")
    assert rows.count() == 3
    names = lambda: page.locator("#machine-rows .machine-row input[type='text']").evaluate_all("e => e.map(i => i.value)")  # noqa: E731
    chips = lambda: page.locator("#node-chips button").evaluate_all("e => e.map(b => b.firstChild.textContent.trim())")  # noqa: E731
    vega_row = rows.filter(has=page.locator('input[aria-label="Vega 的名称"]'))
    probes = lambda: len([p for p, _ in vega.state()["gets"] if p.startswith("/api/")])  # noqa: E731
    vega_row.locator(".machine-enabled").uncheck()
    page.wait_for_function('!Nodes.list.some(n => n.name === "Vega") && Nodes.machines.some(n => n.name === "Vega" && !n.enabled)')
    page.wait_for_function('!S.sessions.some(s => s.node_name === "Vega")')
    assert page.locator("#node-chips button", has_text="Vega").count() == 0
    assert names() == ["NodeA", "NodeB", "Vega"], names()
    before = probes()
    page.wait_for_timeout(1500)
    assert probes() == before, "an unticked machine is not probed"
    assert page.evaluate('fetch("api/nodes/" + "c".repeat(32) + "/api/live").then(r => r.status)') == 404
    vega_row.locator(".machine-enabled").check()
    page.wait_for_function('Nodes.list.some(n => n.name === "Vega") && Nodes.machines.every(n => n.enabled)')
    page.wait_for_function('Nodes.list.find(n => n.name === "Vega")?.online === true', timeout=20000)
    page.wait_for_function('S.sessions.some(s => s.node_name === "Vega" && !s.stale)', timeout=20000)
    # Drag Vega's grip to the top: chips, dropdown and the registry follow.
    grip = page.locator('[data-machine-grip="' + NID["c"] + '"]')
    top = page.locator("#machine-rows .machine-row").first.bounding_box()
    box = grip.bounding_box()
    page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    page.mouse.down()
    page.mouse.move(box["x"] + box["width"] / 2, top["y"] + 4, steps=8)
    assert names() == ["Vega", "NodeA", "NodeB"], names()
    page.mouse.up()
    page.wait_for_function('Nodes.machines[0]?.name === "Vega" && Nodes.list[0]?.name === "Vega"')
    assert chips() == ["Vega", "NodeA", "NodeB"], chips()
    status, listing = hub.json("GET", "/api/nodes")
    assert [n["id"] for n in listing["machines"]] == [NID["c"], NID["a"], NID["b"]], listing
    # Let the drag's async save+reload (which re-renders the rows) settle before
    # the keyboard reorder, so the debounced save reads the keyboard order and
    # not a row list the reload has just rebuilt.
    page.wait_for_function('document.querySelector("#machine-rows .machine-row [data-machine-grip]")?.dataset.machineGrip === "' + NID["c"] + '"')
    page.wait_for_timeout(700)
    grip = page.locator('[data-machine-grip="' + NID["c"] + '"]')
    grip.focus()
    grip.press("ArrowDown")
    grip.press("ArrowDown")
    assert names() == ["NodeA", "NodeB", "Vega"], names()
    page.wait_for_function('Nodes.list[2]?.name === "Vega"', timeout=20000)
    assert chips() == ["NodeA", "NodeB", "Vega"], chips()
    name_input = page.locator('input[aria-label="NodeA 的名称"]')
    name_input.fill("机房 A")
    name_input.press("Enter")
    page.wait_for_function('Nodes.list.some(n => n.name === "机房 A")')
    page.locator("#settings-dialog .modal-actions button").click()
    events = hub.audit_events()
    assert any(e["event"] == "hub.node.order.changed" for e in events), events
    assert any(e["event"] == "hub.node.display.changed" and e["data"]["to"]["name"] == "机房 A" for e in events), events


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", default=str(REPO / "target/release/sessiondock"))
    parser.add_argument("--hub-binary", default=None)
    parser.add_argument("--screenshot", default=None, help="write a desktop screenshot here")
    args = parser.parse_args()
    hub_binary = Path(args.hub_binary) if args.hub_binary else Path(args.binary).resolve().parent / "sessiondock-hub"
    nodes = [FakeNode(NID["a"], "NodeA"), FakeNode(NID["b"], "NodeB"), FakeNode(NID["c"], "Vega")]
    try:
        with tempfile.TemporaryDirectory(prefix="sessiondock-hub-browser-") as temp:
            hub = Hub(hub_binary, Path(temp), nodes)
            hub.start()
            try:
                with sync_playwright() as playwright:
                    launch = {"headless": True}
                    executable = os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE") or (str(CHROMIUM) if CHROMIUM.is_file() else "")
                    if executable:
                        launch["executable_path"] = executable
                    browser = playwright.chromium.launch(**launch)
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    injector = Injector()
                    context.route("**/api/sessions*", injector.handle)
                    page = context.new_page()
                    errors = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    # Page errors matter. A console 404 here is only the test's own
                    # `fetch` probe that an unticked machine returns 404 (an expected
                    # response, logged by Chromium as a resource error), not a page fault.
                    page.on("console", lambda message: errors.append(message.text)
                            if message.type == "error" and "404" not in message.text else None)
                    response = page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="networkidle")
                    assert response.status == 200
                    check_page(page, nodes, hub)
                    check_nesting(page, injector)
                    check_settings(page, nodes, hub)
                    if args.screenshot:
                        page.screenshot(path=args.screenshot)
                    assert not errors, errors
                    browser.close()
            finally:
                hub.stop()
    finally:
        for node in nodes:
            node.stop()
    print("PASS hub_browser: hub page over three fake nodes (filter, nesting, NDJSON search, chip issue tip, proxy session, settings)")


if __name__ == "__main__":
    main()
