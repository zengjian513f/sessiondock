#!/usr/bin/env python3
"""SDK admission through Chromium with portable native process evidence.

Synthetic snapshots cover the platform-neutral decision path. They do not
claim native OS collection validation.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from claude_sdk_fixtures import ClaudeFixture
from codebuddy_sdk_fixtures import CodebuddyFixture
from claude_sdk_browser import open_sdk, post, send, wait_snapshot
from history_fixtures import BINARY


def exercise(binary, kind, native_snapshots=()):
    cls = ClaudeFixture if kind == 'claude' else CodebuddyFixture
    with tempfile.TemporaryDirectory(prefix='sessiondock-sdk-writers-') as temporary:
        fixture = cls(Path(temporary))
        evidence = fixture.root / 'proc/sdk-writers.json'
        evidence.write_text('[]')
        with fixture.server(binary) as base, sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True, args=['--no-sandbox'])
            page = browser.new_page()
            page.goto(base, wait_until='networkidle')
            open_sdk(page, fixture, expect)
            raw = wait_snapshot(page, fixture.uid, lambda raw: 'capabilities' in raw)['raw']
            assert raw['capabilities']['resume'] is True
            own = fixture.frames('spawn')[-1]['pid']
            def row(pid, args, *, ppid=1, cwd=None, files=None, started=1):
                return dict(pid=pid, ppid=ppid, args=args, cwd=cwd, files=files or [], started=started)
            def save(rows):
                pending = evidence.with_suffix('.next')
                pending.write_text(json.dumps(rows))
                pending.replace(evidence)
            # A non-CLI wrapper in the ancestry must not hide SDK ownership.
            save([row(own, [kind]), row(910001, ['shell'], ppid=own),
                  row(910002, [kind, '--resume', fixture.sid], ppid=910001),
                  row(910003, [kind, '--resume', 'unrelated-session'])])
            send(page, 'owned descendants remain usable')
            expect(page.locator('.sdk-live')).to_contain_text('synthetic final owned descendants remain usable')
            wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])
            # A known other SID is not a cwd-wide lock.
            save([row(910003, [kind, '--resume=unrelated-session'], cwd=str(fixture.cwd))])
            send(page, 'other native SID remains usable')
            expect(page.locator('.sdk-live')).to_contain_text('synthetic final other native SID remains usable')
            wait_snapshot(page, fixture.uid, lambda raw: not raw['running'])
            def rejected(label):
                send(page, label)
                page.wait_for_function("text => NativeSdkView.raw.input_queue?.some(r => r.text === text && r.state === 'failed')", arg=label)
                # Remove the failed queue entry through the same public action;
                # it cannot become a later send when evidence changes.
                queue = page.evaluate("text => NativeSdkView.raw.input_queue.find(r => r.text === text)", label)
                post(page, 'queue_cancel', {'uid': fixture.uid, 'request_id': 'cancel-' + queue['request_id'], 'id': queue['id']})
            # A recycled parent PID cannot exempt an independent older child.
            save([row(own, [kind], started=20),
                  row(910002, [kind, '--resume', fixture.sid], ppid=own, started=10)])
            rejected('must not send recycled parent')
            # Exact native file ownership applies even without a CLI argv SID.
            save([row(910003, ['unknown-helper'], files=[str(fixture.path)])])
            rejected('must not send file owner')
            if kind == 'claude':
                save([row(910003, ['claude'], cwd=str(fixture.cwd))])
                rejected('must not send bare cwd')
            save([row(910003, [kind + '.exe', '--resume=' + fixture.sid])])
            rejected('independent writer must stay untouched')
            assert json.loads(evidence.read_text())[0]['pid'] == 910003
            if kind == 'claude':
                for index, native in enumerate(native_snapshots):
                    rows = json.loads(native.read_bytes())
                    assert any('--resume=' + fixture.sid in row['args'] for row in rows), native
                    save(rows)
                    rejected('must not send native OS evidence ' + str(index))
            assert not any('must' in str(frame.get('message', {}).get('content', '')) for frame in fixture.frames('user'))
            browser.close()
    print('PASS portable SDK writer Chromium:', kind)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--native-snapshot', type=Path, action='append', default=[], help='private native collection result; tests the resulting admission through Chromium')
    args = parser.parse_args()
    for kind in ('claude', 'codebuddy'):
        exercise(args.binary, kind, args.native_snapshot)


if __name__ == '__main__':
    main()
