#!/usr/bin/env python3
"""Legacy Codex channel compaction and coalesced streaming through Chromium."""
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
import time

from playwright.sync_api import expect, sync_playwright
from codex_checkpoint_fixtures import CheckpointPeer, CheckpointWrites, legacy_outbox
from codex_sdk_browser import THREAD, build, server
from codex_sdk_actions_browser import environment
from history_fixtures import BINARY
from sdkhost_fixtures import stop_private_sdkhosts
from sdkhost_harness_browser import hosts, open_sid, send


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sd-codex-checkpoint-') as tmp, sync_playwright() as pw:
        root = Path(tmp); corpus = build(root); env = environment(root, args.binary)
        browser = pw.chromium.launch(headless=True)
        page = browser.new_page(); page.set_default_timeout(30000)
        try:
            with CheckpointPeer(root/'native.sock', corpus.paths['sdk'], THREAD) as native:
                with server(corpus, args.binary, root/'native.sock', env) as base:
                    uid = open_sid(page, base, THREAD)
                    send(page, 'approve checkpoint recovery')
                    page.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
                    expect(page.locator('.sdk-live')).to_contain_text('All done after approval')
                    page.wait_for_function('!CodexSdkView.turn')
                    # Real read requests and real sdkhost receipts. Long opaque
                    # cursors make the historical checkpoint comparable to the incident.
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
                    original = hosts(root, 'codex-channel-')
                    initialized = len(native.methods('initialize'))
                    turns = len(native.methods('turn/start'))
                    replies = len(native.responses())
                checkpoint, = (root/'state/sdk-adapters/codex-channels').rglob('channel.json')
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
                    assert len(native.methods('initialize')) == initialized
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
                    assert len(native.methods('initialize')) == initialized
                    assert len(native.methods('turn/start')) == turns + 1
                    page.reload(wait_until='domcontentloaded')
                    page.wait_for_function('CodexSdkView.connected')
                    assert len(native.methods('turn/start')) == turns + 1
                    print(f'PASS Chromium: legacy 4 MB ledger retired, same host, monotonic RPC IDs, no input/approval replay; 12000 deltas in {time.monotonic()-started:.2f}s, {writes} checkpoint writes, live status retained', flush=True)
        finally:
            browser.close()
            stop_private_sdkhosts(root)


if __name__ == '__main__':
    main()
