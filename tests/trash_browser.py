#!/usr/bin/env python3
"""Real legacy page against the Rust session recycle bin.

With SESSIONDOCK_TRASH_DIR configured (and no host directory, so every run
state is unknown) the untouched legacy delete flow shows the explicit force
confirmation, the detail pane's "已移入回收站" receipt, the trash dialog with
restore/purge, and the visible refusal reasons (fork parent, unknown state
declined, restore conflict). Desktop and 390px. Synthetic corpus only.
"""
import argparse
import json
import os
from pathlib import Path
import tempfile
from urllib.error import HTTPError
from urllib.request import Request

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, build_corpus, get_json, isolated_server


class Dialogs:
    """Scripted confirm/alert handling: each expected dialog names a substring
    of its message and whether to accept it; anything unexpected fails."""

    def __init__(self, page):
        self.expected = []
        self.seen = []
        page.on("dialog", self.handle)

    def expect(self, *steps):
        self.expected.extend(steps)

    def handle(self, dialog):
        self.seen.append((dialog.type, dialog.message))
        assert self.expected, f"unexpected {dialog.type}: {dialog.message}"
        needle, accept = self.expected.pop(0)
        assert needle in dialog.message, (needle, dialog.message)
        if accept:
            dialog.accept()
        else:
            dialog.dismiss()

    def drained(self):
        assert not self.expected, f"dialogs never shown: {self.expected}"


def status_of(opener, base, route, method="GET", body=None):
    request = Request(base + route, data=body, method=method,
                      headers={"Content-Type": "application/json"} if body else {})
    try:
        with opener.open(request, timeout=10) as response:
            return response.status
    except HTTPError as error:
        return error.code


def listed_uids(opener, base):
    return {row["uid"] for row in get_json(opener, base, "/api/sessions?force=1")["sessions"]}


def open_session(page, corpus, sid, text):
    page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
    expect(page.locator("#msgs")).to_contain_text(text)
    page.wait_for_function("_es && _es.readyState === EventSource.OPEN")


def click_delete(page, narrow):
    if narrow:
        page.locator("#a-more").click()
    button = page.locator("#a-session-action")
    expect(button).to_have_attribute("title", "删除会话")
    button.click()


def open_trash(page, narrow):
    if narrow:
        page.locator("#header-more-btn").click()
    page.locator("#trash").click()
    expect(page.locator("#trash-dialog")).to_be_visible()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-trash-browser-") as temporary:
        corpus = build_corpus(Path(temporary))
        native_before = {name: path.read_bytes() for name, path in corpus.paths.items()}
        trash = corpus.root / "trash"
        trash.mkdir(mode=0o700)
        # Unconfigured: capability false, routes 501, directory untouched.
        with isolated_server(corpus, args.binary) as (base, opener):
            assert get_json(opener, base, "/api/meta")["capabilities"]["trash"] is False
            assert status_of(opener, base, "/api/session/" + corpus.uid("claude-abandoned"), "DELETE") == 501
            assert status_of(opener, base, "/api/trash") == 501
        assert not any(trash.iterdir())
        with sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                with isolated_server(corpus, args.binary, trash_dir=trash) as (base, opener):
                    assert get_json(opener, base, "/api/meta")["capabilities"]["trash"] is True
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                    page = context.new_page()
                    errors = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    dialogs = Dialogs(page)
                    page.goto(base, wait_until="networkidle")
                    expect(page.locator("#backend-notice")).to_be_hidden()  # batch 44: no standing banner

                    # Delete: confirm, then the explicit unknown-state force prompt.
                    uid = corpus.uid("claude-abandoned")
                    open_session(page, corpus, "claude-abandoned", "Claude abandoned base answer")
                    dialogs.expect(("服务端回收站", True), ("运行状态未知", True))
                    click_delete(page, narrow=False)
                    expect(page.locator("#detail")).to_contain_text("已移入回收站")
                    dialogs.drained()
                    expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(0)
                    assert uid not in listed_uids(opener, base)
                    assert not corpus.paths["claude-abandoned"].exists()
                    entries = [path for path in trash.iterdir() if path.is_dir()]
                    assert len(entries) == 1 and (entries[0] / "manifest.json").is_file(), entries
                    manifest = json.loads((entries[0] / "manifest.json").read_text())
                    assert manifest["uid"] == uid and manifest["state"] == "trashed" and manifest["forced"] is True

                    # Trash dialog from the detail receipt: restore brings it back.
                    page.locator("#detail-open-trash").click()
                    expect(page.locator("#trash-dialog")).to_be_visible()
                    row = page.locator(f'.trash-item[data-id="{entries[0].name}"]')
                    expect(row).to_be_visible()
                    expect(row.locator(".trash-title")).to_contain_text(manifest["title"])
                    expect(row.locator(".trash-origin")).to_contain_text("恢复到")
                    expect(page.locator("#trash-sub")).to_contain_text("1 个已删除会话")
                    row.locator('button[data-act="restore"]').click()
                    expect(page.locator("#trash-note")).to_contain_text("已恢复「")
                    expect(page.locator("#trash-list")).to_contain_text("没有已删除的会话")
                    page.locator("#trash-done").click()
                    expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(1)
                    assert uid in listed_uids(opener, base)
                    assert corpus.paths["claude-abandoned"].read_bytes() == native_before["claude-abandoned"]
                    assert not any(trash.iterdir())

                    # Fork parent: the server refuses with a visible reason.
                    parent = corpus.uid("codex-parent")
                    open_session(page, corpus, "codex-parent", "Codex inherited answer")
                    dialogs.expect(("服务端回收站", True), ("删除失败: 父会话只能隐藏，不能删除", True))
                    click_delete(page, narrow=False)
                    page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                    dialogs.drained()
                    expect(page.locator(f'#side .item[data-uid="{parent}"]')).to_have_count(1)
                    assert corpus.paths["codex-parent"].exists()

                    # Declining the force prompt keeps the session and explains why.
                    open_session(page, corpus, "claude-compact", "Claude post compact answer")
                    dialogs.expect(("服务端回收站", True), ("运行状态未知", False), ("删除失败: 运行状态未知", True))
                    click_delete(page, narrow=False)
                    page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                    dialogs.drained()
                    expect(page.locator(f'#side .item[data-uid="{corpus.uid("claude-compact")}"]')).to_have_count(1)
                    assert corpus.paths["claude-compact"].exists()

                    # Restore conflict: a recreated original is never overwritten.
                    branch = corpus.uid("claude-branch")
                    open_session(page, corpus, "claude-branch", "Claude selected answer")
                    dialogs.expect(("服务端回收站", True), ("运行状态未知", True))
                    click_delete(page, narrow=False)
                    expect(page.locator("#detail")).to_contain_text("已移入回收站")
                    dialogs.drained()
                    assert not corpus.paths["claude-branch"].exists() and not corpus.paths["claude-agent-one"].exists()
                    page.locator("#detail-open-trash").click()
                    entry = next(path for path in trash.iterdir() if path.is_dir())
                    row = page.locator(f'.trash-item[data-id="{entry.name}"]')
                    expect(row.locator('button[data-act="restore"]')).to_be_enabled()
                    # The original reappears after the dialog was rendered: the
                    # restore request itself must be the one that refuses.
                    corpus.paths["claude-branch"].write_bytes(b'{"type":"user","uuid":"x","parentUuid":null,"sessionId":"claude-branch","message":{"role":"user","content":"recreated"}}\n')
                    row.locator('button[data-act="restore"]').click()
                    expect(page.locator("#trash-note")).to_contain_text("原路径已存在")
                    expect(page.locator("#trash-note")).to_have_class("err")
                    assert corpus.paths["claude-branch"].read_bytes().endswith(b'"recreated"}}\n')
                    assert not corpus.paths["claude-agent-one"].exists(), "no partial restore"
                    page.locator("#trash-reload").click()
                    expect(row.locator(".trash-origin.warn")).to_contain_text("原路径已存在")
                    expect(row.locator('button[data-act="restore"]')).to_be_disabled()
                    corpus.paths["claude-branch"].unlink()
                    page.locator("#trash-reload").click()
                    expect(row.locator('button[data-act="restore"]')).to_be_enabled()
                    dialogs.expect(("彻底删除「", True))
                    row.locator('button[data-act="purge"]').click()
                    expect(page.locator("#trash-note")).to_contain_text("已彻底删除「")
                    dialogs.drained()
                    assert not entry.exists() and not any(trash.iterdir())
                    assert not corpus.paths["claude-branch"].exists()
                    page.locator("#trash-done").click()
                    expect(page.locator(f'#side .item[data-uid="{branch}"]')).to_have_count(0)
                    assert not errors, errors
                    context.close()

                    # Mobile: menu-folded delete, force prompt, trash via header menu.
                    context = browser.new_context(viewport={"width": 390, "height": 844}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                    page = context.new_page()
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    dialogs = Dialogs(page)
                    page.goto(base, wait_until="networkidle")
                    compact = corpus.uid("claude-compact")
                    open_session(page, corpus, "claude-compact", "Claude post compact answer")
                    dialogs.expect(("服务端回收站", True), ("运行状态未知", True))
                    click_delete(page, narrow=True)
                    expect(page.locator(f'#side .item[data-uid="{compact}"]')).to_have_count(0)
                    dialogs.drained()
                    assert not corpus.paths["claude-compact"].exists()
                    open_trash(page, narrow=True)
                    entry = next(path for path in trash.iterdir() if path.is_dir())
                    row = page.locator(f'.trash-item[data-id="{entry.name}"]')
                    expect(row).to_be_visible()
                    row.locator('button[data-act="restore"]').click()
                    expect(page.locator("#trash-note")).to_contain_text("已恢复「")
                    page.locator("#trash-done").click()
                    expect(page.locator(f'#side .item[data-uid="{compact}"]')).to_have_count(1)
                    assert corpus.paths["claude-compact"].read_bytes() == native_before["claude-compact"]
                    assert not errors, errors
                    context.close()
                for name, path in corpus.paths.items():
                    if name not in {"claude-branch", "claude-agent-one"}:
                        assert path.read_bytes() == native_before[name], name
            finally:
                browser.close()
    print("PASS trash browser: delete needs explicit force while run state is unknown, detail receipt → trash dialog → restore, "
          "fork parent and declined force refused visibly, restore conflict never overwrites, purge removes the entry; desktop + 390px")


if __name__ == "__main__":
    main()
