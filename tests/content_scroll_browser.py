#!/usr/bin/env python3
"""Actual wheel acceptance for information/resources, with persisted diagnostics."""
import argparse
import json
from pathlib import Path
import tempfile
import uuid
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
from terminal_input_browser import host, open_console
from audit_browser import audit_lines, wait_for_events
from content_scroll_fixtures import route_content, wheel_to_edge, click_info_control


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path(tempfile.gettempdir())/'sessiondock-content-scroll-screenshots')
    parser.add_argument('--baseline', action='store_true')
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-content-scroll-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        for name in ['host', 'work', 'audit', 'state']:
            (root/name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id':sid, 'cwd':str(root/'work')}),
                   codex_message('user', '信息和资源滚动 Chinese / English synthetic session')], [])
        with host(root, 'synthetic-'+uuid.uuid4().hex, corpus.uid(sid)), isolated_server(
                corpus, args.binary, host_dir=root/'host', audit_dir=root/'audit', state_dir=root/'state') as (base, _):
            browser = pw.chromium.launch(headless=True)
            page = browser.new_page(viewport={'width':1280, 'height':800})
            errors = []; page.on('pageerror', lambda e: errors.append(str(e)))
            route_content(page)
            page.goto(base, wait_until='networkidle'); open_console(page, corpus.uid(sid))
            failures = []
            for width, theme, scale in [(1280,'light',100), (390,'light',100),
                                         (390,'dark',125), (1280,'dark',150)]:
                # Use persisted supported preferences on navigation, not DOM mutation.
                preference_script = f"localStorage.setItem('sessiondock.theme', JSON.stringify('{theme}')); localStorage.setItem('sessiondock.interfaceScale', JSON.stringify({scale}));"
                page.evaluate(preference_script)
                page.set_viewport_size({'width':width,'height':800})
                page.reload(wait_until='networkidle')
                if not page.locator('#a-term').is_visible():
                    page.locator('#side .item').first.click()
                page.locator('#a-term').click()
                page.wait_for_function('T.ws?.readyState === 1')
                assert page.evaluate('interfaceScale()') == scale
                assert page.locator('html').get_attribute('data-theme') == theme
                for kind in ['info','resources','info']:
                    case = f'{width}-{theme}-{scale}-{kind}'
                    page.locator('#a-'+kind).click()
                    expect(page.locator('#termpane')).to_be_hidden()
                    page.wait_for_function('T.ws?.readyState === 1')
                    if kind == 'info':
                        frame = page.frame_locator('#session-content-view iframe')
                        expect(frame.locator('#info-predecessors .si-relation')).to_have_count(35)
                        target = frame.locator('html')
                    else:
                        expect(page.locator('.sr-node')).to_have_count(6)
                        target = page.locator('.sr-embedded')
                    head = page.locator('.dhead').bounding_box()
                    info_head = frame.locator('.si-header').bounding_box() if kind == 'info' else None
                    assert page.locator('html').evaluate('e => e.scrollWidth <= innerWidth + 1')
                    page.screenshot(path=str(args.screenshots/(case+'-before.png')))
                    try:
                        before, after = wheel_to_edge(page, target)
                        assert page.locator('.dhead').bounding_box() == head
                        if kind == 'info':
                            assert abs(frame.locator('.si-header').bounding_box()['y'] - info_head['y']) < 1
                        page.screenshot(path=str(args.screenshots/(case+'-bottom.png')))
                        print('PASS wheel', case, before, after, flush=True)
                        wheel_to_edge(page, target, -1)
                    except AssertionError as error:
                        page.screenshot(path=str(args.screenshots/(case+'-failed.png')))
                        print('FAIL', case, error, flush=True)
                        if not args.baseline: raise
                        failures.append(case)
                if args.baseline: break
                # Initial load failure and explicit refresh retry in each view.
                for kind in ['resources', 'info']:
                    page.route('**/api/session/'+('info' if kind=='info' else 'resources')+'?*',
                        lambda route: route.fulfill(status=503, json={'error':'synthetic unavailable'}), times=1)
                    page.locator('#a-'+kind).click()
                    if kind == 'info':
                        frame = page.frame_locator('#session-content-view iframe')
                        expect(frame.locator('#info-status')).to_contain_text('读取失败')
                        click_info_control(page, '#info-refresh')
                        expect(frame.locator('#info-predecessors .si-relation')).to_have_count(35)
                        target = frame.locator('html')
                    else:
                        expect(page.locator('.sr-content')).to_contain_text('暂不可用')
                        page.locator('.sr-refresh').click()
                        expect(page.locator('.sr-node')).to_have_count(6)
                        target = page.locator('.sr-embedded')
                    wheel_to_edge(page, target)
                    page.screenshot(path=str(args.screenshots/f'{width}-{theme}-{scale}-{kind}-retry.png'))
                print('PASS load failure/retry', width, theme, scale, flush=True)
            if args.baseline:
                assert failures, 'expected old-code failure'
                print('BASELINE reproduced', failures, flush=True)
            else:
                # A fresh context prevents Chromium from reusing a parsed stylesheet.
                fault = browser.new_page(viewport={'width':1280, 'height':800})
                route_content(fault)
                def blocked_css(route):
                    response = route.fetch()
                    route.fulfill(response=response, body=response.text() + '\nhtml.session-info-root { overflow-y: hidden; }')
                fault.route('**/*session-info.css*', blocked_css)
                fault.goto(base, wait_until='networkidle')
                fault.locator('#side .item').first.click()
                fault.locator('#a-info').click()
                frame = fault.frame_locator('#session-content-view iframe')
                expect(frame.locator('#info-predecessors .si-relation')).to_have_count(35)
                assert frame.locator('html').evaluate('e => getComputedStyle(e).overflowY') == 'hidden'
                try:
                    wheel_to_edge(fault, frame.locator('html'))
                except AssertionError as error:
                    assert 'wheel did not scroll' in str(error)
                else:
                    raise AssertionError('fault injection did not block wheel')
                fault.locator('#a-resources').click()
                for _ in range(100):
                    if any(r['event']=='browser.content_view.scroll' and not r['data']['moved']
                           and r['data']['before']['overflow_y']=='hidden' for r in audit_lines(root/'audit')):
                        break
                    fault.wait_for_timeout(100)
                else:
                    raise AssertionError('blocked wheel receipt did not reach disk')
                fault.close()  # pagehide flushes to the real audit intake
                page.bring_to_front()
                page.locator('#settings').click()
                expect(page.locator('#settings-dialog')).to_be_visible()
                # Wait for real batched disk writes, including the modal occlusion state.
                for _ in range(100):
                    rows = audit_lines(root/'audit')
                    if any(r['event']=='browser.console.button.state' and r['data'].get('occluded_by_modal') for r in rows) and any(
                            r['event']=='browser.content_view.scroll' and not r['data']['moved'] and r['data']['before']['overflow_y']=='hidden' for r in rows):
                        break
                    page.wait_for_timeout(100)
                else:
                    (args.screenshots/'failed-receipts.json').write_text(json.dumps(rows, indent=2))
                    raise AssertionError('missing persisted blocked-wheel/modal receipts')
                page.locator('#settings-dialog .modal-close').click()
                wait_for_events(page, root/'audit', {'browser.content_view.scroll', 'browser.content_view.changed'})
                rows = audit_lines(root/'audit')
                scrolls = [r['data'] for r in rows if r['event']=='browser.content_view.scroll']
                for kind in ['info','resources']:
                    assert any(d['kind']==kind and d['moved'] and d['before']['scroll_height']>d['before']['client_height'] for d in scrolls), scrolls
                snapshots = [r['data'] for r in rows if r['event']=='browser.dom.snapshot']
                assert any(d.get('view')=='info' and d['terminal']['connected']==1 and not d['terminal']['visible'] for d in snapshots)
                assert any(d.get('view')=='resources' and not d['terminal']['visible'] for d in snapshots)
                loads = [r['data'] for r in rows if r['event']=='browser.content_view.loaded']
                for kind in ['info', 'resources']:
                    assert {'error', 'ready'} <= {d['state'] for d in loads if d['kind']==kind}
                assert not [r for r in rows if r['event']=='browser.console.button.missing' and r['data'].get('occluded_by_modal')]
                assert any(d['attempts'] > 1 for d in scrolls), 'wheel frames were not coalesced'
                assert all(d['pane'] and d['before'] and d['after'] for d in scrolls)
                assert '关联会话 Chinese' not in json.dumps(scrolls, ensure_ascii=False)
                (args.screenshots/'receipts.json').write_text(json.dumps(
                    [r for r in rows if r['event'].startswith('browser.content_view.') or r['event']=='browser.dom.snapshot'], indent=2))
                print('PASS persisted visible view, connected hidden terminal, wheel movement/blockage, coalescing, retry and modal receipts', flush=True)
                assert not errors, errors
            browser.close()


if __name__ == '__main__': main()
