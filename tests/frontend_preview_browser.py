#!/usr/bin/env python3
"""Preview assets and API share one Hub behind private authenticated Nginx.

A legacy main entry and Vue preview reproduce the endless update notice when
preview API routing is wrong, then exercise both authenticated entries with
desktop/phone clicks, filtering and reloads.
Only synthetic nodes, temporary directories and a fixture login cookie are used.
"""
from contextlib import contextmanager
from http.server import ThreadingHTTPServer
from pathlib import Path
import argparse
import shutil
import subprocess
import tempfile
import threading
import time
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, build_opener
from urllib.parse import unquote, urlsplit

from playwright.sync_api import Error as PlaywrightError, expect, sync_playwright
from frontend_framework_browser import launch_chromium
from frontend_paths import entry_asset, frontend_html
from hub_http_suite import REPO, FakeNode, Hub, free_port, scoped
from nginx_upload_auth import Backend, NGINX


@contextmanager
def proxy(root, original, preview):
    auth = ThreadingHTTPServer(('127.0.0.1', 0), Backend)
    auth.auth_paths, auth.auth_bodies = [], []
    thread = threading.Thread(target=auth.serve_forever, daemon=True)
    thread.start()
    port = free_port()
    snippets = root / 'snippets'
    snippets.mkdir()
    (snippets / 'auth.conf').write_text('auth_request /__auth/check;\n')
    template = (REPO / 'deploy/sessiondock-preview.nginx.conf').read_text()
    original_route = template.replace('<PREVIEW_PATH>', '/sessiondock').replace('<HUB_PORT>', str(original.port))
    preview_route = template.replace('<PREVIEW_PATH>', '/sessiondock2').replace('<HUB_PORT>', str(preview.port))
    prefix = f'''pid {root}/nginx.pid;
error_log {root}/error.log;
events {{ worker_connections 128; }}
http {{
    access_log off;
    client_body_temp_path {root}/body;
    proxy_temp_path {root}/proxy;
    server {{
        listen 127.0.0.1:{port};
        location = /__auth/check {{
            internal;
            client_max_body_size 0;
            proxy_pass http://127.0.0.1:{auth.server_port}/__auth/check;
            proxy_pass_request_body off;
            proxy_set_header Content-Length '';
            proxy_set_header Cookie $http_cookie;
        }}
'''
    config = root / 'nginx.conf'

    def configure(split):
        extra = '' if not split else f'''location ^~ /sessiondock2/api/ {{
            include snippets/auth.conf;
            proxy_pass http://127.0.0.1:{original.port}/api/;
        }}'''
        config.write_text(prefix + original_route + preview_route + extra + '\n}}\n')

    configure(True)
    process = subprocess.Popen([NGINX, '-p', str(root) + '/', '-c', str(config), '-g', 'daemon off;'],
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        opener = build_opener(ProxyHandler({}))
        deadline = time.monotonic() + 5
        while True:
            if process.poll() is not None:
                raise AssertionError(process.stderr.read().decode())
            try:
                opener.open(f'http://127.0.0.1:{port}/__auth/check', timeout=.2)
            except HTTPError:
                break
            except (URLError, TimeoutError):
                if time.monotonic() >= deadline:
                    raise AssertionError('private nginx did not start')
                time.sleep(.02)

        def unify():
            configure(False)
            subprocess.run([NGINX, '-p', str(root) + '/', '-c', str(config), '-s', 'reload'],
                           check=True, timeout=5, capture_output=True)

        yield f'http://127.0.0.1:{port}', unify
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
        process.stderr.close()
        auth.shutdown()
        auth.server_close()
        thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=REPO / 'target/release/sessiondock')
    parser.add_argument('--legacy-web-dir', type=Path, default=REPO / 'legacy-web')
    parser.add_argument('--vue-web-dir', type=Path, default=REPO / 'web/dist-migration')
    args = parser.parse_args()
    legacy_web, vue_web = args.legacy_web_dir.resolve(), args.vue_web_dir.resolve()
    legacy_entry = entry_asset(legacy_web).relative_to(legacy_web).as_posix()
    vue_entry = entry_asset(vue_web).relative_to(vue_web).as_posix()
    assert not frontend_html(legacy_web).modules, 'main entry must load classic legacy scripts'
    assert frontend_html(vue_web).modules, 'preview must load the built Vue module'
    assert Path(NGINX).is_file(), 'Nginx required'
    node = FakeNode('a' * 32, 'NodeA')
    try:
        with tempfile.TemporaryDirectory(prefix='sessiondock-preview-browser-') as temporary:
            root = Path(temporary)
            original_web = root / 'original-web'
            shutil.copytree(legacy_web, original_web)
            with (original_web / 'index.html').open('a') as stream:
                stream.write('\n<!-- synthetic original build -->\n')
            hubs = []
            try:
                for name in ('original', 'preview'):
                    directory = root / name
                    directory.mkdir()
                    hub = Hub(args.binary.resolve().parent / 'sessiondock-hub', directory, [node])
                    hub.env['SESSIONDOCK_WEB_DIR'] = str(original_web if name == 'original' else vue_web)
                    hub.start()
                    hubs.append(hub)
                original, preview = hubs
                original_build = original.json('GET', '/api/meta')[1]['build']
                preview_build = preview.json('GET', '/api/meta')[1]['build']
                assert original_build != preview_build
                with proxy(root, original, preview) as (base, unify), sync_playwright() as playwright:
                    browser = launch_chromium(playwright)
                    context = browser.new_context(service_workers='block')
                    context.add_cookies([{'name': 'fixture', 'value': 'valid', 'url': base}])
                    page = context.new_page()
                    page.goto(base + '/sessiondock2/', wait_until='domcontentloaded')
                    expect(page.locator('.version-stale')).to_contain_text('SessionDock 已更新')
                    print('PASS reproduce: split API build pauses the preview', flush=True)
                    unify()
                    # Wait on the new worker's actual API response, not a fixed delay.
                    deadline = time.monotonic() + 5
                    while True:
                        try:
                            if context.request.get(base + '/sessiondock2/api/meta', timeout=1000).json()['build'] == preview_build:
                                break
                        except PlaywrightError:
                            # The retiring Nginx worker can close a reused socket.
                            pass
                        assert time.monotonic() < deadline, 'nginx route did not reload'
                    context.close()
                    entries = (
                        ('legacy', '/sessiondock/', original_build, legacy_entry),
                        ('vue', '/sessiondock2/', preview_build, vue_entry),
                    )
                    for kind, prefix, build, entry in entries:
                        for width in (1280, 390):
                            context = browser.new_context(viewport={'width': width, 'height': 900}, service_workers='block')
                            context.add_cookies([{'name': 'fixture', 'value': 'valid', 'url': base}])
                            page = context.new_page()
                            errors = []
                            page.on('pageerror', lambda error: errors.append(str(error)))
                            if width == 1280:
                                # Keep the reported nested sidebar preference across
                                # reloads; populated counters must still have rows.
                                page.goto(base + prefix, wait_until='domcontentloaded')
                                expect(page.locator('#side .item')).to_have_count(1)
                                page.locator('#nest-toggle').click()
                                expect(page.locator('#nest-toggle')).to_have_attribute('aria-pressed', 'true')
                            for _ in range(3):
                                page.goto(base + prefix, wait_until='domcontentloaded')
                                expect(page.locator('#side .item')).to_have_count(1)
                                if width == 1280:
                                    expect(page.locator('#nest-toggle')).to_have_attribute('aria-pressed', 'true')
                                scripts = page.locator('script[src]').evaluate_all(
                                    "nodes => nodes.map(node => ({src: node.src, type: node.type}))")
                                loaded = [script for script in scripts
                                          if unquote(urlsplit(script['src']).path) == prefix + entry]
                                assert len(loaded) == 1, (kind, entry, scripts)
                                assert (loaded[0]['type'] == 'module') == (kind == 'vue'), loaded
                                assert context.request.get(loaded[0]['src']).status == 200
                                if kind == 'vue':
                                    page.wait_for_function('!!window.SessionDockRuntime')
                                    page.evaluate('window.SessionDockRuntime.build.checkServerBuild()')
                                else:
                                    assert page.evaluate("typeof window.SessionDockRuntime === 'undefined' && typeof S === 'object'")
                                    page.evaluate('checkServerBuild()')
                                expect(page.locator('.version-stale')).to_have_count(0)
                                assert page.locator('meta[name="sessiondock-build"]').get_attribute('content') == build
                                assert context.request.get(base + prefix + 'api/meta').json()['build'] == build
                                if width == 390 and page.locator('.mobile-back').is_visible():
                                    page.locator('.mobile-back').click()
                                page.locator('#q').fill('NodeA')
                                expect(page.locator('#side .item')).to_have_count(1)
                                page.locator('#side .item').click()
                                expect(page.locator('#msgs')).to_contain_text('reply NodeA')
                            response = context.request.post(base + prefix + 'api/session/conversation/send',
                                data={'uid': scoped(node.nid, 'claude:same-native-id'), '_build': build, 'text': 'fixture'})
                            assert response.status == 200, response.text()
                            assert context.request.get(base + '/sessiondock/api/meta').json()['build'] == original_build
                            assert context.request.get(base + '/sessiondock2/api/meta').json()['build'] == preview_build
                            assert not errors, errors
                            context.close()
                            print(f'PASS {kind} {width}px: actual entry, list, filter, session, repeated reload and send build', flush=True)
                    browser.close()
            finally:
                for hub in reversed(hubs):
                    hub.stop()
    finally:
        node.stop()
    print('PASS frontend_preview_browser: legacy main and Vue preview each serve matching authenticated assets and API', flush=True)


if __name__ == '__main__':
    main()
