#!/usr/bin/env python3
"""Missing SDK host installation restores the native model picker on node/Hub.

Private deployment prefix, real deployment handler, synthetic Claude protocol;
no real provider or production data.
"""
import argparse
from contextlib import ExitStack
from pathlib import Path
import shutil
import sys
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from claude_sdk_fixtures import ClaudeFixture
from frontend_paths import frontend_dir
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from new_session_model_browser import open_dialog
from node_auth_fixtures import TOKEN, node_env

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'deploy'))
from sdtargets.base import Artifacts, DeployOptions, ShellError, Target
from sdtargets.linux import LinuxNodeHandler
from sdtargets.macos import MacOSNode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sdkhost', type=Path, default=Path('target/debug/sdkhost'))
    parser.add_argument('--screenshots', type=Path,
                        default=Path(tempfile.gettempdir()) / 'sessiondock-sdkhost-install')
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-host-install-') as tmp, ExitStack() as stack:
        root = Path(tmp)
        fixture = ClaudeFixture(root / 'peer')
        live = root / 'live'
        (live / 'bin').mkdir(parents=True)
        (live / 'etc').mkdir()
        shutil.copytree(frontend_dir(), live / 'web')
        binary = live / 'bin/sessiondock'
        shutil.copy2(args.binary, binary)
        artifact = Artifacts('a' * 40, 'aaaaaaa', False, '2026-10-20T00:00:00+00:00',
                             live / 'web', {'sdkhost': args.sdkhost.resolve(strict=True)}, None)
        opts = DeployOptions(bin_only=True)
        # Both Unix deployment paths must refuse before uploading or restarting
        # a node without its required executable. Explicit installation fixes it.
        for kind, cls in [('linux-node', LinuxNodeHandler), ('macos-node', MacOSNode)]:
            handler = cls(Target('fixture', kind, str(live)), artifact, opts, lambda _: None)
            try:
                handler.stage()
            except ShellError as error:
                assert '--with-sdkhost' in error.out, error.out
            else:
                raise AssertionError(f'{kind} accepted a missing host')
        assert not list(live.glob('backup-deploy-*'))
        assert not list((live / 'bin').glob('*.new'))
        print('PASS Linux/macOS missing-host preflight', flush=True)

        extra = node_env(fixture.root, free_port(), '127.0.0.0/8')
        base = stack.enter_context(fixture.server(binary, extra))
        hub_root = root / 'hub'
        hub_root.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                  [SimpleNamespace(name='ModelNode', token=TOKEN,
                                   port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
        hub.start()
        stack.callback(hub.stop)
        pw = stack.enter_context(sync_playwright())
        browser = pw.chromium.launch(headless=True)
        stack.callback(browser.close)
        pages = []
        for mode, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
            print(f'Opening {mode} fixture', flush=True)
            page = browser.new_page(viewport={'width': 1440, 'height': 900})
            page.goto(url, wait_until='networkidle')
            page.wait_for_function('T.listLoaded')
            pages.append((mode, page))
        failure = pages[0][1].request.post(base + '/api/native-sdk/models', data={'source': 'claude'})
        assert failure.status == 502 and 'claude_sdk_host' in failure.json()['error']
        assert not fixture.frames('spawn'), 'missing host must not start the CLI'

        # Publish only the missing component with the production handler.
        target = Target('fixture', 'linux-node', str(live), binaries=['sdkhost'])
        installer = LinuxNodeHandler(target, artifact, opts, lambda _: None)
        installer.stage()
        installer.backup()
        installer.swap()
        assert (live / 'bin/sdkhost').is_file()
        for kind, cls in [('linux-node', LinuxNodeHandler), ('macos-node', MacOSNode)]:
            handler = cls(Target('fixture', kind, str(live)), artifact, opts, lambda _: None)
            handler.check_sdkhost_dependency(['sessiondock'])
        mac = MacOSNode(Target('fixture', 'macos-node', str(live),
                              service={'launchd_label': 'fixture'},
                              extra={'source_dir': str(root / 'mac-source')}), artifact,
                               DeployOptions(with_sdkhost=True), lambda _: None)
        assert 'sdkhost' in mac.bins
        assert any('sdkhost.new' in line for line in mac.plan())

        for mode, page in pages:
            print(f'Checking restored {mode} picker', flush=True)
            for width, theme in [(1440, 'light'), (390, 'dark')]:
                page.set_viewport_size({'width': width, 'height': 900})
                page.emulate_media(color_scheme=theme)
                page.evaluate('new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)))')
                page.wait_for_timeout(40)
                open_dialog(page)
                expect(page.locator('#new-model')).to_be_enabled()
                page.locator('#new-model').click()
                option = page.locator('#new-model-options [role=option]')
                expect(option).to_have_count(1)
                expect(option).to_contain_text('Synthetic Claude')
                page.screenshot(path=str(args.screenshots / f'{mode}-{width}-{theme}-models.png'),
                                full_page=True)
                option.click()
                expect(page.locator('#new-model-label')).to_have_text('Synthetic Claude')
                page.locator('#new-effort').select_option('low')
                expect(page.locator('#new-effort')).to_have_value('low')
                page.locator('#new-session-dialog .modal-close').click()
        assert fixture.frames('spawn'), 'model catalog must reach the native fake CLI'
        assert not fixture.frames('user'), 'listing models must not create a model turn'
        assert len(list(fixture.projects.rglob('*.jsonl'))) == 1, 'listing created native history'
        print('PASS missing-host preflight and component install; node/Hub model retry, '
              'selection, effort, wide/light and narrow/dark; no model turn', flush=True)


if __name__ == '__main__':
    main()
