#!/usr/bin/env python3
"""Droid CLI catalog, invocation overlay and update checks via Node/Hub Chromium."""
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace

HELP = '''Available Models:
  fake-droid-a                 Droid Fixture A (default)
  fake-droid-b                 Droid Fixture B
Model details:
  - Droid Fixture A: supports reasoning: Yes; supported: [off, low, high]; default: off
  - Droid Fixture B: supports reasoning: Yes; supported: [low, medium]; default: low
'''


def fake():
    args=sys.argv[2:]; log=Path(os.environ['SESSIONDOCK_FAKE_DROID_LOG']); state=log.parent
    if args[-1:]==['--version']:print((state/'version').read_text());return
    if args[-2:]==['exec','--help']:print(HELP);return
    if args[-2:]==['update','--check']:
        print('Droid Update\nCurrent version: '+(state/'version').read_text())
        print('Already up to date!' if (state/'version').read_text()=='1.7.0' else 'Update available: 1.6.0 → 1.7.0');return
    if args[-1:]==['update']:(state/'version').write_text('1.7.0');print('Updated to 1.7.0');return
    overlay=json.loads(Path(args[len(args)-1-args[::-1].index('--settings')+1]).read_text()) if '--settings' in args else None
    with log.open('a') as f:f.write(json.dumps({'type':'cli_start','args':args,'overlay':overlay})+'\n')
    print('Droid CLI ready',flush=True)
    for line in sys.stdin:print(line.strip(),flush=True)


def check(page,base,f,shots,hub):
    from playwright.sync_api import expect
    from new_session_model_browser import open_dialog,choose_model
    from codebuddy_cli_browser import wait_start
    from client_update_browser import open_machines
    page.goto(base,wait_until='networkidle')
    open_machines(page)
    cell=page.locator('td[data-client-source="droid"]')
    expect(cell).to_have_attribute('data-state','outdated',timeout=20000)
    expect(cell.locator('.client-version')).to_have_text('1.6.0')
    cell.locator('.client-update').click()
    expect(cell.locator('.client-version')).to_have_text('1.7.0',timeout=20000)
    page.screenshot(path=str(shots/'clients-wide-light.png'),full_page=True)
    page.locator('.settings-tab[data-tab="features"]').click()
    page.locator('[data-agent-start-mode="droid"]').select_option('cli')
    page.locator('#settings-dialog .modal-close').click()
    open_dialog(page)
    page.locator('#new-session-form label:has(input[value="droid"])').click()
    page.locator('#new-cwd').fill(str(f.cwd))
    choose_model(page,'Droid Fixture B')
    expect(page.locator('#new-effort option')).to_have_text(['low','medium'])
    page.locator('#new-effort').select_option('medium')
    page.screenshot(path=str(shots/'picker-wide-light.png'),full_page=True)
    page.set_viewport_size({'width':390,'height':844});page.emulate_media(color_scheme='dark')
    page.screenshot(path=str(shots/'picker-390-dark.png'),full_page=True)
    with page.expect_response(lambda r:r.url.endswith('/api/term/create')) as response:page.locator('#new-session-go').click()
    assert response.value.ok,response.value.text()
    record=wait_start(f,lambda r:r.get('overlay') is not None)
    assert record['overlay']=={'model':'fake-droid-b','reasoningEffort':'medium','sessionDefaultSettings':{'model':'fake-droid-b','reasoningEffort':'medium'},'outputStyle':'concise'},record
    assert '--model' not in record['args'] and '--reasoning-effort' not in record['args'],record
    assert json.loads(f.overlay.read_text())=={'outputStyle':'concise','sessionDefaultSettings':{'model':'fake-droid-a'}},'profile overlay mutated'
    expect(page.locator('#termpane')).to_be_visible()


def main():
    from playwright.sync_api import sync_playwright
    from droid_sdk_fixtures import DroidFixture
    from history_fixtures import BINARY
    from private_hosts import private_hosts
    from node_auth_fixtures import node_env,free_port,TOKEN
    from hub_fixtures import Hub
    parser=argparse.ArgumentParser();parser.add_argument('--screenshots',type=Path,default=Path('/tmp/sessiondock-droid-clients-browser'));args=parser.parse_args()
    for hub_mode in (False,True):
        with tempfile.TemporaryDirectory(prefix='sessiondock-droid-clients-') as root,ExitStack() as stack:
            f=DroidFixture(root);(f.root/'version').write_text('1.6.0')
            f.overlay=f.root/'profile-settings.json';f.overlay.write_text(json.dumps({'outputStyle':'concise','sessionDefaultSettings':{'model':'fake-droid-a'}}))
            config=json.loads(f.launcher.read_text());config['host_binary']=str(BINARY.with_name('ptyhost'))
            config['profiles'][0]['args']=[str(Path(__file__).resolve()),'--fake-cli','--settings',str(f.overlay)]
            f.launcher.write_text(json.dumps(config));stack.enter_context(private_hosts(f.root/'host'))
            port=free_port();extra=node_env(Path(root),port,'127.0.0.0/8') if hub_mode else None
            base=stack.enter_context(f.server(BINARY,extra))
            if hub_mode:
                hroot=f.root/'hub';hroot.mkdir();hub=Hub(BINARY.with_name('sessiondock-hub'),hroot,[SimpleNamespace(name='DroidClients',token=TOKEN,port=port)])
                hub.start();stack.callback(hub.stop);base=f'http://127.0.0.1:{hub.port}'
            shots=args.screenshots/('hub' if hub_mode else 'node');shots.mkdir(parents=True,exist_ok=True)
            with sync_playwright() as p:
                browser=p.chromium.launch(headless=True);page=browser.new_page(viewport={'width':1280,'height':900});errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
                check(page,base,f,shots,hub_mode);assert not errors,errors;browser.close()
            print('PASS Droid clients', 'hub' if hub_mode else 'node',flush=True)


if __name__=='__main__':
    fake() if '--fake-cli' in sys.argv else main()
