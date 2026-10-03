#!/usr/bin/env python3
"""Use the complete frontend under /sessiondock/ and restore after a private restart.

The loopback proxy strips the same prefix as the production proxy. Both Rust
processes use one synthetic corpus and private state; no deployed service,
production sessions, real CLI, or production proxy configuration is touched.
"""
from contextlib import contextmanager
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import argparse
import re
import tempfile
import threading
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright
from frontend_framework_browser import launch_chromium, open_settings, select_tab
from history_parity import BINARY, build_corpus, codex_message, codex_row, isolated_server

PREFIX = "/sessiondock/"
MAIN_SID = "frontend-entry-main"
OTHER_SID = "frontend-entry-other"


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-frontend-entry-") as temporary:
        root = Path(temporary)
        corpus = build_corpus(root)
        for sid in (MAIN_SID, OTHER_SID):
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
                        context.on("request", lambda request: requests.append(request.url))
                        page = context.new_page()
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        pages.append((context, page, width))
                        page.goto(base, wait_until="domcontentloaded")
                        expect(page.locator("#side .item[data-uid]").first).to_be_visible()
                        choose_preferences(page)
                        page.locator(f'#side .item[data-uid="{corpus.uid(MAIN_SID)}"]').click()
                        expect(page.locator("#msgs")).to_contain_text(corpus.expected[MAIN_SID][-1])
                with isolated_server(corpus, args.binary, state_dir=state) as (node, _):
                    target.url = node
                    for _, page, width in pages:
                        verify_restored(page, base, corpus, width)
                        print(f"PASS prefix/restart {width}px: session, history navigation, theme/font/cache", flush=True)
                assert not errors, errors
                assert requests and all(url.startswith(base) for url in requests), requests
            finally:
                for context, _, _ in pages:
                    context.close()
                browser.close()
        assert all(path.read_bytes() == value for path, value in before.items())
    print("PASS frontend_entry_browser: /sessiondock/ assets/API and private Rust restart", flush=True)


if __name__ == "__main__":
    main()
