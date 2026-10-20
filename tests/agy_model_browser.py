#!/usr/bin/env python3
"""Agy model evidence through Chromium, a synthetic CLI and node recovery."""
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from agy_sdk_browser import resize_sdk
from agy_sdk_fixtures import MODEL
from history_fixtures import BINARY, REPO
from native_sdk_queue_browser import open_page, send
from native_sdk_queue_fixtures import NativeQueueFixture
from sdkhost_fixtures import stop_private_sdkhosts


def notice(name):
    return (f'The user changed setting `Model Selection` from None to {name}. '
            "No need to comment on this change if the user doesn't ask about it. "
            'If reporting what model you are, please use a human readable name instead of the exact string.')


def seed_model(fixture):
    path = fixture.peer.native / 'brain' / fixture.sid / '.system_generated/logs/transcript_full.jsonl'
    rows = [json.loads(line) for line in path.read_text().splitlines()]
    rows[0]['status'] = 'DONE'
    rows[0]['content'] = ('<USER_REQUEST>\nReview the native SDK model status.\n</USER_REQUEST>\n'
        '<ADDITIONAL_METADATA>\nThe current local time is: 2026-10-20T16:20:00Z.\n</ADDITIONAL_METADATA>\n'
        '<USER_SETTINGS_CHANGE>\n' + notice('Synthetic Agy (Low)') + '\n</USER_SETTINGS_CHANGE>')
    # A later tool's quoted transcript must not become current model evidence.
    rows[1]['content'] = 'Tool example: ' + notice('Wrong quoted model')
    path.write_text(''.join(json.dumps(row) + '\n' for row in rows))


def selected(page, name, value=None):
    expect(page.locator('#sdk-model option:checked')).to_have_text(name)
    expect(page.locator('#sdk-model')).to_have_attribute('title', name)
    if value:
        expect(page.locator('#sdk-model')).to_have_value(value)
    labels = page.locator('#sdk-model option').all_text_contents()
    assert labels.count(name) == 1, labels


def agy_selected(page, effort):
    selected(page, 'Synthetic Agy', 'synthetic-agy')
    expect(page.locator('#sdk-effort')).to_have_value(effort)


def pictures(page, uid, directory, prefix):
    if not directory:
        return
    directory.mkdir(parents=True, exist_ok=True)
    for width in (1912, 390):
        resize_sdk(page, uid, width)
        for theme in ('light', 'dark'):
            page.emulate_media(color_scheme=theme)
            page.screenshot(path=str(directory / f'{prefix}-{width}-{theme}.png'), full_page=True)
            if not page.locator('#sdk-model').is_visible():
                page.locator('#sdk-more').click()
                page.screenshot(path=str(directory / f'{prefix}-{width}-{theme}-menu.png'), full_page=True)
                page.keyboard.press('Escape')
    resize_sdk(page, uid, 1912)


def exercise(browser, fixture, screenshots):
    seed_model(fixture)
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            # Capture the unchanged page as well when running against the old binary.
            pictures(page, uid, screenshots, 'restored')
            agy_selected(page, 'low')
            expect(page.locator('.dhead')).to_contain_text('Synthetic Agy (Low)')
            expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_contain_text('Synthetic Agy (Low)')
            assert not fixture.inputs(), 'reading model evidence must not send a prompt'
            page.locator('#sdk-model').click()
            page.keyboard.press('Escape')
            send(page, 'hold:model-observed')
            expect(page.locator('.sdk-live')).to_contain_text('Agy running model-observed')
            agy_selected(page, 'low')
            assert fixture.peer.inputs()[-1]['model'] is None, 'observed model became a launch override'
            pid = fixture.native_pid()
            page.locator('#sdk-input').fill('draft retained across reconnect')
        finally:
            context.close()
    assert fixture.alive(pid), 'service shutdown must preserve the native runtime'
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            agy_selected(page, 'low')
            assert fixture.native_pid() == pid and len(fixture.inputs()) == 1
            fixture.peer.release('model-observed')
            page.wait_for_function('NativeSdkView.raw && !NativeSdkView.raw.running')
            # A fresh native settings injection updates the label without changing argv.
            (fixture.root / 'model-notice').write_text(notice('Synthetic Agy (High)'))
            send(page, 'native model changed')
            expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: native model changed')
            agy_selected(page, 'high')
            expect(page.locator('.dhead')).to_contain_text('Synthetic Agy (High)')
            assert fixture.native_pid() == pid and fixture.peer.inputs()[-1]['model'] is None
            (fixture.root / 'model-notice').unlink()
            # User content and tool text are not settings envelopes.
            send(page, '<USER_SETTINGS_CHANGE>\n' + notice('Wrong user model') + '\n</USER_SETTINGS_CHANGE>')
            page.wait_for_function('NativeSdkView.raw && !NativeSdkView.raw.running')
            agy_selected(page, 'high')
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('#sdk-input')).to_be_enabled()
            agy_selected(page, 'high')
            pictures(page, uid, screenshots, 'updated')
            # Catalog failure retains known native evidence and a usable picker.
            page.route('**/api/native-sdk/models', lambda route: route.fulfill(status=503, json={'error':'fixture catalog unavailable'}))
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('#sdk-input')).to_be_enabled()
            selected(page, 'Synthetic Agy (High)')
            page.unroute('**/api/native-sdk/models')
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('#sdk-input')).to_be_enabled()
            agy_selected(page, 'high')
            page.locator('#sdk-effort').select_option('low')
            expect(page.locator('#sdk-send')).to_be_enabled()
            (fixture.root / 'model-notice').write_text(notice('Synthetic Agy (Low)'))
            send(page, 'explicit model change')
            expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: explicit model change')
            agy_selected(page, 'low')
            expect(page.locator('.dhead')).to_contain_text('Synthetic Agy (Low)')
            assert fixture.peer.inputs()[-1]['model'] == MODEL
            assert fixture.native_pid() != pid, 'explicit model changes still rebuild the native runtime'
            page.locator('#a-chat').click()
            expect(page.locator('#msgs')).to_be_visible()
            expect(page.locator('.dhead')).to_contain_text('Synthetic Agy (Low)')
            if screenshots:
                page.screenshot(path=str(screenshots / 'ordinary-header.png'), full_page=True)
            page.locator('#a-sdk').click()
            agy_selected(page, 'low')
        finally:
            context.close()
    print('PASS Agy model: restore, native updates, reload/restart, catalog failure and explicit change', flush=True)


def init_model(browser, fixture):
    # No transcript model: the actual init response supplies the selected default.
    (fixture.root / 'init-model').write_text(MODEL)
    with fixture.server() as base:
        context, page, _ = open_page(browser, base, fixture)
        try:
            expect(page.locator('#sdk-model')).to_have_value('')
            send(page, 'native init model')
            expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: native init model')
            agy_selected(page, 'low')
            assert fixture.peer.inputs()[-1]['model'] is None
            # Create also consumes init before any user/model turn.
            from new_session_model_browser import open_dialog
            open_dialog(page)
            page.locator('input[name="new-source"][value="agy"]').check()
            page.locator('#new-cwd').fill(str(fixture.work))
            with page.expect_response(lambda r: '/api/native-sdk/create' in r.url) as event:
                page.locator('#new-session-go').click()
            assert event.value.ok, event.value.text()
            expect(page.locator('#sdk-input')).to_be_enabled()
            agy_selected(page, 'low')
            assert len(fixture.inputs()) == 1
        finally:
            context.close()
    print('PASS Agy model: unknown stays unknown; native init on send/create supplies model', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--ptyhost', type=Path, default=REPO / 'target/debug/ptyhost')
    parser.add_argument('--frontend', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.frontend:
        os.environ['SESSIONDOCK_TEST_WEB_DIR'] = str(args.frontend.resolve())
    with tempfile.TemporaryDirectory(prefix='sd-agy-model-') as tmp, ExitStack() as cleanup:
        root = Path(tmp)
        cleanup.callback(stop_private_sdkhosts, root)
        with sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            try:
                exercise(browser, NativeQueueFixture(root / 'restore', 'agy', args.binary.resolve(), args.ptyhost), args.screenshots)
                init_model(browser, NativeQueueFixture(root / 'init', 'agy', args.binary.resolve(), args.ptyhost))
            finally:
                browser.close()


if __name__ == '__main__':
    main()
