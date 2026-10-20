#!/usr/bin/env python3
"""Chromium observes date-version releases, restarts, cached assets and rollback."""
import argparse
from contextlib import ExitStack
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

from playwright.sync_api import expect, sync_playwright
from frontend_paths import frontend_dir
from history_fixtures import BINARY, Corpus, claude_row
from node_auth_fixtures import free_port

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'deploy'))
from sdtargets.base import Artifacts, DeployOptions, ShellError, Target
from sdtargets.linux import LinuxNodeHandler


class FixtureNode(LinuxNodeHandler):
    """Real deployment files and HTTP; only the service manager is fixture-owned."""
    def __init__(self, corpus, artifact, root):
        base = f'http://127.0.0.1:{free_port()}'
        super().__init__(Target('fixture', 'linux-node', str(root / 'live'),
                                health_url=base + '/api/meta'), artifact,
                         DeployOptions(with_ptyhost=False), lambda _: None)
        self.base, self.process = base, None
        self.logfile = (root / 'service.log').open('w')
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('SESSIONDOCK_')}
        self.env.update(SESSIONDOCK_BIND=base.removeprefix('http://'),
                        SESSIONDOCK_WEB_DIR=self.path('web'),
                        SESSIONDOCK_STATE_DIR=str(corpus.root / 'state'),
                        SESSIONDOCK_PROC_ROOT=str(corpus.root / 'proc'))
        for source in ('claude', 'codex', 'grok'):
            self.env['SESSIONDOCK_' + source.upper() + '_ROOT'] = str(corpus.root / source)

    def _probe_script(self):
        lines = []
        for line in super()._probe_script().splitlines():
            if 'is-active' in line:
                line = 'echo active=' + ('active' if self.process and self.process.poll() is None else 'inactive')
            elif 'pgrep -x ptyhost' in line:
                line = 'echo pids='
            lines.append(line)
        return '\n'.join(lines)

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            self.process.wait(timeout=10)

    def restart(self):
        self.stop()
        self.process = subprocess.Popen([self.path('bin', 'sessiondock')], env=self.env,
                                        stdout=self.logfile, stderr=self.logfile)
        assert self.sh.wait_http_json(self.t.health_url, 15), Path(self.logfile.name).read_text()[-5000:]

    def close(self):
        self.stop()
        self.logfile.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sdkhost', type=Path, default=Path('target/debug/sdkhost'))
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-version-') as tmp, ExitStack() as stack:
        root = Path(tmp)
        corpus = Corpus(root / 'corpus')
        for source in ('claude', 'codex', 'grok', 'proc', 'state'):
            (corpus.root / source).mkdir(parents=True, exist_ok=True)
        corpus.put('version', 'claude', [claude_row('version', 'user', 'u1', None, 'Version fixture')], [])
        staged_web = root / 'stage-web'
        shutil.copytree(frontend_dir(), staged_web)
        artifact = Artifacts('a' * 40, 'aaaaaaa', False, '2026-10-11T10:00:00.000001+00:00',
                             staged_web, {'sessiondock': args.binary.resolve()}, None)
        (staged_web / 'release-version.txt').write_text(artifact.version + '\n')
        # Same-size content and preserved mtimes must not prevent a dated release.
        (staged_web / 'version-probe.js').write_text('window.releaseProbe="new";')
        live = root / 'live'
        for directory in ('bin', 'etc', 'host'):
            (live / directory).mkdir(parents=True)
        shutil.copytree(staged_web, live / 'web')
        old = '20261010T100000000001Z'
        (live / 'web/release-version.txt').write_text(old + '\n')
        (live / 'web/version-probe.js').write_text('window.releaseProbe="old";')
        stamp = (staged_web / 'version-probe.js').stat().st_mtime_ns
        os.utime(live / 'web/version-probe.js', ns=(stamp, stamp))
        shutil.copy2(args.binary, live / 'bin/sessiondock')
        (live / 'bin/sessiondock.version').write_text(old + '\n')
        (live / 'etc/deployed-commit').write_text('previous-release\n')
        node = FixtureNode(corpus, artifact, root)
        stack.callback(node.close)
        node.restart()
        pw = stack.enter_context(sync_playwright())
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        stack.callback(browser.close)
        page = browser.new_page()

        def visit(version, text):
            page.goto(node.base)
            expect(page.locator('meta[name="sessiondock-build"]')).to_have_attribute('content', version)
            page.locator(f'#side .item[data-uid="{corpus.uid("version")}"]').click()
            expect(page.locator('#msgs')).to_contain_text('Version fixture')
            page.locator('#sidebar-resources-toggle').click()
            page.add_script_tag(url=f'{node.base}/version-probe.js?v={version}')
            assert page.evaluate('releaseProbe') == text
            assert page.evaluate("fetch('api/meta').then(r=>r.json()).then(d=>d.build)") == version

        visit(old, 'old')
        node.probe()
        try:
            node.stage()
        except ShellError as error:
            assert '--with-sdkhost' in error.out, error.out
        else:
            raise AssertionError('node release accepted a missing sdkhost dependency')
        assert not (live / 'bin/sessiondock.new').exists()
        assert not list(live.glob('backup-deploy-*'))
        visit(old, 'old')
        artifact.binaries['sdkhost'] = args.sdkhost.resolve(strict=True)
        node.o.with_sdkhost = True
        node.stage()
        backup = node.backup()
        node.swap()
        node.restart()
        result = node.verify()
        assert result.ok, result.detail
        node.write_marker()
        visit(artifact.version, 'new')
        node.rollback(backup)
        visit(old, 'old')
        assert (live / 'bin/sessiondock.version').read_text().strip() == old
        assert (live / 'etc/deployed-commit').read_text() == 'previous-release\n'
        # Reusing the same release is valid; no mandatory build-change gate.
        node.probe()
        node.stage()
        node.swap()
        node.restart()
        assert node.verify().ok
        node.probe()
        node.stage()
        node.swap()
        node.restart()
        assert node.verify().ok
        visit(artifact.version, 'new')
        # Old backups lack version files; rollback must remove the new markers.
        for path in ('web/release-version.txt', 'bin/sessiondock.version', 'deployed-commit'):
            (Path(backup) / path).unlink()
        node.rollback(backup)
        assert not (live / 'bin/sessiondock.version').exists()
        assert not (live / 'etc/deployed-commit').exists()
        # Source checkouts without a release marker use file dates.
        page.goto(node.base)
        dev = page.locator('meta[name="sessiondock-build"]').get_attribute('content')
        assert dev.startswith('dev-'), dev
        os.utime(live / 'web/version-probe.js', ns=(time.time_ns(), time.time_ns() + 1_000_000_000))
        node.restart()
        page.reload()
        assert page.locator('meta[name="sessiondock-build"]').get_attribute('content') != dev
        print('PASS dated release, same-mtime assets, browser cache refresh, rollback markers, repeat release and development dates', flush=True)


if __name__ == '__main__':
    main()
