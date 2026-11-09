#!/usr/bin/env python3
"""Droid child provenance, read-only navigation and independent forks in Chromium."""
import argparse
import tempfile
import shutil
import json
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from droid_children_fixtures import DroidChildrenFixture
from claude_sdk_browser import send, start_sdk_if_needed, receipt
from droid_actions_browser import confirm, wait_selected
from sdk_usage_fixtures import reveal
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import node_env, free_port, TOKEN


def check(page, base, fixture, output, hub=False):
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    fork = page.locator('#side .item').filter(has=page.locator('.t', has_text='Droid independent fork')).first
    expect(fork).to_be_visible();fork.click()
    page.locator('#a-fork-chain').click()
    page.locator('#fork-chain-menu .chain-toggle').click()
    page.keyboard.press('Escape')
    parent = page.locator('#side .item').filter(has=page.locator('.t', has_text='Droid parent session')).first
    expect(parent).to_be_visible()
    uid = parent.get_attribute('data-uid')
    parent.click()
    page.locator('#a-view-switch').click()
    child = page.locator(f'#session-view-menu button[data-agent="{fixture.child}"]')
    expect(child).to_contain_text('Droid delegated child')
    expect(page.locator(f'#session-view-menu button[data-agent="{fixture.fork}"]')).to_have_count(0)
    child.click()
    expect(page.locator('#detail')).to_contain_text('Child history answer')
    expect(page.locator('#detail')).not_to_contain_text('Parent history answer')
    expect(page.locator('#a-sdk')).to_be_visible()
    expect(page.locator('#sdk-input')).to_be_visible()
    assert page.evaluate('NativeSdkView.readOnly')
    page.screenshot(path=str(output / 'child-wide-light.png'), full_page=True)
    page.locator('#a-view-switch').click()
    page.locator('#session-view-menu button[data-agent=""]').click()
    expect(page.locator('#detail')).to_contain_text('Parent history answer')
    fork = page.locator('#side .item').filter(has=page.locator('.t', has_text='Droid independent fork')).first
    expect(fork).to_be_visible(); fork.click()
    expect(page.locator('#detail')).to_contain_text('Fork history answer')
    expect(page.locator('#side')).not_to_contain_text('Droid orphan child')
    ambiguous = page.locator('#side .item').filter(has=page.locator('.t', has_text='Droid ambiguous child')).first
    expect(ambiguous).to_be_visible(); ambiguous.click()
    expect(page.locator('#detail')).to_contain_text('父线程 ID 在已配置索引中存在歧义')
    # Inspect the same browser-visible list response to distinguish fork and child.
    rows = page.request.get(base + '/api/sessions').json()['sessions']
    parent_row = next(row for row in rows if row['uid'] == uid)
    assert {row['id'] for row in parent_row['agent_items']} == {fixture.child,fixture.intermediate,fixture.nested}
    assert parent_row['agent_items'][0]['active'] is False
    fork_row = next(row for row in rows if row['sid'] == fixture.fork)
    assert fork_row['forked_from_id'] == fixture.sid and not fork_row.get('agent_items')
    assert not any(row['sid'] == fixture.orphan for row in rows)
    broken = next(row for row in rows if row['sid'] == fixture.ambiguous)
    assert broken['supported'] is False
    parent.click();page.locator('#a-view-switch').click();child.click()
    page.set_viewport_size({'width':390, 'height':844});page.emulate_media(color_scheme='dark')
    # A viewport transition can restore the list; select the same child explicitly.
    if page.locator('#side').is_visible():
        parent.click();page.locator('#a-view-switch').click();child.click()
    expect(page.locator('#detail')).to_contain_text('Child history answer')
    page.screenshot(path=str(output / 'child-mobile-dark.png'), full_page=True)
    assert not errors, errors
    assert not fixture.frames('spawn'), 'read-only child navigation must not start Droid'
    # A parent TUI can own child execution without a separate child PID.
    # Check both Linux proc evidence and the portable process matcher, including
    # an intermediate parent omitted by the flattened owner in the sidebar.
    for writer_sid,selected in [(fixture.sid,fixture.child),(fixture.intermediate,fixture.nested)]:
        observed=fixture.process(424298,sid=writer_sid)
        blocked=page.request.post(base+'/api/native-sdk/open',data={'uid':uid,'agent':selected})
        assert blocked.status==409,blocked.text()
        shutil.rmtree(observed)
        portable=fixture.root/'proc/droid-processes.json'
        portable.write_text(json.dumps([{'pid':424298,'ppid':1,'command':'droid --resume '+writer_sid,'started':1794200000,'cwd':str(fixture.cwd)}]))
        blocked=page.request.post(base+'/api/native-sdk/open',data={'uid':uid,'agent':selected})
        assert blocked.status==409,blocked.text()
        portable.unlink()
    assert not fixture.frames('spawn'), 'parent writer conflicts must never start children'
    # Explicitly open the selected child, using the owner's UI identity and the
    # child's own native UID internally. Parent and child remain exclusive writers.
    page.set_viewport_size({'width':1280,'height':900});page.emulate_media(color_scheme='light')
    if page.locator('#side').is_visible():
        parent.click();page.locator('#a-view-switch').click();child.click()
    if not page.locator('#sdk-input').is_visible():page.locator('#a-sdk').click()
    start_sdk_if_needed(page)
    expect(page.locator('#sdk-input')).to_be_visible()
    send(page,'independent child prompt')
    expect(page.locator('.sdk-live')).to_contain_text('Droid final independent child prompt')
    assert page.evaluate('NativeSdkView.raw.session_id') == fixture.child
    expect(page.locator(f'.item.agent[data-owner="{uid}"][data-agent="{fixture.child}"] .item-status.sdk')).to_be_attached()
    assert 'independent child prompt' in fixture.paths[fixture.child].read_text()
    assert 'independent child prompt' not in fixture.path.read_text()
    blocked=page.request.post(base+'/api/native-sdk/open',data={'uid':uid})
    assert blocked.status == 409 and 'session_busy' in blocked.text(),blocked.text()
    page.screenshot(path=str(output/'child-sdk-wide-light.png'),full_page=True)
    # A separately owned main runtime in the same cwd is not an unknown writer.
    child_pid=fixture.frames('spawn')[-1]['pid']
    child_proc=fixture.process(child_pid)
    fork_uid=next(row['uid'] for row in page.request.get(base+'/api/sessions').json()['sessions'] if row['sid']==fixture.fork)
    fork_open=page.request.post(base+'/api/native-sdk/open',data={'uid':fork_uid})
    assert fork_open.ok,fork_open.text()
    fork_pid=fixture.frames('spawn')[-1]['pid']
    fork_proc=fixture.process(fork_pid)
    stopped=page.request.post(base+'/api/native-sdk/stop',data={'uid':uid,'agent':fixture.child})
    assert stopped.ok,stopped.text()
    shutil.rmtree(child_proc)
    # An unrelated owned root does not authorize its child process to write this
    # selected SID. This is the prior all-owned-ancestors exemption regression.
    delegated=fixture.process(424299,parent=fork_pid,sid=fixture.child)
    before=len(fixture.frames('spawn'))
    blocked=page.request.post(base+'/api/native-sdk/open',data={'uid':uid,'agent':fixture.child})
    assert blocked.status==409,blocked.text()
    assert len(fixture.frames('spawn'))==before
    shutil.rmtree(delegated)
    stopped=page.request.post(base+'/api/native-sdk/stop',data={'uid':fork_uid})
    assert stopped.ok,stopped.text()
    shutil.rmtree(fork_proc)
    opened=page.request.post(base+'/api/native-sdk/open',data={'uid':uid})
    assert opened.ok,opened.text()
    blocked=page.request.post(base+'/api/native-sdk/open',data={'uid':uid,'agent':fixture.child})
    assert blocked.status == 409 and 'session_busy' in blocked.text(),blocked.text()
    stopped=page.request.post(base+'/api/native-sdk/stop',data={'uid':uid})
    assert stopped.ok,stopped.text()
    # Reopen the same selected child through the page, exercising alias recovery
    # after stop and the SSE subscription's owner:agent scope.
    page.reload(wait_until='networkidle')
    parent=page.locator(f'#side .item[data-uid="{uid}"]')
    if not parent.is_visible():page.locator('#detail .mobile-back').click()
    parent.click();page.locator('#a-view-switch').click()
    page.locator(f'#session-view-menu button[data-agent="{fixture.child}"]').click()
    if not page.locator('#sdk-input').is_visible():page.locator('#a-sdk').click()
    start_sdk_if_needed(page)
    send(page,'child resumed prompt')
    expect(page.locator('.sdk-live')).to_contain_text('Droid final child resumed prompt')
    assert page.evaluate('NativeSdkView.raw.session_id') == fixture.child
    page.set_viewport_size({'width':390,'height':844});page.emulate_media(color_scheme='dark')
    expect(page.locator('#sdk-input')).to_be_visible()
    page.screenshot(path=str(output/'child-sdk-mobile-dark.png'),full_page=True)
    page.set_viewport_size({'width':1280,'height':900});page.emulate_media(color_scheme='light')
    def selected_child(agent):
        page.wait_for_function('agent => NativeSdkView.agent===agent && NativeSdkView.connected',arg=agent)
        assert page.evaluate('NativeSdkView.uid')==uid
        expect(page.locator('#sdk-send')).to_be_enabled()
    with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/compact')) as compacted:
        reveal(page,'[data-sdk-action="compact"]').click();confirm(page,'压缩')
    assert compacted.value.ok,compacted.value.text()
    same=receipt(compacted.value.json());assert same['uid']==uid and same['agent']==fixture.child,same
    selected_child(fixture.child)
    (fixture.root/'fork-fail').touch()
    with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/fork')) as failed:
        reveal(page,'[data-sdk-action="fork"]').click();confirm(page,'分支')
    assert not failed.value.ok
    selected_child(fixture.child)
    (fixture.root/'fork-fail').unlink()
    with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/fork')) as forked:
        reveal(page,'[data-sdk-action="fork"]').click();confirm(page,'分支')
    assert forked.value.ok,forked.value.text()
    branch=receipt(forked.value.json());assert branch['uid']==uid and branch['agent']!=fixture.child,branch
    selected_child(branch['agent'])
    repeat=page.request.post(base+'/api/native-sdk/fork',data=forked.value.request.post_data_json)
    assert repeat.ok and receipt(repeat.json())['agent']==branch['agent'],repeat.text()
    send(page,'derived child continuation')
    expect(page.locator('.sdk-live')).to_contain_text('Droid final derived child continuation')
    assert 'derived child continuation' not in fixture.paths[fixture.child].read_text()
    # Native file rewind also keeps calling metadata and navigates a new child.
    reveal(page,'[data-sdk-action="rewind"]').click()
    page.locator('#sdk-action-dialog select').select_option('seed-user-0')
    page.locator('#sdk-action-dialog button[type=submit]').click()
    expect(page.locator('#sdk-action-dialog')).to_contain_text('无法恢复：expired.txt')
    with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/rewind')) as rewound:
        page.locator('#sdk-action-dialog button[type=submit]').click()
    assert rewound.value.ok,rewound.value.text()
    rewind=receipt(rewound.value.json());assert rewind['uid']==uid and rewind['agent']!=branch['agent'],rewind
    selected_child(rewind['agent'])
    expect(page.locator('.native-sdk-pane')).to_contain_text('恢复 0 个文件')
    send(page,'rewound child continuation')
    expect(page.locator('.sdk-live')).to_contain_text('Droid final rewound child continuation')
    (fixture.root/'compact-new-session').touch()
    with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/compact')) as handoff:
        reveal(page,'[data-sdk-action="compact"]').click();confirm(page,'压缩')
    assert handoff.value.ok,handoff.value.text()
    compact=receipt(handoff.value.json());assert compact['uid']!=uid and not compact.get('agent'),compact
    # This synthetic compaction deliberately emits a fresh session_start without
    # calling metadata. The actual result is independent, never invented as a child.
    wait_selected(page,compact['uid'])
    assert page.evaluate('NativeSdkView.agent') is None
    send(page,'compacted child continuation')
    expect(page.locator('.sdk-live')).to_contain_text('Droid final compacted child continuation')
    page.screenshot(path=str(output/'child-derived-actions.png'),full_page=True)
    # A native child may retain a cross-machine/missing parent. A creation
    # receipt already proves this native UID; missing ancestry alone is not busy.
    missing='60000000-0000-4000-8000-999999999998'
    marker=fixture.root/'missing-calling-parent';marker.write_text(missing)
    if not page.locator('#new-session').is_visible():page.locator('#header-more-btn').click()
    page.locator('#new-session').click()
    page.locator('#new-session-form label:has(input[value="droid"])').click()
    page.locator('#new-cwd').fill(str(fixture.cwd))
    with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/create')) as created:
        page.locator('#new-session-go').click()
    assert created.value.ok,created.value.text()
    orphan=receipt(created.value.json());marker.unlink()
    page.wait_for_function('uid => NativeSdkView.uid===uid && NativeSdkView.connected',arg=orphan['uid'])
    page.wait_for_load_state('networkidle')
    page.locator(f'#side .item[data-uid="{compact["uid"]}"]').click()
    stopped=page.request.post(base+'/api/native-sdk/stop',data={'uid':orphan['uid']})
    assert stopped.ok,stopped.text()
    opened=page.request.post(base+'/api/native-sdk/open',data={'uid':orphan['uid']})
    assert opened.ok,opened.text()
    stopped=page.request.post(base+'/api/native-sdk/stop',data={'uid':orphan['uid']})
    assert stopped.ok,stopped.text()
    observed=fixture.process(424298,sid=missing)
    blocked=page.request.post(base+'/api/native-sdk/open',data={'uid':orphan['uid']})
    assert blocked.status==409,blocked.text()
    shutil.rmtree(observed)


    assert not errors,errors



def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('/tmp/sessiondock-droid-children-browser'))
    args=parser.parse_args();args.output.mkdir(parents=True, exist_ok=True)
    for hub_mode in (False, True):
        with tempfile.TemporaryDirectory(prefix='sessiondock-droid-children-') as directory, ExitStack() as stack:
            root=Path(directory);fixture=DroidChildrenFixture(root)
            port=free_port();extra=node_env(root,port,'127.0.0.0/8') if hub_mode else None
            base=stack.enter_context(fixture.server(args.binary,extra))
            if hub_mode:
                hub_root=root/'hub';hub_root.mkdir()
                hub=Hub(args.binary.with_name('sessiondock-hub'),hub_root,[SimpleNamespace(name='DroidNode',port=port,token=TOKEN)])
                hub.start();stack.callback(hub.stop);base=f'http://127.0.0.1:{hub.port}'
            output=args.output/('hub' if hub_mode else 'node');output.mkdir(exist_ok=True)
            with sync_playwright() as playwright:
                browser=playwright.chromium.launch(headless=True)
                page=browser.new_page(viewport={'width':1280,'height':900},color_scheme='light')
                check(page,base,fixture,output,hub_mode);browser.close()
    print('Droid child history Chromium node + Hub: PASS')


if __name__ == '__main__':main()
