#!/usr/bin/env python3
"""Write-side file manager through the real legacy UI: upload, rename, delete
(into the private state-directory trash, never the project tree) and the
explicit overwrite error, at desktop and 390px widths. Synthetic inputs only;
the write root is a private temporary directory."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
from urllib.parse import urlencode

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, build_corpus, claude_row, get_json, isolated_server


def trash_files(state: Path):
    trash = state / "file-trash"
    if not trash.is_dir():
        return []
    return [entry for item in trash.iterdir() for entry in item.iterdir() if entry.name != "manifest.json"]


def submit_dialog(page, **fields):
    dialog = page.locator("#action-dialog")
    expect(dialog).to_be_visible()
    for name, value in fields.items():
        field = page.locator(f"#field-{name}")
        if field.evaluate("node => node.tagName") == "SELECT":
            field.select_option(value)
        else:
            field.fill(value)
    page.locator("#action-submit").click()
    expect(dialog).to_be_hidden()


def close_tasks(manager):
    """submit() opens the modal task drawer; close it as a user would."""
    dialog = manager.locator("#tasks-dialog")
    if dialog.evaluate("node => node.open"):
        manager.locator('[data-close="tasks-dialog"]').first.click()
    expect(dialog).to_be_hidden()


def upload_through_ui(manager, path: Path, conflict="error"):
    with manager.expect_file_chooser() as chooser:
        manager.locator('[data-action="upload"]').first.click()
    chooser.value.set_files(str(path))
    submit_dialog(manager, conflict=conflict)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-files-write-browser-") as temporary:
        corpus = build_corpus(Path(temporary))
        files = corpus.root / "files"
        write = files / "w"
        write.mkdir(parents=True)
        (files / "readonly").mkdir()
        (files / "readonly" / "keep.txt").write_text("read only content")
        (write / "existing.txt").write_text("already here")
        local = Path(temporary) / "local"
        local.mkdir()
        payload = os.urandom(6 * 1024 * 1024 + 123)  # crosses the 4 MiB chunk boundary
        (local / "synthetic-upload.bin").write_bytes(payload)
        (local / "existing.txt").write_text("would overwrite")
        state = corpus.root / "state"
        state.mkdir(mode=0o700)
        sid = "claude-file-write-test"
        row = claude_row(sid, "user", "u0", None, f"Write into `{write}/` and read `{files / 'readonly'}/`.", cwd=str(files))
        corpus.put(sid, "claude", [row], [])
        native_before = {path: path.read_bytes() for path in corpus.paths.values()}
        readonly_before = {path: path.read_bytes() for path in (files / "readonly").rglob("*") if path.is_file()}
        with isolated_server(corpus, args.binary, state_dir=state, file_roots=(files,), file_write_roots=(write,)) as (base, opener), sync_playwright() as playwright:
            capabilities = get_json(opener, base, "/api/meta")["capabilities"]
            assert capabilities["files_jobs"] is True and capabilities["files_write"]["chunk_bytes"] == 4 * 1024 * 1024, capabilities
            assert "delete" in capabilities["files_write"]["actions"], capabilities
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            errors, requests = [], []
            context.on("page", lambda page: page.on("pageerror", lambda error: errors.append(str(error))))
            context.on("request", lambda request: requests.append(request.url))
            try:
                page = context.new_page()
                page.goto(base, wait_until="networkidle")
                page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                expect(page.locator("#msgs")).to_contain_text("readonly")
                with context.expect_page() as opened:
                    page.locator(f'#msgs a[data-file-ref={json.dumps(str(write) + "/")}]').click()
                manager = opened.value
                expect(manager.locator("#entries")).to_contain_text("existing.txt")
                expect(manager.locator("#machine")).not_to_contain_text("只读")
                expect(manager.locator('[data-action="upload"]').first).to_be_enabled()
                expect(manager.locator('[data-action="new"]').first).to_be_enabled()
                # Unsupported operations are not offered on the Rust backend.
                expect(manager.locator('[data-action="compress"]').first).to_be_disabled()
                # Upload through the real file chooser, conflict dialog and chunked XHR.
                upload_through_ui(manager, local / "synthetic-upload.bin")
                expect(manager.locator("#entries")).to_contain_text("synthetic-upload.bin", timeout=30000)
                uploaded = write / "synthetic-upload.bin"
                assert uploaded.read_bytes() == payload
                assert hashlib.sha256(uploaded.read_bytes()).hexdigest() == hashlib.sha256(payload).hexdigest()
                chunk_requests = [url for url in requests if "/api/session/files/upload?" in url]
                assert len(chunk_requests) == 2, chunk_requests
                assert not list((write / ".sessiondock-upload").iterdir())
                assert not (write / ".agenthub-upload").exists()
                # submit() opened the task drawer: the upload shows as completed.
                expect(manager.locator("#tasks-dialog")).to_be_visible()
                expect(manager.locator("#tasks-list")).to_contain_text("上传")
                expect(manager.locator("#tasks-list")).to_contain_text("已完成")
                close_tasks(manager)
                # Overwrite attempt: the explicit server error is shown, nothing changes.
                upload_through_ui(manager, local / "existing.txt")
                expect(manager.locator("#status")).to_contain_text("目标已存在")
                expect(manager.locator("#status")).to_have_class("error")
                assert (write / "existing.txt").read_text() == "already here"
                # Rename through the dialog.
                manager.locator("#entries .entry-link").filter(has_text="synthetic-upload.bin").click()
                manager.locator('[data-action="rename"]').first.click()
                submit_dialog(manager, name="renamed-upload.bin")
                expect(manager.locator("#entries")).to_contain_text("renamed-upload.bin")
                expect(manager.locator("#entries")).not_to_contain_text("synthetic-upload.bin")
                expect(manager.locator("#tasks-list")).to_contain_text("重命名")
                close_tasks(manager)
                assert (write / "renamed-upload.bin").read_bytes() == payload and not uploaded.exists()
                # Rename onto an existing name: explicit error, no overwrite.
                manager.locator("#entries .entry-link").filter(has_text="renamed-upload.bin").click()
                manager.locator('[data-action="rename"]').first.click()
                submit_dialog(manager, name="existing.txt")
                expect(manager.locator("#status")).to_contain_text("目标已存在")
                assert (write / "existing.txt").read_text() == "already here"
                close_tasks(manager)
                # Narrow screen: delete moves the file into the private state-directory trash.
                manager.set_viewport_size({"width": 390, "height": 844})
                expect(manager.locator("#entries")).to_be_visible()
                manager.locator("#entries .entry-link").filter(has_text="renamed-upload.bin").click()
                expect(manager.locator('[data-action="delete"]').first).to_be_enabled()
                manager.locator('[data-action="delete"]').first.click()
                expect(manager.locator("#action-description")).to_contain_text("回收目录")
                expect(manager.locator("#action-description")).to_contain_text("不在项目目录内")
                manager.locator("#action-submit").click()
                expect(manager.locator("#entries")).not_to_contain_text("renamed-upload.bin")
                expect(manager.locator("#tasks-list")).to_contain_text("回收目录")
                close_tasks(manager)
                trashed = trash_files(state)
                assert [entry.name for entry in trashed] == ["renamed-upload.bin"], trashed
                assert not [entry for entry in write.iterdir() if entry.name.endswith("-trash")], list(write.iterdir())
                assert trashed[0].read_bytes() == payload
                manifest = json.loads((trashed[0].parent / "manifest.json").read_text())
                assert manifest["path"] == str(write / "renamed-upload.bin") and manifest["uid"] == corpus.uid(sid)
                assert not (write / "renamed-upload.bin").exists()
                # Read-only sibling directory of the same read root stays read only.
                readonly = context.new_page()
                readonly.goto(base + "/files.html?" + urlencode({"uid": corpus.uid(sid), "ref": str(files / "readonly") + "/"}))
                expect(readonly.locator("#entries")).to_contain_text("keep.txt")
                expect(readonly.locator("#machine")).to_contain_text("只读")
                expect(readonly.locator('[data-action="upload"]').first).to_be_disabled()
                assert not [url for url in requests if "mode=thumbnail" in url], requests
                assert not errors, errors
            finally:
                context.close()
                browser.close()
        assert all(path.read_bytes() == content for path, content in native_before.items())
        assert all(path.read_bytes() == content for path, content in readonly_before.items())
        print("PASS files write browser: capability gate, chunked upload via UI, task panel, overwrite error, rename, "
              "390px delete into the state-directory trash, read-only sibling unchanged, native/read-only bytes unchanged")


if __name__ == "__main__":
    main()
