#!/usr/bin/env python3
"""A failed durable SDK approval write stays pending and is retryable in Chromium."""
import argparse
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from claude_sdk_browser import open_sdk, send, snapshot, wait_snapshot
from claude_sdk_fixtures import ClaudeFixture
from codebuddy_sdk_fixtures import CodebuddyFixture
from droid_sdk_fixtures import DroidFixture
from history_fixtures import BINARY


def replies(fixture, key):
    return [frame for frame in fixture.frames()
            if (frame.get('type') == 'control_response'
                and frame.get('response', {}).get('request_id') == key)
            or (frame.get('type') == 'response' and frame.get('id') == key)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--only', choices=['claude', 'codebuddy', 'droid'])
    args = parser.parse_args()
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source, factory in [('claude', ClaudeFixture), ('codebuddy', CodebuddyFixture), ('droid', DroidFixture)]:
                if args.only and source != args.only:
                    continue
                with tempfile.TemporaryDirectory(prefix='sessiondock-outbox-') as tmp:
                    fixture = factory(Path(tmp))
                    with fixture.server(args.binary) as base:
                        context = browser.new_context(viewport={'width': 1280, 'height': 900})
                        page = context.new_page()
                        page.goto(base, wait_until='networkidle')
                        open_sdk(page, fixture, expect)
                        send(page, 'approval')
                        card = wait_snapshot(page, fixture.uid, lambda raw: bool(raw['requests']))['raw']['requests'][0]
                        key = card['key']
                        outboxes = list((fixture.root/'state/sdk-adapters'/source).glob('*-outbox.json'))
                        assert len(outboxes) == 1, outboxes
                        # Only the private outbox temporary path fails. Checkpoints
                        # and host pipes remain writable, exposing leaked entries.
                        blocked = outboxes[0].with_suffix('.tmp')
                        blocked.mkdir()
                        button = page.locator(f'[data-sdk-request="{key}"]').get_by_role('button', name='拒绝', exact=True)
                        with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/respond')) as failed:
                            button.click()
                        assert not failed.value.ok, failed.value.text()
                        # The hosted writer polls every 25 ms even without a wake.
                        page.wait_for_timeout(1000)
                        assert not replies(fixture, key), (source, 'failed write reached native CLI', replies(fixture, key))
                        assert any(r['key'] == key for r in snapshot(page, fixture.uid)['raw']['requests'])
                        expect(button).to_be_enabled()
                        blocked.rmdir()
                        with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/respond')) as retried:
                            button.click()
                        assert retried.value.ok, retried.value.text()
                        wait_snapshot(page, fixture.uid, lambda raw: not raw['requests'] and not raw['running'])
                        assert len(replies(fixture, key)) == 1, replies(fixture, key)
                        page.reload(wait_until='networkidle')
                        open_sdk(page, fixture, expect)
                        assert not snapshot(page, fixture.uid)['raw']['requests']
                        assert len(replies(fixture, key)) == 1, replies(fixture, key)
                        context.close()
                    print(f'PASS {source}: failed outbox write sends nothing, approval stays pending, retry sends once, reload keeps receipt', flush=True)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
