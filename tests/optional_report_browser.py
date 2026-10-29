#!/usr/bin/env python3
"""Chromium: default report controls hidden; capture-only reports never launch a CLI."""
import argparse
import json
import tempfile
from pathlib import Path
from playwright.sync_api import sync_playwright, expect
from history_fixtures import REPO, Corpus, isolated_server, claude_row
from hub_send_browser import prepare
from bug_report_node_browser import open_report, wait_drafts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=REPO/'target/release/sessiondock')
    args = parser.parse_args()
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
        with isolated_server(corpus, args.binary) as (base, _):
            page = browser.new_page()
            page.goto(base, wait_until='networkidle')
            page.locator('#side .item').first.click()
            expect(page.locator('#report-bug')).to_be_hidden()
            assert page.locator('[data-report-bug]:visible').count() == 0
            page.close()
        capture = root / 'capture'
        capture.mkdir()
        config = prepare(capture)
        with isolated_server(Corpus(capture), args.binary,
                state_dir=capture / 'state', host_dir=capture / 'host',
                lifecycle_dir=capture / 'ledger', launcher_config=config,
                audit_dir=capture / 'audit', extra_env={
                    'SESSIONDOCK_BUG_REPORT_DIR': str(capture / 'reports'),
                    'SESSIONDOCK_BUG_REPORT_REPO': str(capture / 'work'),
                }) as (base, _):
            for width, theme in [(1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark')]:
                context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda e: errors.append(str(e)))
                page.goto(base, wait_until='networkidle')
                expect(page.locator('html')).to_have_attribute('data-theme', theme)
                open_report(page)
                expect(page.locator('#bug-report-type input[value=bug]')).to_be_checked()
                assert page.locator('#bug-report-type label').all_inner_texts() == ['缺陷报告', '功能需求']
                page.locator('#bug-report-type label:has(input[value=feature])').click()
                page.locator('#bug-report-description').fill('重开保留报告草稿')
                page.locator('#bug-report-dialog .modal-close').click()
                open_report(page)
                expect(page.locator('#bug-report-type input[value=bug]')).to_be_checked()
                expect(page.locator('#bug-report-description')).to_have_value('重开保留报告草稿')
                assert page.locator('#bug-report-type').bounding_box()['height'] < 50
                report_type = 'feature' if theme == 'light' else 'bug'
                page.locator(f'#bug-report-type label:has(input[value={report_type}])').click()
                expect(page.locator(f'#bug-report-type input[value={report_type}]')).to_be_checked()
                expect(page.locator('#bug-report-source')).to_be_hidden()
                expect(page.locator('#bug-report-model')).to_be_hidden()
                expect(page.locator('#bug-report-go')).to_have_text('保存需求' if report_type == 'feature' else '保存诊断')
                page.locator('#bug-report-description').fill('功能需求：输入区一直显示，不可用控件置灰。' if report_type == 'feature' else '缺陷报告：会话消息未刷新。')
                wait_drafts(page)
                page.screenshot(path=str(screenshots / f'capture-{width}-{theme}.png'))
                if report_type == 'bug':
                    def remote_capture(route):
                        body = route.request.post_data_json
                        body.update(origin={'node_name': 'Origin', 'uid': 'codex:remote'},
                                    captured={'hostname': 'origin-host', 'session': {'uid': 'codex:remote'},
                                              'events': [{'event': 'origin.capture'}],
                                              'terminal_capture': 'origin terminal frame'})
                        route.continue_(post_data=json.dumps(body))
                    page.route('**/api/bug-report', remote_capture)
                with page.expect_response(lambda r: r.url.endswith('/api/bug-report')) as saved:
                    page.locator('#bug-report-go').click()
                result = saved.value.json()
                assert saved.value.status == 201, result
                assert result['worker'] is None, result
                expect(page.locator('#bug-report-toast')).to_contain_text('需求已保存' if report_type == 'feature' else '诊断已保存')
                expect(page.locator('#bug-report-toast button', has_text='打开')).to_have_count(0)
                toast = page.locator('#bug-report-toast')
                assert toast.evaluate('e => e.scrollWidth <= e.clientWidth'), 'receipt text must wrap inside the card'
                if width > 500:
                    assert toast.bounding_box()['width'] < 430, 'a short receipt must not occupy a fixed 440px card'
                page.screenshot(path=str(screenshots / f'receipt-{width}-{theme}.png'))
                manifest = json.loads((Path(result['path']) / 'manifest.json').read_text())
                assert manifest['status'] == 'captured', manifest
                assert manifest['report_type'] == report_type, manifest
                prompt = (Path(result['path']) / 'worker-prompt.md').read_text()
                assert prompt.startswith('功能需求:' if report_type == 'feature' else 'BUG:'), prompt
                assert ('处理 sessiondock 功能需求' if report_type == 'feature' else '处理 sessiondock 缺陷报告') in prompt
                diagnostic_files = ['browser-state.json', 'events.jsonl', 'environment.json']
                if report_type == 'feature':
                    assert all(not (Path(result['path']) / f).exists() for f in diagnostic_files + ['terminal.txt'])
                    assert manifest['events_file'] == manifest['browser_state_file'] == manifest['terminal_file'] == ''
                    assert manifest['event_count'] == manifest['event_window_seconds'] == 0
                    assert manifest['session'] == {}
                    assert 'manifest.json' not in prompt and '问题机器：' not in prompt
                    assert '诊断包：' not in prompt and '直接依据上述描述和附件实现' in prompt
                else:
                    assert all((Path(result['path']) / f).is_file() for f in diagnostic_files)
                    assert f"诊断包所在机器：处理会话本机（主机 {manifest['hostname']}）" in prompt
                    assert '问题机器：Origin（主机 origin-host）' in prompt
                    assert '不要求每个报告都读取诊断包或完整阅读所有文件' in prompt
                    assert '根据问题选择必要的证据' in prompt
                    assert '请先完整阅读' not in prompt and '先用诊断包里的' not in prompt
                    assert '不要去问题机器寻找这个诊断包目录' in prompt
                    assert (Path(result['path'])/'terminal.txt').read_text() == 'origin terminal frame'
                    assert json.loads((Path(result['path'])/'events.jsonl').read_text().splitlines()[0])['event'] == 'origin.capture'
                assert result['report_type'] == report_type
                assert context.request.get(base + '/api/term/list').json()['pending'] == []
                assert not errors, errors
                context.close()
        browser.close()
    print('PASS optional reports: hidden when disabled; feature/bug capture, wide/mobile light/dark, no worker')


if __name__ == '__main__':
    main()
