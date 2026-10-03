#!/usr/bin/env python3
# run_validation: tags=browser
"""Hub settings: color palette, unsaved machine name, and renderer rollback.

grok-4.7 high headless draft, reviewed by the primary at integration.
All fixtures are synthetic. Hub and FakeNode listen on loopback only; the
working directory is private and removed on exit. The page is the frontend
Hub already serves through frontend_dir(), including SESSIONDOCK_TEST_WEB_DIR.

Real clicks, typing, Enter and select changes drive the existing controls.
Enable, pointer/keyboard order, rename persistence and client update stay in
their own suites. One Playwright route fails a single display POST for a name
save and another for a renderer change; the page's own request handles it.
The uncommitted name is checked by calling renderMachineSettings() once.
That is the redraw the open machines pane already runs after a term list
refresh, invoked here as an explicit simulation rather than a second launcher.
"""
from __future__ import annotations

from browser_runtime import js
import argparse
import json
import os
from pathlib import Path
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from hub_browser import CHROMIUM
from hub_http_suite import REPO, FakeNode, Hub

NID_A = "a" * 32
NID_B = "b" * 32
NAME_A = "NodeA"
NAME_B = "NodeB"
DRAFT = "未提交草稿"
FAILED_NAME = "失败草稿"
NAME_ERROR = "合成名称失败"
RENDER_ERROR = "合成渲染失败"
BLUE = "blue"
BLUE_LABEL = "蓝"
GRID = "grid"
XTERM = "xterm"
GRID_LABEL = "服务端网格（默认）"
XTERM_LABEL = "xterm.js（浏览器解析）"
NOTE = "#machine-note"
SAVED = f"已保存 {NAME_A}。"
NAME_FAIL = f"保存失败：{NAME_ERROR}"
RENDER_FAIL = f"{NAME_A}：切换失败：{RENDER_ERROR}"
RENDER_OK = f"{NAME_A}：控制台改用 {XTERM_LABEL}；重新打开控制台后生效。"


class DisplayFault:
    """Fail the next display POST whose body contains one field, then let it through."""

    def __init__(self):
        self.field = None
        self.error = ""
        self.hits = []

    def arm(self, field, error):
        self.field = field
        self.error = error

    def handle(self, route):
        request = route.request
        if request.method != "POST" or not urlsplit(request.url).path.endswith("/display"):
            route.continue_()
            return
        body = request.post_data_json
        if self.field and isinstance(body, dict) and self.field in body:
            error = self.error
            self.hits.append({"field": self.field, "body": body})
            self.field = None
            route.fulfill(status=500, content_type="application/json",
                          body=json.dumps({"error": error}, ensure_ascii=False))
            return
        route.continue_()


def launch_chromium(playwright):
    launch = {"headless": True}
    executable = os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE") or (
        str(CHROMIUM) if CHROMIUM.is_file() else "")
    if executable:
        launch["executable_path"] = executable
    return playwright.chromium.launch(**launch)


def machine_row(page, name):
    return page.locator("#machine-rows .machine-row").filter(
        has=page.locator(f'input[aria-label="{name} 的名称"]'))


def color_swatch(row):
    return row.locator("button.machine-swatch[data-machine-color]")


def open_machines(page):
    page.locator("#settings").click()
    expect(page.locator("#settings-dialog")).to_be_visible()
    page.locator(".settings-tab[data-tab='machines']").click()
    expect(page.locator("#settings-machines")).to_be_visible()
    expect(page.locator("#settings-sub")).to_have_text("机器设置保存在中央服务端，所有浏览器一致")
    expect(page.locator("#machine-rows .machine-row")).to_have_count(2)


def close_settings(page):
    page.locator("#settings-dialog .modal-actions button").click()
    expect(page.locator("#settings-dialog")).to_be_hidden()


def saved_machine(hub, nid):
    status, body = hub.json("GET", "/api/nodes")
    assert status == 200, body
    return next(row for row in body["machines"] if row["id"] == nid)


def check(page, hub, fault):
    page.goto(f"http://127.0.0.1:{hub.port}/", wait_until="networkidle")
    page.wait_for_function(js("T.listLoaded && Nodes.machines.length === 2", 'runtime.terminal.state.listLoaded && runtime.core.state.nodes.machines.length === 2'))
    open_machines(page)

    row = machine_row(page, NAME_A)
    other = machine_row(page, NAME_B)
    expect(row).to_have_count(1)
    expect(other).to_have_count(1)
    swatch = color_swatch(row)
    palette = row.locator(".machine-palette")
    chip = page.locator(f'#node-chips button[data-node="{NID_A}"]')
    other_chip = page.locator(f'#node-chips button[data-node="{NID_B}"]')
    note = page.locator(NOTE)
    expect(chip).to_be_visible()
    expect(swatch).to_have_attribute("data-node-color", "")
    expect(chip).to_have_attribute("data-node-color", "")
    expect(other_chip).to_have_attribute("data-node-color", "")
    expect(palette).to_be_hidden()

    # Pick blue. The chip changes immediately; the original machine pane reads
    # the registry snapshot again after reload, so verify its saved color then.
    swatch.click()
    expect(palette).to_be_visible()
    expect(swatch).to_have_attribute("aria-expanded", "true")
    palette.get_by_role("option", name=BLUE_LABEL, exact=True).click()
    expect(palette).to_be_hidden()
    expect(chip).to_have_attribute("data-node-color", BLUE)
    expect(other_chip).to_have_attribute("data-node-color", "")
    expect(note).to_have_text(SAVED)
    expect(note).to_have_attribute("data-state", "ok")
    expect(page.locator("#settings-dialog")).to_be_visible()
    page.reload(wait_until="networkidle")
    open_machines(page)
    expect(swatch).to_have_attribute("data-node-color", BLUE)

    # Escape closes the palette only, and focus returns to the swatch.
    swatch.click()
    expect(palette).to_be_visible()
    expect(palette.get_by_role("option", name=BLUE_LABEL, exact=True)).to_be_focused()
    page.keyboard.press("Escape")
    expect(palette).to_be_hidden()
    expect(swatch).to_have_attribute("aria-expanded", "false")
    expect(swatch).to_be_focused()
    expect(page.locator("#settings-dialog")).to_be_visible()
    expect(page.locator("#settings-machines")).to_be_visible()

    # A pointerdown outside the color control closes the palette the same way.
    swatch.click()
    expect(palette).to_be_visible()
    page.locator("#settings-sub").click()
    expect(palette).to_be_hidden()
    expect(swatch).to_have_attribute("aria-expanded", "false")
    expect(page.locator("#settings-dialog")).to_be_visible()
    expect(page.locator("#settings-machines")).to_be_visible()

    # Type a name and redraw before Enter. The draft and the caret stay put.
    name = row.locator("input[type='text']")
    name.click()
    name.fill(DRAFT)
    expect(name).to_have_value(DRAFT)
    expect(name).to_be_focused()
    # The ESM terminal-list refresh calls the actual Machines renderer while
    # settings are open. Exercise that consumer without a fabricated hook.
    page.evaluate(js("renderMachineSettings()", "runtime.terminal.loadTermList()"))
    expect(name).to_have_value(DRAFT)
    expect(name).to_be_focused()
    name.fill(NAME_A)
    page.locator("#settings-title").click()
    expect(name).to_have_value(NAME_A)

    # The name save fails once. The field returns to the stored name.
    expect(name).to_have_value(NAME_A)
    fault.arm("name", NAME_ERROR)
    name.click()
    name.fill(FAILED_NAME)
    name.press("Enter")
    expect(name).to_have_value(NAME_A)
    expect(note).to_have_text(NAME_FAIL)
    expect(note).to_have_attribute("data-state", "error")
    expect(page.locator("#settings-dialog")).to_be_visible()
    expect(chip).to_contain_text(NAME_A)

    # The renderer change fails once, then grid to xterm succeeds and stays.
    select = row.locator("select.machine-renderer")
    expect(select.locator("option")).to_have_text([GRID_LABEL, XTERM_LABEL])
    expect(select).to_have_value(GRID)
    fault.arm("renderer", RENDER_ERROR)
    select.select_option(label=XTERM_LABEL)
    expect(select).to_have_value(GRID)
    expect(select.locator("option:checked")).to_have_text(GRID_LABEL)
    expect(note).to_have_text(RENDER_FAIL)
    expect(note).to_have_attribute("data-state", "error")
    select.select_option(label=XTERM_LABEL)
    expect(select).to_have_value(XTERM)
    expect(select.locator("option:checked")).to_have_text(XTERM_LABEL)
    expect(note).to_have_text(RENDER_OK)
    expect(note).to_have_attribute("data-state", "ok")
    close_settings(page)
    open_machines(page)
    select = machine_row(page, NAME_A).locator("select.machine-renderer")
    expect(select).to_have_value(XTERM)
    expect(select.locator("option:checked")).to_have_text(XTERM_LABEL)
    expect(color_swatch(machine_row(page, NAME_A))).to_have_attribute("data-node-color", BLUE)

    node_a = saved_machine(hub, NID_A)
    node_b = saved_machine(hub, NID_B)
    assert node_a["name"] == NAME_A and node_a["color"] == BLUE and node_a["renderer"] == XTERM, node_a
    assert node_b["name"] == NAME_B and node_b["color"] == "" and node_b["renderer"] == GRID, node_b
    assert [hit["field"] for hit in fault.hits] == ["name", "renderer"], fault.hits
    assert fault.hits[0]["body"]["name"] == FAILED_NAME, fault.hits[0]
    assert fault.hits[1]["body"]["renderer"] == XTERM, fault.hits[1]
    assert fault.field is None


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", default=str(REPO / "target/release/sessiondock"))
    parser.add_argument("--hub-binary", default=None)
    args = parser.parse_args()
    hub_binary = Path(args.hub_binary) if args.hub_binary else Path(args.binary).resolve().parent / "sessiondock-hub"
    nodes = []
    try:
        nodes.append(FakeNode(NID_A, NAME_A))
        nodes.append(FakeNode(NID_B, NAME_B))
        with tempfile.TemporaryDirectory(prefix="sessiondock-machine-controls-") as temp:
            hub = Hub(hub_binary, Path(temp), nodes)
            hub.start()
            try:
                with sync_playwright() as playwright:
                    browser = launch_chromium(playwright)
                    try:
                        context = browser.new_context(viewport={"width": 1280, "height": 900},
                                                       service_workers="block")
                        fault = DisplayFault()
                        context.route("**/api/nodes/*/display", fault.handle)
                        page = context.new_page()
                        page_errors = []
                        page.on("pageerror", lambda error: page_errors.append(str(error)))
                        check(page, hub, fault)
                        assert not page_errors, page_errors
                    finally:
                        browser.close()
            finally:
                hub.stop()
    finally:
        for node in nodes:
            node.stop()
    print("PASS machine_controls_browser: color palette, name draft, display rollback, renderer", flush=True)


if __name__ == "__main__":
    main()
