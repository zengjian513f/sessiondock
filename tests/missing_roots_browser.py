#!/usr/bin/env python3
"""Native roots may disappear and return without breaking other sources or startup."""
import argparse
import os
from pathlib import Path
import shutil
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, REPO, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-missing-roots-") as tmp:
        root = Path(tmp)
        corpus = Corpus(root)
        sources = ("claude", "codex", "grok")
        for source in sources:
            shutil.copytree(REPO / "crates/sessiondock/tests/fixtures" / source,
                            root / (source + "-saved"))
        # All three configured roots are absent at startup, including parents.
        with isolated_server(corpus, args.binary) as (base, _), sync_playwright() as pw:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = pw.chromium.launch(**launch)
            page = browser.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(base, wait_until="networkidle")
            expect(page.locator("#side .item[data-uid]")).to_have_count(0)
            assert all(not (root / source).exists() for source in sources)

            def refresh(present):
                with page.expect_response(lambda r: r.url.split("?")[0].endswith("/api/sessions")) as response:
                    page.reload(wait_until="networkidle")
                assert response.value.status == 200, response.value.text()
                expect(page.locator("#side .item[data-uid]")).to_have_count(len(present))
                assert page.request.get(base + "/api/live").status == 200
                for source in present:
                    page.locator(f'#side .item[data-uid^="{source}:"]').click()
                    expect(page.locator("#msgs")).to_contain_text(
                        source.capitalize() + " 人工样例读取正常")

            # Automatic discovery: restoring a root needs no server restart or config edit.
            for source in sources:
                (root / (source + "-saved")).rename(root / source)
            refresh(sources)
            for missing in sources:
                (root / missing).rename(root / (missing + "-saved"))
                refresh([source for source in sources if source != missing])
                assert not (root / missing).exists()
                (root / (missing + "-saved")).rename(root / missing)
                refresh(sources)
            assert not errors, errors
            browser.close()
    print("PASS: missing roots at startup, removal and recovery of every source; real UI opens surviving sessions")


if __name__ == "__main__":
    main()
