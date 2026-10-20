#!/usr/bin/env python3
"""Codex selections apply immediately; messages/steer cannot restore old settings."""
import argparse
import asyncio
from pathlib import Path
import tempfile
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_queue_browser import QueueServer, open_page
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


class SelectionServer(QueueServer):
    hold_controls = False

    async def _handle(self, writer, message):
        if message.get('method') == 'thread/loaded/list':
            await self.send(writer, {'id':message['id'], 'result':{'data':[THREAD], 'nextCursor':None}})
        elif message.get('method') == 'turn/settings/update':
            # Native default: this separate experimental operation is unavailable.
            await self.send(writer, {'id':message['id'], 'error':{'code':-32600,
                'message':'turn settings updates require the step_model_switching feature'}})
        elif message.get('method') == 'thread/settings/update' and self.hold_controls and not self.reject_policy:
            await self.send(writer, {'id':message['id'], 'result':{}})
        else:
            if message.get('method') == 'turn/start':
                self.active_settings = self.settings.copy()
            await super()._handle(writer, message)

    def publish(self, changes=None):
        async def update():
            self.settings.update({k:v for k,v in (changes or {}).items() if k != 'threadId'})
            await self.notify('thread/settings/updated', {'threadSettings':self.settings.copy()})
        asyncio.run_coroutine_threadsafe(update(), self.loop).result(timeout=5)


def exercise(browser, base, fake, width, theme, screenshots, label):
    fake.settings.update(model='fake-sol', effort='high', serviceTier='priority',
        approvalPolicy='on-request', sandboxPolicy={'type':'readOnly'},
        collaborationMode={'mode':'default', 'settings':{'model':'fake-sol', 'reasoning_effort':'high'}})
    context, page = open_page(browser, base, width, theme)
    fake.publish()
    expect(page.locator('#sdk-mode')).to_have_value('default')
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    box, send = page.locator('#sdk-input'), page.locator('#sdk-send')
    starts = len(fake.methods('turn/start'))
    box.fill('草稿在立即修改设置时保留 / ' + label)
    fake.hold_controls = True
    for selector, value, field in [
        ('sdk-model','fake-luna','model'), ('sdk-effort','medium','effort'),
        ('sdk-mode','plan','collaborationMode'), ('sdk-tier','default','serviceTier'),
        ('sdk-control-approval','never','approvalPolicy'), ('sdk-control-sandbox','workspaceWrite','sandboxPolicy'),
    ]:
        control = page.locator('#' + selector)
        if not control.is_visible(): page.locator('#sdk-more').click()
        with page.expect_response('**/api/codex-sdk/controls/settings'):
            control.select_option(value)
        params = fake.methods('thread/settings/update')[-1]['params']
        assert field in params, params
        expect(control).to_have_value(value); expect(control).to_be_disabled()
        expect(send).to_be_disabled()
        fake.publish()  # An unrelated old snapshot is not confirmation.
        expect(control).to_have_value(value); expect(control).to_be_disabled()
        if screenshots: page.screenshot(path=str(screenshots/f'{label}-{selector}-pending.png'))
        fake.publish(params)
        expect(control).to_have_value(value); expect(control).to_be_enabled()
        expect(send).to_be_enabled()
        assert len(fake.methods('turn/start')) == starts, 'changing an option submitted a model prompt'
        expect(box).to_have_value('草稿在立即修改设置时保留 / ' + label)
        page.keyboard.press('Escape')
    fake.hold_controls = False
    fake.reject_policy = True
    control = page.locator('#sdk-model')
    if not control.is_visible(): page.locator('#sdk-more').click()
    control.select_option('fake-sol')
    expect(page.locator('#sdk-status-menu')).to_contain_text('Synthetic policy rejected')
    expect(control).to_have_value('fake-luna'); expect(control).to_be_enabled()
    fake.reject_policy = False
    page.keyboard.press('Escape')
    def submit(text):
        box.fill(text); send.click(); expect(box).to_have_value('')
    submit('working with immediately selected model ' + label)
    expect(send).to_have_text('排队')
    active_turn = fake.turn
    assert fake.active_settings['model'] == 'fake-luna'
    submit('queued before another model change ' + label)
    box.fill('运行中切模型时保留的草稿 / ' + label)
    control = page.locator('#sdk-model')
    if not control.is_visible(): page.locator('#sdk-more').click()
    with page.expect_response('**/api/codex-sdk/controls/settings') as response:
        control.select_option('fake-sol')
    assert response.value.ok, response.value.text()
    expect(control).to_be_enabled()
    expect(control).to_have_value('fake-sol')
    expect(box).to_have_value('运行中切模型时保留的草稿 / ' + label)
    assert fake.settings['model'] == 'fake-sol'
    assert fake.turn == active_turn and fake.active_settings['model'] == 'fake-luna'
    assert not fake.methods('turn/settings/update')
    # A genuine rejection during a turn restores the confirmed selection.
    fake.reject_policy = True
    control.select_option('fake-luna')
    expect(page.locator('#sdk-status-menu')).to_contain_text('Synthetic policy rejected')
    expect(control).to_have_value('fake-sol'); expect(control).to_be_enabled()
    assert fake.settings['model'] == 'fake-sol' and fake.turn == active_turn
    if screenshots: page.screenshot(path=str(screenshots/f'{label}-running-rejected.png'))
    fake.reject_policy = False
    page.keyboard.press('Escape')
    queued = page.locator('.sdk-queued').filter(has_text='queued before')
    queued.locator('[data-queue-action="steer"]').click()
    expect(queued).to_have_count(0)
    assert fake.settings['model'] == 'fake-sol'
    submit('next ordinary turn uses current native settings ' + label)
    fake.complete()
    expect(page.locator('.sdk-live')).to_contain_text('next ordinary turn')
    assert fake.active_settings['model'] == 'fake-sol'
    for row in fake.methods('turn/start'):
        assert 'model' not in row['params'] and 'effort' not in row['params'], row
    assert fake.settings['model'] == 'fake-sol' and fake.settings['effort'] == 'medium'
    starts = len(fake.methods('turn/start'))
    box.fill('/effort high'); send.click(); expect(box).to_have_value('')
    expect(page.locator('#sdk-effort')).to_have_value('high')
    assert fake.settings['effort'] == 'high' and fake.active_settings['effort'] == 'medium'
    for command, selector, value in [
        ('/model fake-luna', '#sdk-model', 'fake-luna'),
        ('/model fake-sol', '#sdk-model', 'fake-sol'),
    ]:
        with page.expect_response('**/api/codex-sdk/controls/settings') as response:
            box.fill(command); send.click()
        assert response.value.ok, response.value.text()
        expect(box).to_have_value(''); expect(page.locator(selector)).to_have_value(value)
    tier = page.locator('#sdk-tier')
    if not tier.is_visible(): page.locator('#sdk-more').click()
    with page.expect_response('**/api/codex-sdk/controls/settings') as response:
        tier.select_option('priority')
    assert response.value.ok, response.value.text()
    expect(tier).to_be_enabled(); expect(tier).to_have_value('priority')
    assert fake.settings['serviceTier'] == 'priority'
    assert fake.active_settings['serviceTier'] == 'default'
    page.keyboard.press('Escape')
    assert len(fake.methods('turn/start')) == starts, 'slash setting became a prompt'
    if screenshots: page.screenshot(path=str(screenshots/f'{label}-confirmed.png'))
    # Native settings confirmation can arrive after the current turn ends.
    fake.hold_controls = True
    control = page.locator('#sdk-model')
    if not control.is_visible(): page.locator('#sdk-more').click()
    with page.expect_response('**/api/codex-sdk/controls/settings') as response:
        control.select_option('fake-luna')
    assert response.value.ok, response.value.text()
    expect(control).to_be_disabled()
    fake.complete(); expect(send).to_have_text('发送')
    fake.publish(fake.methods('thread/settings/update')[-1]['params'])
    expect(control).to_be_enabled(); expect(control).to_have_value('fake-luna')
    fake.hold_controls = False
    page.keyboard.press('Escape')
    submit('next turn observes confirmed settings ' + label)
    expect(send).to_have_text('排队')
    assert fake.active_settings['model'] == 'fake-luna'
    assert fake.active_settings['effort'] == 'high'
    assert fake.active_settings['serviceTier'] == 'priority'
    fake.complete(); expect(send).to_have_text('发送')
    assert not fake.methods('turn/settings/update')
    assert not fake.methods('turn/interrupt')
    page.reload(wait_until='networkidle')
    if not page.locator('#sdk-input').is_visible(): page.locator('#a-sdk').click()
    expect(page.locator('#sdk-model')).to_have_value('fake-luna')
    expect(page.locator('#sdk-effort')).to_have_value('high')
    assert not errors, errors
    context.close()
    print(f'PASS {label}: six immediate settings, no prompt, delayed confirmation/rejection, queue/steer/new turn/reload', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    hub_binary = args.hub_binary or args.binary.with_name('sessiondock-hub')
    if not args.hub_binary and not hub_binary.is_file():
        hub_binary = Path(__file__).resolve().parents[1] / 'target/release/sessiondock-hub'
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-immediate-') as tmp, sync_playwright() as pw:
        root = Path(tmp); corpus = build(root); port = free_port()
        env = {**environment(root,args.binary), **node_env(root,port,'127.0.0.0/8')}
        browser = pw.chromium.launch(headless=True)
        try:
            with SelectionServer(root/'app.sock',corpus.paths['sdk'],THREAD) as fake:
                with server(corpus,args.binary,root/'app.sock',env) as base:
                    hub_root = root/'hub'; hub_root.mkdir()
                    hub = Hub(hub_binary.resolve(),hub_root,
                              [SimpleNamespace(name='ModelNode',token=TOKEN,port=port)])
                    hub.start()
                    try:
                        for mode,url,width,theme in [('node',base,1280,'light'),('node',base,390,'dark'),
                            ('hub',f'http://127.0.0.1:{hub.port}',1280,'dark'),('hub',f'http://127.0.0.1:{hub.port}',390,'light')]:
                            exercise(browser,url,fake,width,theme,args.screenshots,f'{mode}-{width}-{theme}')
                    finally: hub.stop()
        finally: browser.close()


if __name__ == '__main__': main()
