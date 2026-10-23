#!/usr/bin/env python3
"""Kiro SDK resource ownership, durable hosts, descendants and exited PID reuse."""
import argparse
from contextlib import ExitStack
import os
from pathlib import Path
import subprocess
import shutil
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import server
from history_fixtures import BINARY
from kiro_sdk_fixtures import KiroFixture
from sdkhost_fixtures import stop_private_sdkhosts
from spawned_by_fixtures import START, proc_pid
from send_browser import initialize


def check_bindings(page, sid, pid, present=True):
    page.wait_for_function('''async ({sid,pid,present}) => {
      const report = await (await fetch('/api/process-links')).json();
      const pids = report.bindings.filter(b => b.session.sid === sid).map(b => b.process.pid);
      return present ? pids.includes(pid) && pids.includes(300) && !pids.includes(100)
                     : !pids.includes(pid);
    }''', arg=dict(sid=sid, pid=pid, present=present), timeout=30000, polling=500)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--with-agent', action='store_true')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='kiro-resources-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        for name in ('claude', 'codex', 'grok', 'state'):
            (root / name).mkdir()
        fixture = KiroFixture(root, host_binary=args.binary.resolve().with_name('ptyhost'))
        initialize('--initialize-lifecycle', root / 'ledger', args.binary.resolve())
        (fixture.proc / 'sys/kernel/random').mkdir(parents=True)
        (fixture.proc / 'sys/kernel/random/boot_id').write_text('private-kiro-boot')
        node = root / 'node-id'
        node.write_text('a' * 32)
        extra = {**fixture.env, 'SESSIONDOCK_NATIVE_SDKS':'auto',
                 'SESSIONDOCK_LIFECYCLE_DIR':str(root / 'ledger'),
                 'SESSIONDOCK_PTYHOST_DIR':str(fixture.host_dir),
                 'SESSIONDOCK_STATE_DIR':str(root / 'state'),
                 'SESSIONDOCK_NODE_ID_FILE':str(node),
                 'SESSIONDOCK_RESOURCE_AGENT_SOCKET':str(root / 'collector.sock')}
        browser = pw.chromium.launch(headless=True)
        try:
            with ExitStack() as stack:
                if args.with_agent:
                    log = stack.enter_context(tempfile.TemporaryFile())
                    agent = subprocess.Popen([str(args.binary.resolve().with_name('resource-agent')),
                        '--uid', str(os.getuid()), '--node-id-file', str(node),
                        '--socket', str(root / 'collector.sock'), '--state', str(root / 'agent.json'),
                        '--proc-root', str(fixture.proc), '--events', 'off'], stdout=log, stderr=log)
                    stack.callback(lambda: (agent.terminate(), agent.wait(timeout=10)))
                uid = None
                for restarted in (False, True):
                    with server(SimpleNamespace(root=root), args.binary, 'off', extra) as base:
                        page = browser.new_page(viewport={'width':1280,'height':960})
                        page.goto(base, wait_until='domcontentloaded')
                        page.wait_for_function('T.listLoaded')
                        page.wait_for_function('''async sid => {await loadSessions(true);
                            return S.sessions.some(s => s.sid === sid)}''', arg=fixture.sid)
                        print(f'Kiro list ready; restarted={restarted}', flush=True)
                        if not restarted:
                            uid = page.evaluate('sid => S.sessions.find(s => s.sid === sid).uid', fixture.sid)
                            START[700] = 10000
                            proc_pid(fixture.proc, 700, 'kiro-cli', ['kiro-cli', 'chat', '--resume-id', fixture.sid], 1)
                            proc_pid(fixture.proc, 300, 'python', ['python', 'cli-worker.py'], 700)
                            check_bindings(page, fixture.sid, 700)
                            page.locator(f'#side .item[data-uid="{uid}"]').click()
                            page.locator('#a-resources').click()
                            expect(page.locator('.session-resources')).to_be_visible()
                            shutil.rmtree(fixture.proc / '700'); shutil.rmtree(fixture.proc / '300')
                            check_bindings(page, fixture.sid, 700, False)
                            print('PASS Kiro external CLI and worker', flush=True)
                            page.locator(f'#side .item[data-uid="{uid}"]').click()
                            page.locator('#a-sdk').click()
                            page.locator('#sdk-input').click()
                            page.wait_for_function('SdkView.connected && !SdkView.readOnly')
                            print('Kiro SDK connected', flush=True)
                            pid = [r['fixture_start']['pid'] for r in fixture.received() if 'fixture_start' in r][-1]
                            START[pid] = 20000
                            proc_pid(fixture.proc, 100, 'sdkhost', ['sdkhost'], 1)
                            proc_pid(fixture.proc, pid, 'kiro-cli', ['kiro-cli', 'acp'], 100)
                            proc_pid(fixture.proc, 300, 'python', ['python', 'worker.py'], pid)
                            for ident in (pid, 300):
                                stat = fixture.proc / str(ident) / 'stat'
                                stat.write_text(stat.read_text().rstrip() + ' 0 0\n')
                                (stat.parent / 'smaps_rollup').write_text('Pss: 1024 kB\n')
                        # On restart, check before opening SDK: resource reads
                        # must recover existing host ownership without launching.
                        starts = len([r for r in fixture.received() if 'fixture_start' in r])
                        check_bindings(page, fixture.sid, pid)
                        print('Kiro owner and descendant attributed', flush=True)
                        assert len([r for r in fixture.received() if 'fixture_start' in r]) == starts
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                        page.locator('#a-resources').click()
                        page.get_by_role('button', name='仅当前会话', exact=True).click()
                        if args.with_agent:
                            page.wait_for_function('''async uid => {
                              const r = await (await fetch('/api/session/resources?uid='+encodeURIComponent(uid)+'&scope=direct')).json();
                              const m=r.totals.metrics || r.totals;
                              return m.memory_pss_bytes?.value === 2097152;
                            }''', arg=uid, timeout=45000, polling=500)
                            page.get_by_role('button', name='刷新资源').click()
                            expect(page.locator('.sr-totals .sr-metric').filter(has=page.locator('dt', has_text='内存 · PSS')).locator('dd')).to_have_text('2 MiB')
                            expect(page.locator('.sr-node .sr-status')).to_have_text('在线')
                        if args.screenshots:
                            args.screenshots.mkdir(parents=True, exist_ok=True)
                            page.screenshot(path=str(args.screenshots / f'kiro-resources-{args.with_agent}-{restarted}.png'), full_page=True)
                        if restarted:
                            stopped = page.evaluate('''async uid => (await fetch('/api/native-sdk/stop', {
                                method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({uid})})).json()''', uid)
                            assert stopped.get('ok'), stopped
                            # Reuse the numeric PID as an unrelated incarnation.
                            stat = fixture.proc / str(pid) / 'stat'
                            stat.write_text(stat.read_text().replace(' 20000 ', ' 30000 '))
                            check_bindings(page, fixture.sid, pid, False)
                        page.close()
        finally:
            browser.close()
            stop_private_sdkhosts(root)
    print(f'PASS Kiro resources: collector={args.with_agent}, restart, descendants and PID reuse')


if __name__ == '__main__':
    main()
