#!/usr/bin/env python3
"""Operator-only Agy native delete through Chromium; private HOME and synthetic gateway.

No production data, account or paid requests. Uses the installed real CLI,
explicit low model variant and asserts its actual wire model in the gateway.
"""
import json
from pathlib import Path
import sqlite3
from playwright.sync_api import expect
import agy_real_browser as h
from agy_interactions_real_browser import gateway
from agy_tools_real_browser import MODEL


def run(browser, base, root, agy, records, evidence, holds):
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
    page = context.new_page()
    page.set_default_timeout(30000)
    native = root / 'home/.gemini/antigravity-cli'

    def catalog():
        with sqlite3.connect(f'file:{native}/conversation_summaries.db?mode=ro', uri=True) as c:
            return {r[0] for r in c.execute('SELECT conversation_id FROM conversation_summaries')}

    def open_menu():
        if not page.locator('#a-more').is_visible() and not page.locator('#a-session-action').is_visible():
            selected = page.evaluate('S.sel')
            page.locator(f'#side .item[data-uid="{selected}"]').click()
        if not page.locator('#a-session-action').is_visible():
            page.locator('#a-more').click()
        page.locator('#a-session-action').click()
        return page.locator('dialog.app-popup[open]')

    def stop():
        if not page.locator('#a-session-toggle').is_visible():
            page.locator('#a-more').click()
        page.locator('#a-session-toggle').click()
        page.wait_for_function("document.querySelector('dialog.app-popup[open]') || document.querySelector('#a-session-toggle')?.getAttribute('aria-label') === '启动会话'")
        if page.locator('dialog.app-popup[open]').count():
            page.locator('dialog.app-popup[open] [data-popup-action="ok"]').click()
        page.wait_for_function("document.querySelector('#a-session-toggle')?.getAttribute('aria-label') === '启动会话'")

    try:
        page.goto(base)
        page.wait_for_function('T.listLoaded && T.sources.agy === true')
        made = []
        for index in range(2):
            h.open_picker(page)
            page.locator('label:has(input[name="new-source"][value="agy"])').click()
            page.locator('#new-model').click()
            page.locator('#new-model-options [data-model-option][title="gemini-3.1-pro"]').click()
            page.locator('#new-effort').select_option('low')
            page.locator('#new-cwd').fill(str(root / 'work'))
            page.locator('#new-session-go').click()
            if index == 0:
                h.terminal(page)
                h.wait_screen(page, 'Do you trust')
                h.terminal(page, key='Enter')
            h.send(page, 'same title deletion fixture')
            page.wait_for_function("S.sel?.startsWith('agy:')")
            h.history(page, 'DONE same title deletion fixture')
            uid = page.evaluate('S.sel')
            row = next(r for r in page.request.get(base + '/api/sessions?force=1').json()['sessions'] if r['uid'] == uid)
            made.append(row)
            if index == 1:
                if not page.locator('#a-session-action').is_visible():
                    page.locator('#a-more').click()
                expect(page.locator('#a-session-action')).to_have_attribute('aria-disabled', 'true')
                response = page.request.delete(base + '/api/session/' + uid)
                assert response.status == 409 and response.json()['code'] == 'session_running'
                assert row['sid'] in catalog()
                evidence.pass_('running session delete control disabled and backend rejects deletion')
                page.locator('#a-more').click()
            if index == 1:
                stop()
        target, other = made[1], made[0]
        assert target['sid'] != other['sid']
        for width in (1280, 390):
            page.set_viewport_size({'width': width, 'height': 900})
            for theme in ('light', 'dark'):
                page.emulate_media(color_scheme=theme)
                popup = open_menu()
                expect(popup).to_contain_text('Agy 会话会永久删除')
                expect(popup).to_contain_text('无法恢复')
                page.screenshot(path=str(root / f'delete-{width}-{theme}.png'))
                popup.locator('[data-popup-action="cancel"]').click()
                assert target['sid'] in catalog()
        page.set_viewport_size({'width': 1280, 'height': 900})
        before_requests = len(records)
        # Only this fixture's HOME is changed. A private deletion worker must
        # surface native trust instead of waiting 25s or granting it itself.
        settings = native / 'settings.json'
        config = json.loads(settings.read_text())
        config['trustedWorkspaces'] = []
        settings.write_text(json.dumps(config))
        popup = open_menu()
        with page.expect_response(lambda r: r.request.method == 'DELETE', timeout=10000) as blocked:
            popup.locator('[data-popup-action="ok"]').click()
        assert blocked.value.status == 500, blocked.value.text()
        expect(page.locator('dialog.app-popup[open]')).to_contain_text('工作目录信任确认')
        assert target['sid'] in catalog() and other['sid'] in catalog()
        assert json.loads(settings.read_text()).get('trustedWorkspaces', []) == []
        assert len(records) == before_requests, 'blocked deletion invoked a model'
        assert not list((root / 'host').glob('.agy-delete-*'))
        page.screenshot(path=str(root / 'delete-trust-blocked.png'), full_page=True)
        page.locator('dialog.app-popup[open] [data-popup-action="ok"]').click()
        if not page.locator('#a-session-toggle').is_visible():
            page.locator('#a-more').click()
        page.locator('#a-session-toggle').click()
        h.terminal(page)
        h.wait_screen(page, 'Do you trust')
        h.terminal(page, key='Enter')
        h.wait_screen(page, '? for shortcuts')
        stop()
        assert target['sid'] in catalog()
        evidence.pass_('untrusted workspace fails promptly without changing trust or deleting; explicit browser trust permits retry')
        popup = open_menu()
        with page.expect_response(lambda r: r.request.method == 'DELETE', timeout=40000) as response:
            popup.locator('[data-popup-action="ok"]').click()
        assert response.value.status == 200, response.value.text()
        assert target['sid'] not in catalog() and other['sid'] in catalog()
        assert not (native / 'conversations' / (target['sid'] + '.db')).exists()
        assert not (native / 'brain' / target['sid']).exists()
        assert (native / 'conversations' / (other['sid'] + '.db')).is_file()
        assert len(records) == before_requests, 'delete invoked a model'
        page.reload()
        page.wait_for_function('T.listLoaded')
        expect(page.locator(f'#side .item[data-uid="{target["uid"]}"]')).to_have_count(0)
        page.locator(f'#side .item[data-uid="{other["uid"]}"]').click()
        h.history(page, 'DONE same title deletion fixture')
        h.terminal(page)
        h.wait_screen(page, '? for shortcuts')
        h.send(page, 'survivor resumes')
        h.history(page, 'DONE survivor resumes')
        for _ in range(70):
            page.wait_for_timeout(1000)
            assert target['sid'] not in catalog(), 'another live CLI restored the deleted catalog row'
        evidence.pass_('real native permanent delete, cancel in four layouts, refresh stays deleted, other SID resumes; delete makes no model request')
        assert not list((root / 'host').glob('.agy-delete-*'))
        evidence.pass_('private deletion host cleaned up')
    except Exception as error:
        evidence.fail('agy-delete', error, page)
        raise
    finally:
        context.close()


if __name__ == '__main__':
    h.run = run
    h.gateway = gateway
    h.MODELS = (MODEL,)
    h.main()
