#!/usr/bin/env python3
"""Browser-driven diagnostic retention and sink isolation, synthetic homes only.

No internal JS handler calls or API mutation: real navigation and row clicks
produce receipts; independent fixture reads assert their persistence and expiry.
"""
import argparse
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import tempfile
import time

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, build_corpus, isolated_server, get_json


def stamp(hours):
    return (datetime.now(timezone.utc) + timedelta(hours=hours)).isoformat()


def seed(path, *hours):
    path.write_text(''.join(json.dumps({'received_at': stamp(h), 'event': f'fixture.{h}'}) + '\n'
                            for h in hours))


def rows(directory):
    result = []
    for path in directory.glob('browser-*.jsonl'):
        for line in path.read_bytes().splitlines():
            try:
                result.append(json.loads(line))
            except (ValueError, UnicodeDecodeError):
                pass
    return result


def until(page, condition, seconds=12):
    deadline = time.monotonic() + seconds
    while not condition():
        assert time.monotonic() < deadline, 'condition deadline exceeded'
        page.wait_for_timeout(100)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('target/diagnostic-retention'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    evidence = {'scenarios': [], 'metrics': {}, 'functional': 'browser only'}
    with tempfile.TemporaryDirectory(prefix='sessiondock-retention-') as temporary, sync_playwright() as pw:
        corpus = build_corpus(Path(temporary))
        native = {p: p.read_bytes() for p in corpus.paths.values()}
        audit = corpus.root / 'audit'
        audit.mkdir()
        old = audit / 'browser-2000-01-01.jsonl'
        mixed = audit / 'browser-2000-01-02.0001.jsonl'
        recent = audit / 'browser-2000-01-03.jsonl'
        seed(old, -25)
        seed(mixed, -25, -23.99)
        seed(recent, -1)
        corrupt = audit / 'browser-2000-01-06.jsonl'
        corrupt.write_bytes(b'{broken\n\xff\xfe\n{}\n' + json.dumps({'received_at': stamp(-1), 'event': 'recent-in-corrupt'}).encode() + b'\n')
        old_time = time.time() - 26 * 3600
        os.utime(corrupt, (old_time, old_time))
        crash = audit / 'browser-2000-01-07.jsonl.retention-tmp'
        crash.write_bytes(b'partial rewrite')
        os.utime(crash, (old_time, old_time))
        fresh_crash = audit / 'browser-2000-01-08.jsonl.retention-tmp'
        fresh_crash.write_bytes(b'recent rewrite')
        large = audit / 'browser-2000-01-09.0001.jsonl'
        record = json.dumps({'received_at': stamp(-12), 'event': 'volume', 'data': {'fixture': 'x' * 420}}) + '\n'
        with large.open('w') as stream:
            for _ in range(140000):
                stream.write(record)
        large_size = large.stat().st_size
        protected = audit / 'saved-defect-evidence.jsonl'
        seed(protected, -300)
        protected_bytes = protected.read_bytes()
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**launch)
        try:
            with isolated_server(corpus, args.binary, audit_dir=audit) as (base, opener):
                page = browser.new_page(viewport={'width': 1280, 'height': 900})
                posts = []
                page.on('request', lambda request: posts.append(time.monotonic())
                        if request.url.endswith('/api/audit/browser') else None)
                page.goto(base)
                page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
                until(page, lambda: any(r.get('event') == 'browser.session.opened' for r in rows(audit)))
                assert not old.exists(), 'expired segment survived startup'
                assert [r['event'] for r in map(json.loads, mixed.read_text().splitlines())] == ['fixture.-23.99']
                assert recent.exists() and protected.read_bytes() == protected_bytes
                assert not crash.exists() and fresh_crash.exists(), 'owned crash temporary age policy'
                assert [r['event'] for r in map(json.loads, corrupt.read_text().splitlines())] == ['recent-in-corrupt'], 'malformed owned bytes must expire'
                scanned = get_json(opener, base, '/api/health')['audit']['retention_scanned_bytes']
                assert scanned >= large_size
                receipt = next(r for r in rows(audit) if r.get('event') == 'browser.session.opened')
                assert receipt['page_id'] and receipt['received_at']
                print('scenario complete', flush=True); evidence['scenarios'].append('startup: old removed, mixed numbered segment filtered, recent/protected retained; real row click persisted')
                page.screenshot(path=str(args.output / 'wide.png'))
                # Input only. Capture response cost and volume for repeated user navigation.
                latencies = []
                start = time.monotonic()
                for i in range(12):
                    uid = corpus.uid('claude-branch' if i % 2 else 'codex-fork')
                    tick = time.monotonic()
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator('#msgs')).to_contain_text('Claude selected answer' if i % 2 else 'Codex fork answer')
                    latencies.append((time.monotonic() - tick) * 1000)
                page.wait_for_timeout(5500)
                evidence['metrics']['navigation'] = {'n': 12, 'p50_ms': sorted(latencies)[6],
                    'p95_ms': max(latencies), 'elapsed_s': time.monotonic()-start, 'audit_posts': len(posts)}
                # Seed after startup: cleanup must run with no later navigation/write required.
                idle_old = audit / 'browser-2000-01-04.jsonl'
                seed(idle_old, -26)
                before = len(rows(audit)); byte_before = sum(p.stat().st_size for p in audit.glob('browser-*.jsonl'))
                tick = time.monotonic()
                until(page, lambda: not idle_old.exists(), seconds=70)
                elapsed = time.monotonic() - tick
                scanned_after = get_json(opener, base, '/api/health')['audit']['retention_scanned_bytes']
                assert scanned_after - scanned < large_size / 10, 'recent immutable volume reparsed on idle tick'
                evidence['metrics']['retention_volume'] = {'fixture_bytes': large_size, 'records': 140000,
                    'initial_scanned_bytes': scanned, 'idle_additional_scanned_bytes': scanned_after - scanned}
                if os.environ.get('SESSIONDOCK_TEST_EXPECT_BROWSER_FAILURE') == '1':
                    expect(page.locator('#msgs')).to_contain_text('INTENTIONALLY ABSENT runner failure proof', timeout=1000)
                evidence['metrics']['idle'] = {'seconds': elapsed, 'record_delta': len(rows(audit))-before,
                    'byte_delta': sum(p.stat().st_size for p in audit.glob('browser-*.jsonl'))-byte_before}
                print('scenario complete', flush=True); evidence['scenarios'].append('idle cleanup within 70 seconds without rotation')
                # Fail the cleanup temporary sink; original must survive, UI remains usable.
                failed = audit / 'browser-2000-01-05.jsonl'
                seed(failed, -26, -1)
                blocker = failed.with_suffix('.jsonl.retention-tmp'); blocker.mkdir()
                failures = get_json(opener, base, '/api/health')['audit']['write_errors']
                page.reload()
                page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
                until(page, lambda: get_json(opener, base, '/api/health')['audit']['write_errors'] > failures, seconds=70)
                assert len(failed.read_text().splitlines()) == 2
                blocker.rmdir()
                print('scenario complete', flush=True); evidence['scenarios'].append('cleanup failure observable, original preserved, navigation succeeds')
                page.set_viewport_size({'width': 390, 'height': 844})
                page.screenshot(path=str(args.output / 'narrow.png'))
                page.close()
            # Restart must clean failed/old data before any new audit batch.
            with isolated_server(corpus, args.binary, audit_dir=audit) as (base, opener):
                page = browser.new_page()
                page.goto(base)
                page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
                until(page, lambda: len(failed.read_text().splitlines()) == 1)
                print('scenario complete', flush=True); evidence['scenarios'].append('restart retries previous cleanup failure')
                # Rename only private fixture log directory; existing file handle survives,
                # subsequent rotate/reopen fails on a file in place of the directory.
                moved = corpus.root / 'audit-saved'; audit.rename(moved); audit.write_text('blocked')
                page.reload()
                page.locator(f'#side .item[data-uid="{corpus.uid("codex-fork")}"]').click()
                expect(page.locator('#msgs')).to_be_visible()
                # Let the idle worker close its old append handle before the next
                # actual navigation produces a batch for the unavailable sink.
                page.wait_for_timeout(61000)
                page.reload()
                page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
                until(page, lambda: get_json(opener, base, '/api/health')['audit']['write_errors'] > 0)
                print('scenario complete', flush=True); evidence['scenarios'].append('diagnostic sink failure does not prevent page reload and selection')
                page.close()
            with isolated_server(corpus, args.binary) as (base, opener):
                page = browser.new_page(viewport={'width': 1280, 'height': 900})
                page.goto(base)
                latencies = []
                for i in range(12):
                    tick = time.monotonic()
                    uid = corpus.uid('claude-branch' if i % 2 else 'codex-fork')
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator('#msgs')).to_contain_text('Claude selected answer' if i % 2 else 'Codex fork answer')
                    latencies.append((time.monotonic() - tick) * 1000)
                evidence['metrics']['navigation_audit_disabled'] = {
                    'n': 12, 'p50_ms': sorted(latencies)[6], 'p95_ms': max(latencies)}
                page.close()
            assert all(p.read_bytes() == data for p, data in native.items()), 'native fixture changed'
            assert (moved / protected.name).read_bytes() == protected_bytes
            print('scenario complete', flush=True); evidence['scenarios'].append('native session and saved evidence unchanged')
        finally:
            browser.close()
    (args.output / 'result.json').write_text(json.dumps(evidence, indent=2) + '\n')
    print(json.dumps(evidence, indent=2), flush=True)


if __name__ == '__main__':
    from browser_evidence_fixtures import run_with_evidence
    run_with_evidence(main, 'target/diagnostic-retention')
