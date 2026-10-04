#!/usr/bin/env python3
"""Use the complete frontend under /sessiondock/ and restore after a private restart.

Initial history reads cover loading, a late response after a real session switch,
and a visible 503 retry on desktop and phone. The loopback proxy strips the same
prefix as the production proxy. Both Rust processes use one synthetic corpus and
private state; no deployed service, production sessions, real CLI, or production
proxy configuration is touched.
"""
from contextlib import contextmanager
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import argparse
import json
import re
import tempfile
import threading
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import parse_qs, quote, urlsplit

from playwright.sync_api import expect, sync_playwright
from browser_runtime import scoped_frontend
from frontend_framework_browser import launch_chromium, open_settings, select_tab
from history_parity import BINARY, build_corpus, codex_message, codex_row, isolated_server

PREFIX = "/sessiondock/"
MAIN_SID = "frontend-entry-main"
OTHER_SID = "frontend-entry-other"
RETRY_SID = "frontend-entry-retry"


def initial_read_observer(context, base, corpus):
    # Let one genuine HTTP response arrive after selection cancellation. This
    # exercises the stale-result guard independently of transport abort, without
    # replacing a UI operation or writing application/store state.
    path = urlsplit(base).path + "api/messages/" + corpus.uid(MAIN_SID)
    context.add_init_script("""(() => {
      const nativeFetch = window.fetch.bind(window);
      let protectedRead = false;
      window.__entryInitialReadConsumed = false;
      window.__entryManualRetryClicked = false;
      window.fetch = async (input, options) => {
        const url = new URL(input instanceof Request ? input.url : input, location.href);
        if (protectedRead || decodeURIComponent(url.pathname) !== """ + json.dumps(path) + """
            || url.searchParams.get('window') !== '1' || url.searchParams.has('start')) {
          return nativeFetch(input, options);
        }
        protectedRead = true;
        const response = await nativeFetch(input, {...options, signal: undefined});
        const getReader = response.body.getReader.bind(response.body);
        response.body.getReader = (...args) => {
          const reader = getReader(...args), read = reader.read.bind(reader);
          reader.read = async (...args) => {
            const result = await read(...args);
            if (result.done) window.__entryInitialReadConsumed = true;
            return result;
          };
          return reader;
        };
        return response;
      };
      // Observe the real user event so automatic retries cannot satisfy the
      // manual-retry assertion or make the fixture recover before the click.
      document.addEventListener('click', event => {
        const button = event.target.closest('#detail .empty button');
        if (button?.textContent.trim() === '重试读取') window.__entryManualRetryClicked = true;
      }, true);
    })();""")


def show_sidebar(page, width):
    if width == 390:
        page.locator(".mobile-back").click()


def verify_initial_reads(page, base, corpus, width):
    main_route = base + "api/messages/" + quote(corpus.uid(MAIN_SID), safe="") + "?window=1"
    retry_route = base + "api/messages/" + quote(corpus.uid(RETRY_SID), safe="") + "?window=1"
    held = []
    failures = []

    def hold_initial(route):
        query = parse_qs(urlsplit(route.request.url).query)
        assert query.get("window") == ["1"] and "start" not in query, route.request.url
        held.append(route)

    def fail_until_clicked(route):
        query = parse_qs(urlsplit(route.request.url).query)
        assert query.get("window") == ["1"] and "start" not in query, route.request.url
        if not page.evaluate("window.__entryManualRetryClicked"):
            failures.append(route.request.url)
            route.fulfill(status=503, json={"error": "会话在读取期间变化，请重试（初次打开夹具）"})
        else:
            route.continue_()

    page.route(main_route, hold_initial)
    try:
        with page.expect_request(main_route):
            page.locator(f'#side .item[data-uid="{corpus.uid(MAIN_SID)}"]').click()
        expect(page.locator("#detail .spin")).to_be_visible()
        expect(page.locator("#detail .spin")).to_have_text("正在读取会话…")
        assert len(held) == 1, held
        show_sidebar(page, width)
        page.locator(f'#side .item[data-uid="{corpus.uid(OTHER_SID)}"]').click()
        expect(page.locator("#msgs")).to_contain_text(corpus.expected[OTHER_SID][-1])
        heading = page.locator("#detail .dhead h2")
        expect(heading).to_contain_text(corpus.expected[OTHER_SID][0])
        heading_before = heading.inner_text()
        history_before = page.locator("#msgs").inner_text()
        url_before = page.url
        old_read = held.pop()
        old_read.fulfill(response=old_read.fetch())
        page.wait_for_function("window.__entryInitialReadConsumed === true")
        page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
        expect(heading).to_have_text(heading_before, use_inner_text=True)
        expect(page.locator("#msgs")).to_have_text(history_before, use_inner_text=True)
        expect(page.locator("#msgs")).not_to_contain_text(corpus.expected[MAIN_SID][-1])
        expect(page.locator("#detail .spin, #detail .empty")).to_have_count(0)
        assert page.url == url_before, page.url
    finally:
        page.unroute(main_route, hold_initial)

    page.route(retry_route, fail_until_clicked)
    try:
        show_sidebar(page, width)
        page.locator(f'#side .item[data-uid="{corpus.uid(RETRY_SID)}"]').click()
        failure = page.locator("#detail .empty")
        expect(failure).to_be_visible()
        expect(failure).to_contain_text("读取失败")
        expect(failure).to_contain_text("会话在读取期间变化，请重试（初次打开夹具）")
        retry = failure.get_by_role("button", name="重试读取", exact=True)
        expect(retry).to_be_visible()
        expect(retry).to_be_enabled()
        expect(page.locator("#migration-read-error")).to_have_count(0)
        assert failures, "initial history request did not receive the fixture 503"
        with page.expect_response(lambda response: response.ok
                                  and response.url == retry_route):
            retry.click()
        assert page.evaluate("window.__entryManualRetryClicked") is True
        expect(page.locator("#msgs")).to_contain_text(corpus.expected[RETRY_SID][-1])
        expect(page.locator("#detail .dhead h2")).to_contain_text(corpus.expected[RETRY_SID][0])
        expect(page.locator("#detail .spin, #detail .empty, #migration-read-error")).to_have_count(0)
        assert parse_qs(urlsplit(page.url).query)["sid"] == ["codex:" + RETRY_SID], page.url
    finally:
        page.unroute(retry_route, fail_until_clicked)
    show_sidebar(page, width)
    print(f"PASS initial read {width}px: loading, stale response isolation, visible 503 retry", flush=True)


@contextmanager
def prefixed_proxy():
    target = SimpleNamespace(url=None)

    class Proxy(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def forward(self):
            if not self.path.startswith(PREFIX):
                self.send_error(404)
                return
            backend = urlsplit(target.url)
            connection = HTTPConnection(backend.hostname, backend.port, timeout=15)
            try:
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                headers = {key: value for key, value in self.headers.items()
                           if key.lower() != "connection"}
                connection.request(self.command, "/" + self.path[len(PREFIX):], body, headers)
                response = connection.getresponse()
                payload = response.read()
                self.send_response(response.status)
                for key, value in response.getheaders():
                    if key.lower() not in {"connection", "transfer-encoding", "content-length"}:
                        self.send_header(key, value)
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
            except OSError:
                # A poll may land between the two private processes.
                self.send_error(502)
            finally:
                connection.close()

        do_GET = do_POST = do_HEAD = forward

    proxy = ThreadingHTTPServer(("127.0.0.1", 0), Proxy)
    thread = threading.Thread(target=proxy.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{proxy.server_port}{PREFIX}", target
    finally:
        proxy.shutdown()
        proxy.server_close()
        thread.join(timeout=5)


def choose_preferences(page):
    open_settings(page)
    select_tab(page, "appearance")
    page.locator("#setting-theme").select_option("dark")
    page.locator("#setting-font").select_option("cascadia")
    select_tab(page, "features")
    page.locator("#setting-cache").select_option("128")
    page.keyboard.press("Escape")
    expect(page.locator("#settings-dialog")).to_be_hidden()


def verify_restored(page, base, corpus, width):
    page.reload(wait_until="domcontentloaded")
    expect(page.locator("#msgs")).to_contain_text(corpus.expected[MAIN_SID][-1])
    expect(page.locator("html")).to_have_attribute("data-theme", "dark")
    assert urlsplit(page.url).path == PREFIX, page.url
    assert parse_qs(urlsplit(page.url).query)["sid"] == ["codex:" + MAIN_SID], page.url
    if width == 390:
        expect(page.locator("body")).to_have_class(re.compile(r".*\bmobile-detail\b.*"))
        page.locator(".mobile-back").click()
    open_settings(page)
    select_tab(page, "appearance")
    expect(page.locator("#setting-theme")).to_have_value("dark")
    expect(page.locator("#setting-font")).to_have_value("cascadia")
    select_tab(page, "features")
    expect(page.locator("#setting-cache")).to_have_value("128")
    page.keyboard.press("Escape")
    parent = page.locator(f'#side .item[data-uid="{corpus.uid(MAIN_SID)}"]')
    expect(parent).to_be_visible()
    page.locator(f'#side .item[data-uid="{corpus.uid(OTHER_SID)}"]').click()
    expect(page.locator("#msgs")).to_contain_text(corpus.expected[OTHER_SID][-1])
    page.go_back(wait_until="domcontentloaded")
    expect(page.locator("#msgs")).to_contain_text(corpus.expected[MAIN_SID][-1])
    assert urlsplit(page.url).path == PREFIX
    assert page.url.startswith(base)


def verify_root_disposal(browser, base, corpus, errors):
    if not scoped_frontend():
        return
    context = browser.new_context(service_workers='block')
    context.add_init_script('window.EventSource = undefined')
    page = context.new_page()
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.clock.install()
    try:
        page.goto(base, wait_until='networkidle')
        item = page.locator(f'#side .item[data-uid="{corpus.uid(MAIN_SID)}"]')
        expect(item).to_be_visible()
        page.evaluate("""() => {
          const r=window.SessionDockRuntime, fetch=r.core.network.fetch;
          window.__rootLifetime={fetch, held:[], urls:[], catalog:r.core.state.catalog.sessions};
          r.core.network.fetch=async (url, options) => {
            const path=new URL(url, location.href).pathname;
            if (!/api\/(sessions|messages\/)/.test(path)) return fetch(url, options);
            __rootLifetime.urls.push(path);
            const response=await fetch(url, {...options, signal:undefined});
            return new Promise(done=>__rootLifetime.held.push(()=>done(response)));
          };
          void r.core.list.loadSessions(true);
        }""")
        item.click()
        expect(page.locator('#detail .spin')).to_contain_text('正在读取会话')
        page.wait_for_function('__rootLifetime.held.length >= 2')
        page.evaluate("""() => {
          const r=window.SessionDockRuntime;
          __rootLifetime.count=__rootLifetime.urls.length;
          document.querySelector('#app').__vue_app__.unmount();
          __rootLifetime.held.splice(0).forEach(release=>release());
        }""")
        page.clock.fast_forward(30000)
        page.evaluate("""async () => {
          await new Promise(done=>setTimeout(done,0));
          dispatchEvent(new Event('online'));
          dispatchEvent(new Event('pageshow'));
          document.dispatchEvent(new Event('visibilitychange'));
        }""")
        expect(page.locator('#msgs, #side, .dhead')).to_have_count(0)
        assert page.evaluate("""() => {
          const r=window.SessionDockRuntime;
          return __rootLifetime.urls.length===__rootLifetime.count
            && r.core.state.catalog.sessions===__rootLifetime.catalog
            && !r.core.sync.watching && !r.core.events.connection;
        }""")
        page.evaluate('window.SessionDockRuntime.core.network.fetch=__rootLifetime.fetch')
        print('PASS Vue root disposal aborts initial/list reads and prevents late render or restart', flush=True)
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-frontend-entry-") as temporary:
        root = Path(temporary)
        corpus = build_corpus(root)
        for sid in (MAIN_SID, OTHER_SID, RETRY_SID):
            corpus.put(sid, "codex", [
                codex_row("session_meta", {"id": sid, "cwd": "/synthetic/frontend-entry", "source": "cli"}),
                codex_message("user", sid + " question"),
                codex_message("assistant", sid + " answer"),
            ], [sid + " question", sid + " answer"])
        before = {path: path.read_bytes() for path in corpus.paths.values()}
        state = root / "state"
        state.mkdir()
        with prefixed_proxy() as (base, target), sync_playwright() as playwright:
            browser = launch_chromium(playwright)
            pages = []
            errors = []
            requests = []
            try:
                with isolated_server(corpus, args.binary, state_dir=state) as (node, _):
                    target.url = node
                    for width in (1280, 390):
                        context = browser.new_context(viewport={"width": width, "height": 900},
                                                      service_workers="block")
                        context.add_init_script("window.EventSource = undefined;")
                        initial_read_observer(context, base, corpus)
                        context.on("request", lambda request: requests.append(request.url))
                        page = context.new_page()
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        pages.append((context, page, width))
                        page.goto(base, wait_until="domcontentloaded")
                        expect(page.locator("#side .item[data-uid]").first).to_be_visible()
                        verify_initial_reads(page, base, corpus, width)
                        choose_preferences(page)
                        page.locator(f'#side .item[data-uid="{corpus.uid(MAIN_SID)}"]').click()
                        expect(page.locator("#msgs")).to_contain_text(corpus.expected[MAIN_SID][-1])
                with isolated_server(corpus, args.binary, state_dir=state) as (node, _):
                    target.url = node
                    for _, page, width in pages:
                        verify_restored(page, base, corpus, width)
                        print(f"PASS prefix/restart {width}px: session, history navigation, theme/font/cache", flush=True)
                    verify_root_disposal(browser, base, corpus, errors)
                assert not errors, errors
                assert requests and all(url.startswith(base) for url in requests), requests
            finally:
                for context, _, _ in pages:
                    context.close()
                browser.close()
        assert all(path.read_bytes() == value for path, value in before.items())
    print("PASS frontend_entry_browser: initial loading/stale/retry, /sessiondock/ assets/API and private Rust restart", flush=True)


if __name__ == "__main__":
    main()
