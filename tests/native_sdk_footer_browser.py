#!/usr/bin/env python3
"""Four native SDK footers: bounded labels, model selection and usage disclosure."""
import argparse
from contextlib import contextmanager
import json
import os
import re
from pathlib import Path
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

from playwright.sync_api import expect, sync_playwright
from agy_sdk_fixtures import AgyFixture
from claude_sdk_fixtures import ClaudeFixture
from codex_sdk_browser import server
from grok_sdk_fixtures import GrokFixture
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from opencode_sdk_fixtures import FakeOpenCode
from sdk_usage_fixtures import assert_inline_usage, toggle_usage_details
from send_browser import initialize


@contextmanager
def provider(source, root, binary):
    if source == 'claude':
        fixture = ClaudeFixture(root)
        with fixture.server(binary) as base:
            yield base
    elif source == 'grok':
        for name in ('claude', 'codex', 'grok', 'state'):
            (root / name).mkdir()
        fixture = GrokFixture(root)
        initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
        with server(SimpleNamespace(root=root), binary, 'off', {
            **fixture.env, 'SESSIONDOCK_NATIVE_SDKS': 'auto',
            'SESSIONDOCK_LIFECYCLE_DIR': str(root/'ledger'),
            'SESSIONDOCK_PTYHOST_DIR': str(root/'unused-host'),
            'SESSIONDOCK_STATE_DIR': str(root/'state'),
        }) as base:
            yield base
    elif source == 'agy':
        fixture = AgyFixture(root, REPO/'target/debug/ptyhost')
        with patch.dict(os.environ, fixture.env):
            initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
            with isolated_server(Corpus(root), binary, host_dir=root/'host',
                    lifecycle_dir=root/'ledger', state_dir=root/'state',
                    launcher_config=fixture.launcher, extra_env=fixture.extra_env) as (base, _):
                yield base
    else:
        for name in ('claude', 'codex', 'grok', 'host', 'state', 'proc'):
            (root/name).mkdir()
        (root/'proc/stat').write_text('btime 1700000000\n')
        with FakeOpenCode(root) as native:
            launcher = root/'launcher.json'
            launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO/'target/debug/ptyhost'),
                'host_dir': str(root/'host'), 'profiles': [native.profile()]}))
            initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
            with isolated_server(Corpus(root), binary, host_dir=root/'host', state_dir=root/'state',
                lifecycle_dir=root/'ledger', launcher_config=launcher, extra_env={
                    'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_OPENCODE_DB': str(native.db),
                    'SESSIONDOCK_OPENCODE_ROOT': str(root/'mirror'),
                    'SESSIONDOCK_PROC_ROOT': str(root/'proc')}) as (base, _):
                yield base


def exercise(browser, base, source, screenshots):
    for theme in ('light', 'dark'):
        context = browser.new_context(viewport={'width': 1440, 'height': 900},
                                      color_scheme=theme, service_workers='block')
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base+'/') else route.abort())
        # Native transport fixtures remain private; stress the UI's common catalog
        # and usage response contracts with realistic long provider labels.
        long_model = 'provider/very-long-model-name-2026-10-19-thinking-preview'
        long_quota = 'Enterprise shared premium model quota 中文长名称'
        models = [{'id': 'short', 'efforts': ['low', 'high']},
                  {'id': 'long', 'displayName': long_model, 'efforts': ['low', 'high']}]
        context.route('**/api/native-sdk/open', lambda route: route.fulfill(json={
            'ok': True, 'capabilities': {'models': True}}))
        packet = {'type': 'snapshot', 'seq': 1, 'raw': {'running': False, 'model': 'short',
            'usage': {'tokens': {'total_tokens': 123456789},
                      'context': {'percent': 25, 'used_tokens': 50000, 'max_tokens': 200000}}}}
        context.route('**/api/native-sdk/events?*', lambda route: route.fulfill(
            content_type='text/event-stream', body='event: sdk\ndata: '+json.dumps(packet)+'\n\n'))
        context.route('**/api/native-sdk/models', lambda route: route.fulfill(json={'ok': True, 'raw': models}))
        context.route('**/api/native-sdk/usage', lambda route: route.fulfill(json={
            'ok': True, 'usage': {'tokens': {'total_tokens': 123456789},
                'context': {'percent': 25, 'used_tokens': 50000, 'max_tokens': 200000}},
            'quota': {'buckets': [{'group': long_quota, 'remaining_percent': 30}]}}))
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible():
            # Source tabs use their native button IDs in the production frontend.
            page.locator(f'#chips button[data-source="{source}"]').click()
        row.click()
        page.locator('#a-sdk').click()
        pane = page.locator('.native-sdk-pane')
        model = pane.locator('#sdk-model')
        expect(model.locator('option[value="long"]')).to_have_count(1)
        expect(model).to_be_visible()
        model.select_option('long')
        expect(model).to_have_attribute('title', long_model)
        width_long = model.bounding_box()['width']
        assert width_long <= 160.5, width_long
        model.select_option('short')
        assert abs(model.bounding_box()['width'] - width_long) <= 1
        page.locator('#sdk-input').fill('底栏测试草稿 retained while changing models')
        for width in (1440, 390, 320):
            page.set_viewport_size({'width': width, 'height': 900})
            if not page.locator('#sdk-input').is_visible():
                row.click()
                page.locator('#a-sdk').click()
            expect(pane.locator('.sdk-gauges')).to_contain_text('123.5M tok')
            expect(pane.locator('.sdk-gauges')).to_contain_text('Ctx')
            assert_inline_usage(page)
            assert model.bounding_box()['width'] <= 160.5
            labels = pane.locator('.sdk-status-row .sdk-gauge > span').evaluate_all('nodes => nodes.map(n => n.getBoundingClientRect().width)')
            assert max(labels) <= 72.5, labels
            expect(pane.locator('.sdk-status-row .sdk-gauge').filter(has_text=long_quota)).to_have_attribute('title', re.compile(long_quota))
            toggle_usage_details(page)
            expect(page.locator('#native-usage-detail')).to_contain_text('123,456,789')
            expect(page.locator('#native-usage-detail')).to_contain_text(long_quota)
            toggle_usage_details(page)
            page.locator('#sdk-more').click()
            page.keyboard.press('Escape')
            expect(page.locator('#sdk-input')).to_have_value('底栏测试草稿 retained while changing models')
            page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}.png'))
        assert not errors, errors
        context.close()
    print(f'PASS {source}: compact footer, long models/quota, exact usage details, 3 widths, light/dark', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-native-footer-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in ('grok', 'agy', 'opencode', 'claude'):
                root = Path(temporary)/source
                root.mkdir()
                with provider(source, root, args.binary) as base:
                    exercise(browser, base, source, args.screenshots)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
