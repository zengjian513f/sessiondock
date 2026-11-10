#!/usr/bin/env python3
"""Measure real UI responsiveness during slow/offline/buffered Codex transports.

Private Node/Hub, fake app-server, real SSE and Chromium. No production state.
"""
# run_validation: serial
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_queue_browser import QueueServer
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from network_fault_fixtures import PausedTransport
from node_auth_fixtures import TOKEN, node_env
from sdk_ui_fixtures import start_sdk


MONITOR = """() => {
  window.stallMetrics = {timer_gap_ms: 0, frames: [], clicks: 0};
  let last = performance.now();
  window.stallTimer = setInterval(() => {
    const now = performance.now();
    stallMetrics.timer_gap_ms = Math.max(stallMetrics.timer_gap_ms, now - last);
    last = now;
  }, 50);
  window.stallObserver = new PerformanceObserver(list => {
    for (const entry of list.getEntries()) {
      stallMetrics.frames.push({duration: entry.duration, blocking: entry.blockingDuration});
      if (stallMetrics.frames.length > 100) stallMetrics.frames.shift();
    }
  });
  stallObserver.observe({type: 'long-animation-frame'});
  document.addEventListener('click', () => stallMetrics.clicks++);
}"""


def exercise(browser, base, fake, label, width, theme, output):
    with PausedTransport(base) as transport:
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
        page = context.new_page()
        page.set_default_timeout(5000)
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.goto(transport.url, wait_until='networkidle')
        page.locator('#side .item').filter(has_text='earlier synthetic question').first.click()
        start_sdk(page)
        page.wait_for_function('CodexSdkView.connected')
        box = page.locator('#sdk-input')
        box.fill('network stress ' + label)
        page.locator('#sdk-send').click()
        expect(box).to_have_value('')
        # Clearing the composer acknowledges queueing, before the fake native
        # peer necessarily receives turn/start. Observe acceptance first.
        fake.wait_for(lambda: fake.methods('turn/start'))
        starts = len(fake.methods('turn/start'))
        assert starts == 1
        page.evaluate(MONITOR)
        cdp = context.new_cdp_session(page)
        cdp.send('Network.enable')
        results = []

        def emit(method, **raw):
            asyncio.run_coroutine_threadsafe(fake.notify(method, {
                'threadId': THREAD, 'turnId': fake.turn, **raw}), fake.loop).result(10)

        def interact(seconds):
            end = time.monotonic() + seconds
            samples = []
            while time.monotonic() < end:
                before = time.monotonic()
                if not page.locator('#a-turns').is_visible():
                    page.locator('#a-more').click()
                page.locator('#a-turns').click()
                box.click()
                box.fill('unsent network draft ' + label)
                box.press('End')
                box.press('x')
                expect(box).to_have_value('unsent network draft ' + label + 'x')
                samples.append(round((time.monotonic() - before) * 1000))
                page.wait_for_timeout(1000)
            return max(samples)

        def record(phase, interaction_ms):
            page.wait_for_timeout(100)
            metrics = page.evaluate('stallMetrics')
            row = {'target': label, 'phase': phase, 'interaction_ms': interaction_ms,
                   'timer_gap_ms': round(metrics['timer_gap_ms']),
                   'longest_frame_ms': round(max((f['duration'] for f in metrics['frames']), default=0)),
                   'clicks': metrics['clicks']}
            results.append(row)
            print(json.dumps(row), flush=True)
            (output / f'{label}-metrics.json').write_text(json.dumps(results, indent=2))
            assert row['timer_gap_ms'] < 2000 and row['longest_frame_ms'] < 2000, row
            assert not errors, errors
            page.evaluate('stallMetrics.timer_gap_ms=0; stallMetrics.frames=[]; stallMetrics.clicks=0')

        try:
            # Real browser transport failure, while the user keeps editing.
            context.set_offline(True)
            transport.cut()
            expect(page.locator('#network-status')).to_contain_text('网络已断开')
            record('offline-30s', interact(30))
            context.set_offline(False)
            page.wait_for_function('CodexSdkView.connected', timeout=20000)
            expect(page.locator('#network-status')).to_have_count(0, timeout=20000)

            # Slow responses without replacing fetch or application deadlines.
            cdp.send('Network.emulateNetworkConditions', {'offline': False, 'latency': 2000,
                'downloadThroughput': 16 * 1024, 'uploadThroughput': 8 * 1024})
            page.evaluate("""() => {
              window.slowProbe = null; const start = performance.now();
              fetch('api/health?network-probe=1').then(r => r.text()).then(() => {
                window.slowProbe = performance.now() - start;
              });
            }""")
            record('2s-latency-16KiBps', interact(15))
            page.wait_for_function('slowProbe !== null', timeout=15000)
            assert page.evaluate('slowProbe') >= 1500, 'network throttle was not effective'
            cdp.send('Network.emulateNetworkConditions', {'offline': False, 'latency': 0,
                'downloadThroughput': -1, 'uploadThroughput': -1})

            # Hold established TCP response bytes while the native peer keeps
            # producing real events, then flush them at full speed.
            emit('item/started', item={'id': 'buffered-answer', 'type': 'agentMessage', 'text': 'BUFFER BEGIN\n'})
            expect(page.locator('[data-sdk-item="buffered-answer"]')).to_be_attached()
            transport.paused = True
            async def burst():
                for i in range(120):
                    await fake.notify('item/started', {'threadId': THREAD, 'turnId': fake.turn,
                        'item': {'id': f'buffered-tool-{i}', 'type': 'commandExecution',
                                 'command': f'network-check-{i}', 'status': 'completed',
                                 'aggregatedOutput': ('synthetic tool output\n' * 300)}})
                for i in range(2400):
                    await fake.notify('item/agentMessage/delta', {'threadId': THREAD,
                        'turnId': fake.turn, 'itemId': 'buffered-answer', 'delta': f'{i:04d} buffered text\n'})
                await fake.notify('item/started', {'threadId': THREAD, 'turnId': fake.turn,
                    'item': {'id': 'buffered-end', 'type': 'agentMessage', 'text': 'BUFFERED END'}})
            asyncio.run_coroutine_threadsafe(burst(), fake.loop).result(30)
            assert transport.held_bytes > 0, 'downstream transport did not pause'
            assert page.locator('[data-sdk-item="buffered-end"]').count() == 0
            record('tcp-stall-30s', interact(30))
            cdp.send('Profiler.enable')
            cdp.send('Profiler.start')
            transport.paused = False
            interaction_ms = interact(10)
            expect(page.locator('[data-sdk-item="buffered-end"]')).to_contain_text('BUFFERED END', timeout=20000)
            expect(page.locator('[data-sdk-item="buffered-answer"] .mb')).to_have_text(
                'BUFFER BEGIN\n' + ''.join(f'{i:04d} buffered text\n' for i in range(2400)), timeout=20000)
            (output / f'{label}-release-profile.json').write_text(json.dumps(cdp.send('Profiler.stop')['profile']))
            record('release-120-tools-2400-deltas', interaction_ms)

            # Repeatedly lose established streams, without reloading the page.
            for _ in range(5):
                transport.cut()
                interact(2)
            page.wait_for_function('CodexSdkView.connected', timeout=20000)
            record('five-connection-resets', interact(5))
            expect(box).to_have_value('unsent network draft ' + label + 'x')
            assert len(fake.methods('turn/start')) == starts, ('recovery replayed SEND', starts, len(fake.methods('turn/start')))
            page.screenshot(path=str(output / f'{label}-recovered.png'), full_page=True)
            print(f'PASS {label}: faults effective; UI usable; complete output and draft; no duplicate SEND', flush=True)
        finally:
            transport.paused = False
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, required=True)
    parser.add_argument('--targets', nargs='+', choices=('node', 'hub'), default=['node', 'hub'])
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for label, width, theme in [('node', 1280, 'light'), ('hub', 390, 'dark')]:
                if label not in args.targets: continue
                with tempfile.TemporaryDirectory(prefix='sd-network-stall-') as tmp:
                    root = Path(tmp)
                    data = build(root)
                    env = environment(root, args.binary)
                    node_port = free_port()
                    env.update(node_env(root, node_port, '127.0.0.0/8'))
                    with QueueServer(root/'app.sock', data.paths['sdk'], THREAD) as fake, \
                            server(data, args.binary, root/'app.sock', env) as base:
                        hub = None
                        try:
                            if label == 'hub':
                                hubroot = root/'hub'; hubroot.mkdir()
                                hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot,
                                    [SimpleNamespace(name='NetworkNode', port=node_port, token=TOKEN)])
                                hub.start()
                                base = f'http://127.0.0.1:{hub.port}'
                            exercise(browser, base, fake, label, width, theme, args.screenshots)
                        finally:
                            if hub: hub.stop()
        finally:
            browser.close()


if __name__ == '__main__':
    main()
