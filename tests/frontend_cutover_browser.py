#!/usr/bin/env python3
"""Compiled frontend cutover: prefixed entry, retired cache and worker cleanup.

Only a private loopback proxy, synthetic sessions and a temporary Chromium
profile are used. The old worker is actually installed before navigation.
"""
import argparse
import tempfile
from pathlib import Path

from playwright.sync_api import expect, sync_playwright
from browser_runtime import js, scoped_frontend, wait_for_async
from frontend_entry_browser import PREFIX, prefixed_proxy
from frontend_framework_browser import launch_chromium, open_settings
from history_parity import BINARY, build_corpus, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-cutover-browser-') as temporary:
        corpus = build_corpus(Path(temporary))
        with isolated_server(corpus, args.binary) as (node, _), prefixed_proxy() as (base, target), sync_playwright() as pw:
            target.url = node
            target.fixtures[PREFIX + '__seed.html'] = ('text/html', b'<title>Private previous shell</title>')
            target.fixtures[PREFIX + '__previous_shell.js'] = ('text/javascript', b'''
                self.addEventListener('install', () => self.skipWaiting());
                self.addEventListener('activate', event => event.waitUntil(clients.claim()));
            ''')
            browser = launch_chromium(pw)
            try:
                context = browser.new_context(service_workers='allow')
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base + '__seed.html')
                page.evaluate('''async () => {
                  await navigator.serviceWorker.register('__previous_shell.js', {scope: './'});
                  await navigator.serviceWorker.ready;
                  const old = await caches.open('sessiondock-shell-fixture');
                  await old.put('./', new Response('<title>Retired shell</title>'));
                  const unrelated = await caches.open('unrelated-fixture');
                  await unrelated.put('./sentinel', new Response('keep'));
                }''')
                assert page.evaluate('async () => (await navigator.serviceWorker.getRegistrations()).length') == 1
                assert 'sessiondock-shell-fixture' in page.evaluate('() => caches.keys()')
                page.goto(base, wait_until='networkidle')
                page.wait_for_function(js('S.sessions.length > 0', 'runtime.core.state.catalog.sessions.length > 0'))
                wait_for_async(page, '''async () =>
                  !(await navigator.serviceWorker.getRegistrations()).length
                  && !(await caches.keys()).some(key => key.startsWith('sessiondock-shell-'))''')
                assert 'unrelated-fixture' in page.evaluate('() => caches.keys()')
                page.locator('#q').fill('Claude common')
                session = page.locator('#side .item .t[title="Claude common question"]').first
                expect(session).to_be_visible()
                session.click()
                expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
                open_settings(page)
                expect(page.locator('#settings-dialog')).to_be_visible()
                page.keyboard.press('Escape')
                # Vue file adapters declare mode/build; restored legacy file
                # adapters retain their original capability-script contract.
                for name in ('index.html', 'grid.html', 'records.html', 'file.html', 'files.html'):
                    response = context.request.get(base + name)
                    assert response.ok, (name, response.status)
                    body = response.text()
                    assert '__SESSIONDOCK_' not in body, name
                    if scoped_frontend() or name not in ('file.html', 'files.html'):
                        assert 'name="sessiondock-capabilities"' in body, name
                    assert response.headers.get('cache-control') == 'no-store', name
                for name in ('app.js', 'term.js', 'capabilities.js', 'framework/settings.js'):
                    assert context.request.get(base + name).status == (404 if scoped_frontend() else 200), name
                assert not errors, errors
                context.close()
            finally:
                browser.close()
    print('PASS frontend cutover: actual previous worker/cache retired, unrelated cache retained, '
          'prefixed search/history/settings work, five HTML entries injected, scripts match selected frontend')


if __name__ == '__main__':
    main()
