#!/usr/bin/env python3
"""Agy SDK model/effort grouping and exact native argv through Chromium."""
import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from agy_model_browser import pictures, seed_model, selected
from history_fixtures import BINARY, REPO
from native_sdk_queue_browser import open_page, send
from native_sdk_queue_fixtures import NativeQueueFixture

CATALOG = [('synthetic-agy-low', 'Synthetic Agy (Low)'),
           ('synthetic-agy-high', 'Synthetic Agy (High)'),
           ('synthetic-single-low-thinking', 'Synthetic Single (Low Thinking)'),
           ('synthetic-mixed', 'Synthetic Mixed'),
           ('synthetic-mixed-medium', 'Synthetic Mixed (Medium)'),
           ('synthetic-plain', 'Synthetic Plain')]


def choose(page, key, value):
    control = page.locator('#sdk-' + key)
    if not control.is_visible():
        page.locator('#sdk-more').click()
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/settings')) as response:
        control.select_option(value)
    assert response.value.ok, response.value.text()
    expect(control).to_be_enabled()
    page.keyboard.press('Escape')
    return response.value.request.post_data_json


def turn(page, fixture, text, model, effort=None):
    send(page, text)
    page.wait_for_function('NativeSdkView.raw && !NativeSdkView.raw.running')
    row = fixture.peer.inputs(text)[-1]
    assert (row['model'], row['effort']) == (model, effort), row
    return row


def exercise(browser, fixture, screenshots):
    seed_model(fixture)
    (fixture.root / 'models.tsv').write_text(''.join(f'{id}\t{name}\n' for id, name in CATALOG))
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            selected(page, 'Synthetic Agy', 'synthetic-agy')
            expect(page.locator('#sdk-model option')).to_have_text(['Synthetic Agy', 'Synthetic Single', 'Synthetic Mixed', 'Synthetic Plain'])
            expect(page.locator('#sdk-effort')).to_have_value('low')
            assert page.locator('#sdk-effort option').evaluate_all('rows => rows.map(row => row.value)') == ['low', 'high']
            assert not fixture.inputs()
            pictures(page, uid, screenshots, 'grouped')
            # Reproduce a conflicting saved setting from the old frontend.
            response = page.request.post(base + '/api/native-sdk/settings', data={
                'uid': uid, 'model': 'synthetic-agy-low', 'effort': 'max', 'request_id': 'old-conflict'})
            assert response.ok
            page.wait_for_function('NativeSdkView.nativeEffort === "max"')
            expect(page.locator('#sdk-effort')).to_have_value('low')
            turn(page, fixture, 'legacy conflicting settings', 'synthetic-agy-low')
            send(page, 'hold:effort')
            expect(page.locator('.sdk-live')).to_contain_text('Agy running effort')
            pid = fixture.native_pid()
            request = choose(page, 'effort', 'high')
            assert request['model'] == 'synthetic-agy-high' and request['effort'] == '', request
            assert fixture.native_pid() == pid, 'changing settings interrupted the active turn'
            fixture.peer.release('effort')
            page.wait_for_function('NativeSdkView.raw && !NativeSdkView.raw.running')
            row = turn(page, fixture, 'new native variant', 'synthetic-agy-high')
            assert row['pid'] != pid
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('#sdk-input')).to_be_enabled()
            selected(page, 'Synthetic Agy', 'synthetic-agy')
            expect(page.locator('#sdk-effort')).to_have_value('high')
            # A model with one native variant cannot inherit a different effort.
            request = choose(page, 'model', 'synthetic-single')
            assert request['model'] == 'synthetic-single-low-thinking' and request['effort'] == ''
            assert page.locator('#sdk-effort option').evaluate_all('rows => rows.map(row => row.value)') == ['low']
            if screenshots:
                page.screenshot(path=str(screenshots / 'single-variant.png'), full_page=True)
            turn(page, fixture, 'single native variant', 'synthetic-single-low-thinking')
            # A group may also contain a real base model with CLI-default effort.
            choose(page, 'model', 'synthetic-mixed')
            expect(page.locator('#sdk-effort')).to_have_value('')
            expect(page.locator('#sdk-effort option:checked')).to_have_text('CLI 默认')
            choose(page, 'effort', 'medium')
            turn(page, fixture, 'mixed medium', 'synthetic-mixed-medium')
            request = choose(page, 'effort', '')
            assert request['model'] == 'synthetic-mixed' and request['effort'] == ''
            if screenshots:
                page.screenshot(path=str(screenshots / 'mixed-default.png'), full_page=True)
            turn(page, fixture, 'mixed default', 'synthetic-mixed')
            # Plain native models retain the CLI's independent effort flag.
            choose(page, 'model', 'synthetic-plain')
            choose(page, 'effort', 'max')
            turn(page, fixture, 'plain independent effort', 'synthetic-plain', 'max')
            if screenshots:
                page.screenshot(path=str(screenshots / 'plain-independent.png'), full_page=True)
            choose(page, 'model', 'synthetic-agy')
            turn(page, fixture, 'plain to variant clears effort', 'synthetic-agy-high')
            pictures(page, uid, screenshots, 'selected')
        finally:
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--host-binary', type=Path, default=REPO / 'target/debug/ptyhost')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-effort-') as temp, sync_playwright() as pw:
        fixture = NativeQueueFixture(Path(temp) / 'agy', 'agy', args.binary.resolve(strict=True), args.host_binary)
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        try:
            exercise(browser, fixture, args.screenshots)
        finally:
            browser.close()
            fixture.close()
    print('PASS Agy effort: grouped catalog, exact variants, base/default, plain flag, legacy conflict, active turn and reload')


if __name__ == '__main__':
    main()
