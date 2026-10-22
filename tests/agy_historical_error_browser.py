#!/usr/bin/env python3
"""Agy stale error receipts, actual Chromium sends and durable-host replay."""
import argparse
import json
import tempfile
from pathlib import Path

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO
from native_sdk_queue_fixtures import NativeQueueFixture
from native_sdk_queue_browser import open_page, send, settled, pictures


def exercise(browser, fixture, screenshots):
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            for marker, success in [
                ('__MALFORMED_MISSING__', False), ('__MALFORMED_SEED__', False),
                ('__MALFORMED_HISTORY__', True), ('__MALFORMED_MISMATCH__', False),
                ('__MALFORMED_CURRENT__', False), ('__MALFORMED_DIFFERENT__', False),
                ('__MALFORMED_HISTORY__', False),
                ('__RECOVERED_STREAM__', False), ('__HISTORICAL_ERROR__', True),
                ('__HISTORICAL_WITH_CURRENT_ERROR__', False),
                ('__MALFORMED_SEED__', False), ('__MALFORMED_HISTORY__', True),
            ]:
                receipt = send(page, marker)
                settled(page, fixture, receipt['request_id'])
                page.wait_for_function('NativeSdkView.raw?.running === false')
                raw = page.evaluate('NativeSdkView.raw')
                assert not raw.get('input_queue'), raw.get('input_queue')
                assert fixture.entry(receipt['request_id'])['consumption'] == 'observed'
                if success:
                    assert not raw.get('error'), raw.get('error')
                    assert raw['last_turn']['status'] == 'completed'
                    assert 'prior turn; current native response completed' in raw['diagnostics']
                else:
                    assert raw.get('error'), marker
                    assert raw['error_kind'] == ('uncertain' if marker == '__RECOVERED_STREAM__' else 'turn')
                    expect(page.locator('.native-sdk-pane .sdk-status')).to_have_text(
                        '结果待核对' if marker == '__RECOVERED_STREAM__' else '错误')
                print(f'{marker}: historical={success}, delivery completed, native verdict checked', flush=True)
            expect(page.locator('.sdk-live')).to_contain_text('历史函数调用错误未污染本轮')
            pictures(page, uid, screenshots, 'historical-error', all_sizes=True)
            pid = fixture.native_pid()
            inputs = fixture.inputs()
        finally:
            context.close()
    # Simulate the previous bridge verdict in a private delivery ledger only.
    journal_path = fixture.root / 'state/native-sdk-inputs.json'
    journal = json.loads(journal_path.read_text())
    entry = next(row for rows in journal.values() for row in rows
                 if row['request_id'] == receipt['request_id'])
    entry['runtime_error'] = 'agy_sdk_native_ERROR: stale malformed function call'
    journal_path.write_text(json.dumps(journal))
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            expect(page.locator('.sdk-live')).to_contain_text('历史函数调用错误未污染本轮')
            page.wait_for_function('NativeSdkView.raw?.running === false')
            assert not page.evaluate('NativeSdkView.raw.error')
            assert fixture.native_pid() == pid and fixture.inputs() == inputs
            # Settled delivery diagnostics are archival; replay corrects the live verdict.
            assert fixture.entry(receipt['request_id'])['state'] == 'completed'
            following = send(page, 'continue after historical error replay')
            settled(page, fixture, following['request_id'])
            expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply: continue after historical error replay')
            assert fixture.native_pid() == pid
            pictures(page, uid, screenshots, 'historical-error-replay')
        finally:
            context.close()
    print('PASS Agy historical error: native evidence, real errors, same PID, restart replay without resend', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--host-binary', type=Path, default=REPO / 'target/debug/ptyhost')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-historical-') as tmp, sync_playwright() as pw:
        fixture = NativeQueueFixture(Path(tmp) / 'agy', 'agy', args.binary.resolve(), args.host_binary.resolve())
        browser = pw.chromium.launch(headless=True)
        try:
            exercise(browser, fixture, args.screenshots)
        finally:
            browser.close()
            fixture.close()


if __name__ == '__main__':
    main()
