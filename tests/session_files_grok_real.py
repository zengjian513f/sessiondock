#!/usr/bin/env python3
"""Operator-only Grok native family clone experiment in a private GROK_HOME.
Offline staging is not production publication. Requires an existing controlled
Grok login; no authentication contents or everyday model defaults are changed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import uuid

MODEL='grok-4.6'
EFFORT='low'


def fingerprint(paths):
    return {str(p):hashlib.sha256(p.read_bytes()).hexdigest()
            for directory in paths for p in directory.rglob('*') if p.is_file()}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--grok',type=Path,default=Path.home()/'.local/bin/grok')
    parser.add_argument('--transfer-binary',type=Path,default=Path('target/debug/sessiondock-transfer'))
    parser.add_argument('--report',type=Path,default=Path('target/session-files-grok-report.json'))
    args=parser.parse_args()
    real=Path(os.environ.get('GROK_HOME',Path.home()/'.grok'))
    auth=real/'auth.json';config=real/'config.toml'
    if not auth.is_file():parser.error('Existing controlled Grok login required')
    config_before=hashlib.sha256(config.read_bytes()).hexdigest() if config.exists() else None
    with tempfile.TemporaryDirectory(prefix='sessiondock-grok-clone-real-') as tmp:
        base=Path(tmp);home=base/'home';home.mkdir(mode=0o700);cwd=base/'cwd';cwd.mkdir()
        login=home/'auth.json';login.symlink_to(auth.resolve())
        env=dict(os.environ,GROK_HOME=str(home))
        def run(label,prompt,extra):
            argv=[str(args.grok),'-p',prompt,'-m',MODEL,'--reasoning-effort',EFFORT,
                  '--cwd',str(cwd),'--max-turns','8','--disable-web-search','--output-format','json',
                  '--leader-socket',str(base/'leader.sock'),*extra]
            with (base/(label+'.log')).open('w') as log:
                result=subprocess.run(argv,env=env,stdout=log,stderr=subprocess.STDOUT,timeout=180)
            assert result.returncode==0,(label,result.returncode)
        def command(request):
            r=subprocess.run([str(args.transfer_binary.resolve())],input=json.dumps(request),capture_output=True,text=True,timeout=90)
            assert r.returncode==0,r.stdout
            return json.loads(r.stdout)
        def summaries():
            return {json.loads(p.read_text())['info']['id']:p for p in home.glob('sessions/*/*/summary.json')}
        def models():
            for p in summaries().values():
                row=json.loads(p.read_text());assert row['current_model_id']==MODEL and row['reasoning_effort']==EFFORT
                for line in (p.parent/'chat_history.jsonl').read_text().splitlines():
                    item=json.loads(line)
                    if item.get('model_id'):
                        assert item['model_id'] in (MODEL,'grok-4.6-build'),item['model_id']
                        assert item['reasoning_effort']==EFFORT,item.get('reasoning_effort')
        try:
            parent=str(uuid.uuid4());fork=str(uuid.uuid4())
            run('parent','Reply exactly TRANSFER_PARENT.',['--session-id',parent])
            run('fork','Reply exactly TRANSFER_FORK.',['--resume',parent,'--fork-session','--session-id',fork])
            run('agent','This is an isolated session storage test. Use spawn_subagent exactly once with model grok-4.6, '
                'asking it to reply exactly TRANSFER_CHILD without tools. Wait for it and reply TRANSFER_AGENT_DONE. '
                'Do not read or modify files or run commands.',['--resume',parent,'--always-approve'])
            original=summaries();assert len(original)==3
            child=next(sid for sid,p in original.items() if json.loads(p.read_text()).get('session_kind')=='subagent')
            models();before=fingerprint([p.parent for p in original.values()])
            selected='grok:'+hashlib.sha1(str(original[parent].parent).encode()).hexdigest()[:16]
            plan=command({'operation':'plan_files','roots':{'grok':str(home/'sessions')},'uid':selected,'new_ids':True})
            assert {m['sid'] for m in plan['group']['members']}=={parent,fork,child}
            stage=base/'stage';result=command({'operation':'stage_files','plan':plan,'destination':str(stage)})
            assert result['publishable'] is False
            # Experimental importer only inside this test-created home.
            for project in (stage/'grok').iterdir():
                for session in project.iterdir():shutil.copytree(session,home/'sessions'/project.name/session.name)
            assert fingerprint([p.parent for p in original.values()])==before
            ids=plan['sessions'];cloned_parent=ids['grok:'+parent];cloned_child=ids['grok:'+child]
            copied=summaries();assert set(copied)==set(original)|{ids['grok:'+sid] for sid in original}
            meta=json.loads((copied[cloned_parent].parent/'subagents'/cloned_child/'meta.json').read_text())
            assert meta['parent_session_id']==cloned_parent and meta['child_session_id']==cloned_child
            assert json.loads(copied[ids['grok:'+fork]].read_text())['parent_session_id']==cloned_parent
            for label,new in [('parent',cloned_parent),('child',cloned_child)]:
                run('clone-'+label,'Reply exactly CLONE_CONTINUED.',['--resume',new])
                assert 'CLONE_CONTINUED' in (copied[new].parent/'chat_history.jsonl').read_text()
                assert fingerprint([p.parent for p in original.values()])==before
                print('PASS real Grok cloned '+label+' resumes with new identity; original family unchanged',flush=True)
            clones_before=fingerprint([copied[ids['grok:'+sid]].parent for sid in original])
            run('original-continue','Reply exactly SOURCE_CONTINUED.',['--resume',parent])
            assert fingerprint([copied[ids['grok:'+sid]].parent for sid in original])==clones_before
            models()
            report={'model':MODEL,'effort':EFFORT,'native_model':'grok-4.6-build',
                'family_members':3,'checks':['native_fork','native_subagent','whole_family','new_ids',
                'native_parent_and_child_resume','independent_continuation','unchanged_source'],
                'production_publish':False,'cross_machine':False}
            args.report.parent.mkdir(parents=True,exist_ok=True);args.report.write_text(json.dumps(report,indent=2)+'\n')
        finally:
            login.unlink(missing_ok=True)
            assert (hashlib.sha256(config.read_bytes()).hexdigest() if config.exists() else None)==config_before,'daily config changed'


if __name__=='__main__':main()
