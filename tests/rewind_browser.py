#!/usr/bin/env python3
"""Persisted Claude timeline pins through the real legacy UI (desktop + 390px).

Synthetic records only: no CLI, no native rewind. The page pins the display
to before a chosen input, shows the explicit "CLI 未回滚" explanation, watches
the pin retire when native records move past it, and keeps state across a
reload and a Web restart.
"""
import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, claude_row, encoded, get_json, isolated_server

SID = "claude-pinned"


def build(root: Path) -> Corpus:
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    rows = [claude_row(SID, "user", "u1", None, "Pin question one"),
            claude_row(SID, "assistant", "a1", "u1", "Pin answer one"),
            claude_row(SID, "user", "u2", "a1", "Pin question two"),
            claude_row(SID, "assistant", "a2", "u2", "Pin answer two"),
            claude_row(SID, "user", "u3", "a2", "Pin question three"),
            claude_row(SID, "assistant", "a3", "u3", "Pin answer three")]
    corpus.put(SID, "claude", rows, [row["message"]["content"] for row in rows])
    return corpus


def append(path: Path, rows):
    with path.open("ab") as handle:
        handle.write(b"".join(encoded(row) for row in rows))


def open_session(page, base, uid):
    page.goto(base, wait_until="networkidle")
    expect(page.locator("#backend-notice")).to_be_hidden()  # no standing banner
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator("#msgs")).to_contain_text("Pin answer one")
    page.wait_for_function("_es && _es.readyState === EventSource.OPEN")


def pin_button(page, text):
    return page.locator('#msgs .msg[data-role="user"]', has_text=text).locator(".timeline-pin-action")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-rewind-browser-") as temporary:
        corpus = build(Path(temporary))
        path = corpus.paths[SID]
        uid = corpus.uid(SID)
        state = corpus.root / "state"
        state.mkdir(mode=0o700)
        with sync_playwright() as playwright:
            launch = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**launch)
            try:
                with isolated_server(corpus, args.binary, state_dir=state) as (base, opener):
                    assert get_json(opener, base, "/api/meta")["capabilities"]["timeline_pin"] is True
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                    errors = []
                    page = context.new_page()
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    open_session(page, base, uid)
                    expect(page.locator("#timeline-pin-notice")).to_have_count(0)
                    expect(page.locator("#msgs .timeline-pin-action")).to_have_count(3)
                    expect(pin_button(page, "Pin question three")).to_have_attribute("data-pin-target", "u3")

                    # Desktop: pin to before the third input.
                    pin_button(page, "Pin question three").click()
                    expect(page.locator("#msgs")).not_to_contain_text("Pin question three")
                    expect(page.locator("#msgs")).to_contain_text("Pin answer two")
                    notice = page.locator("#timeline-pin-notice")
                    expect(notice).to_contain_text("CLI 未回滚")
                    expect(notice).to_have_attribute("data-retired", "false")
                    expect(page.locator("#timeline-pin-clear")).to_have_text("取消固定")
                    row = next(r for r in get_json(opener, base, "/api/sessions")["sessions"] if r["uid"] == uid)
                    assert row["timeline_pin"]["target"] == "u3" and row["timeline_pin"]["tip"] == "a2"
                    assert row["timeline_pin"]["native_rewind"] is False
                    native_pinned = path.read_bytes()

                    # Reload keeps the pinned view and the explanation.
                    page.reload(wait_until="networkidle")
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Pin answer two")
                    expect(page.locator("#msgs")).not_to_contain_text("Pin answer three")
                    expect(page.locator("#timeline-pin-notice")).to_contain_text("CLI 未回滚")
                    page.wait_for_function("_es && _es.readyState === EventSource.OPEN")

                    # The CLI never rewound: native records continue after a3 and
                    # the pin retires with a visible reason over SSE.
                    append(path, [claude_row(SID, "user", "u4", "a3", "Pin question four"),
                                  claude_row(SID, "assistant", "a4", "u4", "Pin answer four")])
                    expect(page.locator("#msgs")).to_contain_text("Pin answer four", timeout=15000)
                    expect(page.locator("#msgs")).to_contain_text("Pin answer three")
                    retired = page.locator("#timeline-pin-notice")
                    expect(retired).to_have_attribute("data-retired", "true")
                    expect(retired).to_have_attribute("data-retired-reason", "native_advanced")
                    expect(retired).to_contain_text("固定显示已失效")
                    expect(retired).to_contain_text("CLI 未回滚")
                    expect(page.locator("#timeline-pin-clear")).to_have_text("清除记录")
                    assert path.read_bytes().startswith(native_pinned)

                    # Reload keeps the retired explanation; clearing removes it.
                    page.reload(wait_until="networkidle")
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator("#timeline-pin-notice")).to_have_attribute("data-retired", "true")
                    page.wait_for_function("_es && _es.readyState === EventSource.OPEN")
                    page.locator("#timeline-pin-clear").click()
                    expect(page.locator("#timeline-pin-notice")).to_have_count(0)
                    expect(page.locator("#msgs")).to_contain_text("Pin answer four")
                    row = next(r for r in get_json(opener, base, "/api/sessions")["sessions"] if r["uid"] == uid)
                    assert "timeline_pin" not in row

                    # 390px: the action stays visible/clickable and the notice wraps.
                    page.set_viewport_size({"width": 390, "height": 844})
                    if not page.locator("#msgs").is_visible():
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Pin answer four")
                    button = pin_button(page, "Pin question two")
                    button.scroll_into_view_if_needed()
                    expect(button).to_be_visible()
                    button.click()
                    expect(page.locator("#msgs")).not_to_contain_text("Pin question two")
                    expect(page.locator("#msgs")).to_contain_text("Pin answer one")
                    mobile_notice = page.locator("#timeline-pin-notice")
                    expect(mobile_notice).to_be_visible()
                    expect(mobile_notice).to_contain_text("CLI 未回滚")
                    box = mobile_notice.bounding_box()
                    assert box and box["x"] >= 0 and box["x"] + box["width"] <= 390, box
                    expect(page.locator("#timeline-pin-clear")).to_be_visible()
                    assert not errors, errors
                    context.close()  # Release SSE before the writer shuts down.
                # Web restart: the pin persists in the metadata store.
                with isolated_server(corpus, args.binary, state_dir=state) as (base, opener):
                    row = next(r for r in get_json(opener, base, "/api/sessions")["sessions"] if r["uid"] == uid)
                    assert row["timeline_pin"]["target"] == "u2" and row["timeline_pin"]["retired"] is False
                    context = browser.new_context(viewport={"width": 390, "height": 844}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                    page = context.new_page()
                    page.goto(base, wait_until="networkidle")
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Pin answer one")
                    expect(page.locator("#msgs")).not_to_contain_text("Pin answer two")
                    expect(page.locator("#timeline-pin-notice")).to_contain_text("CLI 未回滚")
                    page.locator("#timeline-pin-clear").click()
                    expect(page.locator("#timeline-pin-notice")).to_have_count(0)
                    expect(page.locator("#msgs")).to_contain_text("Pin answer four")
                    context.close()
                print("PASS rewind browser: desktop pin/trimmed history/explanation, SSE retirement reason, reload, 390px pin/unpin, Web restart persistence, native file only appended")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
