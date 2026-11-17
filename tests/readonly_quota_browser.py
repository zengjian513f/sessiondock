"""Read-only model matching and unmatched quota groups on node and Hub."""
import argparse
import re
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_mode_fixtures import provider
from sdk_usage_fixtures import assert_single_status_row


def exercise(browser, base, source, output, label):
    groups = {'droid': ['Standard', 'Core'], 'agy': ['Gemini Models', 'Claude and GPT models'],
              'codex': ['Synthetic One', 'Synthetic Two']}[source]
    names = {'droid': groups, 'agy': ['Gemini', 'Claude/GPT'], 'codex': groups}[source]
    known = {'droid': 'claude-synthetic', 'agy': 'gemini-synthetic', 'codex': 'synthetic-one'}[source]
    windows=['5h', 'weekly'] if source=='codex' else ['5h', 'weekly', 'monthly']
    labels=['5h', '7d'] if source=='codex' else ['5h', '7d', '1m']
    quota = {'source': source, 'captured_at': '2026-11-17T10:00:00Z', 'buckets': [
        {'id': f'{group}:{window}', 'group': group, 'window': window,
         'remaining_percent': 83-index*11, 'resets_at': '2027-01-01T00:00:00Z'}
        for index, group in enumerate(groups) for window in windows]}
    for width, theme in [(1440, 'light'), (390, 'dark')]:
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
        page = context.new_page()
        errors, opens, held = [], [], []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda req: opens.append(req.url) if req.url.split('?')[0].endswith('/open') else None)
        state = {'model': None, 'hold': True, 'failed': False}
        def quotas(route):
            if state['failed']: route.fulfill(status=503, json={'error': 'synthetic offline'})
            else: route.fulfill(json={'agents': [quota]})
        page.route('**/api/quotas**', quotas)
        def inventory(route):
            response=route.fetch();data=response.json()
            def models(value):
                if isinstance(value, dict):
                    if value.get('source')==source and 'sid' in value: value['model']=state['model']
                    for child in value.values(): models(child)
                elif isinstance(value, list):
                    for child in value: models(child)
            models(data)
            route.fulfill(response=response,json=data)
        page.route(re.compile(r'/api/sessions(?:\?|$)'), inventory)
        def history(route):
            response = route.fetch()
            data = response.json()
            data['meta']['model'] = state['model']
            if state['hold']: held.append((route, response, data))
            else: route.fulfill(response=response, json=data)
        page.route('**/api/messages/**', history)
        # This fixture varies HTTP metadata; keep the native fixture's fixed
        # SSE model from contradicting it. The page exercises HTTP fallback.
        page.route('**/api/watch?*', lambda route: route.abort())
        page.route('**/api/codex-sdk/recorded-usage?*', lambda route: route.fulfill(json={
            'model': state['model'], 'usage': {'modelContextWindow': 10000, 'last': {'totalTokens': 2000}}}))
        page.goto(base, wait_until='networkidle')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        expect(row).to_be_attached()
        if not row.is_visible(): page.locator(f'#chips button[data-source="{source}"]').click()
        row.click()
        footer = page.locator('.sdk-pane .sdk-status-row')
        gauges = footer.locator(':scope > .sdk-gauges')
        # Account data arrives before the history model: show both groups.
        expect(gauges.locator('.sdk-quota-name')).to_have_text(names)
        assert held
        state['hold'] = False
        for route, response, data in held: route.fulfill(response=response, json=data)
        held.clear()
        expect(page.locator('.sdk-history')).not_to_be_empty()
        for model, count in [(None, 6), ('unmatched-synthetic', 6), (known, 3)]:
            state['model'] = model
            page.reload(wait_until='networkidle')
            expect(gauges.locator('.sdk-gauge > span').filter(has_text='5h')).to_have_count(2 if count==6 else 1)
            if source=='codex' and count==3:
                # The usual Codex single-pool slots retain their existing shape.
                expect(gauges.locator('[data-gauge="5h"]')).to_contain_text('5h')
            else:
                expect(gauges.locator('.sdk-quota-name')).to_have_text(names if count==6 else names[:1])
                expect(gauges.locator('.sdk-quota-windows .sdk-gauge > span')).to_have_text(labels*(2 if count==6 else 1))
            if count==6:
                expect(page.locator('#sdk-more-menu .sdk-quota-name')).to_have_count(0)
                assert_single_status_row(footer)
                page.screenshot(path=str(output/f'{label}-{source}-{width}-{model or "missing"}.png'), full_page=True)
                if width<720:
                    gauges.hover(); page.mouse.wheel(1200, 0)
                    expect(gauges.locator('.sdk-quota-group').last.locator('.sdk-gauge').last).to_be_in_viewport()
                    page.screenshot(path=str(output/f'{label}-{source}-{width}-scrolled.png'), full_page=True)
            else:
                page.locator('#sdk-more').click()
                expect(page.locator('#sdk-more-menu .sdk-quota-name')).to_have_text(names[1:])
                page.keyboard.press('Escape')
            # Sources with ordinary conversation share the same grouping.
            if source!='droid':
                page.locator('#a-chat').click()
                conversation = page.locator('#conversation-status .sdk-gauges')
                expect(conversation.locator('.sdk-quota-name')).to_have_text(names if count==6 else (names[:1] if source!='codex' else []))
                expect(conversation.locator('.sdk-gauge > span')).to_have_text(labels*(2 if count==6 else 1))
                assert_single_status_row(page.locator('#conversation-status'))
                page.locator('#a-sdk').click()
                expect(gauges).to_contain_text('5h')
        # Model-only history updates must re-evaluate the current native pool.
        if source!='codex':
            state['model']='unmatched-synthetic'
            expect(gauges.locator('.sdk-quota-name')).to_have_text(names, timeout=15000)
        # Failed quota reads preserve the shared account data on warm re-entry.
        state['failed']=True
        if width<720: page.locator('#detail .mobile-back').click()
        row.click()
        expect(gauges).to_contain_text('5h')
        assert not opens, opens
        assert not errors, errors
        page.unroute_all(behavior="wait")
        context.close()
        print('PASS read-only quotas', label, source, width, theme, flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--screenshots', type=Path, required=True)
    parser.add_argument('--sources', default='droid,agy,codex')
    args = parser.parse_args(); args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                with tempfile.TemporaryDirectory(prefix='sd-readonly-quota-') as tmp:
                    root=Path(tmp)
                    with provider(source, root, BINARY) as (base, peer, env):
                        if source=='codex': peer.unload()
                        hubroot=root/'hub';hubroot.mkdir()
                        port=int(env['SESSIONDOCK_NODE_BIND'].rsplit(':',1)[1])
                        hub=Hub(BINARY.with_name('sessiondock-hub'),hubroot,[SimpleNamespace(name='Quota node',token=TOKEN,port=port)])
                        hub.start()
                        try:
                            for label,url in [('node',base),('hub',f'http://127.0.0.1:{hub.port}')]:
                                exercise(browser,url,source,args.screenshots,label)
                        finally: hub.stop()
        finally: browser.close()


if __name__=='__main__': main()
