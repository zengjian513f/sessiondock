#!/usr/bin/env python3
"""Operator-only Claude native fork/subagent clone probe in a temporary home.
Uses existing controlled authentication without reading/copying its contents.
Default mode exercises offline staging. --production-peer uses Chromium and
production cross-node move APIs; --new-ids also rewrites the native identities.
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

MODEL = 'claude-haiku-4-5-20251001'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--claude', type=Path, default=Path.home()/'.local/bin/claude')
    parser.add_argument('--transfer-binary', type=Path, default=Path('target/debug/sessiondock-transfer'))
    parser.add_argument('--report', type=Path, default=Path('target/session-files-claude-report.json'))
    parser.add_argument('--production-peer',help='Opt-in SSH peer for a real browser move and native resume')
    parser.add_argument('--binary',type=Path,default=Path('target/debug/sessiondock'))
    parser.add_argument('--return-before-resume',action='store_true',help='Move back through the browser before native resume on the final target')
    parser.add_argument('--new-ids',action='store_true',help='Rewrite identities during the production move')
    args = parser.parse_args()
    args.report.unlink(missing_ok=True)
    settings = Path.home()/'.claude/settings.json'
    before = hashlib.sha256(settings.read_bytes()).hexdigest() if settings.exists() else None
    with tempfile.TemporaryDirectory(prefix='sessiondock-claude-clone-real-') as tmp:
        base = Path(tmp); base.chmod(0o700)
        home = base/'home'; home.mkdir(); cwd = base/'cwd'; cwd.mkdir()
        login = home/'.credentials.json'; login.symlink_to(Path.home()/'.claude/.credentials.json')
        (home/'.claude.json').write_text(json.dumps({'hasCompletedOnboarding': True}))
        env = dict(os.environ, CLAUDE_CONFIG_DIR=str(home), HOME=str(home))
        def run(label, prompt, extra, agent=False):
            argv = [str(args.claude), '-p', prompt, '--safe-mode', '--setting-sources', '',
                    '--tools', 'Agent,SendMessage' if agent else '', '--model', MODEL, '--effort', 'low',
                    '--max-turns', '6', '--output-format', 'json', *extra]
            with (base/(label+'.log')).open('w') as log:
                r = subprocess.run(argv, env=env, cwd=cwd, stdout=log, stderr=subprocess.STDOUT, timeout=180)
            report = json.loads((base/(label+'.log')).read_text())
            assert r.returncode == 0, (label, report.get('result'))
            assert set(report['modelUsage']) == {MODEL}, report['modelUsage'].keys()
            print('PASS Claude native '+label, flush=True)
            return report
        def command(request):
            r = subprocess.run([str(args.transfer_binary.resolve())], input=json.dumps(request),
                               text=True, capture_output=True, timeout=90)
            assert r.returncode == 0, r.stdout
            return json.loads(r.stdout)
        def fingerprint():
            return {str(p): p.read_bytes() for p in home.glob('projects/**/*.jsonl')}
        def models():
            actual = {json.loads(line).get('message', {}).get('model')
                      for p in home.glob('projects/**/*.jsonl') for line in p.read_text().splitlines()
                      if json.loads(line).get('type') == 'assistant'}
            assert actual == {MODEL}, actual
        try:
            parent = str(uuid.uuid4()); fork = str(uuid.uuid4())
            run('parent', 'Review this Python helper: def add(a, b): return a + b. Explain its behavior briefly.', ['--session-id', parent])
            run('fork', 'Continue the review for negative inputs. Give one example.', ['--resume', parent, '--fork-session', '--session-id', fork])
            agent_report=run('agent', 'Use the Agent tool with subagent_type general-purpose and model haiku to get an independent review of '
                'the Python helper def add(a, b): return a + b. The reviewer should describe one boundary case without tools '
                'and explain why it matters. Summarize its review.',
                ['--resume', parent, '--allowedTools', 'Agent'], True)
            assert any(home.glob('projects/*/*/subagents/*.jsonl')),agent_report.get('result')
            agent_path=next(home.glob('projects/*/*/subagents/*.jsonl'))
            agent_id=agent_path.stem.removeprefix('agent-')
            agent_lines=len(agent_path.read_text().splitlines())
            run('agent-followup', f'Use SendMessage with to={agent_id} to continue the existing review. '
                'Ask the reviewer to explain one floating-point limitation of the helper, without tools. '
                'Wait for its answer and summarize it.', ['--resume',parent,'--allowedTools','SendMessage'],True)
            added=[json.loads(line) for line in agent_path.read_text().splitlines()[agent_lines:]]
            assert any(row.get('type')=='assistant' for row in added),'original reviewer did not resume'
            models(); original = fingerprint()
            path = next(home.glob('projects/*/'+parent+'.jsonl'))
            if args.production_peer:
                from session_native_transfer import move_native
                selected='claude:'+hashlib.sha1(str(path).encode()).hexdigest()[:16]
                report=move_native(base,home,'claude',selected,[parent,fork],args.production_peer,args.binary,args.new_ids,args.return_before_resume)
                args.report.parent.mkdir(parents=True,exist_ok=True)
                args.report.write_text(json.dumps(report,indent=2)+'\n')
                return
            plan = command({'operation': 'plan_files', 'roots': {'claude': str(home/'projects')},
                            'uid': 'claude:'+hashlib.sha1(str(path).encode()).hexdigest()[:16], 'new_ids': True})
            assert len(plan['group']['members']) == 3
            stage = base/'stage'; result = command({'operation': 'stage_files', 'plan': plan, 'destination': str(stage)})
            assert result['publishable'] is False
            # Experimental import only into this test-created native home.
            for directory in (stage/'claude').iterdir():
                shutil.copytree(directory, home/directory.name, dirs_exist_ok=True)
            assert all(Path(p).read_bytes() == raw for p, raw in original.items())
            for sid, label in [(parent, 'parent'), (fork, 'fork')]:
                clone = plan['sessions']['claude:'+sid]
                run('clone-'+label, 'Reply exactly CLONE_CONTINUED.', ['--resume', clone])
                cloned = next(home.glob('projects/*/'+clone+'.jsonl'))
                assert 'CLONE_CONTINUED' in cloned.read_text()
                assert all(Path(p).read_bytes() == raw for p, raw in original.items())
            copies = {p: raw for p, raw in fingerprint().items() if p not in original}
            run('source-continue', 'Reply exactly SOURCE_CONTINUED.', ['--resume', parent])
            assert all(Path(p).read_bytes() == raw for p, raw in copies.items())
            models()
            args.report.write_text(json.dumps({'model': MODEL, 'effort': 'low', 'family_members': 3,
                'checks': ['native_fork', 'native_subagent', 'whole_family', 'new_ids',
                           'native_parent_and_fork_resume', 'independent_continuation', 'unchanged_source'],
                'production_publish': False, 'cross_machine': False}, indent=2)+'\n')
        finally:
            login.unlink(missing_ok=True)
            assert (hashlib.sha256(settings.read_bytes()).hexdigest() if settings.exists() else None) == before


if __name__ == '__main__':
    main()
