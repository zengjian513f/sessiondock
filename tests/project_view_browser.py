#!/usr/bin/env python3
"""Embedded project view with native SessionDock new-session and SDK composer."""
from __future__ import annotations

import argparse
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
import tempfile
from urllib.parse import urlencode

from playwright.sync_api import expect, sync_playwright

from claude_sdk_fixtures import ClaudeFixture
from history_fixtures import BINARY
from hub_fixtures import FakeNode, Hub, free_port
from node_auth_fixtures import TOKEN, node_env


def exercise(browser, base, project, width, theme, screenshots):
    page = browser.new_page(viewport={'width': width, 'height': 760}, color_scheme=theme,
                            service_workers='block')
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.add_init_script("""(() => {
      window.removedWorkerScopes = [];
      navigator.serviceWorker.getRegistrations = async () =>
        ['/', '/other-app/'].map(path => ({
          scope: new URL(path, location.origin).href,
          unregister: async () => window.removedWorkerScopes.push(path),
        }));
    })()""")
    project_url = base + '/?' + urlencode({'embedded_project': str(project)})
    page.route(base + '/embed-fixture', lambda route: route.fulfill(
        content_type='text/html',
        body=f'<style>body{{margin:0}} iframe{{width:100%;height:100dvh;border:0}}</style>'
             f'<iframe title="project" src="{project_url}"></iframe>'))
    page.goto(base + '/embed-fixture')
    frame = page.frame_locator('iframe')
    expect(frame.locator('html')).to_have_class('project-view')
    assert frame.locator('html').evaluate('() => window.removedWorkerScopes') == ['/']
    toggle = frame.get_by_role('button', name='会话列表', exact=True)
    if width <= 720:
        expect(frame.locator('#left')).to_be_hidden()
        toggle.click()
        expect(frame.locator('#project-list-close')).to_be_focused()
    else:
        expect(toggle).to_be_hidden()
    expect(frame.locator('#side .item[data-uid^="claude:"]').first).to_be_visible()
    expect(frame.locator('#side .item').filter(has_text='Not this project')).to_have_count(0)
    expect(frame.locator('#side .cwd-path').first).not_to_be_visible()
    expect(frame.locator('#project-view-tools #new-session')).to_be_visible()
    if screenshots:
        page.screenshot(path=str(screenshots / f'{width}-{theme}-list.png'))
    frame.locator('#project-view-tools #new-session').click()
    dialog = frame.get_by_role('dialog', name='新建会话')
    expect(dialog).to_be_visible()
    expect(dialog.locator('#new-cwd')).not_to_be_visible()
    expect(dialog.locator('#new-node')).to_be_visible()
    expect(dialog.locator('#new-model')).to_be_visible()
    expect(dialog.locator('#new-effort')).to_be_visible()
    expect(dialog.locator('.new-source label').filter(has=frame.locator('input[value="claude"]'))).to_be_visible()
    dialog.locator('label:has(input[value="claude"])').click()
    expect(dialog.locator('#new-model-label')).not_to_have_text('读取模型…')
    if screenshots:
        page.screenshot(path=str(screenshots / f'{width}-{theme}-new.png'))
    with page.expect_response(lambda res: res.url.endswith('/api/native-sdk/create')) as created:
        dialog.locator('#new-session-go').click()
    assert created.value.ok, created.value.text()
    assert created.value.request.post_data_json['cwd'] == str(project)
    expect(frame.locator('#sdk-input')).to_be_visible()
    expect(frame.locator('#detail .meta-cwd').first).not_to_be_visible()
    frame.locator('#sdk-input').fill(f'Project reply fixture {width} {theme}')
    frame.locator('#sdk-send').click()
    expect(frame.locator('.sdk-live')).to_contain_text(f'Claude synthetic final Project reply fixture {width} {theme}')
    if screenshots:
        page.screenshot(path=str(screenshots / f'{width}-{theme}-reply.png'))
    expect(frame.locator('#side .item').filter(has_text='Not this project')).to_have_count(0)
    draft = frame.locator('#sdk-input')
    draft.fill('调整窗口时保留草稿')
    draft.evaluate('node => { window.originalDraft = node; }')
    header = frame.locator('#detail .dhead')
    uid = header.get_attribute('data-uid')
    if width > 720:
        expect(frame.locator('#left')).to_be_visible()
        page.locator('iframe').evaluate("node => { node.style.width = '660px'; }")
    expect(toggle).to_be_visible()
    expect(frame.locator('#left')).to_be_hidden()
    expect(draft).to_have_value('调整窗口时保留草稿')
    toggle.click()
    expect(toggle).to_have_attribute('aria-expanded', 'true')
    expect(frame.locator('#left')).to_be_visible()
    expect(frame.locator('#right')).to_have_attribute('inert', '')
    expect(frame.locator('#project-view-tools #new-session')).to_be_visible()
    close = frame.locator('#project-list-close')
    expect(close).to_be_focused()
    if screenshots:
        page.screenshot(path=str(screenshots / f'{width}-{theme}-drawer.png'))
    close.press('Escape')
    expect(toggle).to_be_focused()
    expect(frame.locator('#left')).to_be_hidden()
    expect(draft).to_have_value('调整窗口时保留草稿')
    toggle.click()
    if width > 720:
        page.locator('iframe').evaluate("node => { node.style.width = ''; }")
        expect(frame.locator('#left')).to_be_visible()
        expect(toggle).to_be_hidden()
    else:
        frame.locator('#project-list-scrim').click(position={'x': width - 8, 'y': 110})
        expect(frame.locator('#left')).to_be_hidden()
    expect(frame.locator('#right')).not_to_have_attribute('inert', '')
    expect(draft).to_have_value('调整窗口时保留草稿')
    assert draft.evaluate('node => node === window.originalDraft')
    expect(header).to_have_attribute('data-uid', uid)
    if width <= 720:
        toggle.click()
        frame.locator('#side .item').filter(has_text='earlier synthetic Claude prompt').click()
        expect(frame.locator('#left')).to_be_hidden()
        expect(frame.locator('#a-session-title')).to_have_text('earlier synthetic Claude prompt')
    page.goto(project_url)
    expect(page.locator('html')).not_to_have_class('project-view')
    expect(page.locator('#side .item').filter(has_text='Not this project')).to_have_count(1)
    assert not errors, errors
    page.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-project-view-') as tmp, ExitStack() as stack:
        root = Path(tmp)
        fixture = ClaudeFixture(root)
        other = FakeNode('b' * 32, 'Outside')
        stack.callback(other.stop)
        outside = dict(other.state()['row'], title='Not this project', cwd=str(fixture.cwd) + '-archive')
        other.set(row=outside)
        port = free_port()
        stack.enter_context(fixture.server(args.binary, node_env(root, port, '127.0.0.0/8')))
        hub_root = root / 'hub'
        hub_root.mkdir()
        native = SimpleNamespace(name='Project', token=TOKEN, port=port)
        hub = Hub(args.binary.with_name('sessiondock-hub'), hub_root, [native, other])
        hub.start()
        stack.callback(hub.stop)
        with sync_playwright() as p:
            browser = p.chromium.launch()
            for width, theme in ((1100,'light'), (390,'dark'), (1100,'dark'), (390,'light')):
                exercise(browser, f'http://127.0.0.1:{hub.port}', fixture.cwd, width, theme, args.screenshots)
            browser.close()
    print('PASS embedded project view browser')


if __name__ == '__main__':
    main()
