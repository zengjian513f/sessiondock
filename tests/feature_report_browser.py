#!/usr/bin/env python3
"""Feature requests retain attachments and hand off once to SDK workers, node and Hub."""
import argparse
import json
from conversation_storage_fixtures import read_document
import os
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit
from playwright.sync_api import expect, sync_playwright
from sdk_start_stop_fixtures import provider
from history_fixtures import BINARY, REPO, codex_row, encoded
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from bug_report_node_browser import open_report, wait_drafts


def exercise(browser, base, source, root, label, peer=None, reject=False):
    print(label, source, 'feature/report SDK handoff', flush=True)
    page = browser.new_page(viewport={'width': 1280, 'height': 900})
    page.set_default_timeout(20000)
    shots = REPO/'target/validation/report-feature'
    shots.mkdir(parents=True, exist_ok=True)
    errors = []
    page.on('pageerror', lambda e: errors.append(str(e)))
    page.goto(base, wait_until='networkidle')
    from session_start_fixtures import choose_mode
    choose_mode(page, source, 'sdk')
    page.locator('#settings-dialog .modal-close').click()
    open_report(page)
    page.locator(f'input[name="bug-report-source"][value="{source}"]').check()
    report_type = 'feature' if label == 'node' else 'bug'
    page.locator(f'#bug-report-type label:has(input[value={report_type}])').click()
    tier_id = None
    if source == 'codex':
        tier = page.locator('#bug-report-service-tier')
        expect(tier).to_have_value('')
        tier_id = 'ultrafast' if label == 'node' else 'default'
        tier.select_option(tier_id)
    else:
        expect(page.locator('#bug-report-service-tier')).to_be_hidden()
    description = 'Launch preference report '+label+' '+source
    if source == 'kiro' and label == 'node':
        description += ' · SDK模式报告标题持久化检查' * 8
    page.locator('#bug-report-description').fill(description)
    page.locator('#bug-report-file').set_input_files({
        'name': 'reference.txt', 'mimeType': 'text/plain', 'buffer': b'Feature acceptance notes'})
    wait_drafts(page)
    replays = []
    if label == 'node':
        def legacy_fields(route):
            # Older callers may still send diagnostics for features. Ignore even
            # a capture without a hostname; replay must not duplicate the worker.
            body = route.request.post_data_json
            body.update(snapshot={'data': {'diagnostic_sentinel': True}},
                        captured={'events': [{'event': 'diagnostic_sentinel'}],
                                  'terminal_capture': 'diagnostic_sentinel'})
            first = route.fetch(post_data=json.dumps(body))
            second = route.fetch(post_data=json.dumps(body))
            if tier_id:
                different_tier = route.fetch(post_data=json.dumps({**body, 'service_tier': 'priority'}))
                assert different_tier.status == 400, different_tier.json()
            replays.extend([first.json(), second.json()])
            route.fulfill(response=second)
        page.route('**/api/bug-report', legacy_fields, times=1)
    # Exercise Hub -> node past the ordinary 10 s proxy deadline while
    # native creation is in flight (BUG-20261031-145343-8da58e).
    slow_create = source == 'agy' and label == 'hub'
    if slow_create:
        (root/'init-delay').write_text('12')
    starts_before = len(peer.methods('thread/start')) if tier_id else 0
    if tier_id:
        page.screenshot(path=str(shots / f'{label}-codex-picker.png'))
        page.set_viewport_size({'width': 390, 'height': 844})
        page.emulate_media(color_scheme='dark')
        expect(tier).to_be_visible()
        page.screenshot(path=str(shots / f'{label}-codex-picker-mobile-dark.png'))
        page.set_viewport_size({'width': 1280, 'height': 900})
        page.emulate_media(color_scheme='light')
    started = time.monotonic()
    ledger_path = root/'state/conversations/conversation-ledger.json'
    initial_draft = None
    if source == 'codex' and not reject:
        def on_name():
            nonlocal initial_draft
            ledger = read_document(ledger_path)
            initial_draft = next(draft for draft in ledger['drafts'].values()
                                 if draft['value'].get('session', {}).get('thread_id') == peer.thread_id)
            # The creation receipt stops supplying a draft identity as soon as
            # the rollout exists, while the new file has yet to become readable
            # as a session. Hold that window through draft settlement.
            peer.rollout.touch()
        peer.before_name_reply = on_name
    with page.expect_response(lambda r: r.url.endswith('/api/bug-report') and r.request.method=='POST') as response:
        page.locator('#bug-report-go').click()
    reply = response.value.json()
    elapsed = time.monotonic() - started
    if slow_create:
        assert elapsed >= 12, (elapsed, reply)
    if tier_id:
        assert response.value.request.post_data_json['service_tier'] == tier_id
        starts = peer.methods('thread/start')[starts_before:]
        assert len(starts) == 1 and starts[0]['params']['serviceTier'] == tier_id, starts

    if reject:
        assert response.value.status == 500, reply
        assert 'Synthetic report rename rejected' in reply['error'], reply
        manifest = root/'reports'/reply['report_id']/'manifest.json'
        data = json.loads(manifest.read_text())
        assert data['status'] == 'failed', data
        sid = data['worker']['session']['sid']
        assert not [m for m in peer.methods('turn/start') if m['params']['threadId'] == sid]
        ledger = read_document(ledger_path)
        draft = next(d for d in ledger['drafts'].values()
                     if d['value'].get('session', {}).get('report_id') == reply['report_id'])
        assert draft['value']['text'] == (manifest.parent/'worker-prompt.md').read_text()
        assert not [r for r in ledger['requests'].values()
                    if r.get('id') == 'report-send:' + reply['report_id']]
        page.close()
        return
    assert response.value.ok, reply
    assert response.value.request.post_data_json['mode'] == 'sdk'
    assert reply['worker']['mode'] == 'sdk', reply
    if replays:
        assert replays[0] == replays[1], replays
    expect(page.locator('#bug-report-toast')).to_be_visible()
    open_worker = page.locator('#bug-report-toast').get_by_role('button', name='打开', exact=True)
    if initial_draft is None:
        open_worker.click()
        page.wait_for_function('SdkView.connected')
    # Wait for the node's durable SDK receipt, not a fabricated browser success.
    manifest=root/'reports'/reply['report_id']/'manifest.json'
    deadline=time.monotonic()+20
    while time.monotonic()<deadline:
        data=json.loads(manifest.read_text())
        if data['status']!='starting': break
        page.wait_for_timeout(100)
    if initial_draft is not None:
        peer.wait_for(lambda: [call for call in peer.methods('turn/start')
                              if call['params']['threadId'] == reply['worker']['thread_id']])
    assert data['status']=='submitted', data.get('error')
    deferred_reads = []
    if initial_draft is not None:
        peer.before_name_reply = None
        assert peer.rollout.stat().st_size == 0
        peer.rollout.write_bytes(encoded(codex_row('session_meta', {
            'id': peer.thread_id, 'cwd': peer.created[peer.thread_id]['thread']['cwd'],
            'timestamp': '2026-10-17T10:00:00Z'})))
        uid = reply['worker']['uid']
        route_pattern = lambda url: (urlsplit(url).path.endswith('/api/session/conversation')
                                     and parse_qs(urlsplit(url).query) == {'uid': [uid]})
        def draft_read(route):
            if not deferred_reads:
                deferred_reads.append(None)
                # Deliver a read issued before SDK acceptance after the page
                # opens, then let a normal follow read pick up the receipt.
                route.fulfill(json={'ok': True, 'draft': initial_draft})
            else:
                deferred_reads.append(route)
        page.route(route_pattern, draft_read)
        open_worker.click()
        page.wait_for_function('SdkView.connected')
        expect(page.locator('#sdk-input')).to_have_value(initial_draft['value']['text'])
        page.screenshot(path=str(shots / f'{label}-codex-draft-before-sync.png'))
        page.unroute(route_pattern, draft_read)
        for route in deferred_reads[1:]:
            route.continue_()
        expect(page.locator('#sdk-input')).to_have_value('')
        page.screenshot(path=str(shots / f'{label}-codex-draft-after-sync.png'))
    expect(page.locator('#sdk-input')).to_be_visible()
    expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label','停止会话')
    page.wait_for_function('SdkView.connected')
    assert data['worker_mode']=='sdk'
    assert data['report_type'] == reply['report_type'] == report_type
    title = ('功能需求: ' if report_type == 'feature' else 'BUG: ') + description
    if source == 'kiro' and len(title) > 80:
        title = title[:77] + '...'
    assert reply['worker']['session']['title'] == title
    page.screenshot(path=str(shots / f'{label}-{source}-before-title-check.png'))
    expect(page.locator('.dhead h2')).to_have_text(title)
    expect(page.locator(f'#side .item[data-uid="{reply["worker"]["uid"]}"]')).to_contain_text(title)
    page.reload(wait_until='domcontentloaded')
    page.wait_for_function('SdkView.connected')
    expect(page.locator('.dhead h2')).to_have_text(title)
    expect(page.locator('.sdk-msgs')).to_contain_text('Launch preference report '+label+' '+source)
    prompt = (manifest.parent/'worker-prompt.md').read_text()
    assert '附件1: ./sessiondock_attachments/' in prompt
    assert len(data['attachments']) == 1
    attachment = data['attachments'][0]
    assert Path(attachment['path']).is_file()
    assert attachment['name'] == 'reference.txt'
    assert (manifest.parent/attachment['bundle_file']).is_file()
    if report_type == 'feature':
        assert not any((manifest.parent/name).exists() for name in (
            'events.jsonl', 'environment.json', 'browser-state.json', 'terminal.txt'))
        assert data['session'] == {} and data['event_count'] == 0
        assert 'diagnostic_sentinel' not in prompt and 'manifest.json' not in prompt
        assert '诊断包：' not in prompt and '问题机器：' not in prompt
        assert prompt.startswith('功能需求:')
    else:
        assert '诊断包所在机器：处理会话本机' in prompt
        assert str(manifest.parent) in prompt
    # The first SEND has one durable SDK receipt, including after a repeated request.
    ledger = read_document(ledger_path)
    submitted = [r for r in ledger['requests'].values()
                 if r.get('id') == 'report-send:' + reply['report_id']]
    assert len(submitted) == 1, submitted
    assert submitted[0]['phase'] == 'sent', submitted
    events = [json.loads(line) for file in (root/'audit').rglob('*.jsonl')
              for line in file.read_text().splitlines()]
    events = [event['event'] for event in events if event.get('trace_id') == reply['report_id']]
    assert events.index('bug_report.worker_named') < events.index('bug_report.worker_submission'), events
    if source == 'codex':
        sid = reply['worker']['session']['sid']
        calls = [m['method'] for m in peer.received if m.get('params', {}).get('threadId') == sid]
        assert calls.count('thread/name/set') == 1, calls
        assert calls.count('turn/start') == 1, calls
        assert calls.index('thread/name/set') < calls.index('turn/start'), calls
    page.screenshot(path=str(shots / f'{label}-{source}-worker.png'))
    page.set_viewport_size({'width': 390, 'height': 844})
    page.emulate_media(color_scheme='dark')
    expect(page.locator('.dhead h2')).to_have_text(title)
    page.screenshot(path=str(shots / f'{label}-{source}-mobile-dark.png'))
    page.locator(f'#side .item[data-uid="{reply["worker"]["uid"]}"]').click()
    expect(page.locator('.dhead h2')).to_be_visible()
    expect(page.locator('.dhead h2')).to_have_text(title)
    page.screenshot(path=str(shots / f'{label}-{source}-mobile-detail-dark.png'))
    expect(page.locator('#sdk-input')).to_have_value('')
    assert not errors, errors
    page.close()


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,default=BINARY)
    p.add_argument('--sources',default='codex,claude,grok,agy,opencode,kiro')
    a=p.parse_args()
    os.environ.setdefault("SDKHOST", str(REPO/"target/debug/sdkhost"))
    with tempfile.TemporaryDirectory(prefix='sd-feature-report-') as tmp, sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        for source in a.sources.split(','):
            root=Path(tmp)/source;root.mkdir()
            with provider(source,root,a.binary,report=True) as (base,peer,extra):
                if source == 'codex':
                    peer.service_tiers['fake-sol'] = [
                        {'id': 'priority', 'name': 'Fast', 'description': 'Faster responses'},
                        {'id': 'ultrafast', 'name': 'Ultrafast', 'description': 'Fastest responses'}]
                exercise(browser,base,source,root,'node',peer)
                hr=root/'hub';hr.mkdir()
                hub=Hub(a.binary.resolve().with_name('sessiondock-hub'),hr,[SimpleNamespace(name='Worker',token=TOKEN,port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':',1)[1]))])
                hub.start()
                try: exercise(browser,f'http://127.0.0.1:{hub.port}',source,root,'hub',peer)
                finally: hub.stop()
                if source == 'codex':
                    peer.reject_report_names = True
                    exercise(browser,base,source,root,'rejected',peer,reject=True)
        browser.close()
    print('PASS feature/report native titles, attachments, single SDK handoff, retained evidence, six providers node/Hub')

if __name__=='__main__': main()
