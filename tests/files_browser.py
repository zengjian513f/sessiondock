#!/usr/bin/env python3
"""Read-only scoped file navigation through legacy pages, synthetic inputs only."""
import argparse
import json
import os
from pathlib import Path
import re
import tempfile
from urllib.parse import urlencode

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, build_corpus, claude_row, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-files-browser-") as temporary:
        corpus = build_corpus(Path(temporary))
        files = corpus.root / "files"
        files.mkdir()
        (files / "notes.md").write_text("# Synthetic document\n\n<script>window.fileInjected=true</script>\n\nscoped file content\n")
        (files / "unmentioned.txt").write_text("not a session reference")
        (files / "nested").mkdir()
        (files / "nested" / "child.txt").write_text("nested directory content")
        (files / "picture.png").write_bytes(bytes.fromhex("89504e470d0a1a0a"))
        (files / "invalid.pdf").write_bytes(b"synthetic damaged PDF without magic")
        sid = "claude-file-test"
        row = claude_row(sid,"user","u0",None,f"Open `{files}/` and `{files / 'notes.md'}` and `{files / 'invalid.pdf'}`. Missing `missing.txt`.",cwd=str(files))
        corpus.put(sid,"claude",[row],[])
        native_before = {path:path.read_bytes() for path in corpus.paths.values()}
        before = {path:path.read_bytes() for path in files.rglob("*") if path.is_file()}
        with isolated_server(corpus,args.binary,file_roots=(files,)) as (base,_), sync_playwright() as playwright:
            launch = {"headless":True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            context = browser.new_context(viewport={"width":1280,"height":900},service_workers="block")
            context.route("**/*",lambda route:route.continue_() if route.request.url.startswith(base+"/") else route.abort())
            errors,requests = [],[]
            context.on("page",lambda page:page.on("pageerror",lambda error:errors.append(str(error))))
            context.on("request",lambda request:requests.append(request.url))
            try:
                page = context.new_page()
                page.goto(base,wait_until="networkidle")
                page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                expect(page.locator("#msgs")).to_contain_text("notes.md")
                with context.expect_page() as opened:
                    page.locator(f'#msgs a[data-file-ref={json.dumps(str(files / "notes.md"))}]').click()
                preview = opened.value
                expect(preview.locator("#file-content")).to_contain_text("scoped file content")
                expect(preview.locator("#file-title")).to_have_text("notes.md")
                assert not preview.evaluate("!!window.fileInjected")
                preview.get_by_role("button",name="源码",exact=True).click()
                expect(preview.locator(".reader-source")).to_contain_text("<script>")
                with preview.expect_download() as saved:
                    preview.locator("#file-download").click()
                assert Path(saved.value.path()).read_bytes() == before[files / "notes.md"]
                with context.expect_page() as opened:
                    page.locator(f'#msgs a[data-file-ref={json.dumps(str(files)+"/")}]').click()
                manager = opened.value
                expect(manager.locator("#entries")).to_contain_text("unmentioned.txt")
                expect(manager.locator("#machine")).to_contain_text("只读")
                expect(manager.locator('[data-action="new"]').first).to_be_disabled()
                expect(manager.locator('[data-action="delete"]').first).to_be_disabled()
                manager.locator("#view").select_option("grid")
                manager.locator('#entries .entry-link').filter(has_text="nested").dblclick()
                expect(manager.locator("#entries")).to_contain_text("child.txt")
                # Crumbs above the bounding root are labels; the root and below are links.
                crumbs = manager.locator("#breadcrumbs a")
                expect(crumbs.last).to_have_text("nested")
                expect(crumbs.last).to_have_attribute("data-navigate", "1")
                expect(crumbs.first).to_have_text("根目录")
                expect(crumbs.first).to_have_attribute("aria-disabled", "true")
                expect(crumbs.filter(has_text=re.compile(f"^{re.escape(files.name)}$")).first).to_have_attribute("data-navigate", "1")
                assert manager.evaluate("[...document.querySelectorAll('#breadcrumbs a')].filter(a => a.dataset.navigate).map(a => a.textContent)") == list(Path(files).parts[-1:]) + ["nested"]
                # A refused navigation (address bar outside the root) keeps the last listing,
                # shows the server reason and restores the URL, so a refresh does not re-fail.
                manager.locator("#edit-address").click()
                manager.locator("#address").fill("/etc")
                manager.locator("#address").press("Enter")
                expect(manager.locator("#status")).to_have_class("error")
                expect(manager.locator("#status")).to_contain_text("越出")
                expect(manager.locator("#entries")).to_contain_text("child.txt")
                expect(manager.locator("#breadcrumbs a").last).to_have_text("nested")
                expect(manager.locator("#page-info")).to_contain_text("1–1 / 1")
                assert manager.evaluate("new URLSearchParams(location.search).get('path')") == str(files / "nested")
                manager.locator("#refresh").click()
                expect(manager.locator("#entries")).to_contain_text("child.txt")
                expect(manager.locator("#status")).not_to_have_class("error")
                manager.locator('#entries .entry-link').filter(has_text="child.txt").dblclick()
                expect(manager.locator("#preview-content")).to_contain_text("nested directory content")
                manager.locator('[data-close="preview-dialog"]').first.click()
                manager.set_viewport_size({"width":390,"height":844})
                expect(manager.locator("#entries")).to_be_visible()
                manager.locator("#tasks-open").click()
                expect(manager.locator("#tasks-list")).to_contain_text("尚未实现")
                # Let the existing task-poll interval elapse: unsupported work
                # must not be launched silently in the background.
                manager.wait_for_timeout(1700)
                assert not [url for url in requests if "mode=jobs" in url or "mode=thumbnail" in url], requests
                denied = context.new_page()
                denied.goto(base+"/file.html?"+urlencode({"uid":corpus.uid(sid),"ref":str(files/"unmentioned.txt"),"open":1}))
                expect(denied.locator("#file-content")).to_contain_text("所选会话分支")
                broken = context.new_page()
                broken.goto(base+"/file.html?"+urlencode({"uid":corpus.uid(sid),"ref":str(files/"invalid.pdf"),"open":1}))
                expect(broken.locator("#file-content")).to_contain_text("PDF 标识")
                expect(broken.locator("#file-download")).to_be_visible()
                with broken.expect_download() as saved:
                    broken.locator("#file-download").click()
                assert Path(saved.value.path()).read_bytes() == before[files/"invalid.pdf"]
                assert not errors, errors
            finally:
                context.close()
                browser.close()
        assert all(path.read_bytes()==content for path,content in native_before.items())
        assert all(path.read_bytes()==content for path,content in before.items())
        print("PASS scoped files browser: session links, Markdown/source safety, exact download, directory navigation, bounded crumbs + refused-navigation rollback, readonly/mobile, capability gates, forbidden unmentioned file, native/files unchanged")


if __name__ == "__main__":
    main()
