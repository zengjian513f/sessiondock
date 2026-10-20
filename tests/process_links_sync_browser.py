#!/usr/bin/env python3
"""Chromium + real Hub/node: boot-long evidence crosses the wire only once."""
import argparse
from contextlib import ExitStack
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
from hub_fixtures import Hub
from node_auth_fixtures import node_env, TOKEN, free_port
from process_links_fixtures import counted_node, history_collector

NODE = 'a' * 32
COUNT = 140_000


def wait(page, condition):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        result = condition()
        if result:
            return result
        page.wait_for_timeout(100)
    raise AssertionError('coordinator did not reach expected transport state')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='process-sync-') as temporary, ExitStack() as stack:
        root = Path(temporary)
        corpus = Corpus(root / 'node')
        corpus.put('owner', 'codex', [codex_row('session_meta', dict(id='owner', cwd='/synthetic/sync')),
            codex_message('user', 'Connection history question'),
            codex_message('assistant', 'Connection history answer')], [])
        proc = corpus.root / 'proc'
        (proc / 'sys/kernel/random').mkdir(parents=True)
        (proc / 'sys/kernel/random/boot_id').write_text('sync-boot')
        (proc / 'stat').write_text('btime 1700000000\n')
        (corpus.root / 'ids').mkdir()
        (corpus.root / 'ids/node-id').write_text(NODE)
        owner = dict(node_id=NODE, source='codex', sid='owner', title='Retained synthetic launch ' * 8)
        records = [dict(process=dict(pid=1000 + i, start=100000 + i),
            connection=dict(client_ip='10.0.0.1', client_port=50000, server_ip='10.0.0.2', server_port=50022),
            opened_at=1700000000.123456 + i, closed_at=None, ssh=True, shared=False, session=owner,
            launch_chain=[]) for i in range(COUNT)]
        report = dict(version=1, node_id=NODE, boot_id='sync-boot', supported=True, sampled_at=1.,
            outgoing=[], incoming=[], bindings=[], connections=records,
            collector=dict(service='resource-agent', events='synthetic', lost_events=0, ssh_connections=True))
        collector = stack.enter_context(history_collector(corpus.root, report))
        node = SimpleNamespace(name='sync', nid=NODE, port=free_port(), token=TOKEN)
        env = node_env(corpus.root, node.port, '127.0.0.0/8')
        env.update(SESSIONDOCK_PROC_ROOT=str(proc),
                   SESSIONDOCK_RESOURCE_AGENT_SOCKET=str(corpus.root / 'collector.sock'))
        servers = []
        def start_node():
            server = isolated_server(corpus, args.binary, extra_env=env)
            result = server.__enter__()
            servers.append(server)
            return result[0]
        def stop_node():
            servers.pop().__exit__(None, None, None)
        base = start_node()
        stack.callback(lambda: stop_node() if servers else None)
        proxy, counts = stack.enter_context(counted_node(node))
        hubroot = root / 'hub'
        hubroot.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [proxy])
        hub.start()
        stack.callback(hub.stop)
        with sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            try:
                page = browser.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(f'http://127.0.0.1:{hub.port}', wait_until='domcontentloaded')
                page.locator('#side .item').first.click()
                expect(page.locator('#msgs')).to_contain_text('Connection history answer')
                wait(page, lambda: len(counts.reports) >= 3)
                first, *steady = counts.reports
                assert first['sync'] and first['base'] is None and first['connections'] == COUNT, first
                # Boot-long snapshots already exceed the ordinary 64 MiB
                # JSON API limit in production; they must establish a baseline.
                assert first['bytes'] > 64 * 1024 * 1024, first
                assert all(r['base'] and r['connections'] == 0 and r['bytes'] < 2000 for r in steady), steady
                print(f'Initial {first["bytes"]} bytes; unchanged {steady[-1]["bytes"]} bytes', flush=True)

                # The ordinary browser API stays a complete report. Unknown
                # cursors recover without changing or rejecting any evidence.
                page.goto(base, wait_until='domcontentloaded')
                page.locator('#side .item').first.click()
                expect(page.locator('#msgs')).to_contain_text('Connection history answer')
                full = page.evaluate("""async () => {
                    const plain=await (await fetch('api/process-links')).json();
                    const r=await fetch('api/process-links?connection_sync=1&since=unknown');
                    const v=await r.json(); window.syncRevision=v.revision;
                    return {plain:plain.connections.length, full:v.report.connections.length, base:v.base};
                }""")
                assert full == dict(plain=COUNT, full=COUNT, base=None), full
                changed = deepcopy(report)
                changed['sampled_at'] = 2.
                changed['connections'][0].update(closed_at=1700000025., shared=True, session=None)
                changed['connections'][1]['launch_chain'] = [
                    dict(process=dict(node_id=NODE, boot_id='sync-boot', process=dict(pid=9, start=90)),
                         session=owner)]
                del changed['connections'][3]
                new = deepcopy(records[-1])
                new['process'] = dict(pid=999999, start=999999)
                changed['connections'].append(new)
                changed['connections'].reverse()
                collector.replace(changed)
                delta = page.evaluate("""async () => {
                    const r=await fetch('api/process-links?connection_sync=1&since='+window.syncRevision);
                    const text=await r.text(), v=JSON.parse(text);
                    return {bytes:text.length, base:v.base, remove:v.remove.length,
                        rows:v.report.connections, time:v.report.sampled_at};
                }""")
                assert delta['base'] and delta['bytes'] < 5000 and delta['remove'] == 1, delta
                rows = {r['process']['pid']: r for r in delta['rows']}
                assert set(rows) == {1000, 1001, 999999}, rows
                assert rows[1000]['closed_at'] == 1700000025 and rows[1000]['shared'] and rows[1000]['session'] is None
                assert rows[1001]['launch_chain'][0]['process']['process']['pid'] == 9
                wait(page, lambda: any(r['sampled_at'] == 2 and r['base'] and r['connections'] == 3
                                       for r in counts.reports))
                print(f'Close/late attribution/add/remove/reorder: {delta["bytes"]} bytes', flush=True)

                before = len(counts.reports)
                collector.fail = True
                wait(page, lambda: any(not r['sync'] for r in counts.reports[before:]))
                before = len(counts.reports)
                collector.fail = False
                recovered = wait(page, lambda: next((r for r in counts.reports[before:] if r['sync']), None))
                assert recovered['base'] and recovered['connections'] == 0, recovered
                print('Collector outage recovery retained baseline', flush=True)

                before = len(counts.reports)
                counts.fail_gets = 1
                wait(page, lambda: counts.fail_gets == 0 and len(counts.reports) > before)
                assert counts.reports[-1]['base'] and counts.reports[-1]['bytes'] < 2000
                counts.legacy = True
                # Ordinary old-node replies still obey their original JSON
                # limit; compatibility uses a report within that limit.
                collector.replace(dict(changed, connections=changed['connections'][:1000]))
                wait(page, lambda: not counts.reports[-1]['sync'])
                assert counts.reports[-1]['connections'] == 1000
                collector.replace(changed)
                counts.legacy = False
                wait(page, lambda: counts.reports[-1]['sync'])
                assert counts.reports[-1]['base'], counts.reports[-1]
                print('Failed poll and old-node compatibility recovered', flush=True)

                before = len(counts.reports)
                counts.corrupt_base = True
                wait(page, lambda: not counts.corrupt_base)
                recovered = wait(page, lambda: next((r for r in counts.reports[before:]
                    if r['sync'] and r['base'] is None), None))
                assert recovered['connections'] == COUNT
                before = len(counts.reports)
                hub.stop()
                hub.start()
                restarted = wait(page, lambda: next(iter(counts.reports[before:]), None))
                assert restarted['base'] is None and restarted['connections'] == COUNT, restarted
                wait(page, lambda: counts.reports[-1]['base'])
                before = len(counts.reports)
                stop_node()
                base = start_node()
                node_restart = wait(page, lambda: next((r for r in counts.reports[before:]
                    if r['sync'] and r['base'] is None), None))
                assert node_restart['connections'] == COUNT
                wait(page, lambda: counts.reports[-1]['base'])
                before = len(counts.reports)
                stop_node()
                (proc / 'sys/kernel/random/boot_id').write_text('sync-next-boot')
                changed['boot_id'] = 'sync-next-boot'
                changed['connections'] = [new]
                collector.replace(changed)
                base = start_node()
                boot_restart = wait(page, lambda: next((r for r in counts.reports[before:]
                    if r['sync'] and r['boot'] == 'sync-next-boot'), None))
                assert boot_restart['base'] is None and boot_restart['connections'] == 1, boot_restart
                page.goto(base, wait_until='domcontentloaded')
                page.locator('#side .item').first.click()
                expect(page.locator('#msgs')).to_contain_text('Connection history answer')
                assert not errors, errors
                print('Invalid baseline, Hub/node restart and boot change safely resynchronized', flush=True)
            finally:
                browser.close()
    print('PASS process connection history incremental transport + Chromium navigation', flush=True)


if __name__ == '__main__':
    main()
