#!/usr/bin/env python3
"""Typing into long histories must not restyle thousands of message controls.

Private fake SDKs and a synthetic HTTP history projection; real Chromium input,
node/Hub navigation and draft persistence. CDP counts affected elements rather
than gating on machine-dependent timing. No native CLI or production data.
"""
import argparse
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from frontend_framework_browser import launch_chromium
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_start_stop_fixtures import provider


def exercise(browser, base, source, width, theme, screenshots, label):
    context = browser.new_context(viewport={'width': width, 'height': 900},
                                  color_scheme=theme, service_workers='block')
    page = context.new_page()
    page.set_default_timeout(30000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    # Keep native metadata/cursors and only substitute the read-only projection.
    # Thousands of user/assistant bubbles and folded processes match an already
    # loaded long history, independently of each provider's on-disk format.
    messages = [{'role': ('user', 'thinking', 'assistant')[i % 3],
                 'text': f'History {i}: ' + 'Long conversation paragraph. ' * 15,
                 'ts': '2026-09-11T10:00:00.000Z'} for i in range(4000)]

    def history(route):
        response = route.fetch()
        data = response.json()
        if data.get('reset'):
            data.update(messages=messages, partial=None, message_total=len(messages))
        route.fulfill(response=response, json=data)

    page.route('**/api/messages/**', history)
    try:
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible():
            page.locator(f'#chips button[data-source="{source}"]').click()
        row.click()
        expect(page.locator('.sdk-history')).to_contain_text('History 3999:')
        editor = page.locator('#sdk-input')
        expect(editor).to_be_enabled()
        editor.fill('')
        editor.click()
        # Wait for startup/layout and the real draft store, before measuring keys.
        page.wait_for_function('''() => {
          const draft = composerDrafts.get(composerDraftOwner(S.sel));
          return draft && !draft.loading && draft.editVersion === draft.savedVersion;
        }''')
        # Some bridges finish their initial history reconciliation after the
        # first paint. Measure editing after that work, not startup rendering.
        page.evaluate('''() => new Promise(resolve => {
          let timer;
          const history = document.querySelector('.sdk-history');
          const observer = new MutationObserver(quiet);
          function quiet() {
            clearTimeout(timer);
            timer = setTimeout(() => { observer.disconnect(); resolve(); }, 800);
          }
          observer.observe(history, {childList:true, subtree:true});
          quiet();
        })''')
        cdp = context.new_cdp_session(page)
        events, completed = [], []
        cdp.on('Tracing.dataCollected', lambda data: events.extend(data['value']))
        cdp.on('Tracing.tracingComplete', lambda data: completed.append(True))
        cdp.send('Tracing.start', {'categories': 'devtools.timeline', 'transferMode': 'ReportEvents'})
        started = time.monotonic()
        editor.press_sequentially('typing stays responsive', delay=5)
        elapsed = round((time.monotonic() - started) * 1000)
        # A genuine composer DOM change must also stay local. This independently
        # catches the broad CSS invalidation even when SEND avoids no-op writes.
        page.locator('#sdk-add').click()
        page.locator('#sdk-attach-menu [data-attach="quote"]').click()
        page.get_by_role('textbox', name='引用文字').fill('quoted context')
        page.locator('#sdk-compose-items .draft-quote .draft-remove').click()
        editor.click()
        cdp.send('Tracing.end')
        deadline = time.monotonic() + 10
        while not completed and time.monotonic() < deadline:
            page.wait_for_timeout(20)
        assert completed, 'Chromium trace did not complete'
        counts = [event['args'].get('elementCount', 0) for event in events
                  if event['name'] == 'UpdateLayoutTree']
        assert counts, 'Missing Chromium style-recalculation evidence'
        if max(counts) >= 500:
            trace = (screenshots or Path(tempfile.gettempdir())) / f'{label}-trace.json'
            trace.write_text(json.dumps(events))
            print(f'Unexpected history style work: {trace}', flush=True)
        # Before the fix, each key restyled >5,000 elements, including hidden
        # ordinary-history buttons. Input should touch only local controls.
        assert max(counts) < 500, (label, max(counts), sum(counts))
        expect(editor).to_have_value('typing stays responsive')
        expect(editor).to_be_focused()
        page.wait_for_function('''() => {
          const draft = composerDrafts.get(composerDraftOwner(S.sel));
          return draft.text === 'typing stays responsive' && draft.editVersion === draft.savedVersion;
        }''')
        # Editing must not discard the loaded history or make its controls inert.
        assert page.locator('.sdk-history .msg').count() > 3000
        box = page.locator('.sdk-msgs')
        box.hover()
        page.mouse.wheel(0, -650)
        process = page.locator('.sdk-history .turn-process').last
        process.locator('.turn-toolbar .fold-toggle').click()
        expect(process.locator('.turn-process-body')).to_be_visible()
        process.locator('.turn-toolbar .fold-toggle').click()
        expect(process.locator('.turn-process-body')).to_be_hidden()
        editor.click()
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}.png'))
        assert not errors, errors
        print(f'PASS {label}: 4000 messages, typing_ms={elapsed}, '
              f'max_restyled={max(counts)}, total_restyled={sum(counts)}, saved draft and history controls', flush=True)
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--sources', default='codex,claude,grok,agy,opencode,kiro,codebuddy')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    hub_binary = args.hub_binary or args.binary.with_name('sessiondock-hub')
    with tempfile.TemporaryDirectory(prefix='sd-message-input-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        browser = launch_chromium(pw)
        try:
            for source in args.sources.split(','):
                work = root / source
                work.mkdir()
                with provider(source, work, args.binary) as (base, peer, extra):
                    hub_root = work / 'hub'
                    hub_root.mkdir()
                    port = int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1])
                    hub = Hub(hub_binary.resolve(), hub_root,
                              [SimpleNamespace(name='InputNode', token=TOKEN, port=port)])
                    hub.start()
                    try:
                        for mode, url, width, theme in [('node', base, 1600, 'light'),
                                ('hub', f'http://127.0.0.1:{hub.port}', 390, 'dark')]:
                            print(f'RUN {source}-{mode}-{width}-{theme}', flush=True)
                            exercise(browser, url, source, width, theme, args.screenshots,
                                     f'{source}-{mode}-{width}-{theme}')
                    finally:
                        hub.stop()
        finally:
            browser.close()


if __name__ == '__main__':
    main()
