#!/usr/bin/env python3
"""Pending receipt dismissal using real legacy functions in isolated Chromium.

Only the surrounding history renderer and server responses are fixtures. The
actual outbox renderer, click handler, POST helper and snapshot merger run in
the browser. Every HTTP request is intercepted at a private loopback origin;
no server, CLI or production session is accessed.
"""
import json
import os
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

REPO = Path(__file__).resolve().parents[1]
ORIGIN = "http://127.0.0.1:18799"
UID = "claude:fixture"


def function(source, name):
    for prefix in ("function ", "async function "):
        start = source.find("\n" + prefix + name + "(")
        if start >= 0:
            end = source.index("\n}\n", start)
            return source[start + 1:end + 2]
    raise AssertionError(f"Missing production function {name}")


SETUP = """
const S = {sel: 'claude:fixture', agent: null, queued: new Map(),
  outboxVersions: new Map(), retiredOutboxEpochs: new Set(), dismissedOutboxIds: new Map()};
const cache = new Map();
const viewKey = uid => uid;
const SessionDockCapabilities = {allows: name => name === 'outbox'};
const sessiondockCli = uid => ({source: uid.split(':')[0], name: 'Claude',
  queuedMessageLabel: item => item.state === 'ambiguous' ? '状态待核对' : item.state});
const store = {set: (key, value) => localStorage.setItem('sessiondock.' + key, JSON.stringify(value))};
const alerts = [];
const alert = message => alerts.push(message);
const browserAuditEvent = () => {};
const BUILD_ID = 'fixture-build', TERM_PAGE_ID = 'fixture-page';
const appUrl = path => new URL(path, location.href).href;
const markStaleBuild = () => {};
const $ = selector => document.querySelector(selector);
const el = (tag, cls = '', text = '') => {
  const node = document.createElement(tag); node.className = cls; node.textContent = text; return node;
};
const stampMessageTime = node => node;
const msgNode = message => el('div', 'msg user', message.text);
function renderConversationTail() { $('#msgs').replaceChildren(); renderQueuedMessages(); }
function reopen() { renderConversationTail(); }
function snapshot(items, revision = 10, uid = S.sel) {
  return syncServerOutbox(uid, items, {epoch: S.outboxVersions.get(uid)?.epoch || 'fixture-epoch', revision});
}
"""


def main():
    app = (REPO / "legacy-web/app.js").read_text()
    term = (REPO / "legacy-web/term.js").read_text()
    code = SETUP + "\n" + "\n".join(function(app, name) for name in (
        "saveQueuedMessages", "queuedMessages", "discardQueuedUserMessage",
        "validOutboxVersion", "staleServerOutbox", "acceptServerOutboxVersion",
        "syncServerOutbox", "discardServerQueuedMessage", "renderQueuedMessages",
    )) + "\n" + function(term, "post")
    with sync_playwright() as playwright:
        options = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**options)
        try:
            for width in (1280, 390):
                context = browser.new_context(viewport={"width": width, "height": 900})
                calls, errors = [], []
                response = {"ok": True, "outbox": [],
                            "outbox_version": {"epoch": "fixture-epoch", "revision": 11}}
                status = [200]

                def route_request(route):
                    path = urlsplit(route.request.url).path
                    assert route.request.url.startswith(ORIGIN + "/")
                    if path == "/":
                        route.fulfill(body='<!doctype html><div id="msgs"></div>', content_type="text/html")
                    elif path == "/api/session/outbox/discard":
                        calls.append(route.request.post_data_json)
                        if status[0] == 0:
                            route.abort("connectionfailed")
                        else:
                            route.fulfill(status=status[0], body=json.dumps(response), content_type="application/json")
                    else:
                        raise AssertionError(f"Unexpected request (must not send or interrupt): {path}")

                context.route("**/*", route_request)
                page = context.new_page()
                page.set_default_timeout(5000)
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(ORIGIN)
                page.add_script_tag(content=code)
                receipt = {"id": "stuck", "text": "这是同事hzh那边的最新代码", "state": "ambiguous",
                           "server": True, "created": 1, "attempts": 1}
                other = {**receipt, "id": "other", "text": "另一条仍待正文的回执", "created": 2}
                page.evaluate("items => snapshot(items)", [receipt, other])
                row = page.locator('[data-queued-id="stuck"]')
                expect(row.get_by_role("button", name="检查终端", exact=True)).to_be_visible()
                expect(row.get_by_role("button", name="移除", exact=True)).to_be_visible()
                row.get_by_role("button", name="移除", exact=True).click()
                expect(row).to_have_count(0)
                expect(page.locator('[data-queued-id="other"]')).to_have_count(1)
                assert calls[-1]["uid"] == UID and calls[-1]["id"] == "stuck"

                # A later send response, a stale in-flight snapshot, reopening
                # and another process epoch must not resurrect the dismissed ID.
                fresh = {**receipt, "id": "fresh", "text": "新发送的消息", "created": 3}
                for items, revision in (([receipt, other], 10), ([fresh], 12)):
                    page.evaluate("([items, revision]) => { snapshot(items, revision); reopen(); }", [items, revision])
                    expect(row).to_have_count(0)
                    expect(page.locator('[data-queued-id="other"]')).to_have_count(1)
                page.evaluate("item => { syncServerOutbox(S.sel, [item], {epoch: 'restarted', revision: 1}); reopen(); }", receipt)
                expect(row).to_have_count(0)

                # An already removed/confirmed server item still has a usable
                # local dismissal button; other server failures keep it visible.
                status[0] = 404
                response.clear()
                response.update({"error": "待核对消息不存在", "code": "delivery_missing"})
                page.evaluate("item => snapshot([item], 20)", {**receipt, "id": "missing"})
                missing = page.locator('[data-queued-id="missing"]')
                missing.get_by_role("button", name="移除", exact=True).click()
                expect(missing).to_have_count(0)
                status[0] = 503
                response.update({"error": "账本暂不可用", "code": "delivery_unavailable"})
                page.evaluate("item => snapshot([item], 21)", {**receipt, "id": "failure"})
                failed = page.locator('[data-queued-id="failure"]')
                failed.get_by_role("button", name="移除", exact=True).click()
                page.wait_for_function("alerts.length === 1")
                expect(failed).to_have_count(1)
                status[0] = 0
                failed.get_by_role("button", name="移除", exact=True).click()
                page.wait_for_function("alerts.length === 2")
                expect(failed).to_have_count(1)
                assert len(calls) == 4 and not errors, (calls, errors)

                # A real page reload starts from the current server snapshot,
                # rather than restoring server receipts from localStorage.
                assert page.evaluate("JSON.parse(localStorage.getItem('sessiondock.queuedMessages'))") == []
                page.reload()
                page.add_script_tag(content=code)
                page.evaluate("() => { snapshot([], 22); reopen(); }")
                expect(page.locator(".client-outbox")).to_have_count(0)
                context.close()
                print(f"PASS {width}px: dismissal button, exact receipt, reopen/new send/stale snapshots, missing/error, reload", flush=True)
        finally:
            browser.close()


if __name__ == "__main__":
    main()
