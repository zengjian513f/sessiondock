#!/usr/bin/env python3
"""Bubble scroll axes and gutters in conversation and SDK history on node and Hub."""
import argparse
from pathlib import Path
import re
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from bubble_scroll_fixtures import check_scroll_inset
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_start_stop_fixtures import provider

TEXT = '\n\n'.join(f'第 {i:02d} 行：检查气泡顶部与底部留白，保持原文和展开操作。' for i in range(32))
OUTPUT = '\n'.join(f'output {i:02d}: synthetic result' for i in range(60))
TABLE_HEADERS = ['Region', 'Request volume', 'Median latency', 'Slowest response',
                 'Successful requests', 'Failed requests', 'Availability', 'Reporting interval']
TABLE = '\n'.join('| ' + ' | '.join(row) + ' |' for row in (
    TABLE_HEADERS, ['---'] * len(TABLE_HEADERS),
    ['South East Asia', '31,240 req/min', '92 milliseconds', '1,420 milliseconds',
     '31,239 req/min', '1 req/min', '99.99 percent', '2026-11-18 00:00 UTC']))
DIFF_OUTPUT = '\n'.join(f'{line}; route = /api/synthetic/builds/{i}/' + 'segments/' * 20
                        for i, line in enumerate(OUTPUT.splitlines()))
HISTORY = [dict(role=role, text=f'{role} START\n\n{TEXT}\n\n{role} END')
           for role in ('user', 'assistant', 'thinking', 'system', 'command', 'answer')]
HISTORY[1]['text'] += '\n\n' + TABLE
HISTORY += [dict(role='tool', name='Bash', summary='$ synthetic-command', text='synthetic-command',
                 result=dict(role='tool_result', text=OUTPUT, exit_code=0)),
            dict(role='tool_result', text=OUTPUT),
            dict(role='tool', name='Edit', text='synthetic diff', changes=[dict(
                path='synthetic.txt', operation='edit', added=60, removed=0,
                patch='--- a/synthetic.txt\n+++ b/synthetic.txt\n@@ -0,0 +1,60 @@\n' + '\n'.join('+' + line for line in DIFF_OUTPUT.splitlines()),
                before='', after=DIFF_OUTPUT, before_available=True, after_available=True,
                before_complete=True, after_complete=True)]),
            dict(role='event', event_kind='task', event_status='completed', text='任务结果',
                 details=OUTPUT, details_plain=True)]


def check_horizontal_wheel(page, body, shot):
    body.hover()
    assert body.evaluate('e => !e.closest(".text-scroll-active")')
    assert body.evaluate('e => e.scrollWidth > e.clientWidth')
    handle = body.element_handle()
    initial = body.evaluate('e => e.scrollLeft')
    before = body.evaluate('e => e.closest(".msgs").scrollTop')
    page.screenshot(path=str(shot.with_name(shot.stem + '-start.png')))
    page.mouse.wheel(160, 0)
    page.wait_for_function('([e, initial]) => e.scrollLeft > initial', arg=[handle, initial])
    assert body.evaluate('e => e.closest(".msgs").scrollTop') == before
    page.screenshot(path=str(shot))


def exercise(browser, base, label, shots):
    for width, theme in ((1280, 'light'), (390, 'light'), (1280, 'dark'), (390, 'dark')):
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        def messages(route):
            response = route.fetch()
            data = response.json()
            data.update(messages=HISTORY, reset=True, start=0, end=100,
                        message_total=len(HISTORY), partial=None, activity={'state': 'working'})
            route.fulfill(response=response, json=data)
        context.route('**/api/messages/**', messages)
        context.route('**/api/watch?*', lambda r: r.fulfill(content_type='text/event-stream', body=': fixture\n\n'))
        page.goto(base, wait_until='networkidle')
        page.locator('#side .item[data-uid^="codex:"]').first.click()
        for entry, selector in [('sdk', '.sdk-history'), ('chat', '#msgs')]:
            page.locator('#a-' + entry).click()
            scope = page.locator(selector)
            expect(scope).to_contain_text('user END')
            for button in scope.get_by_role('button', name='展开工具调用组', exact=True).all():
                button.click()
            for role in ('user', 'assistant', 'thinking', 'system', 'command', 'answer'):
                bubble = scope.locator(f'.msg[data-role="{role}"]').filter(has_text=f'{role} START').first
                body = bubble.locator('.mb')
                body.scroll_into_view_if_needed()
                try:
                    check_scroll_inset(body)
                except AssertionError:
                    page.screenshot(path=str(shots / f'{label}-{width}-{theme}-{entry}-{role}-failed.png'))
                    raise
                if role != 'assistant':
                    page.wait_for_function('e => e.scrollTop > 0', arg=body.element_handle())
                    body.click(position={'x': 35, 'y': 35})
                    expect(bubble).to_have_class(re.compile('text-scroll-active'))
                    before = body.evaluate('e => e.scrollTop')
                    body.hover(); page.mouse.wheel(0, -100)
                    page.wait_for_function('([e, before]) => e.scrollTop < before', arg=[body.element_handle(), before])
                    check_scroll_inset(body)
                    bubble.get_by_role('button', name='展开全文', exact=False).click()
                    assert body.evaluate('e => e.scrollHeight <= e.clientHeight + 1')
                    check_scroll_inset(body)
                    bubble.get_by_role('button', name='收起', exact=True).click()
                    page.wait_for_function('e => e.scrollHeight - e.clientHeight - e.scrollTop < 2', arg=body.element_handle())
                bubble.locator('.message-header').scroll_into_view_if_needed()
                page.screenshot(path=str(shots / f'{label}-{width}-{theme}-{entry}-{role}.png'))
            check_horizontal_wheel(page, scope.locator('.mb .tw'),
                                   shots / f'{label}-{width}-{theme}-{entry}-table-horizontal.png')
            for index, output in enumerate(scope.locator('.tool-out').all()):
                output.scroll_into_view_if_needed()
                check_scroll_inset(output)
                frame = output.locator('..')
                frame.screenshot(path=str(shots / f'{label}-{width}-{theme}-{entry}-tool-{index}.png'))
            diff = scope.locator('.file-change-body')
            diff.scroll_into_view_if_needed()
            check_scroll_inset(diff)
            card = scope.locator('.file-change-card')
            card.get_by_role('button', name='原行', exact=True).click()
            for view in ('并排', '统一'):
                card.get_by_role('button', name=view, exact=True).click()
                check_scroll_inset(diff)
                check_horizontal_wheel(page, diff,
                                       shots / f'{label}-{width}-{theme}-{entry}-diff-{view}.png')
            card.get_by_role('button', name='换行', exact=True).click()
            assert diff.evaluate('e => e.scrollWidth <= e.clientWidth + 1')
            card.get_by_role('button', name='展开全部差异', exact=True).click()
            check_scroll_inset(diff)
            card.get_by_role('button', name='收起差异', exact=True).click()
            card.screenshot(path=str(shots / f'{label}-{width}-{theme}-{entry}-diff.png'))
            event = scope.locator('.timeline-event')
            event.locator('summary').click()
            body = event.locator('.event-detail-body')
            body.scroll_into_view_if_needed()
            check_scroll_inset(body)
            body.click(position={'x': 35, 'y': 35})
            expect(body).to_have_class(re.compile('text-scroll-active'))
            body.hover(); page.mouse.wheel(0, 220)
            page.wait_for_function('e => e.scrollTop > 0', arg=body.element_handle())
            check_scroll_inset(body)
            page.screenshot(path=str(shots / f'{label}-{width}-{theme}-{entry}-details.png'))
            event.locator('summary').click()
            print(f'PASS {label} {width}/{theme} {entry}: messages, tools, diff and details insets; unselected table and diff horizontal wheel', flush=True)
        assert not errors, errors
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-bubble-spacing-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        with provider('codex', root, BINARY) as (base, _, env):
            browser = pw.chromium.launch(headless=True)
            try:
                exercise(browser, base, 'node', args.screenshots)
                hubroot = root / 'hub'; hubroot.mkdir()
                hub = Hub(BINARY.with_name('sessiondock-hub'), hubroot,
                    [SimpleNamespace(name='BubbleNode', token=TOKEN, port=int(env['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                hub.start()
                try:
                    exercise(browser, f'http://127.0.0.1:{hub.port}', 'hub', args.screenshots)
                finally:
                    hub.stop()
            finally:
                browser.close()


if __name__ == '__main__':
    main()
