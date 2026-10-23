#!/usr/bin/env python3
"""Feature requests retain attachments and hand off once to SDK workers, node and Hub."""
import argparse
import json
import os
import tempfile
from pathlib import Path
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from sdk_start_stop_fixtures import provider
from history_fixtures import BINARY, REPO
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from bug_report_node_browser import open_report, wait_drafts


def exercise(browser, base, source, root, label, peer=None, reject=False):
    print(label, source, 'feature/report SDK handoff', flush=True)
    page = browser.new_page(viewport={'width': 1280, 'height': 900})
    page.set_default_timeout(20000)
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
            replays.extend([first.json(), second.json()])
            route.fulfill(response=second)
        page.route('**/api/bug-report', legacy_fields, times=1)
    with page.expect_response(lambda r: r.url.endswith('/api/bug-report') and r.request.method=='POST') as response:
        page.locator('#bug-report-go').click()
    reply = response.value.json()
    if reject:
        assert response.value.status == 500, reply
        assert 'Synthetic report rename rejected' in reply['error'], reply
        manifest = root/'reports'/reply['report_id']/'manifest.json'
        data = json.loads(manifest.read_text())
        assert data['status'] == 'failed', data
        sid = data['worker']['session']['sid']
        assert not [m for m in peer.methods('turn/start') if m['params']['threadId'] == sid]
        ledger = json.loads((root/'state/conversations/conversation-ledger.json').read_text())
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
    page.locator('#bug-report-toast').get_by_role('button',name='打开',exact=True).click()
    expect(page.locator('#sdk-input')).to_be_visible()
    expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label','停止会话')
    page.wait_for_function('SdkView.connected')
    # Wait for the node's durable SDK receipt, not a fabricated browser success.
    manifest=root/'reports'/reply['report_id']/'manifest.json'
    import time
    deadline=time.monotonic()+20
    while time.monotonic()<deadline:
        data=json.loads(manifest.read_text())
        if data['status']!='starting': break
        page.wait_for_timeout(100)
    assert data['status']=='submitted', data
    assert data['worker_mode']=='sdk'
    assert data['report_type'] == reply['report_type'] == report_type
    title = ('功能需求: ' if report_type == 'feature' else 'BUG: ') + description
    if source == 'kiro' and len(title) > 80:
        title = title[:77] + '...'
    assert reply['worker']['session']['title'] == title
    shots = REPO/'target/validation/report-feature'
    shots.mkdir(parents=True, exist_ok=True)
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
    ledger = json.loads((root/'state/conversations/conversation-ledger.json').read_text())
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
        if 'turn/start' in calls:
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
