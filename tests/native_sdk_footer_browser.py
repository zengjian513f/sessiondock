#!/usr/bin/env python3
"""Four native SDK footers: bounded labels, model selection and usage disclosure."""
import argparse
from contextlib import contextmanager
import json
import html
import os
import re
from pathlib import Path
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace
from unittest.mock import patch

from playwright.sync_api import expect, sync_playwright
from agy_sdk_fixtures import AgyFixture
from claude_sdk_fixtures import ClaudeFixture
from codex_sdk_browser import server
from grok_sdk_fixtures import GrokFixture
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from opencode_sdk_fixtures import FakeOpenCode
from sdk_usage_fixtures import assert_inline_usage, assert_single_status_row, toggle_usage_details
from send_browser import initialize


@contextmanager
def native_events():
    stop = threading.Event()
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header('Content-Type','text/event-stream')
            self.send_header('Access-Control-Allow-Origin','*')
            self.end_headers()
            try:
                self.wfile.write(('event: sdk\ndata: '+json.dumps(self.server.packet)+'\n\n').encode())
                self.wfile.flush()
                stop.wait()
            except (BrokenPipeError,ConnectionResetError):
                pass
        def log_message(self,*args):
            pass
    server = ThreadingHTTPServer(('127.0.0.1',0),Handler)
    server.daemon_threads = True
    server.url = f'http://127.0.0.1:{server.server_port}/events'
    worker = threading.Thread(target=server.serve_forever,daemon=True)
    worker.start()
    try:
        yield server
    finally:
        stop.set()
        server.shutdown()
        server.server_close()
        worker.join()


@contextmanager
def provider(source, root, binary):
    if source == 'claude':
        fixture = ClaudeFixture(root)
        with fixture.server(binary) as base:
            yield base
    elif source == 'grok':
        for name in ('claude', 'codex', 'grok', 'state'):
            (root / name).mkdir()
        fixture = GrokFixture(root)
        initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
        with server(SimpleNamespace(root=root), binary, 'off', {
            **fixture.env, 'SESSIONDOCK_NATIVE_SDKS': 'auto',
            'SESSIONDOCK_LIFECYCLE_DIR': str(root/'ledger'),
            'SESSIONDOCK_PTYHOST_DIR': str(root/'unused-host'),
            'SESSIONDOCK_STATE_DIR': str(root/'state'),
        }) as base:
            yield base
    elif source == 'agy':
        fixture = AgyFixture(root, REPO/'target/debug/ptyhost')
        with patch.dict(os.environ, fixture.env):
            initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
            with isolated_server(Corpus(root), binary, host_dir=root/'host',
                    lifecycle_dir=root/'ledger', state_dir=root/'state',
                    launcher_config=fixture.launcher, extra_env=fixture.extra_env) as (base, _):
                yield base
    else:
        for name in ('claude', 'codex', 'grok', 'host', 'state', 'proc'):
            (root/name).mkdir()
        (root/'proc/stat').write_text('btime 1700000000\n')
        with FakeOpenCode(root) as native:
            launcher = root/'launcher.json'
            launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO/'target/debug/ptyhost'),
                'host_dir': str(root/'host'), 'profiles': [native.profile()]}))
            initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
            with isolated_server(Corpus(root), binary, host_dir=root/'host', state_dir=root/'state',
                lifecycle_dir=root/'ledger', launcher_config=launcher, extra_env={
                    'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_OPENCODE_DB': str(native.db),
                    'SESSIONDOCK_OPENCODE_ROOT': str(root/'mirror'),
                    'SESSIONDOCK_PROC_ROOT': str(root/'proc')}) as (base, _):
                yield base


def exercise(browser, base, source, screenshots, events):
    for theme in ('light', 'dark'):
        context = browser.new_context(viewport={'width': 1440, 'height': 900},
                                      color_scheme=theme, service_workers='block', permissions=['local-network-access'])
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base+'/') else route.abort())
        def document(route):
            response=route.fetch(); body=response.text()
            match=re.search(r'(<meta name="sessiondock-capabilities" content=")(.*?)(">)',body)
            config=json.loads(html.unescape(match[2]));config.update(conversation_send=True,terminal=True)
            body=body[:match.start(2)]+html.escape(json.dumps(config),quote=True)+body[match.end(2):]
            route.fulfill(response=response,body=body)
        context.route(re.compile(re.escape(base)+r'/(?:\?.*)?$'),document)
        # Native transport fixtures remain private; stress the UI's common catalog
        # and usage response contracts with realistic long provider labels.
        long_model = 'provider/very-long-model-name-2026-10-19-thinking-preview'
        long_quota = 'Enterprise shared premium model quota 中文长名称'
        models = [{'id': 'short', 'efforts': ['low', 'high']},
                  {'id': 'long', 'displayName': long_model, 'efforts': ['low', 'high']}]
        context.route('**/api/native-sdk/open', lambda route: route.fulfill(json={
            'ok': True, 'capabilities': {'models': True,'fork':True,'compact':True,'settings':True, 'permission_mode':source=='claude','fast_mode':source=='claude','mode':source=='agy'}}))
        packet = {'type': 'snapshot', 'seq': 1, 'raw': {'running': False, 'model': 'short', 'effort':'low',
            'settings':{'permission_mode':'default','fast_mode':False,'mode':'auto'},'modes':['auto','plan'] if source=='agy' else [],
            'usage': {'tokens': {'total_tokens': 123456789},
                      'context': {'percent': 25, 'used_tokens': 50000, 'max_tokens': 200000}}}}
        events.packet = packet
        context.route('**/api/native-sdk/events?*', lambda route: route.continue_(url=events.url))
        context.route('**/api/native-sdk/models', lambda route: route.fulfill(json={'ok': True, 'raw': models}))
        context.route('**/api/native-sdk/usage', lambda route: route.fulfill(json={
            'ok': True, 'usage': {'tokens': {'total_tokens': 123456789},
                'context': {'percent': 25, 'used_tokens': 50000, 'max_tokens': 200000}},
            'quota': {'buckets': [{'group': long_quota, 'remaining_percent': 30}, {'group':long_quota,'window':'seven_day','remaining_percent':80}]}}))
        quota_rows = [{'group':'Gemini Models' if source=='agy' else source,'window':'5h','remaining_percent':80},
                      {'group':'Gemini Models' if source=='agy' else source,'window':'weekly','remaining_percent':60}]
        if source=='agy':
            quota_rows += [{'group':'Claude and GPT models','window':'5h','remaining_percent':95},
                           {'group':'Claude and GPT models','window':'7d','remaining_percent':50}]
        context.route('**/api/quotas',lambda route:route.fulfill(json={'agents':[{'source':source,'buckets':quota_rows}]}))
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible():
            # Source tabs use their native button IDs in the production frontend.
            page.locator(f'#chips button[data-source="{source}"]').click()
        row.click()
        page.locator('#a-sdk').click()
        pane = page.locator('.native-sdk-pane')
        model = pane.locator('#sdk-model')
        expect(model.locator('option[value="long"]')).to_have_count(1)
        page.wait_for_function('NativeSdkView.nativeEffort === "low"')
        expect(model).to_be_visible()
        model.select_option('long')
        expect(model).to_have_attribute('title', long_model)
        width_long = model.bounding_box()['width']
        assert width_long <= 160.5, width_long
        model.select_option('short')
        expect(page.locator('#sdk-effort option:checked')).to_have_text('低')
        page.locator('#sdk-effort').select_option('high')
        expect(page.locator('#sdk-effort option:checked')).to_have_text('高')
        assert abs(model.bounding_box()['width'] - width_long) <= 1
        page.locator('#sdk-input').fill('底栏测试草稿 retained while changing models')
        for width in (1440, 390, 320):
            page.set_viewport_size({'width': width, 'height': 900})
            if not page.locator('#sdk-input').is_visible():
                row.click()
                page.locator('#a-sdk').click()
            expect(pane.locator('.sdk-gauges')).to_contain_text('123.5M tok')
            expect(pane.locator('.sdk-gauges')).to_contain_text('上下文')
            expect(pane.locator('.sdk-gauges')).to_contain_text('7天')
            for raw_label in ('weekly','seven_day','7d'):
                expect(pane.locator('.sdk-gauges')).not_to_contain_text(raw_label)
            assert_inline_usage(page)
            assert_single_status_row(pane.locator('.sdk-status-row'))
            gauges = pane.locator('.sdk-status-row > .sdk-gauges')
            if width == 320 and gauges.evaluate('e => e.scrollWidth > e.clientWidth + 1'):
                gauges.focus()
                gauges.press('ArrowRight')
                page.wait_for_function('e => e.scrollLeft > 0', arg=gauges.element_handle())
                gauges.press('ArrowLeft')
                page.wait_for_function('e => e.scrollLeft < 1', arg=gauges.element_handle())
            labels = pane.locator('.sdk-status-row .sdk-gauge > span').evaluate_all('nodes => nodes.map(n => n.getBoundingClientRect().width)')
            assert max(labels) <= 72.5, labels
            expect(pane.locator('.sdk-status-row .sdk-gauge[title]').filter(has=page.locator('[role=progressbar]')).last).to_have_attribute('title', re.compile(long_quota))
            toggle_usage_details(page)
            expect(page.locator('#native-usage-detail')).to_contain_text('123,456,789')
            expect(page.locator('#native-usage-detail')).to_contain_text(long_quota)
            expect(page.locator('#native-usage-detail')).to_contain_text('7天')
            expect(page.locator('#native-usage-detail')).not_to_contain_text('seven_day')
            expect(page.locator('#sdk-usage-toggle')).to_have_count(0)
            expect(page.locator('#sdk-actions .btn')).to_have_count(0)
            expect(page.locator('#sdk-actions .sdk-action:visible')).to_have_count(3 if source=='opencode' else 2)
            for picker in page.locator('#sdk-more-menu > .new-effort').all():
                bounds=picker.locator('.sdk-picker-label').bounding_box()
                assert bounds['x']+bounds['width'] <= picker.locator('select').bounding_box()['x']-4
            page.screenshot(path=str(screenshots/f'{source}-menu-{width}-{theme}.png'))
            toggle_usage_details(page)
            page.locator('#sdk-more').click()
            page.keyboard.press('Escape')
            expect(page.locator('#sdk-input')).to_have_value('底栏测试草稿 retained while changing models')
            page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}.png'))
        if source=='claude':
            page.locator('#sdk-more').click()
            page.route('**/api/native-sdk/settings',lambda route:route.fulfill(json={'ok':True}))
            with page.expect_request('**/api/native-sdk/settings') as changed:
                page.locator('#native-sdk-permission_mode').select_option('plan')
            assert changed.value.post_data_json['permission_mode']=='plan'
            with page.expect_request('**/api/native-sdk/settings') as changed:
                page.locator('#native-sdk-fast_mode').select_option('true')
            assert changed.value.post_data_json['fast_mode'] is True
            page.keyboard.press('Escape')
        # All native SDKs use the same state vocabulary and disclosure behavior.
        states = [
            ('ready', '就绪', {}),
            ('working', '处理中', {'running': True}),
            ('waiting', '待批准', {'requests': [{'key':'approval-check','type':'approval','title':'Approval fixture'}]}),
            ('waiting', '待回答', {'requests': [{'key':'question-check','type':'question','title':'Question fixture'}]}),
            ('maintenance', '压缩中', {'status':'compacting'}),
            ('retrying','正在重试',{'retry':{'attempt':2}}),
            ('blocked', '错误：模型暂不可用，稍后重试。Capacity unavailable for the selected model. ' * 3,
             {'error':'模型暂不可用，稍后重试。Capacity unavailable for the selected model. ' * 3}),
        ]
        initial = dict(packet['raw'])
        for state, label, extra in states:
            packet['seq'] += 1
            packet['raw'] = {**initial, **extra}
            page.reload(wait_until='domcontentloaded')
            page.wait_for_function('T.listLoaded')
            status = page.locator('.native-sdk-pane .sdk-status')
            if not page.locator('#sdk-input').is_visible():
                page.locator('#a-sdk').click()
            expect(status).to_have_attribute('data-state', state)
            if state == 'blocked':
                expect(status).to_have_text('错误')
                expect(status).to_be_enabled()
                status.click()
                expect(page.get_by_role('dialog',name='完整状态')).to_be_visible()
                page.screenshot(path=str(screenshots/f'{source}-status-full-{theme}.png'))
                expect(page.locator('#sdk-status-menu')).to_contain_text('Capacity unavailable')
                page.keyboard.press('Escape')
            else:
                expect(status).to_have_text(label)
                if state=='retrying':
                    expect(status).to_be_enabled()
                    status.click()
                    expect(page.get_by_role('dialog',name='完整状态')).to_be_visible()
                    page.keyboard.press('Escape')
                else:
                    expect(status).to_be_disabled()
            page.screenshot(path=str(screenshots/f'{source}-status-{state}-{label[:3]}-{theme}.png'))
        uid=row.get_attribute('data-uid')
        context.route('**/api/live*',lambda route:route.fulfill(json={'known':True,'uids':[uid],'working_uids':[],'tmux_uids':[]}))
        context.route('**/api/term/list*',lambda route:route.fulfill(json={'enabled':True,'sources':{source:True},'pending':[],
            'sessions':[{'name':'footer-'+source,'uid':uid,'sid':uid.split(':',1)[1],'source':source,'instance_id':'1'*32}]}))
        context.route('**/api/session/conversation?*',lambda route:route.fulfill(json={'draft':{'revision':0,'value':None}}))
        context.route('**/api/session/conversation/check',lambda route:route.fulfill(json={'ok':True,'input':{'state':'ready','code':'','message':''},'prompt':None}))
        page.reload(wait_until='domcontentloaded');page.wait_for_function('T.listLoaded')
        for width in (1440,390,320):
            page.set_viewport_size({'width':width,'height':900})
            page.locator('#a-chat').click()
            footer=page.locator('#conversation-status')
            expect(footer).to_be_visible()
            expect(footer.locator('.sdk-gauges')).to_contain_text('5小时')
            expect(footer.locator('.sdk-gauges')).to_contain_text('7天')
            expect(footer.locator('.sdk-gauges')).not_to_contain_text('weekly')
            if source=='agy':
                expect(footer.locator('.sdk-quota-name')).to_have_text(['Gemini','Claude/GPT'])
            assert_single_status_row(footer)
            page.screenshot(path=str(screenshots/f'{source}-conversation-{width}-{theme}.png'))
            page.locator('#a-sdk').click()
        assert not errors, errors
        context.close()
    print(f'PASS {source}: compact footer, long models/quota, exact usage details, 3 widths, light/dark', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-native-footer-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in ('grok', 'agy', 'opencode', 'claude'):
                root = Path(temporary)/source
                root.mkdir()
                with provider(source, root, args.binary) as base, native_events() as events:
                    exercise(browser, base, source, args.screenshots, events)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
