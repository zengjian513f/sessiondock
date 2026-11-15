#!/usr/bin/env python3
"""Changing node signatures must replace snapshots, not retain old full lists."""
from __future__ import annotations
import argparse
import json
import os
import time
import threading
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright, expect
from hub_fixtures import REPO, FakeNode, Hub


def rss(pid):
    fields = dict(line.split(':', 1) for line in Path(f'/proc/{pid}/status').read_text().splitlines())
    return int(fields['VmRSS'].split()[0]) * 1024


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default=str(REPO / 'target/release/sessiondock'))
    parser.add_argument('--hub-binary')
    args = parser.parse_args()
    binary = Path(args.hub_binary) if args.hub_binary else Path(args.binary).resolve().with_name('sessiondock-hub')
    node = FakeNode('a' * 32, 'Cache')
    try:
        row = node.state()['row']
        node.set(pad=2 * 1024 * 1024)
        with tempfile.TemporaryDirectory(prefix='sessiondock-cache-browser-') as temp:
            hub = Hub(binary, Path(temp), [node])
            # Match a small Hub host; hundreds of workers on a build server
            # otherwise measure allocator warm-up across cores, not retention.
            hub.env['TOKIO_WORKER_THREADS'] = '4'
            interval=5
            hub.env['SESSIONDOCK_HUB_SNAPSHOT_INTERVAL_SECS']=str(interval)
            snapshot=Path(temp)/'hub-cache'/f'{node.nid}.sessions.json'
            changes=[]
            stop=threading.Event()
            def observe():
                previous=None
                while not stop.wait(0.01):
                    try:
                        stat=snapshot.stat();current=(stat.st_ino,stat.st_mtime_ns)
                    except FileNotFoundError:continue
                    if current!=previous:
                        changes.append(time.monotonic());previous=current
            observer=threading.Thread(target=observe);observer.start()
            hub.start()
            try:
                with sync_playwright() as pw:
                    launch = {'headless': True}
                    if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                        launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                    browser = pw.chromium.launch(**launch)
                    page = browser.new_page(service_workers='block')
                    page.goto(f'http://127.0.0.1:{hub.port}/')
                    page.locator('#nest-flat').click()
                    expect(page.locator('#side .item')).to_have_count(1)
                    baseline = None
                    for revision in range(90):
                        title = f'Cache revision {revision}'
                        node.set(row={**row, 'title': title})
                        # A real reload exercises the page -> hub -> conditional node poll.
                        page.reload()
                        expect(page.locator('#side .item')).to_contain_text(title)
                        if revision == 39:
                            baseline = rss(hub.process.pid)
                        if revision % 10 == 9:
                            print(f'revision={revision} rss_mib={rss(hub.process.pid) / 2**20:.1f}', flush=True)
                    growth = rss(hub.process.pid) - baseline
                    assert growth < 64 * 2**20, f'old snapshots retained: RSS grew {growth / 2**20:.1f} MiB'
                    def saved_title():
                        try:return json.loads(snapshot.read_text())['data']['sessions'][0]['title']
                        except (FileNotFoundError,KeyError,IndexError):return None
                    deadline=time.monotonic()+interval+3
                    while saved_title()!=title and time.monotonic()<deadline:
                        page.wait_for_timeout(50)
                    assert saved_title()==title
                    assert len(changes)>=2, changes
                    assert all(b-a>=interval-0.15 for a,b in zip(changes,changes[1:])), changes
                    print(f'periodic snapshots: {len(changes)}, minimum spacing={min(b-a for a,b in zip(changes,changes[1:])):.3f}s',flush=True)
                    assert snapshot.stat().st_mode & 0o777 == 0o600
                    # An unchanged refresh must not rewrite the persisted snapshot.
                    modified = snapshot.stat().st_mtime_ns
                    page.reload()
                    expect(page.locator('#side .item')).to_contain_text(title)
                    assert snapshot.stat().st_mtime_ns == modified
                    page.wait_for_timeout((interval+1)*1000)
                    assert snapshot.stat().st_mtime_ns == modified
                    # Offline transition must flush the new pending list before
                    # any graceful shutdown can hide a missing transition flush.
                    node.set(row={**row,'title':'Saved before outage'})
                    page.reload();expect(page.locator('#side .item')).to_contain_text('Saved before outage')
                    deadline=time.monotonic()+interval+3
                    while saved_title()!='Saved before outage' and time.monotonic()<deadline:
                        page.wait_for_timeout(20)
                    assert saved_title()=='Saved before outage'
                    title='Latest before node outage'
                    node.set(row={**row,'title':title})
                    page.reload();expect(page.locator('#side .item')).to_contain_text(title)
                    assert saved_title()!=title, 'fixture needs a pending snapshot'
                    node.set(offline=True)
                    deadline=time.monotonic()+15
                    while saved_title()!=title and time.monotonic()<deadline:
                        page.reload();page.wait_for_timeout(100)
                    assert saved_title()==title
                    hub.process.kill();hub.process.wait()
                    hub.start()
                    page.reload()
                    expect(page.locator('#side .item')).to_contain_text(title)
                    # Filtering still works on the offline snapshot loaded after restart.
                    page.locator('#q').fill(title)
                    expect(page.locator('#side .item')).to_have_count(1)
                    # Recover online, establish a saved baseline, then stop
                    # before the next periodic write. SIGTERM must flush latest.
                    page.locator('#q').fill('')
                    node.set(offline=False,row={**row,'title':'Online again'})
                    deadline=time.monotonic()+20
                    while saved_title()!='Online again' and time.monotonic()<deadline:
                        page.reload();page.wait_for_timeout(100)
                    assert saved_title()=='Online again'
                    title='Latest before graceful shutdown'
                    node.set(row={**row,'title':title})
                    page.reload();expect(page.locator('#side .item')).to_contain_text(title)
                    assert saved_title()!=title
                    hub.stop()
                    assert hub.process.returncode==0, 'shutdown did not complete gracefully'
                    assert saved_title()==title
                    node.set(offline=True);hub.start();page.reload()
                    expect(page.locator('#side .item')).to_contain_text(title)
                    browser.close()
            finally:
                hub.stop()
                stop.set();observer.join()
    finally:
        node.stop()
    print('PASS hub_cache_browser: bounded RSS, latest list, unchanged disk snapshot, offline restart', flush=True)


if __name__ == '__main__':
    main()
