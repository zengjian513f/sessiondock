#!/usr/bin/env python3
"""Free Chromium check of advanced native history through the unchanged legacy UI.

Requires a prebuilt Rust server and Python Playwright/Chromium. All transcripts
are generated in temporary directories by history_parity; no real CLI state or
paid commands are accessed. No frontend private method is used to switch views.
"""

import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, build_corpus, claude_row, codex_message, encoded, get_json, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-history-browser-") as temporary:
        corpus = build_corpus(Path(temporary))
        with isolated_server(corpus, args.binary) as (base, opener), sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                context.route("**/*", lambda route: route.continue_()
                              if route.request.url.startswith(base + "/") else route.abort())
                # Observe actual SSE traffic, including reset, without invoking
                # application methods or replacing the production transport.
                context.add_init_script("""(() => {
                  window.__historyEvents = [];
                  const Native = window.EventSource;
                  window.EventSource = class extends Native {
                    constructor(url, options) {
                      super(url, options);
                      this.addEventListener('message', event => {
                        try { window.__historyEvents.push({url: String(url), data: JSON.parse(event.data)}); }
                        catch {}
                        if (window.__historyEvents.length > 200) window.__historyEvents.shift();
                      });
                    }
                  };
                })();""")
                page = context.new_page()
                errors, requests = [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("request", lambda request: requests.append(request.url))
                response = page.goto(base, wait_until="networkidle")
                assert response.status == 200
                expect(page.locator("#backend-notice")).to_be_hidden()  # no standing banner

                def select(sid, text):
                    page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                    expect(page.locator("#msgs")).to_contain_text(text)
                    expect(page.locator("#a-term")).to_be_visible()
                    expect(page.locator("#a-term")).to_be_enabled()
                    expect(page.locator("#migration-read-error")).to_have_count(0)

                def agent(sid, text):
                    page.locator("#a-view-switch").click()
                    page.locator(f'#session-view-menu button[data-agent="{sid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text(text)
                    expect(page.locator("#a-term")).to_be_visible()
                    expect(page.locator("#a-term")).to_be_enabled()

                select("claude-branch", "Claude selected answer")
                expect(page.locator("#msgs")).not_to_contain_text("Claude discarded completed")
                agent("claude-agent-one", "Claude agent answer")
                expect(page.locator("#msgs")).not_to_contain_text("Claude selected answer")
                agent("", "Claude selected answer")
                expect(page.locator("#msgs")).not_to_contain_text("Claude agent answer")
                # The server's cached main/agent views already exist. Refresh
                # only a sibling sidecar. Shared SSE must update the displayed
                # menu without a list refresh/reload or a leaf-byte change.
                page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                corpus.paths["claude-agent-one"].with_suffix(".meta.json").write_text(json.dumps({
                    "description": "Renamed synthetic Claude child", "agentType": "reviewer"}))
                expect(page.locator("#msgs")).to_contain_text("Claude selected answer")
                page.locator("#a-view-switch").click()
                renamed_agent = page.locator('#session-view-menu button[data-agent="claude-agent-one"]')
                expect(renamed_agent).to_contain_text("Renamed synthetic Claude child")
                if not renamed_agent.is_visible():
                    page.locator("#a-view-switch").click()
                expect(renamed_agent).to_be_visible()
                renamed_agent.click()
                expect(page.locator("#msgs")).to_contain_text("Claude agent answer")
                agent("", "Claude selected answer")
                select("claude-compact", "Claude post compact answer")
                expect(page.locator("#msgs")).to_contain_text("Claude selected answer")
                expect(page.locator("#msgs")).to_contain_text("已压缩")
                expect(page.locator("#msgs")).not_to_contain_text("INTERNAL COMPACT SUMMARY")
                expect(page.locator("#msgs")).not_to_contain_text("Claude discarded completed")
                select("claude-current-compact", "Claude post compact answer")
                expect(page.locator("#msgs")).to_contain_text("已压缩")
                select("claude-abandoned", "Claude replacement answer")
                expect(page.locator("#msgs")).to_contain_text("Claude fast Esc input")
                expect(page.locator("#msgs")).to_contain_text("已中断")

                # A last-prompt append retracts a completed branch. A proper SSE
                # reset must replace the already rendered timeline, not append.
                select("claude-branch", "Claude selected answer")
                page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                page.evaluate("window.__historyEvents = []")
                with corpus.paths["claude-branch"].open("ab") as stream:
                    stream.write(encoded(claude_row("claude-branch", "user", "new-u", "a0", "Claude browser new branch")))
                    stream.write(encoded(claude_row("claude-branch", "assistant", "new-a", "new-u", "Claude browser new answer")))
                expect(page.locator("#msgs")).to_contain_text("Claude browser new answer", timeout=10000)
                with corpus.paths["claude-branch"].open("ab") as stream:
                    stream.write(encoded({"type": "last-prompt", "leafUuid": "a0"}))
                expect(page.locator("#msgs")).not_to_contain_text("Claude browser new answer", timeout=10000)
                expect(page.locator("#msgs")).to_contain_text("Claude selected answer")
                page.wait_for_function("window.__historyEvents.some(e => e.data.reset === true)")

                select("codex-grandchild", "Codex nested fork answer")
                expect(page.locator("#msgs")).to_contain_text("Codex parent prefix OLD")
                expect(page.locator("#msgs")).not_to_contain_text("Codex discarded parent tail")
                page.locator("#a-fork-chain").click()
                expect(page.locator("#fork-chain-menu .chain-open")).to_have_count(2)
                page.locator(f'#fork-chain-menu .chain-row[data-uid="{corpus.uid("codex-parent")}"] .chain-open').click()
                expect(page.locator("#msgs")).to_contain_text("Codex discarded parent tail")
                expect(page.locator("#msgs")).not_to_contain_text("Codex fork answer")
                # The parent's own menu lists its branch so the reader can go
                # back the way they came.
                expect(page.locator("#a-fork-chain")).to_have_attribute("title", "子会话")
                page.locator("#a-fork-chain").click()
                expect(page.locator("#fork-chain-menu .chain-open")).to_have_count(1)
                page.locator(f'#fork-chain-menu .chain-row[data-uid="{corpus.uid("codex-fork")}"] .chain-open').click()
                expect(page.locator("#msgs")).to_contain_text("Codex fork answer")
                expect(page.locator("#msgs")).not_to_contain_text("Codex discarded parent tail")
                page.locator("#a-fork-chain").click()
                expect(page.locator("#fork-chain-menu .chain-open")).to_have_count(2)
                page.locator(f'#fork-chain-menu .chain-row[data-uid="{corpus.uid("codex-parent")}"] .chain-open').click()
                expect(page.locator("#msgs")).to_contain_text("Codex discarded parent tail")
                agent("codex-agent", "Synthetic codex-agent answer")
                expect(page.locator("#msgs")).not_to_contain_text("Codex parent prefix OLD")
                agent("codex-nested-agent", "Synthetic codex-nested-agent answer")
                expect(page.locator("#msgs")).not_to_contain_text("Synthetic codex-agent answer")
                agent("", "Codex parent prefix OLD")

                select("codex-fork", "Codex fork answer")
                expect(page.locator("#a-view-switch")).to_have_count(0)
                page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                page.evaluate("window.__historyEvents = []")
                parent = corpus.paths["codex-parent"]
                original = parent.read_bytes()
                changed = original.replace(b"Codex parent prefix OLD", b"Codex parent prefix NEW")
                assert len(changed) == len(original) and changed != original
                parent.write_bytes(changed)
                expect(page.locator("#msgs")).to_contain_text("Codex parent prefix NEW", timeout=10000)
                expect(page.locator("#msgs")).not_to_contain_text("Codex parent prefix OLD")
                expect(page.locator("#msgs")).not_to_contain_text("Codex discarded parent tail")
                page.wait_for_function("window.__historyEvents.some(e => e.data.reset === true)")
                with corpus.paths["codex-fork"].open("ab") as stream:
                    stream.write(encoded(codex_message("assistant", "Codex browser leaf append", 3)))
                expect(page.locator("#msgs")).to_contain_text("Codex browser leaf append", timeout=10000)
                assert page.locator("#msgs").inner_text().count("Codex parent prefix NEW") == 1
                assert page.locator("#msgs").inner_text().count("Codex browser leaf append") == 1
                expect(page.locator("#migration-read-error")).to_have_count(0)

                # The cached inherited transcript must still be correct after
                # leaving the view and using the ordinary sidebar to return.
                select("claude-branch", "Claude selected answer")
                select("codex-fork", "Codex browser leaf append")
                expect(page.locator("#msgs")).to_contain_text("Codex parent prefix NEW")
                assert not errors, errors
                assert all(url.startswith(base + "/") for url in requests), "browser escaped isolated loopback origin"
                for suffix in ("/api/audit/browser", "/api/session/outbox", "/api/session/resolve-files"):
                    assert not any(suffix in url for url in requests), suffix
                print("PASS advanced legacy browser: Claude branch/compact/interrupted inputs, refreshed agent menus, parent-chain navigation, nested subagent views, ancestor-prefix SSE reset, cache re-entry")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
