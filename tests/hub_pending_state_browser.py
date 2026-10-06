#!/usr/bin/env python3
"""New-session receipts survive partial hub lists; recovery and real exit stay authoritative."""

import argparse
import json
import os
from pathlib import Path
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from hub_fixtures import REPO, FakeNode, Hub
from hub_browser import CHROMIUM
from popups import on_popup  # noqa: E402


def scenario(browser, hub, node, other):
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    errors, dialogs, partials = [], [], []
    page.on("pageerror", lambda error: errors.append(str(error)))
    on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
    # A new receipt is newer than the hub's cached empty terminal list. Fail
    # the node immediately after create succeeds, before its next list read.
    def create_then_disconnect(route):
        response = route.fetch()
        assert response.status == 200
        node.set(term_error=True)
        other.set(pending=[])
        route.fulfill(response=response)

    def observe(response):
        if urlsplit(response.url).path == "/api/term/list" and response.status == 200:
            data = response.json()
            if data.get("partial"):
                partials.append(data)

    page.on("response", observe)
    context.route("**/api/term/create", create_then_disconnect)
    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="networkidle")
    page.wait_for_function("T.listLoaded && Nodes.list.length === 2 && T.pending.length === 1")
    page.locator("#new-session").click()
    page.locator("#new-node").select_option(node.nid)
    page.locator('#new-session-form label:has(input[name="new-source"][value="codex"])').click()
    page.locator("#new-cwd").fill("/synthetic/work")
    page.locator("#new-session-go").click()
    uid = f"tmux:{node.nid}~same-terminal"
    row = page.locator(f'#side .item[data-uid="{uid}"]')
    page.wait_for_function("uid => S.sel === uid", arg=uid)
    page.locator("#cinput").fill("keep this unsent draft")
    page.wait_for_function("Nodes.errors.get('term')?.length > 0", timeout=20000)
    expect(row).to_contain_text("运行状态不确定")
    expect(row).not_to_contain_text("已结束")
    expect(page.locator(".new-session-wait")).to_have_text("暂时无法确认会话状态。")
    assert partials and all(not data.get("pending") for data in partials), partials
    receipt = page.evaluate("T.pending[0]")
    assert receipt["instance_id"] == "fixture-new-instance" and receipt["stale"]
    assert page.evaluate("T.pending.length") == 1, "healthy node removal must still apply"
    row.click()
    expect(page.locator("#cinput")).to_have_value("keep this unsent draft")
    expect(page.locator(".new-session-wait")).not_to_contain_text("已结束")

    # No in-memory receipt after reload: the persisted draft must also remain
    # uncertain while this node's list is unavailable.
    page.wait_for_function("composerDrafts.get(S.sel)?.savedVersion === composerDrafts.get(S.sel)?.editVersion")
    page.reload(wait_until="networkidle")
    page.wait_for_function("T.listLoaded && Nodes.errors.get('term')?.length > 0")
    row.click()
    expect(row).to_contain_text("运行状态不确定")
    expect(page.locator(".new-session-wait")).to_have_text("暂时无法确认会话状态。")
    expect(page.locator("#cinput")).to_have_value("keep this unsent draft")

    node.pop("term_error")
    # Normal monitor + page polling recover the node, without injecting JS state.
    page.wait_for_function("!Nodes.errors.get('term')?.length && T.pending.some(r => !r.stale)", timeout=30000)
    expect(row).to_contain_text("等待首条消息")
    expect(page.locator(".new-session-wait")).not_to_contain_text("已结束")
    expect(page.locator("#cinput")).to_have_value("keep this unsent draft")
    assert page.evaluate("T.pending[0].instance_id") == receipt["instance_id"]

    # A successful authoritative exit is still shown as ended.
    ended = {**node.state()["pending"][0], "state": "exited", "running": False, "stale": True}
    node.set(pending=[ended])
    page.wait_for_function("T.pending.some(r => r.state === 'exited')", timeout=20000)
    expect(row).to_contain_text("已结束")
    expect(page.locator(".new-session-wait")).to_have_text("会话已结束。")
    # A later partial result cannot erase an already confirmed exit either.
    node.set(term_error=True)
    page.wait_for_function("Nodes.errors.get('term')?.length > 0", timeout=20000)
    expect(row).to_contain_text("已结束")
    expect(page.locator("#cinput")).to_have_value("keep this unsent draft")
    assert not errors, errors
    assert not dialogs, dialogs
    context.close()


def node_source_picker(browser, hub, node, other):
    """The picker follows the chosen machine: OpenCode only where configured,
    and the machine select plus five sources stay inside the dialog."""
    node.set(term_sources={"claude": True, "codex": True, "grok": True, "shell": True})
    other.set(term_sources={"claude": True, "codex": True, "grok": True, "opencode": True, "shell": True})
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="networkidle")
    page.wait_for_function("T.listLoaded && Nodes.list.length === 2")
    page.locator("#new-session").click()
    opencode = page.locator('input[name="new-source"][value="opencode"]')
    page.locator("#new-node").select_option(node.nid)
    expect(opencode).to_be_disabled()
    page.locator("#new-node").select_option(other.nid)
    expect(opencode).to_be_enabled()
    page.locator('#new-session-form label:has(input[value="opencode"])').click()
    expect(opencode).to_be_checked()
    # The model list comes from the selected machine's own CLI.
    expect(page.locator("#new-model-label")).to_have_text("选择模型")
    page.locator("#new-model").click()
    expect(page.locator("#new-model-options [role=option]", has_text=f"{other.name}-opencode")).to_be_visible()
    page.keyboard.press("Escape")
    for width in (1280, 390):
        page.set_viewport_size({"width": width, "height": 900})
        dialog = page.locator("#new-session-dialog").bounding_box()
        for item in page.locator("#new-session-form .new-row > *:not([hidden]), #new-session-form .new-source label, #new-model, #new-session-form .new-effort").all():
            box = item.bounding_box()
            assert box and box["x"] >= dialog["x"] and box["x"] + box["width"] <= dialog["x"] + dialog["width"] + 0.5, (width, box, dialog)
        rows = page.evaluate("""() => new Set([...document.querySelectorAll('#new-session-form .new-source label')]
            .map(l => Math.round(l.getBoundingClientRect().top))).size""")
        assert rows == 1, (width, rows)
        # The machine picker is first, sharing the top row with the agent buttons.
        where = page.evaluate("""() => { const r = s => document.querySelector(s).getBoundingClientRect();
            const node = r('#new-node-label'), agents = r('#new-session-form .new-source'),
                  cwd = r('#new-session-form .new-cwd-field'), dialog = r('#new-session-dialog');
            return {top: Math.abs(node.top - agents.top), order: node.right <= agents.left + 0.5,
                    inside: cwd.right <= dialog.right, directory_below: cwd.top > node.bottom,
                    node_width: node.width, cwd_width: cwd.width,
                    in_agent_row: document.querySelector('#new-session-form .new-row').firstElementChild.id === 'new-node-label'}; }""")
        assert where["top"] < 1 and where["order"] and where["inside"] and where["in_agent_row"], (width, where)
        assert where["directory_below"], (width, where)
        assert where["node_width"] <= 140 and where["cwd_width"] >= 180, (width, where)
        # Buttons never overlap, and each keeps a usable tap width.
        spans = page.evaluate("""() => [...document.querySelectorAll('#new-session-form .new-source label > span')]
            .map(e => { const r = e.getBoundingClientRect(); return [r.left, r.right]; })""")
        assert all(b[0] >= a[1] - 0.5 for a, b in zip(spans, spans[1:])), (width, spans)
        assert all(r - l >= 34 for l, r in spans), (width, spans)
        # Like the header Agent filter, the buttons are one joined segment: no gaps between them.
        assert all(abs(b[0] - a[1]) <= 0.5 for a, b in zip(spans, spans[1:])), (width, spans)
        if os.environ.get("SESSIONDOCK_TEST_SHOTS"):
            page.screenshot(path=os.path.join(os.environ["SESSIONDOCK_TEST_SHOTS"], f"hub-picker-{width}.png"))
    assert not errors, errors
    context.close()
    node.pop("term_sources")
    other.pop("term_sources")


def node_switch_cwd(browser, hub, node, other):
    """Switching machine keeps a typed directory that also exists on the new one."""
    node.set(dirs=["/shared/proj"])
    other.set(dirs=["/shared/proj", "/only-b/work"])
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="networkidle")
    page.wait_for_function("T.listLoaded && Nodes.list.length === 2")
    page.locator("#new-session").click()
    cwd = page.locator("#new-cwd")
    page.locator("#new-node").select_option(node.nid)
    # Typing swaps the list title, groups and the lookup note; the dialog and
    # the list keep their geometry on every frame instead of jumping.
    cwd.fill("")
    page.wait_for_timeout(100)
    page.evaluate("""() => { window.__cwdFrames = []; const rect = s => {
        const r = document.querySelector(s).getBoundingClientRect(); return [Math.round(r.top), Math.round(r.height)]; };
        const sample = () => { window.__cwdFrames.push([...rect('#new-session-dialog'), ...rect('#new-cwd-options')]);
          if (!window.__cwdStop) requestAnimationFrame(sample); };
        requestAnimationFrame(sample); }""")
    with page.expect_response(lambda r: "/api/term/complete-dir" in r.url and "path=%2Fsh" in r.url):
        cwd.press_sequentially("/sh", delay=40)
    page.wait_for_timeout(200)
    expect(page.locator("#new-cwd-options [data-cwd-kind=completion]", has_text="/shared/proj")).to_be_visible()
    frames = page.evaluate("() => { window.__cwdStop = true; return window.__cwdFrames; }")
    assert len(frames) > 5 and len({tuple(f) for f in frames}) == 1, sorted({tuple(f) for f in frames})
    cwd.fill("/shared/proj")
    with page.expect_response(lambda r: "/api/term/complete-dir" in r.url and f"node={other.nid}" in r.url):
        page.locator("#new-node").select_option(other.nid)
    page.wait_for_timeout(200)
    expect(cwd).to_have_value("/shared/proj")
    cwd.fill("/only-b/work")
    with page.expect_response(lambda r: "/api/term/complete-dir" in r.url and f"node={node.nid}" in r.url):
        page.locator("#new-node").select_option(node.nid)
    expect(cwd).not_to_have_value("/only-b/work")
    expect(cwd).not_to_have_value("")
    assert not errors, errors
    context.close()
    node.pop("dirs")
    other.pop("dirs")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release/sessiondock"))
    parser.add_argument("--hub-binary")
    args = parser.parse_args()
    binary = Path(args.hub_binary) if args.hub_binary else Path(args.binary).parent / "sessiondock-hub"
    nodes = [FakeNode("a" * 32, "NodeA"), FakeNode("b" * 32, "NodeB")]
    try:
        nodes[0].set(create_info={"sid": None, "record_id": "fixture-new-record",
            "instance_id": "fixture-new-instance", "launch_id": "fixture-new-launch",
            "state": "running", "running": True})
        nodes[1].set(pending=[{"name": "other-terminal", "source": "codex", "cwd": "/synthetic/work",
            "record_id": "fixture-other-record", "instance_id": "fixture-other-instance",
            "state": "running", "running": True}])
        with tempfile.TemporaryDirectory(prefix="sessiondock-hub-pending-") as temp:
            hub = Hub(binary, Path(temp), nodes)
            hub.start()
            try:
                with sync_playwright() as playwright:
                    launch = {"headless": True}
                    executable = os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE") or (str(CHROMIUM) if CHROMIUM.is_file() else "")
                    if executable:
                        launch["executable_path"] = executable
                    browser = playwright.chromium.launch(**launch)
                    node_source_picker(browser, hub, *nodes)
                    node_switch_cwd(browser, hub, *nodes)
                    scenario(browser, hub, *nodes)
                    browser.close()
            finally:
                hub.stop()
    finally:
        for node in nodes:
            node.stop()
    print("PASS hub_pending_state_browser: per-node OpenCode picker, machine select on the first row, steady cwd list while typing, node switch keeps existing cwd, create, partial list, draft/reload, recovery, confirmed exit")


if __name__ == "__main__":
    main()
