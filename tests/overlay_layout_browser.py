#!/usr/bin/env python3
"""Scaled overlays stay reachable: real clicks over a private Chromium fixture.

HTTP fixtures supply drift, models and long errors, never a real restart/CLI.
Covers phone, report viewport, landscape and desktop, themes and 50–150% scale.
"""
import argparse
import json
import html
import re
from pathlib import Path
import tempfile
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, build_corpus, isolated_server
from frontend_framework_browser import launch_chromium, open_settings
from new_session_model_browser import open_dialog
from input_hit_fixtures import hit, wheel_to


def bounded(page, locator, content=True):
    expect(locator).to_be_visible()
    box = locator.bounding_box()
    size = page.viewport_size
    assert box and box['width'] > 0 and box['height'] > 0, str(locator)
    assert box['x'] >= -1 and box['y'] >= -1, (str(locator), box)
    assert box['x'] + box['width'] <= size['width'] + 1, (str(locator), box, size)
    assert box['y'] + box['height'] <= size['height'] + 1, (str(locator), box, size)
    if content:
        fits = locator.evaluate('e => e.scrollWidth <= e.clientWidth + 1')
        if not fits:
            page.screenshot(path=str(page.evidence_output/'overflow.png'))
            raise AssertionError((str(locator), locator.evaluate('''e => [...e.querySelectorAll('*')].filter(n=>n.scrollWidth>n.clientWidth+1).map(n=>[n.id,n.className,n.clientWidth,n.scrollWidth]).slice(0,20)''')))

    if locator.evaluate("e => e.matches('[role=menu], .session-actions-menu, .new-model-menu')"):
        assert locator.evaluate('''e => {const r=e.getBoundingClientRect();return [[16,16],[r.width-16,16],[16,r.height-16],[r.width-16,r.height-16]].every(([x,y])=>e.contains(document.elementFromPoint(r.left+x,r.top+y)));}'''), ('menu clipped or covered', str(locator), box)
    return box


def header(page, selector):
    if not page.locator(selector).is_visible():
        page.locator('#header-more-btn').click()
    page.locator(selector).click()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('target/overlay-layout'))
    parser.add_argument('--case')
    args = parser.parse_args(); args.output.mkdir(parents=True, exist_ok=True)
    results = []
    with tempfile.TemporaryDirectory(prefix='sessiondock-overlays-') as tmp, sync_playwright() as pw:
        root = Path(tmp); corpus = build_corpus(root/'corpus')
        for sid, path in corpus.paths.items():
            if sid not in ('claude-branch', 'claude-agent-one'):
                path.unlink(missing_ok=True)
        for name in ('trash', 'state'):
            (corpus.root/name).mkdir(mode=0o700)
        with isolated_server(corpus, args.binary, trash_dir=corpus.root/'trash', state_dir=corpus.root/'state') as (base, _):
            browser = launch_chromium(pw)
            try:
                for width, height, scale in [(675,846,125), (675,548,150), (320,568,150),
                                              (390,844,100), (390,844,50), (844,390,150),
                                              (1280,900,100), (1440,900,150)]:
                    for theme in ('light', 'dark'):
                        case = f'{width}x{height}-{scale}-{theme}'
                        if args.case and args.case != case: continue
                        context = browser.new_context(viewport={'width':width,'height':height}, service_workers='block')
                        context.add_init_script(f"localStorage.setItem('sessiondock.theme',JSON.stringify('{theme}'));localStorage.setItem('sessiondock.interfaceScale','{scale}')")
                        page = context.new_page(); page.set_default_timeout(10000); page.evidence_output = args.output; errors = []
                        page.on('pageerror', lambda e: errors.append(str(e)))
                        def meta(route):
                            response = route.fetch(); data = response.json()
                            data['capabilities'].update(terminal=True, bug_report=True)
                            route.fulfill(response=response, json=data)
                        page.route('**/api/meta', meta)
                        def document(route):
                            response = route.fetch()
                            body = response.text()
                            def capabilities(match):
                                data = json.loads(html.unescape(match[2]))
                                data.update(terminal=True, terminal_create=True, bug_report=True,
                                            bug_report_worker=True, conversation_send=True)
                                return match[1] + html.escape(json.dumps(data), quote=True) + match[3]
                            body = re.sub(r'(<meta name="sessiondock-capabilities" content=")([^"]*)(")', capabilities, body)
                            route.fulfill(response=response, body=body)
                        page.route(re.compile(re.escape(base) + r'/(?:\?.*)?$'), document)
                        page.route('**/api/term/list', lambda r:r.fulfill(json={
                            'enabled':True,'sessions':[],'pending':[], 'sources':{'claude':True,'codex':True}, 'bug_report':True}))
                        page.route('**/api/term/models*', lambda r:r.fulfill(json={
                            'models':[{'id':f'fixture-model-{i:02}', 'name':f'Model {i:02} 长模型名称', 'efforts':['low','high']} for i in range(20)],
                            'default_model':'fixture-model-00', 'efforts':['low','high']}))
                        page.route('**/api/shell-env', lambda r:r.fulfill(json={
                            'configured':True,'stale':True,'changed':['SYNTHETIC_LONG_ENVIRONMENT_VARIABLE_'*4,'PATH'], 'started_at':'before'}))
                        restart_calls = []
                        def restart(route):
                            restart_calls.append(route.request.url)
                            route.fulfill(status=503, json={'error':'Synthetic restart failed / 合成错误\n' * 60})
                        page.route('**/api/shell-env/restart', restart)
                        page.goto(base, wait_until='networkidle')
                        card = page.locator('#shell-env-notice'); expect(card).to_be_visible(timeout=10000)
                        bounded(page, card); assert card.locator('table').evaluate('e => e.scrollWidth <= e.clientWidth + 1'); print('CHECK',case,'notice',flush=True)
                        page.screenshot(path=str(args.output/(case+'-notice.png')))
                        action = card.get_by_role('button', name=re.compile('^重启 .* 后端$'))
                        wheel_to(page, action, card); hit(action); action.click()
                        popup = page.locator('dialog.app-popup'); expect(popup).to_be_visible()
                        bounded(page, popup)
                        ok = popup.get_by_role('button', name='知道了')
                        wheel_to(page, ok, popup); hit(ok)
                        page.screenshot(path=str(args.output/(case+'-error.png')))
                        ok.click(); assert len(restart_calls) == 1
                        card.get_by_role('button', name=re.compile('^忽略 ')).click(); expect(card).to_be_hidden()
                        print('CHECK',case,'dialogs',flush=True)
                        open_settings(page); bounded(page, page.locator('#settings-dialog'))
                        page.get_by_role('tab', name='功能', exact=True).click()
                        page.locator('#settings-dialog .modal-close').click()
                        open_dialog(page); dialog = page.locator('#new-session-dialog'); bounded(page, dialog)
                        page.screenshot(path=str(args.output/(case+'-new-session.png')))
                        page.locator('#new-cwd').fill('/synthetic/long-directory/'+'中文-path/'*8)
                        page.locator('#new-model').click(); menu = page.locator('#new-model-menu')
                        bounded(page, menu); page.locator('#new-model-search').fill('19')
                        menu.get_by_role('option').click()
                        page.locator('#new-session-dialog .modal-cancel').click()
                        header(page, '#trash'); bounded(page, page.locator('#trash-dialog'))
                        page.locator('#trash-reload').click(); page.locator('#trash-close').click()
                        header(page, '#session-cleanup'); bounded(page, page.locator('#session-cleanup-dialog'))
                        page.locator('#session-cleanup-close').click()
                        page.locator('#header-more-btn').click()
                        bounded(page, page.locator('#header-menu'))
                        trigger = page.locator('#header-menu [data-toolbar-menu="view"]')
                        trigger.click(); bounded(page, page.locator('#toolbar-view-menu'))
                        page.keyboard.press('Escape'); page.keyboard.press('Escape')
                        row = page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]')
                        wheel_to(page, row, page.locator('#side'))
                        page.wait_for_timeout(100)  # Let the user's wheel finish before opening a context menu.
                        row.click(button='right'); bounded(page, page.locator('#item-menu'))
                        page.locator('#item-menu [data-act="group"]').click()
                        bounded(page, page.locator('#session-group-menu'))
                        page.locator('#session-group-menu-add').click()
                        page.locator('#session-group-menu-name').fill('长分组名称 layout regression')
                        bounded(page, page.locator('#session-group-menu'))
                        page.keyboard.press('Escape'); page.keyboard.press('Escape'); page.keyboard.press('Escape')
                        row.click()
                        bounded(page, page.locator('#msgs'), content=False)
                        page.locator('#a-view-switch').click()
                        bounded(page, page.locator('#session-view-menu'))
                        page.locator('#session-view-menu button[data-agent="claude-agent-one"]').click()
                        expect(page.locator('#msgs')).to_contain_text('Claude agent answer')
                        page.locator('#a-view-switch').click()
                        bounded(page, page.locator('#session-view-menu'))
                        page.locator('#session-view-menu button[data-agent=""]').click()
                        page.locator('#a-more').click()
                        menu = page.locator('#session-actions-menu'); bounded(page, menu)
                        page.screenshot(path=str(args.output/(case+'-actions.png')))
                        report = page.locator('#detail [data-report-bug]')
                        if not report.evaluate("e => !!e.closest('#session-actions-menu')"):
                            page.keyboard.press('Escape')
                        hit(report); report.click()
                        dialog = page.locator('#bug-report-dialog'); bounded(page, dialog)
                        page.locator('#bug-report-description').fill('长内容 / layout regression ' * 20)
                        page.locator('#bug-report-model').click()
                        bounded(page, page.locator('#bug-report-model-menu'))
                        page.locator('#bug-report-model-search').fill('19')
                        page.locator('#bug-report-model-menu').get_by_role('option').click()
                        page.screenshot(path=str(args.output/(case+'-report.png')))
                        close = page.locator('#bug-report-dialog .modal-close')
                        wheel_to(page, close, dialog, direction=-1); hit(close); close.click()
                        # Restart progress and a second card must remain reachable together.
                        page.route('**/api/shell-env/restart', lambda r:r.fulfill(json={'ok':True,'restarting':True}))
                        page.reload(wait_until='networkidle')
                        expect(card).to_be_visible(timeout=10000)
                        action = card.get_by_role('button', name=re.compile('^重启 .* 后端$'))
                        wheel_to(page, action, card); action.click()
                        expect(card).to_contain_text('正在重启…'); bounded(page, card)
                        page.screenshot(path=str(args.output/(case+'-restarting.png')))
                        page.route('**/api/meta', lambda r:r.fulfill(json={'build':'overlay-fixture-new-build'}))
                        page.evaluate('checkServerBuild()')
                        stale = page.locator('.version-stale'); expect(stale).to_be_visible()
                        later = stale.get_by_role('button', name='稍后', exact=True)
                        wheel_to(page, later, stale); hit(later); later.click()
                        expect(stale).to_be_hidden(); expect(card).to_contain_text('正在重启…')
                        assert not errors, errors
                        results.append(case); print('PASS overlays',case,flush=True)
                        context.close()
            except BaseException:
                print('GEOMETRY',page.evaluate('''() => ['html','body','#bug-report-dialog','#bug-report-form','#bug-report-dialog .modal-close'].map(s=>{const e=document.querySelector(s),r=e.getBoundingClientRect(),c=getComputedStyle(e);return [s,r.toJSON(),e.scrollTop,e.scrollHeight,e.clientHeight,c.maxHeight,c.overflowY]})'''),flush=True)
                page.screenshot(path=str(args.output/'failure.png'))
                raise
            finally:
                browser.close()
    (args.output/'results.json').write_text(json.dumps(results,indent=2))
    print(f'PASS overlay_layout_browser: {len(results)} configurations',flush=True)


if __name__ == '__main__': main()
