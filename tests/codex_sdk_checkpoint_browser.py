#!/usr/bin/env python3
"""Legacy Codex channel compaction and coalesced streaming through Chromium."""
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
import subprocess
import sys
import sqlite3
import os
import time

from playwright.sync_api import expect, sync_playwright
from codex_checkpoint_fixtures import CheckpointPeer, CheckpointWrites, legacy_outbox, seed_legacy_reads, owner_initializations
from codex_sdk_browser import THREAD, build, server
from codex_ledger_fixtures import seed_completed
from codex_sdk_actions_browser import environment
from history_fixtures import BINARY
from sdkhost_fixtures import stop_private_sdkhosts
from sdkhost_harness_browser import hosts, open_sid, send


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sdkhost', type=Path)
    parser.add_argument('--sustained', action='store_true')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sd-codex-checkpoint-') as tmp, sync_playwright() as pw:
        root = Path(tmp); corpus = build(root); env = environment(root, args.binary)
        seed_completed(root, THREAD)
        if args.sdkhost: env['SESSIONDOCK_SDKHOST_BIN'] = str(args.sdkhost.resolve())
        browser = pw.chromium.launch(headless=True)
        page = browser.new_page(); page.set_default_timeout(30000)
        try:
            with CheckpointPeer(root/'native.sock', corpus.paths['sdk'], THREAD) as native:
                with server(corpus, args.binary, root/'native.sock', env) as base:
                    uid = open_sid(page, base, THREAD)
                    before_sends = len(native.methods('turn/start'))
                    receipt = page.evaluate("""async uid => {const r=await fetch("/api/codex-sdk/turn",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({uid,text:"must not replay old input",request_id:"legacy-complete-0"})});return {ok:r.ok,body:await r.json()}}""", uid)
                    assert receipt['ok'] and receipt['body']['raw']['state'] == 'sent', receipt
                    assert len(native.methods('turn/start')) == before_sends
                    with sqlite3.connect(root/'state/codex-sdk-queue.sqlite') as db:
                        assert db.execute('SELECT count(*) FROM records WHERE active=0').fetchone()[0] == 5000
                    send(page, 'approve checkpoint recovery')
                    page.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
                    expect(page.locator('.sdk-live')).to_contain_text('All done after approval')
                    page.wait_for_function('!CodexSdkView.turn')
                    checkpoint, = (root/'state/sdk-adapters/codex-channels').rglob('channel.json')
                    page.wait_for_timeout(200)
                    counter = CheckpointWrites(checkpoint)
                    channel_dir, = (root/'state/sdkhost').glob('codex-channel-*')
                    before_output = (channel_dir/'stdout').stat().st_size
                    before_receipts = (channel_dir/'receipts.jsonl').stat().st_size
                    page.evaluate('''async uid => {
                        for (let batch=0; batch<25; batch++) {
                            await Promise.all(Array.from({length:20}, async (_,i) => {
                                const r = await fetch('/api/codex-sdk/turns', {
                                    method:'POST', headers:{'Content-Type':'application/json'},
                                    body:JSON.stringify({uid,cursor:`${batch}-${i}-`+'c'.repeat(8192)})});
                                if (!r.ok) throw new Error(await r.text());
                            }));
                        }
                    }''', uid)
                    assert counter.count() == 0, 'read-only polling rewrote the durable checkpoint'
                    counter.close()
                    assert (channel_dir/'stdout').stat().st_size == before_output
                    assert (channel_dir/'receipts.jsonl').stat().st_size == before_receipts
                    original = hosts(root, 'codex-channel-')
                    initialized = len(owner_initializations(native))
                    turns = len(native.methods('turn/start'))
                    replies = len(native.responses())
                checkpoint, = (root/'state/sdk-adapters/codex-channels').rglob('channel.json')
                # Exercise rollback export and one-time JSON reimport on the
                # same private state before attaching the existing native host.
                subprocess.run([sys.executable, str(Path(__file__).resolve().parents[1]/'deploy/storage_rollback.py'), str(root/'state')], check=True)
                assert (root/'state/codex-sdk-queue.json').is_file()
                seed_legacy_reads(root, native)
                record = json.loads(checkpoint.read_text())
                record['outbox'] = legacy_outbox(native.received)
                prior_id = max(f['id'] for f in native.received if isinstance(f.get('id'), int) and 'method' in f)
                checkpoint.write_text(json.dumps(record))
                assert checkpoint.stat().st_size > 4_000_000
                prior_frames = len(native.received)
                with server(corpus, args.binary, root/'native.sock', env) as base:
                    open_sid(page, base, THREAD)
                    page.wait_for_function('CodexSdkView.connected')
                    deadline = time.monotonic() + 15
                    while time.monotonic() < deadline:
                        compact = json.loads(checkpoint.read_text())
                        if len(compact['outbox']) <= 3:
                            break
                        page.wait_for_timeout(50)
                    assert len(compact['outbox']) <= 3, len(compact['outbox'])
                    assert {k:v['pid'] for k,v in hosts(root, 'codex-channel-').items()} == {k:v['pid'] for k,v in original.items()}
                    assert len(owner_initializations(native)) == initialized
                    assert len(native.methods('turn/start')) == turns
                    assert len(native.responses()) == replies
                    new_ids = [f['id'] for f in native.received[prior_frames:] if isinstance(f.get('id'), int) and 'method' in f]
                    assert new_ids and min(new_ids) > prior_id and len(new_ids) == len(set(new_ids)), new_ids
                    send(page, 'stream checkpoint pressure')
                    page.wait_for_function('CodexSdkView.turn?.status === "inProgress"')
                    counter = CheckpointWrites(checkpoint)
                    started = time.monotonic()
                    try:
                        burst = asyncio.run_coroutine_threadsafe(native.burst(12000), native.loop)
                        expect(page.locator('.sdk-live')).to_contain_text('checkpoint burst finished', timeout=20000)
                        burst.result(5)
                        text = page.locator('.sdk-live').inner_text()
                        assert 'x' * 12000 + ' checkpoint burst finished' in text
                        writes = counter.count()
                    finally:
                        counter.close()
                    assert writes < 600, f'{writes} full snapshots for 12000 deltas'
                    live = page.evaluate('fetch("/api/live?force=1").then(r=>r.json())')
                    assert uid in live['sdk_uids'], live
                    assert len(owner_initializations(native)) == initialized
                    assert len(native.methods('turn/start')) == turns + 1
                    burst_seconds = time.monotonic()-started
                    large = asyncio.run_coroutine_threadsafe(native.large_frame(), native.loop)
                    expect(page.locator('.sdk-live')).to_contain_text('large-frame-finished', timeout=30000)
                    large.result(5)
                    if not args.sdkhost:
                        assert list(channel_dir.glob('stdout.parts/*.gz')), 'active output prefix was not compressed'
                        assert sum(p.stat().st_size for p in (channel_dir/'stdout.parts').iterdir()) < 8*1024*1024 + 256*1024
                    asyncio.run_coroutine_threadsafe(native.disconnect_queries(), native.loop).result(5)
                    page.wait_for_timeout(200)
                    result = page.evaluate("""async uid => {const r=await fetch("/api/codex-sdk/turns",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({uid})}); return {ok:r.ok,body:await r.json()}}""", uid)
                    assert result['ok'], result
                    assert page.evaluate('CodexSdkView.connected')
                    assert len(owner_initializations(native)) == initialized
                    if args.sustained:
                        query_initializations = len(native.methods('initialize'))
                        page.evaluate("""async uid => {const pending=[];const end=performance.now()+125000;let n=0;while(performance.now()<end){pending.push(fetch("/api/codex-sdk/turns",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({uid,cursor:"sustained-"+(n++)})}).then(async r=>{if(!r.ok)throw new Error(await r.text());}));await new Promise(r=>setTimeout(r,750));}await Promise.all(pending)}""",uid)
                        assert len(native.methods('initialize')) == query_initializations
                        assert page.evaluate('CodexSdkView.connected')
                        print('PASS continuously overlapping queries beyond 120 seconds keep the same connection',flush=True)
                    page.reload(wait_until='domcontentloaded')
                    page.wait_for_function('CodexSdkView.connected')
                    assert len(native.methods('turn/start')) == turns + 1
                    print(f'PASS Chromium: legacy 4 MB ledger retired, same host, monotonic RPC IDs, no input/approval replay; 12000 deltas in {burst_seconds:.2f}s, {writes} checkpoint writes, live status retained; 5000 completed receipts migrated/rollback exported without replay; 10 MB frame and query-only reconnect', flush=True)
            if not args.sdkhost:
                # A dead host's segments must remain readable after rollback,
                # without modifying a still-running host's segment files.
                abandoned = root/'state/sdkhost/abandoned'
                abandoned.mkdir(mode=0o700)
                (abandoned/'host.pid').write_text(str(os.getpid()))
                # The native peer has now exited; its output is a single gzip.
                deadline = time.monotonic()+10
                while not (channel_dir/'stdout.gz').exists() and time.monotonic()<deadline:
                    time.sleep(.05)
                import gzip
                raw = gzip.decompress((channel_dir/'stdout.gz').read_bytes())
                parts = abandoned/'stdout.parts'; parts.mkdir(mode=0o700)
                (parts/'00000000000000000000.gz').write_bytes(gzip.compress(raw[:8*1024*1024]))
                (parts/f'{8*1024*1024:020}.raw').write_bytes(raw[8*1024*1024:])
                subprocess.run([sys.executable,str(Path(__file__).resolve().parents[1]/'deploy/storage_rollback.py'),str(root/'state')],check=True)
                assert not (abandoned/'stdout').exists(), 'rollback touched an active host'
                (abandoned/'host.pid').write_text('2147483647')
                subprocess.run([sys.executable,str(Path(__file__).resolve().parents[1]/'deploy/storage_rollback.py'),str(root/'state')],check=True)
                assert (abandoned/'stdout').read_bytes() == raw
        finally:
            browser.close()
            stop_private_sdkhosts(root)


if __name__ == '__main__':
    main()
