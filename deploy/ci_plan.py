#!/usr/bin/env python3
"""Plan CI from changed behavior; run only the selected checks and prerequisites."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

import testplan

ROOT = Path(__file__).resolve().parents[1]


def git(*args):
    return subprocess.run(['git', *args], cwd=ROOT, capture_output=True, text=True,
                          check=True, timeout=60).stdout.strip()


def plan(base=None, head='HEAD', full=False):
    if base is None:
        base, _ = testplan.resolve_base(None, {}, head)
    full = full or len(git('rev-list', '--parents', '-n', '1', head).split()) == 1
    paths = git('diff', '--name-only', base, head).splitlines()
    names = testplan.list_suites('target/debug/sessiondock')
    patches = {p: git('diff', '--unified=0', base, head, '--', p) for p in paths if p.startswith('web/')}
    selected = testplan.plan_for('full' if full else 'affected', paths, names, patches=patches)
    if selected['unmapped']:
        raise ValueError('add affected-suite mapping for: ' + ', '.join(selected['unmapped']))
    if full:
        selected['packages'] = ['sessiondock', 'ptyhost', 'ptyhost-client', 'ptyhost-screen', 'process-links', 'resource-agent']
        selected['suites'] = [n for n in names if not n.startswith('cargo_')]
    selected['suites'] = [n for n in selected['suites'] if n not in {'brand_names_check', 'check_docs_links', 'check_agents_md'}]
    selected['base'], selected['head'] = base, git('rev-parse', head)
    platforms = ['ubuntu-24.04']
    if full or any('windows' in p.lower() for p in paths if p.startswith('crates/')):
        platforms.append('windows-latest')
    if full or any('macos' in p.lower() or 'darwin' in p.lower() for p in paths if p.startswith('crates/')):
        platforms.append('macos-latest')
    selected['platforms'] = platforms
    return selected


def event_scope():
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text()) if os.environ.get('GITHUB_EVENT_PATH') else {}
    inputs = event.get('inputs') or {}
    full = os.environ.get('GITHUB_REF', '').startswith('refs/tags/v') or str(inputs.get('full', '')).lower() == 'true'
    base = (event.get('pull_request') or {}).get('base', {}).get('sha') or event.get('before')
    if base and not base.strip('0'):
        base = git('hash-object', '-t', 'tree', '/dev/null') if os.name != 'nt' else None
    return base, full, str(inputs.get('collector', '')).lower() == 'true'


def run(argv):
    print('+ ' + ' '.join(argv), flush=True)
    subprocess.run(argv, cwd=ROOT, check=True)


def run_rust(selected):
    packages = [p for p in selected['packages'] if sys.platform.startswith('linux') or p != 'resource-agent']
    if not packages:
        return
    flags = [a for p in packages for a in ('-p', p)]
    run(['cargo', 'fmt', *flags, '--check'])
    run(['cargo', 'clippy', *flags, '--all-targets', '--locked', '--', '-D', 'warnings'])
    # Type/lint checks cover ordinary source changes. Distribution builds belong
    # to release acceptance; browser prerequisites are built by their own job.
    if selected['full']:
        run(['cargo', 'build', '--release', '--locked', *flags])


def run_browser(selected):
    if not selected['suites']:
        return
    run(['cargo', 'build', '--locked', '-p', 'sessiondock', '-p', 'ptyhost'])
    if selected['full']:
        run(['cargo', 'build', '--release', '--locked', '-p', 'sessiondock', '-p', 'ptyhost'])
    run([sys.executable, 'tests/run_validation.py', '--only', ','.join(selected['suites']),
         '--binary', 'target/debug/sessiondock', '--browser-jobs', '2', '--keep-going',
         '--json', 'target/validation/ci.json'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base')
    parser.add_argument('--full', action='store_true')
    parser.add_argument('--output', type=Path, default=Path('ci-plan.json'))
    parser.add_argument('--run', choices=['rust', 'browser'])
    args = parser.parse_args()
    if args.run:
        selected = json.loads(args.output.read_text())
        (run_rust if args.run == 'rust' else run_browser)(selected)
        return
    base, full, collector = event_scope()
    selected = plan(args.base or base, full=args.full or full)
    if collector and 'resource-agent' not in selected['packages']:
        selected['packages'].append('resource-agent')
    args.output.write_text(json.dumps(selected, indent=2) + '\n')
    print(json.dumps(selected, indent=2))
    if os.environ.get('GITHUB_OUTPUT'):
        outputs = dict(rust=bool(selected['packages']), browser=bool(selected['suites']),
                       collector='resource-agent' in selected['packages'], platforms=selected['platforms'])
        with open(os.environ['GITHUB_OUTPUT'], 'a') as file:
            for key, value in outputs.items():
                file.write(key + '=' + json.dumps(value, separators=(',', ':')) + '\n')


if __name__ == '__main__':
    main()
