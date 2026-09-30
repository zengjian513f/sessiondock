#!/usr/bin/env python3
"""Operator-only Claude native fork/subagent clone probe in a temporary home.
Uses existing controlled authentication without reading/copying its contents.
This exercises offline staging; production publication is tested separately.
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
    args = parser.parse_args()
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
                    '--tools', 'Agent' if agent else '', '--model', MODEL, '--effort', 'low',
                    '--max-turns', '6', '--output-format', 'json', *extra]
            with (base/(label+'.log')).open('w') as log:
                r = subprocess.run(argv, env=env, cwd=cwd, stdout=log, stderr=subprocess.STDOUT, timeout=180)
            report = json.loads((base/(label+'.log')).read_text())
            assert r.returncode == 0, (label, report.get('result'))
            assert set(report['modelUsage']) == {MODEL}, report['modelUsage'].keys()
            print('PASS Claude native '+label, flush=True)
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
            run('parent', 'Reply exactly TRANSFER_PARENT.', ['--session-id', parent])
            run('fork', 'Reply exactly TRANSFER_FORK.', ['--resume', parent, '--fork-session', '--session-id', fork])
            run('agent', 'Use the Agent tool exactly once with subagent_type general-purpose and model haiku. '
                'Ask it to reply exactly TRANSFER_CHILD without using tools. After its result reply TRANSFER_AGENT_DONE.',
                ['--resume', parent, '--allowedTools', 'Agent'], True)
            assert any(home.glob('projects/*/*/subagents/*.jsonl'))
            models(); original = fingerprint()
            path = next(home.glob('projects/*/'+parent+'.jsonl'))
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
