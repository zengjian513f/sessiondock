#!/usr/bin/env python3
"""Unchanged metadata and SDK host identities stay cheap on real page refreshes."""
import argparse
import json
import os
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY
from droid_sdk_fixtures import DroidFixture, native_uid
from claude_sdk_browser import open_sdk, send, wait_snapshot


def rchar(pid):
    return int(dict(line.split(':', 1) for line in Path(f'/proc/{pid}/io').read_text().splitlines())['rchar'])


def agy_reads(binary):
    from unittest.mock import patch
    from agy_sdk_fixtures import AgyFixture
    from agy_sdk_browser import select_sdk
    from sdk_usage_fixtures import refresh_usage
    from history_fixtures import Corpus, isolated_server
    from send_browser import initialize
    from sdkhost_fixtures import stop_private_sdkhosts
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-periodic-') as tmp, sync_playwright() as pw:
        root=Path(tmp);fixture=AgyFixture(root,binary.resolve().with_name('ptyhost'))
        # Launch Chromium before applying the CLI's private HOME/cache variables.
        browser=pw.chromium.launch(headless=True)
        try:
            with patch.dict(os.environ,fixture.env):
                initialize('--initialize-lifecycle',root/'ledger',binary.resolve())
                with isolated_server(Corpus(root),binary,host_dir=root/'host',lifecycle_dir=root/'ledger',
                        state_dir=root/'state',launcher_config=fixture.launcher,extra_env=fixture.extra_env) as (base,opener):
                    page=browser.new_page();page.goto(base,wait_until='networkidle');select_sdk(page)
                    send(page,'periodic Agy turn')
                    expect(page.locator('.native-sdk-pane')).to_contain_text('Synthetic Agy reply: periodic Agy turn')
                    page.wait_for_function('!NativeSdkView.raw.running && !NativeSdkView.loadingUsage')
                    (root/'context-usage.json').write_text(json.dumps({'estimatedTokensUsed':12000,'maxContextTokens':200000}))
                    log=next((root/'state').rglob('native.log'));original=log.read_text()
                    with log.open('a') as out:out.write(('synthetic appended log '*250+'\n')*1000)
                    def refresh():
                        with page.expect_response(lambda r:'/api/native-sdk/usage' in r.url):refresh_usage(page)
                        page.wait_for_function('!NativeSdkView.loadingUsage')
                    refresh()
                    page.wait_for_function('NativeSdkView.raw.context_usage?.used_tokens===12000')
                    before=rchar(opener.sessiondock_pid)
                    for _ in range(12):refresh()
                    delta=rchar(opener.sessiondock_pid)-before
                    # /usage also runs the synthetic Python quota CLI. Linux
                    # accounts its collected child I/O here (~21 MiB/12 calls);
                    # re-reading the 5.5 MiB log would add at least 66 MiB.
                    assert delta<32*1024*1024, f'unchanged native.log reread: {delta}'
                    print(f'Agy 12 usage refreshes (including quota CLI): rchar={delta} (limit 33554432)',flush=True)
                    before=rchar(opener.sessiondock_pid)
                    with log.open('a') as out:out.write('append only\n'*20000)
                    refresh()
                    assert rchar(opener.sessiondock_pid)-before<3*1024*1024, 'append reread old log prefix'
                    # A trailing partial port line is interpreted, then a
                    # truncated/replaced log invalidates its cached endpoint.
                    log.write_text(original.rstrip('\n'));refresh()
                    page.wait_for_function('NativeSdkView.raw.context_usage?.used_tokens===12000')
                    log.write_text(original.splitlines()[0]+'\n');refresh()
                    page.wait_for_function("NativeSdkView.raw.usage_errors?.context?.includes('未提供')")
                    replacement=log.with_suffix('.replacement');replacement.write_text(original);replacement.replace(log)
                    refresh();page.wait_for_function('NativeSdkView.raw.context_usage?.used_tokens===12000')
                    log.write_text(original.replace('Starting language server process with pid ', 'Other process with pid '));refresh()
                    page.wait_for_function("NativeSdkView.raw.usage_errors?.context?.includes('身份不匹配')")
                    log.write_text(original);refresh()
                    page.wait_for_function('NativeSdkView.raw.context_usage?.used_tokens===12000')
        finally:
            browser.close();stop_private_sdkhosts(root)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-periodic-reads-') as tmp:
        fixture=DroidFixture(tmp)
        with fixture.server(args.binary, {'SESSIONDOCK_RESOURCE_AGENT_MODE':'off'}) as base, sync_playwright() as pw:
            browser=pw.chromium.launch(headless=True)
            try:
                page=browser.new_page(service_workers='block');page.goto(base,wait_until='networkidle')
                open_sdk(page,fixture,expect);send(page,'periodic fixture turn')
                wait_snapshot(page,fixture.uid,lambda r:not r['running'])
                checkpoint=fixture.root/'state/sdk-adapters/droid'/(fixture.uid.encode().hex()+'.json')
                saved=json.loads(checkpoint.read_text())
                other=fixture.path.with_name('periodic-idle.jsonl')
                other.write_text(fixture.path.read_text().replace(fixture.sid,'periodic-idle'))
                saved.update(sid='periodic-idle',host_key=None,measurement_padding='x'*(1800*1024))
                checkpoint=checkpoint.with_name(native_uid(other).encode().hex()+'.json')
                checkpoint.write_text(json.dumps(saved))
                metadata=fixture.root/'state/session-metadata.json'
                document={'schema_version':1,'revision':1,'sessions':{},'measurement_padding':'x'*(288*1024)}
                metadata.write_text(json.dumps(document))
                def refresh():
                    page.reload(wait_until='networkidle')
                    row=page.locator(f'#side .item[data-uid="{fixture.uid}"]')
                    expect(row).to_be_visible();row.click()
                    response=page.request.get(base+'/api/process-links')
                    assert response.ok,response.text()
                    page.wait_for_timeout(1100)  # existing one-second report cache
                refresh();refresh()
                before=rchar(fixture.server_pid)
                for _ in range(12):refresh()
                delta=rchar(fixture.server_pid)-before
                assert delta<4*1024*1024, f'unchanged checkpoint/metadata reread: {delta}'
                print(f'12 page refreshes: rchar={delta} (limit 4194304)',flush=True)
                # Same inode/length/mtime still changes Unix ctime. A cache of
                # just (size, mtime) would silently miss this external edit.
                prior=checkpoint.stat();payload=checkpoint.read_bytes()
                changed=payload.replace(b'"sid": "periodic-idle"',b'"sid": "periodic-else"')
                assert len(payload)==len(changed) and payload!=changed
                checkpoint.write_bytes(changed);os.utime(checkpoint,ns=(prior.st_atime_ns,prior.st_mtime_ns))
                before=rchar(fixture.server_pid);refresh()
                assert rchar(fixture.server_pid)-before>=len(changed), 'changed identity was not re-read'
                checkpoint.write_text('{invalid');refresh()
                checkpoint.write_bytes(payload);refresh()
                checkpoint.unlink();refresh()
                # Visible metadata behavior: external replace, corruption,
                # recovery and removal immediately change the star state.
                star=page.locator(f'#side .star-toggle[data-star-uid="{fixture.uid}"]')
                document.update(revision=2,sessions={fixture.uid:{'starred':True}})
                metadata.write_text(json.dumps(document));refresh()
                expect(star).to_have_attribute('aria-pressed','true')
                metadata.write_text('{invalid');refresh()
                expect(star).to_have_attribute('aria-pressed','false')
                metadata.write_text(json.dumps(document));refresh()
                expect(star).to_have_attribute('aria-pressed','true')
                metadata.unlink();refresh()
                expect(star).to_have_attribute('aria-pressed','false')
            finally:browser.close()
    agy_reads(args.binary)
    print('PASS periodic reads: bounded I/O, changed SDK identity, corrupt/missing recovery, visible external metadata',flush=True)


if __name__=='__main__':main()
