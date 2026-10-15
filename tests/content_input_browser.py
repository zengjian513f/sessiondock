#!/usr/bin/env python3
"""Native touch, keyboard and scrollbar input in the two embedded content views.

Long fleet/relation responses are presentation fixtures, not backend persistence
proof. Navigation and diagnostics use the isolated real service. No DOM focus or
scroll writes are used; geometry reads only observe actual Chromium input.
"""
import argparse
import json
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
from content_scroll_fixtures import route_content, scroll_metrics
from browser_evidence_fixtures import run_with_evidence


def edge(metrics, direction):
    return metrics['total'] - metrics['height'] - metrics['top'] <= 2 if direction > 0 else metrics['top'] <= 1


def tab_to(page, locator):
    if locator.evaluate('e => e === document.activeElement'):
        return
    for _ in range(180):
        page.keyboard.press('Tab')
        if locator.evaluate('e => e === document.activeElement'):
            return
    raise AssertionError('Tab did not reach ' + str(locator))


def scroll_input(page, target, kind, method, direction, cdp):
    before = scroll_metrics(target)
    box = page.locator('#session-content-view iframe' if kind == 'info' else '.sr-embedded').bounding_box()
    if method == 'keyboard':
        page.keyboard.press('PageDown' if direction > 0 else 'PageUp')
        page.wait_for_timeout(250)
        assert (scroll_metrics(target)['top'] - before['top']) * direction > 0, ('page key', before, scroll_metrics(target))
        page.keyboard.press('End' if direction > 0 else 'Home')
    elif method == 'scrollbar':
        # Chromium's native scrollbar track excludes the two arrow buttons.
        scale = box['height'] / before['height']
        track = box['height'] - 32 * scale
        thumb = max(20 * scale, track * before['height'] / before['total'])
        x = box['x'] + box['width'] - 7 * scale
        y = box['y'] + 16 * scale + (thumb / 2 if direction > 0 else track - thumb / 2)
        page.mouse.move(x, y)
        page.mouse.down()
        page.mouse.move(x, box['y'] + (box['height'] - 18 * scale if direction > 0 else 18 * scale), steps=20)
        page.mouse.up()
    else:
        for _ in range(60):
            x = box['x'] + box['width'] * .6
            y = box['y'] + box['height'] * (.85 if direction > 0 else .2)
            cdp.send('Input.dispatchTouchEvent', {'type':'touchStart', 'touchPoints':[{'x':x,'y':y}]})
            for step in range(1, 13):
                cdp.send('Input.dispatchTouchEvent', {'type':'touchMove', 'touchPoints':[
                    {'x':x,'y':y-direction*box['height']*.6*step/12}]})
                page.wait_for_timeout(16)
            cdp.send('Input.dispatchTouchEvent', {'type':'touchEnd', 'touchPoints':[]})
            page.wait_for_timeout(120)
            if edge(scroll_metrics(target), direction):
                break
    for _ in range(30):
        page.wait_for_timeout(50)
        after = scroll_metrics(target)
        if edge(after, direction):
            break
    assert (after['top'] - before['top']) * direction > 0, (method, 'did not move', before, after)
    assert edge(after, direction), (method, 'did not reach edge', before, after)
    assert after['totalWidth'] <= after['width'] + 1, after
    return {'method':method, 'direction':direction, 'before':before, 'after':after}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('target/content-input'))
    args = parser.parse_args(); args.output.mkdir(parents=True, exist_ok=True)
    results = []
    with tempfile.TemporaryDirectory(prefix='sessiondock-content-input-') as tmp, sync_playwright() as pw:
        root = Path(tmp); corpus = Corpus(root)
        for name in ['state', 'audit']:
            (root/name).mkdir(mode=0o700)
        sid = 'content-input-session'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id':sid, 'cwd':str(root)}),
                    codex_message('user', '信息资源滚动 Chinese / English 长内容')], [])
        with isolated_server(corpus, args.binary, state_dir=root/'state', audit_dir=root/'audit') as (base, _):
            browser = pw.chromium.launch(headless=True, ignore_default_args=['--hide-scrollbars'])
            try:
                for width, theme, scale in [(1280,'light',100), (390,'light',100), (390,'dark',125), (1280,'dark',150)]:
                    context = browser.new_context(viewport={'width':width,'height':800}, has_touch=True)
                    context.add_init_script(f"localStorage.setItem('sessiondock.theme', JSON.stringify('{theme}'));localStorage.setItem('sessiondock.interfaceScale', JSON.stringify({scale}));")
                    page = context.new_page(); errors = []
                    page.on('pageerror', lambda e: errors.append(str(e)))
                    route_content(page); page.goto(base, wait_until='networkidle')
                    page.locator('#side .item').first.click()
                    cdp = context.new_cdp_session(page)
                    for kind in ['info', 'resources']:
                        case = f'{width}-{theme}-{scale}-{kind}'
                        page.locator('#a-'+kind).click()
                        if kind == 'info':
                            frame = page.frame_locator('#session-content-view iframe')
                            expect(frame.locator('#info-predecessors .si-relation')).to_have_count(35)
                            target = frame.locator('html'); first = frame.locator('#info-summary'); last = frame.locator('#info-predecessors .si-relation').last
                            keyboard_start = frame.locator('#info-refresh')
                        else:
                            expect(page.locator('.sr-node')).to_have_count(6)
                            target = page.locator('.sr-embedded'); first = page.locator('#sr-title'); last = page.locator('.sr-node').last
                            keyboard_start = page.locator('#sr-title')
                        head = page.locator('.dhead').bounding_box()
                        inner_head = frame.locator('.si-header').bounding_box() if kind == 'info' else None
                        assert page.locator('html').evaluate('e => e.scrollWidth <= innerWidth + 1')
                        assert scroll_metrics(target)['total'] > scroll_metrics(target)['height']
                        for method in ['keyboard', 'scrollbar', 'touch']:
                            if method == 'keyboard':
                                tab_to(page, keyboard_start)
                                if kind == 'resources':
                                    tab_to(page, page.locator('.sr-metric').first)
                                    with page.expect_response('**/api/session/resources?*'):
                                        pass
                                    page.wait_for_timeout(200)
                                    expect(page.locator('.sr-metric').first).to_be_focused()
                            for direction in [1, -1]:
                                receipt = scroll_input(page, target, kind, method, direction, cdp)
                                expect(last if direction > 0 else first).to_be_in_viewport()
                                assert page.locator('.dhead').bounding_box() == head
                                if inner_head:
                                    assert abs(frame.locator('.si-header').bounding_box()['y'] - inner_head['y']) < 1
                                page.screenshot(path=str(args.output/f'{case}-{method}-{direction}.png'))
                                results.append({'case':case, 'kind':kind, **receipt})
                                (args.output/'inputs.json').write_text(json.dumps(results, indent=2))
                            print('PASS', case, method, flush=True)
                        if kind == 'info':
                            tab_to(page, frame.locator('#add-predecessor'))
                            page.keyboard.press('Enter')
                            tab_to(page, frame.locator('#relation-search'))
                            page.keyboard.type('synthetic query')
                            before = scroll_metrics(target)['top']
                            page.keyboard.press('Home'); page.keyboard.press('ArrowRight'); page.keyboard.press('End')
                            assert frame.locator('#relation-search').input_value() == 'synthetic query'
                            assert frame.locator('#relation-search').evaluate('e => e.selectionStart') == len('synthetic query')
                            assert abs(scroll_metrics(target)['top'] - before) <= 1
                            tab_to(page, frame.locator('#relation-cancel'))
                            page.keyboard.press('Enter')
                            expect(frame.locator('#info-editor')).to_be_hidden()
                            print('PASS', case, 'Tab/search editing does not scroll; cancel', flush=True)
                    page.reload(wait_until='networkidle')
                    page.locator('#a-info').click()
                    expect(page.frame_locator('#session-content-view iframe').locator('#info-predecessors .si-relation')).to_have_count(35)
                    assert not errors, errors
                    context.close()
            finally:
                browser.close()
    print('PASS content input:', len(results), 'directional edge checks', flush=True)


if __name__ == '__main__':
    run_with_evidence(main, 'target/content-input')
