#!/usr/bin/env python3
"""Browser-created fake OpenCode failure must not put CLI output in diagnostics."""
import argparse
import json
from pathlib import Path
import sys
import tempfile
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from new_session_model_browser import open_dialog, pick_source
from private_hosts import private_hosts
from send_browser import initialize

MARKER='PRIVATE-CLI-OUTPUT-fixture-only'


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--output',type=Path,default=Path('target/cli-diagnostics'))
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-cli-diagnostics-') as tmp, private_hosts(Path(tmp)), sync_playwright() as pw:
        root=Path(tmp)
        for name in ['host','ledger','state','home','work']:(root/name).mkdir()
        marker=root/'fail';marker.touch()
        fake=root/'fake.py'
        fake.write_text('import sys, pathlib, runpy\n'
            f'if sys.argv[1:3] in [["api", "session.create"], ["api", "session.get"]] and pathlib.Path({str(marker)!r}).exists():\n'
            f' print({MARKER!r});print({MARKER!r},file=sys.stderr);sys.exit(7)\n'
            f'runpy.run_path({str(REPO/"tests/fake_model_cli.py")!r},run_name="__main__")\n')
        launcher=root/'launcher.json'
        launcher.write_text(json.dumps({'schema':2,'host_binary':str(REPO/'target/debug/ptyhost'),'host_dir':str(root/'host'),'adapters':[],
            'profiles':[{'id':'opencode-cli-v1','source':'opencode','executable':str(Path(sys.executable).resolve()),'args':[str(fake)],
             'env':{'PATH':'/usr/bin:/bin','HOME':str(root/'home'),'TERM':'xterm-256color','SESSIONDOCK_TEST_MODELS':'fixture/model',
                    'SESSIONDOCK_TEST_ARGV_LOG':str(root/'argv.jsonl')}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle',root/'ledger',args.binary)
        log=root/'server.log'
        with isolated_server(Corpus(root),args.binary,host_dir=root/'host',lifecycle_dir=root/'ledger',state_dir=root/'state',launcher_config=launcher,log_path=log) as (base,_):
            browser=pw.chromium.launch(headless=True)
            try:
                page=browser.new_page()
                page.goto(base);open_dialog(page);pick_source(page,'opencode')
                page.locator('#new-cwd').fill(str(root/'work'))
                with page.expect_response(lambda r:r.url.endswith('/api/term/create')) as failed:
                    page.locator('#new-session-go').click()
                result=failed.value.json()
                assert failed.value.status == 200 and result['state']=='failed', result
                expect(page.locator('#new-session-dialog')).to_be_hidden()
                expect(page.locator('body')).to_contain_text('启动失败')
                page.screenshot(path=str(args.output/'failure.png'))
                assert MARKER not in log.read_text(), 'private CLI stdout/stderr leaked into ordinary diagnostics'
                assert 'lifecycle.opencode_create_failed' in log.read_text()
                marker.unlink()
                open_dialog(page);pick_source(page,'opencode')
                page.locator('#new-cwd').fill(str(root/'work'))
                with page.expect_response(lambda r:r.url.endswith('/api/term/create')) as success:
                    page.locator('#new-session-go').click()
                assert success.value.status==200,success.value.text()
                expect(page.locator('#new-session-dialog')).to_be_hidden()
                assert list((root/'host').glob('*.json')), 'successful retry did not create a private host'
                page.screenshot(path=str(args.output/'recovered.png'))
                (args.output/'result.json').write_text(json.dumps({'passed':['real new-session submit fails','CLI output absent from server log','failure metadata present','explicit UI retry creates private fake host']},indent=2))
                print('PASS CLI diagnostics privacy and browser retry',flush=True)
            finally:browser.close()


if __name__ == '__main__':
    from browser_evidence_fixtures import run_with_evidence
    run_with_evidence(main, 'target/cli-diagnostics')
