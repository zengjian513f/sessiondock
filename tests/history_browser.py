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

from history_parity import batch35_meta, batch35_agent_meta, BINARY, build_corpus, claude_row, codex_message, encoded, get_json, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-history-browser-") as temporary:
        corpus = build_corpus(Path(temporary))
        # BUG-20260922-092750-387c1c: Claude 2.1.278 repeats the paste id
        # on both tags. Bodies are literal, including protocol-looking text.
        paste = lambda ident, text: f'<pasted_content id="{ident}">\n{text}\n</pasted_content id="{ident}">'
        paste_cases = [
            ("\n\n" + paste("a202", "Pasted needle 正文") + "\n", "Pasted needle 正文"),
            ("before " + paste("b", "  indented\n    body  ") + " after " + paste("c", "second"),
             "before   indented\n    body   after second"),
            (paste("d", "<system-reminder>literal user paste</system-reminder>"),
             "<system-reminder>literal user paste</system-reminder>"),
            ('<pasted_content id="x">unclosed', '<pasted_content id="x">unclosed'),
            ('<pasted_content id="x">mismatch</pasted_content id="y">',
             '<pasted_content id="x">mismatch</pasted_content id="y">'),
            ('discuss <unknown>literal</unknown>', 'discuss <unknown>literal</unknown>'),
        ]
        paste_rows = []
        parent = None
        for index, (raw, _) in enumerate(paste_cases):
            uid = f"paste-{index}"
            content = raw if index == 0 else [{"type": "text", "text": raw}]
            paste_rows.append(claude_row("claude-pasted", "user", uid, parent, content))
            parent = uid
        assistant_paste = paste("assistant", "assistant literal envelope")
        paste_rows.append(claude_row("claude-pasted", "assistant", "paste-answer", parent, assistant_paste))
        corpus.put("claude-pasted", "claude", paste_rows, [])
        # Codex rotates a physical rollout but keeps the same native thread id.
        old_meta = batch35_meta("codex-rotation", "2026-09-11T08:00:00Z")
        old_meta["ordinal"] = 10
        old_meta["payload"]["synthetic_padding"] = "x" * 8000
        old_rows = [old_meta, codex_message("user", "Rotation inherited question", 11),
                    codex_message("assistant", "Rotation inherited answer", 12)]
        old = corpus.put("rotation-old", "codex", old_rows, [])
        cut = old.stat().st_size
        with old.open("ab") as stream:
            stream.write(encoded(codex_message("assistant", "Rotation excluded old tail", 13)))
        new_meta = batch35_meta("codex-rotation", "2026-09-11T09:00:00Z",
                               history_base={"thread_id": "codex-rotation", "end_byte_offset": cut,
                                             "end_ordinal_exclusive": 13})
        new_meta["ordinal"] = 13
        corpus.put("rotation-new", "codex", [new_meta, codex_message("user", "Rotation continued question", 14)], [])
        corpus.put("rotation-agent", "codex", [batch35_agent_meta("rotation-agent", "codex-rotation"),
                   codex_message("assistant", "Rotation attached agent answer", 15)], [])
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

                select("rotation-new", "Rotation continued question")
                expect(page.locator("#msgs")).to_contain_text("Rotation inherited question")
                expect(page.locator("#msgs")).to_contain_text("Rotation inherited answer")
                expect(page.locator("#msgs")).not_to_contain_text("Rotation excluded old tail")
                rows = get_json(opener, base, "/api/sessions")["sessions"]
                rotated = next(row for row in rows if row["uid"] == corpus.uid("rotation-new"))
                assert rotated["supported"] and {a["id"] for a in rotated["agent_items"]} == {"rotation-agent"}
                agent("rotation-agent", "Rotation attached agent answer")
                select("rotation-old", "Rotation excluded old tail")
                select("rotation-new", "Rotation continued question")
                with corpus.paths["rotation-new"].open("ab") as stream:
                    stream.write(encoded(codex_message("assistant", "Rotation live appended answer", 15)))
                expect(page.locator("#msgs")).to_contain_text("Rotation live appended answer", timeout=15000)
                titles = get_json(opener, base, "/api/sessions/titles?ids=codex:codex-rotation")
                assert not titles.get("missing"), titles
                # An unrelated duplicate is still a real conflict; removing it
                # restores the explicit continuation without touching transcripts.
                duplicate = corpus.put("rotation-conflict", "codex", [
                    batch35_meta("codex-rotation", "2026-09-11T07:00:00Z"),
                    codex_message("user", "Unrelated duplicate")], [])
                page.reload(wait_until="networkidle")
                page.locator(f'#side .item[data-uid="{corpus.uid("rotation-new")}"]').click()
                expect(page.locator("#migration-read-error")).to_contain_text("父线程 ID 在已配置索引中存在歧义", timeout=15000)
                duplicate.unlink()
                page.reload(wait_until="networkidle")
                select("rotation-new", "Rotation live appended answer")
                next_meta = batch35_meta("codex-rotation", "2026-09-11T10:00:00Z",
                    history_base={"thread_id": "codex-rotation", "end_ordinal_exclusive": 16,
                                  "end_byte_offset": corpus.paths["rotation-new"].stat().st_size})
                next_meta["ordinal"] = 16
                corpus.put("rotation-next", "codex", [next_meta,
                    codex_message("user", "Rotation second continuation", 17)], [])
                page.reload(wait_until="networkidle")
                select("rotation-next", "Rotation second continuation")
                expect(page.locator("#msgs")).to_contain_text("Rotation inherited answer")
                expect(page.locator("#msgs")).to_contain_text("Rotation live appended answer")
                expect(page.locator("#msgs")).not_to_contain_text("Rotation excluded old tail")
                print("PASS Codex same-ID rollout rotation: inherited prefix, excluded old tail, agent ownership, live append, titles, duplicate conflict and recovery")
                select("claude-pasted", "Pasted needle 正文")
                # Assert the wire result too: a renderer-only fix is insufficient.
                wire = get_json(opener, base, "/api/messages/" + corpus.uid("claude-pasted"))
                users = [m["text"].strip() for m in wire["messages"] if m.get("role") == "user"]
                assert users == [expected for _, expected in paste_cases], users
                assert wire["meta"]["title"] == "Pasted needle 正文"
                expect(page.locator("#msgs")).not_to_contain_text('<pasted_content id="a202">')
                expect(page.locator("#msgs")).to_contain_text("literal user paste")
                expect(page.locator("#msgs")).to_contain_text(assistant_paste)
                page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                with corpus.paths["claude-pasted"].open("ab") as stream:
                    stream.write(encoded(claude_row("claude-pasted", "user", "paste-live", "paste-answer", paste("live", "Live pasted 正文"))))
                expect(page.locator("#msgs")).to_contain_text("Live pasted 正文", timeout=10000)
                expect(page.locator("#msgs")).not_to_contain_text('<pasted_content id="live">')
                matches = get_json(opener, base, "/api/search?q=Pasted%20needle")
                assert any("Pasted needle" in hit.get("snippet", "") for hit in matches["results"]), matches
                assert not get_json(opener, base, "/api/search?q=a202")["results"]
                listed = get_json(opener, base, "/api/sessions")["sessions"]
                assert next(row for row in listed if row["uid"] == corpus.uid("claude-pasted"))["title"] == "Pasted needle 正文"
                print("PASS Claude paste envelopes: wire, list title, search, browser, live append, literal bodies and malformed tags")

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
