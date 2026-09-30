#!/usr/bin/env python3
"""Operator-only Haiku file edit, Chromium copy and native cloned checkpoint rewind.
Uses a private home/cwd and existing authentication without reading credentials.
"""
# run_validation: skip
import argparse
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import uuid
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, isolated_server
from session_files_browser import uid
from session_files_claude_real import MODEL
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--claude',default=str(Path.home()/'.local/bin/claude'))
    args=parser.parse_args()
    settings=Path.home()/'.claude/settings.json';before=digest(settings)
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-rewind-') as tmp, sync_playwright() as pw:
        root=Path(tmp);root.chmod(0o700);corpus=Corpus(root/'node')
        for name in ('codex','claude/projects','grok','cwd','state','proc','ids','trash'):
            (corpus.root/name).mkdir(parents=True)
        home=corpus.root/'claude';cwd=corpus.root/'cwd'
        login=home/'.credentials.json';login.symlink_to(Path.home()/'.claude/.credentials.json')
        (home/'.claude.json').write_text(json.dumps({'hasCompletedOnboarding':True}))
        env=dict(os.environ,CLAUDE_CONFIG_DIR=str(home),HOME=str(home),CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING='true')
        common=[args.claude,'-p','--safe-mode','--setting-sources','','--model',MODEL,'--effort','low']
        target=cwd/'sample.txt';original=b'Before native checkpoint\n';changed=b'After native checkpoint\n'
        target.write_bytes(original);sid=str(uuid.uuid4())
        try:
            result=subprocess.run([*common,'--session-id',sid,'--tools','Read,Edit,Write',
                '--allowedTools','Read,Edit,Write','--permission-mode','acceptEdits','--max-turns','6',
                '--output-format','json','Read sample.txt, then use Edit to replace its entire contents with exactly '
                '"After native checkpoint" followed by a newline. Do not create other files.'],
                cwd=cwd,env=env,text=True,capture_output=True,timeout=180)
            assert result.returncode==0,(result.returncode,result.stderr)
            response=json.loads(result.stdout)
            assert not response.get('is_error'),response.get('result')
            assert set(response['modelUsage'])=={MODEL},response['modelUsage'].keys()
            assert target.read_bytes()==changed
            path=next(home.glob('projects/*/'+sid+'.jsonl'))
            rows=[json.loads(line) for line in path.read_text().splitlines()]
            assert {r['message']['model'] for r in rows if r.get('type')=='assistant'}=={MODEL}
            checkpoints=[r for r in rows if r.get('type')=='file-history-snapshot']
            assert checkpoints,'native CLI did not write a checkpoint'
            checkpoint=checkpoints[0]['snapshot']['messageId']
            backups={p.relative_to(home/'file-history'/sid):p.read_bytes() for p in (home/'file-history'/sid).rglob('*') if p.is_file()}
            assert original in backups.values(), 'native original-file backup absent'
            source_bytes=path.read_bytes()
            print('PASS native Haiku edit records checkpoint and original backup',flush=True)
            node=SimpleNamespace(name='source',nid='c'*32,port=free_port(),token=TOKEN)
            (corpus.root/'ids/node-id').write_text(node.nid+'\n');hubroot=root/'hub';hubroot.mkdir()
            with ExitStack() as stack:
                nodeenv=node_env(corpus.root,node.port,'127.0.0.0/8')
                nodeenv.update(SESSIONDOCK_PROC_ROOT=str(corpus.root/'proc'),SESSIONDOCK_CLAUDE_ROOT=str(home/'projects'))
                stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',trash_dir=corpus.root/'trash',extra_env=nodeenv))
                hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node]);hub.start();stack.callback(hub.stop)
                browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
                page=browser.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                selected=scoped(node.nid,uid('claude',path))
                page.locator(f'#side .item[data-uid="{selected}"]').click()
                with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
                    page.locator('#a-clone-group').click()
                assert planned.value.ok,planned.value.text()
                dialog=page.locator('#clone-group-dialog');expect(dialog.locator('.clone-members tbody tr')).to_have_count(1)
                with page.expect_response(lambda r:r.url.endswith('/api/session/clone')) as copied:
                    dialog.locator('.clone-confirm').click()
                assert copied.value.ok,copied.value.text()
                operation=json.loads((corpus.root/'state/transfers'/copied.value.json()['operation_id']/'operation.json').read_text())
                plan=operation['file_plan'];clone=plan['sessions']['claude:'+sid]
                cloned_checkpoint=plan['records']['claude:'+checkpoint]
                assert clone!=sid and cloned_checkpoint!=checkpoint
                page.wait_for_function('(id)=>S.sel===id',arg=copied.value.json()['target_uid'])
                expect(page.locator('#msgs')).to_contain_text('After native checkpoint')
            assert {p.relative_to(home/'file-history'/clone):p.read_bytes() for p in (home/'file-history'/clone).rglob('*') if p.is_file()}==backups
            # CLI rewind is local and sends no model request. Same cwd is intentional:
            # cloned sessions share project files; only history identities are isolated.
            rewind=subprocess.run([*common,'--resume',clone,'--rewind-files',cloned_checkpoint],
                cwd=cwd,env=env,text=True,capture_output=True,timeout=60)
            assert rewind.returncode==0,(rewind.stdout,rewind.stderr)
            assert 'Files rewound' in rewind.stdout,rewind.stdout
            assert target.read_bytes()==original,'cloned native checkpoint failed to restore original bytes'
            assert path.read_bytes()==source_bytes,'rewind changed source transcript'
            assert {p.relative_to(home/'file-history'/sid):p.read_bytes() for p in (home/'file-history'/sid).rglob('*') if p.is_file()}==backups
            print('PASS Chromium copies native checkpoint; cloned CLI rewind restores bytes and preserves source history/backups',flush=True)
        finally:
            login.unlink(missing_ok=True)
            assert digest(settings)==before,'daily Claude configuration changed'


if __name__=='__main__':main()
