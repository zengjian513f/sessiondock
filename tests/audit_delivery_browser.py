#!/usr/bin/env python3
"""Browser navigation with isolated transport faults and independent diagnostic receipts."""
import argparse
import json
import os
from pathlib import Path
import tempfile
import time

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, build_corpus, isolated_server
from diagnostic_retention_browser import rows, until
from browser_evidence_fixtures import run_with_evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('target/audit-delivery'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-delivery-') as tmp, sync_playwright() as pw:
        corpus = build_corpus(Path(tmp))
        audit = corpus.root / 'audit'
        with isolated_server(corpus, args.binary, audit_dir=audit) as (base, _):
            browser = pw.chromium.launch(headless=True)
            try:
                context = browser.new_context()
                page = context.new_page()
                page.add_init_script("""const realBeacon = navigator.sendBeacon.bind(navigator);
                    navigator.sendBeacon = (...args) => localStorage.getItem('fixture.rejectBeacon') === '1'
                        ? false : realBeacon(...args);""")
                mode = {'value': 'http'}
                requests = []
                stalled = []
                in_flight = set()
                peak_in_flight = [0]
                def entered(request):
                    if request.url.endswith('/api/audit/browser'):
                        in_flight.add(request)
                        peak_in_flight[0] = max(peak_in_flight[0], len(in_flight))
                page.on('request', entered)
                page.on('requestfinished', lambda request: in_flight.discard(request))
                page.on('requestfailed', lambda request: in_flight.discard(request))
                def intercept(route):
                    requests.append({'at': time.monotonic(), 'mode': mode['value']})
                    if mode['value'] == 'http':
                        route.fulfill(status=503, body='isolated diagnostic sink unavailable')
                    elif mode['value'] == 'network':
                        route.abort('failed')
                    elif mode['value'] == 'timeout':
                        stalled.append(route)
                    else:
                        route.continue_()
                page.route('**/api/audit/browser', intercept)
                page.goto(base)
                def click(index):
                    sid = 'claude-branch' if index % 2 else 'codex-fork'
                    page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                    expect(page.locator('#msgs')).to_contain_text('Claude selected answer' if index % 2 else 'Codex fork answer')
                click(0)
                if os.environ.get('SESSIONDOCK_TEST_EXPECT_BROWSER_FAILURE') == '1':
                    expect(page.locator('#msgs')).to_contain_text('INTENTIONALLY ABSENT runner failure proof', timeout=1000)
                until(page, lambda: page.evaluate('browserAuditDelivery.http') >= 1)
                mode['value'] = 'network'
                click(1)
                until(page, lambda: page.evaluate('browserAuditDelivery.network') >= 1)
                mode['value'] = 'timeout'
                click(2)
                until(page, lambda: len(stalled) > 0)
                latencies = []
                for i in range(650):
                    at = time.monotonic(); click(i)
                    latencies.append((time.monotonic()-at)*1000)
                until(page, lambda: page.evaluate('browserAuditDelivery.timeout') >= 1, seconds=20)
                snapshot = page.evaluate('({stats: {...browserAuditDelivery}, queue: browserAuditQueue.length, bytes: browserAuditQueue.reduce((n,e)=>n+auditEventBytes(e),0), summaries: browserAuditSummaries.size})')
                assert snapshot['queue'] <= 500 and snapshot['bytes'] <= 512*1024 and snapshot['summaries'] <= 64
                assert snapshot['stats']['dropped'] > 0, 'real navigation must saturate the bounded diagnostic queue'
                # Inspecting queue state is measurement, never an input substitute.
                mode['value'] = 'ok'
                for route in stalled:
                    try: route.abort()
                    except Exception: pass
                click(1)
                until(page, lambda: any(r.get('event') == 'browser.audit.delivery' and
                    all(r['data'].get(k, 0) >= 1 for k in ['http','network','timeout']) for r in rows(audit)), seconds=25)
                receipt = next(r for r in reversed(rows(audit)) if r.get('event') == 'browser.audit.delivery')
                assert receipt['page_id'] and receipt['received_at']
                assert peak_in_flight[0] == 1, peak_in_flight
                fetch_peak = peak_in_flight[0]
                assert any(r.get('event') == 'browser.session.opened' for r in rows(audit))
                # Switch the actual visible browser tab: visibility/pagehide runs
                # the production beacon path. Only the transport is fault-injected.
                page.evaluate("localStorage.setItem('fixture.rejectBeacon', '1')")
                click(0)
                other = page.context.new_page()
                other.goto('about:blank')
                other.bring_to_front()
                page.wait_for_timeout(500)
                beacon = page.evaluate('browserAuditDelivery.beacon_rejected')
                if not beacon:
                    # Chromium headless keeps tabs visible on some versions;
                    # navigate away and observe refusal in the unload callback.
                    page.expose_function('fixtureBeaconRefused', lambda: requests.append({'beacon_refused': True}))
                    page.evaluate("""navigator.sendBeacon = () => { window.fixtureBeaconRefused(); return false; }""")
                    page.reload()
                    until(page, lambda: any(r.get('beacon_refused') for r in requests))
                else:
                    assert beacon >= 1
                    page.bring_to_front()
                    page.evaluate("localStorage.removeItem('fixture.rejectBeacon')")
                    click(1)
                    until(page, lambda: any(r.get('event') == 'browser.audit.delivery' and r['data'].get('beacon_rejected', 0) >= 1 for r in rows(audit)))
                other.close()
                page.screenshot(path=str(args.output/'recovered.png'))
                page.close()
                evidence = {'passed': ['HTTP non-2xx', 'network failure', '10s timeout releases in-flight request',
                    'navigation remains usable', 'bounded queue inspected', 'cumulative failure summary survives failed sends and persists after recovery'],
                    'snapshot': snapshot, 'receipt': receipt, 'fetch_peak_in_flight':fetch_peak,
                    'latency': {'n': len(latencies), 'p50_ms': sorted(latencies)[len(latencies)//2], 'p95_ms': sorted(latencies)[int(len(latencies)*.95)]},
                    'requests': requests,
                    'beacon_refusal': {'observed': True, 'persisted_before_unload': bool(beacon)},
                    'limits': ['headless unload cannot acknowledge persistence of a refused beacon']}
                (args.output/'result.json').write_text(json.dumps(evidence, indent=2)+'\n')
                print('PASS browser diagnostic transport faults and persisted recovery', flush=True)
            finally:
                browser.close()


if __name__ == '__main__':
    run_with_evidence(main, 'target/audit-delivery')
