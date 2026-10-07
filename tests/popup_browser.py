#!/usr/bin/env python3
"""Popups are centered and share the dialog look.

No page uses the browser's native alert/confirm any more: a delete asks in a
centered `.app-popup` dialog (title from the first line, 取消/Esc keep the
session, the confirm button deletes), and floating notices such as the stale
build card sit in the centered `#float-stack` with the same panel look.
Later keeps the stale card's DOM identity while polling is paused. An HTTP
login redirect shows the matching login card; its button opens the local app
URL in a real new page with no opener.
Real clicks at desktop and 390px against an isolated server; synthetic corpus.
"""

import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, build_corpus, isolated_server
from trash_browser import click_delete, listed_uids, open_session


def centered(page, locator, width, height):
    box = locator.bounding_box()
    assert box, "not rendered"
    assert box["x"] >= 0 and box["x"] + box["width"] <= width + 0.5, (box, width)
    assert abs((box["x"] + box["width"] / 2) - width / 2) <= 2, (box, width)
    assert abs((box["y"] + box["height"] / 2) - height / 2) <= height * 0.1, (box, height)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-popup-") as temporary, sync_playwright() as pw:
        root = Path(temporary)
        corpus = build_corpus(root / "corpus")
        trash = corpus.root / "trash"
        trash.mkdir(mode=0o700)
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = pw.chromium.launch(**launch)
        with isolated_server(corpus, args.binary, trash_dir=trash) as (base, opener):
            for width, height, sid, text in ((1280, 900, "claude-abandoned", "Claude abandoned base answer"),
                                             (390, 844, "claude-compact", "Claude post compact answer")):
                narrow = width < 700
                context = browser.new_context(viewport={"width": width, "height": height}, service_workers="block")
                context.route("**/*", lambda route: route.continue_()
                              if route.request.url.startswith(base + "/") else route.abort())
                page = context.new_page()
                errors, native = [], []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.on("dialog", lambda dialog: (native.append(dialog.message), dialog.dismiss()))
                page.goto(base, wait_until="networkidle")
                uid = corpus.uid(sid)
                open_session(page, corpus, sid, text)
                popup = page.locator("dialog.app-popup")
                # 取消 keeps the session.
                click_delete(page, narrow)
                expect(popup).to_be_visible()
                expect(popup.locator("h2")).to_have_text(f"删除会话「{page.evaluate('uid => S.sessions.find(s => s.uid === uid).title', uid)}」?")
                expect(popup.locator(".app-popup-message")).to_contain_text("回收站")
                centered(page, popup, width, height)
                look = popup.evaluate("""d => { const s = getComputedStyle(d), t = getComputedStyle(document.querySelector('#trash-dialog'));
                    return [s.borderRadius === t.borderRadius, s.backgroundColor === t.backgroundColor, s.boxShadow === t.boxShadow]; }""")
                assert look == [True, True, True], look
                popup.get_by_role("button", name="取消").click()
                expect(popup).to_have_count(0)
                assert uid in listed_uids(opener, base)
                # Esc is a cancel too.
                click_delete(page, narrow)
                expect(popup).to_be_visible()
                page.keyboard.press("Escape")
                expect(popup).to_have_count(0)
                assert uid in listed_uids(opener, base)
                # The confirm button deletes.
                click_delete(page, narrow)
                popup.get_by_role("button", name="确定").click()
                expect(page.locator("#detail")).to_contain_text("已移入回收站")
                assert uid not in listed_uids(opener, base)
                # A stale build: the notice card is centered in the float stack, 稍后 folds it.
                page.route("**/api/meta", lambda route: route.fulfill(
                    status=200, content_type="application/json",
                    body='{"build":"popup-test-newer","hostname":"popup","capabilities":{}}'))
                page.evaluate("checkServerBuild()")
                card = page.locator("#float-stack .version-stale")
                expect(card).to_be_visible()
                expect(card.locator("strong")).to_have_text("SessionDock 已更新")
                centered(page, card, width, height)
                stale_card = card.element_handle()
                expect(page.locator("#float-stack > .app-float:visible")).to_have_count(1)
                assert card.evaluate("d => d.parentElement.id === 'float-stack'")
                reference = card.evaluate("""d => { const s = getComputedStyle(d);
                    return [s.width, s.borderRadius, s.backgroundColor, s.boxShadow,
                            s.padding, s.fontSize, s.flexDirection]; }""")
                card.get_by_role("button", name="稍后").click()
                expect(card).to_be_hidden()
                # Exercise the actual polling/visibility handlers. A paused build
                # check must not recreate the hidden card or silently unhide it.
                page.evaluate("checkServerBuild()")
                page.evaluate("document.dispatchEvent(new Event('visibilitychange'))")
                expect(card).to_be_hidden()
                expect(card).to_have_count(1)
                assert stale_card.evaluate("d => d.isConnected && d === document.querySelector('.version-stale')")
                stale_card.dispose()

                # Actionable receipts retain the update card layout; text-only floats are gone.
                reference = card.evaluate("""d => { const s = getComputedStyle(d);
                    return [s.width, s.borderRadius, s.backgroundColor, s.boxShadow,
                            s.padding, s.fontSize, s.flexDirection]; }""")
                for selector, trigger, clear in (
                    ('#bug-report-toast', "showBugReportToast('BUG-' + '1234567890'.repeat(8), {source:'codex', name:'fixture'})",
                     None),
                ):
                    page.evaluate(trigger)
                    notice = page.locator(selector)
                    expect(notice).to_be_visible()
                    centered(page, notice, width, height)
                    appearance = notice.evaluate("""d => { const s = getComputedStyle(d);
                        return [s.width, s.borderRadius, s.backgroundColor, s.boxShadow,
                                s.padding, s.fontSize, s.flexDirection]; }""")
                    assert appearance == reference, (selector, appearance, reference)
                    assert notice.evaluate('(d) => d.scrollWidth <= d.clientWidth')
                    if clear:
                        page.evaluate(clear)
                    else:
                        notice.get_by_role('button', name='忽略').click()
                    expect(notice).to_be_hidden()

                # Reload normally before the next network scenario: stale pages
                # deliberately stop HTTP polling, so no product state is reset here.
                page.unroute("**/api/meta")
                page.reload(wait_until="networkidle")
                page.wait_for_function("S.sessions.length > 0")
                page.route("**/__auth/login", lambda route: route.fulfill(
                    content_type="text/html", body="<p>Fixture login</p>"))
                page.route("**/api/meta", lambda route: route.fulfill(
                    status=302, headers={"Location": "/__auth/login"}))
                page.evaluate("checkServerBuild()")
                login = page.locator("#float-stack > .login-expired")
                expect(login).to_be_visible()
                expect(login.locator("strong")).to_have_text("登录已失效")
                expect(page.locator("#float-stack > .app-float:visible")).to_have_count(1)
                centered(page, login, width, height)
                appearance = login.evaluate("""d => { const s = getComputedStyle(d);
                    return [s.width, s.borderRadius, s.backgroundColor, s.boxShadow,
                            s.padding, s.fontSize, s.flexDirection]; }""")
                assert appearance == reference, (appearance, reference)
                assert login.evaluate("d => d.scrollWidth <= d.clientWidth")
                # The button opens the app's local base URL in a real new page,
                # rather than navigating this page or opening the redirect target.
                source_url = page.url
                with context.expect_page() as opened:
                    login.get_by_role("button", name="打开登录页").click()
                login_page = opened.value
                login_page.wait_for_load_state("domcontentloaded")
                expect(login_page).to_have_url(base + "/")
                assert page.url == source_url
                assert login_page.evaluate("window.opener === null")
                login_page.close()
                expect(login).to_be_visible()
                assert not native, native
                assert not errors, errors
                context.close()
        browser.close()
    print("PASS popup browser: delete confirm centered with the dialog look, 取消/Esc keep, 确定 deletes; "
          "stale-build card centered in the float stack, 稍后 keeps the same card hidden across polling/visibility; "
          "HTTP login redirect shows a matching card, its actual button opens the local app URL; "
          "no native dialogs; desktop + 390px")


if __name__ == "__main__":
    main()
