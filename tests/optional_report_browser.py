#!/usr/bin/env python3
"""Chromium: default report controls hidden; capture-only reports never launch a CLI."""
import json
import tempfile
from pathlib import Path
from playwright.sync_api import sync_playwright, expect
from history_fixtures import REPO, Corpus, isolated_server, claude_row
from hub_send_browser import prepare
from bug_report_node_browser import open_report, wait_drafts


def main():
    screenshots = REPO / 'target/validation/public-optional/reports'
    screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-optional-report-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        browser = pw.chromium.launch(headless=True)
        plain = root / 'plain'
        plain.mkdir()
        corpus = Corpus(plain)
        sid = '11111111-1111-4111-8111-111111111111'
        corpus.put(sid, 'claude', [claude_row(sid, 'user', 'row-user', None, 'Public standalone fixture')], ['Public standalone fixture'])
        with isolated_server(corpus, REPO / 'target/release/sessiondock') as (base, _):
            page = browser.new_page()
            page.goto(base, wait_until='networkidle')
            page.locator('#side .item').first.click()
            expect(page.locator('#report-bug')).to_be_hidden()
            assert page.locator('[data-report-bug]:visible').count() == 0
            page.close()
        capture = root / 'capture'
        capture.mkdir()
        config = prepare(capture)
        with isolated_server(Corpus(capture), REPO / 'target/release/sessiondock',
                state_dir=capture / 'state', host_dir=capture / 'host',
                lifecycle_dir=capture / 'ledger', launcher_config=config,
                audit_dir=capture / 'audit', extra_env={
                    'SESSIONDOCK_BUG_REPORT_DIR': str(capture / 'reports'),
                    'SESSIONDOCK_BUG_REPORT_REPO': str(capture / 'work'),
                }) as (base, _):
            for width, theme in [(1280, 'light'), (390, 'dark')]:
                context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda e: errors.append(str(e)))
                page.goto(base, wait_until='networkidle')
                expect(page.locator('html')).to_have_attribute('data-theme', theme)
                open_report(page)
                expect(page.locator('#bug-report-source')).to_be_hidden()
                expect(page.locator('#bug-report-model')).to_be_hidden()
                expect(page.locator('#bug-report-go')).to_have_text('保存诊断')
                page.locator('#bug-report-description').fill('公开版诊断：缺少外部组件时保存上下文，不启动处理会话。')
                wait_drafts(page)
                page.screenshot(path=str(screenshots / f'capture-{width}-{theme}.png'))
                with page.expect_response(lambda r: r.url.endswith('/api/bug-report')) as saved:
                    page.locator('#bug-report-go').click()
                result = saved.value.json()
                assert saved.value.status == 201, result
                assert result['worker'] is None, result
                expect(page.locator('#bug-report-toast')).to_contain_text('诊断已保存')
                expect(page.locator('#bug-report-toast button', has_text='打开')).to_have_count(0)
                manifest = json.loads((Path(result['path']) / 'manifest.json').read_text())
                assert manifest['status'] == 'captured', manifest
                assert context.request.get(base + '/api/term/list').json()['pending'] == []
                assert not errors, errors
                context.close()
        browser.close()
    print('PASS optional reports: hidden when disabled; capture-only wide/light and mobile/dark, no worker')


if __name__ == '__main__':
    main()
