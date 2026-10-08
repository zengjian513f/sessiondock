#!/usr/bin/env python3
"""Chromium: live counts, full-catalog stale cleanup, cancellation and recheck.

Two loopback fake nodes behind the real Hub; synthetic API rows and running state.
Stops traverse the Hub to fake nodes, whose recorded native UIDs are asserted.
No CLI, user home, or production session is touched.
"""
import argparse
from datetime import datetime, timedelta, timezone
from pathlib import Path
import tempfile
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright
from hub_fixtures import FakeNode, Hub, REPO, scoped
from machine_controls_browser import launch_chromium


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=REPO / 'target/debug/sessiondock')
    args = parser.parse_args()
    nodes = [FakeNode('a' * 32, 'NodeA'), FakeNode('b' * 32, 'NodeB')]
    hub = None
    try:
        with tempfile.TemporaryDirectory(prefix='sessiondock-cleanup-') as temporary:
            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), Path(temporary), nodes)
            hub.start()
            now = datetime.now(timezone.utc)
            old = (now - timedelta(hours=49)).isoformat()
            recent = (now - timedelta(hours=47)).isoformat()
            rows = []
            for index, (name, source, updated) in enumerate([
                ('stale-root', 'claude', old), ('recent', 'claude', recent),
                ('inactive', 'codex', old), ('updated-during-confirm', 'codex', old),
                ('hidden-child', 'codex', old), ('unknown-time', 'claude', ''),
                ('stop-error', 'claude', old), ('stop-uncertain', 'codex', old),
            ]):
                node = nodes[index // 4]
                rows.append({**node.state()['row'], 'uid': scoped(node.nid, source + ':' + name),
                    'sid': name, 'source': source, 'title': name, 'updated': updated,
                    'node_id': node.nid, 'node_name': node.name,
                    **({'nest_parent': {'source': 'claude', 'sid': 'parent-not-loaded'}}
                       if name == 'hidden-child' else {})})
            live = {row['uid'] for row in rows if row['sid'] != 'inactive'}
            stopped = []
            pending_stops = []
            stop_mode = 'normal'
            live_failure = False
            with sync_playwright() as pw:
                browser = launch_chromium(pw)
                context = browser.new_context(viewport={'width': 1800, 'height': 900}, service_workers='block')
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))

                def sessions(route):
                    response = route.fetch(headers={key: value for key, value in route.request.headers.items()
                                                       if key.lower() != 'x-sessiondock-list'})
                    data = response.json()
                    hidden = parse_qs(urlsplit(route.request.url).query).get('children') == ['hidden']
                    data.update(sessions=[row for row in rows if not hidden or not row.get('nest_parent')],
                                sig='cleanup-fixture', unchanged=False)
                    route.fulfill(response=response, json=data)

                def running(route):
                    if live_failure:
                        route.fulfill(status=503, json={'error': 'synthetic live unavailable'})
                        return
                    response = route.fetch()
                    data = response.json()
                    data.update(uids=list(live), tmux_uids=list(live))
                    route.fulfill(response=response, json=data)

                def stop(route):
                    body = route.request.post_data_json
                    uid = body['uid']
                    assert body.get('request_id'), body
                    stopped.append(uid)
                    pending_stops.append(route)

                def finish_stop(route):
                    uid = route.request.post_data_json['uid']
                    if stop_mode == 'still-running':
                        # A 200/no-op and a claimed stop both need live recheck.
                        route.fulfill(json={'ok': True, 'stopped': uid.endswith('~stop-uncertain')})
                        return
                    if stop_mode == 'unverified':
                        route.fulfill(json={'ok': True, 'stopped': False})
                        return
                    if stop_mode == 'already-exited':
                        live.discard(uid)
                        route.fulfill(json={'ok': True, 'stopped': False})
                        return
                    if uid.endswith(':stop-error') or uid.endswith('~stop-error'):
                        route.fulfill(status=500, json={'error': 'synthetic stop failure'})
                    elif uid.endswith('~stop-uncertain'):
                        route.fulfill(json={'ok': True, 'stage': 'uncertain', 'stopped': False})
                    else:
                        response = route.fetch()  # real Hub strips the node namespace
                        assert response.ok, response.text()
                        live.discard(uid)
                        route.fulfill(json={'ok': True, 'stage': 'stopped', 'stopped': True})

                context.route('**/api/sessions?*', sessions)
                context.route('**/api/sessions', sessions)
                context.route('**/api/live*', running)
                context.route('**/api/session/stop', stop)
                page.goto(f'http://127.0.0.1:{hub.port}/', wait_until='networkidle')
                page.wait_for_function('S.sessions.length === 8 && S.live.size === 7')
                a = page.locator('#node-chips button').filter(has_text='NodeA')
                b = page.locator('#node-chips button').filter(has_text='NodeB')
                claude = page.locator('#chips button[data-source=claude]')
                codex = page.locator('#chips button[data-source=codex]')
                expect(a.locator('b')).to_have_text('4')
                expect(codex.locator('b')).to_have_text('4')
                page.locator('#livecount').click()
                expect(a.locator('b')).to_have_text('3')
                expect(b.locator('b')).to_have_text('4')
                expect(codex.locator('b')).to_have_text('3')
                # Background process exit must repaint chips without switching scope.
                live.remove(rows[1]['uid'])
                page.evaluate('refreshLive(true)')
                expect(a.locator('b')).to_have_text('2')
                expect(claude.locator('b')).to_have_text('3')
                page.locator('#allcount').click()
                expect(a.locator('b')).to_have_text('4')
                expect(claude.locator('b')).to_have_text('4')
                live.add(rows[1]['uid'])
                page.evaluate('refreshLive(true)')
                page.locator('#livecount').click()

                def header_action(selector):
                    page.evaluate('() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))')
                    if not page.locator(selector).is_visible():
                        page.locator('#header-more-btn').click()
                    page.locator(selector).click()

                def cleanup_setting(days, previous):
                    header_action('#settings')
                    page.get_by_role('tab', name='功能', exact=True).click()
                    field = page.get_by_role('spinbutton', name='清扫未更新天数', exact=True)
                    expect(field).to_have_value(str(previous))
                    field.fill(str(days))
                    field.press('Tab')
                    page.locator('#settings-dialog .modal-actions button').click()

                cleanup_setting(3, 2)
                header_action('#session-cleanup')
                expect(page.locator('#session-cleanup-status')).to_have_text('没有超过 3 天未更新的活跃会话。')
                expect(page.locator('#session-cleanup-description')).to_contain_text('超过 3 天')
                page.locator('#session-cleanup-close').click()
                cleanup_setting(1, 3)
                page.reload(wait_until='networkidle')
                page.wait_for_function('S.sessions.length === 8 && S.live.size === 7')
                header_action('#session-cleanup')
                expect(page.locator('#session-cleanup-status')).to_contain_text('找到 6 个超过 1 天')
                page.locator('#session-cleanup-close').click()
                assert not stopped
                cleanup_setting(2, 1)
                b.click()  # machine-filtered Agent counts, including zero
                expect(codex.locator('b')).to_have_text('1')
                codex.click()
                page.locator('#q').fill('no-visible-session')
                page.locator('#nest-hidden').click()
                page.wait_for_function('S.childMode === "hidden" && S.sessions.length === 7')

                def open_cleanup():
                    page.evaluate('() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))')
                    if not page.locator('#session-cleanup').is_visible():
                        page.locator('#header-more-btn').click()
                    page.locator('#session-cleanup').click()
                    expect(page.locator('#session-cleanup-start')).to_be_enabled()

                open_cleanup()
                expect(page.locator('#session-cleanup-status')).to_contain_text('找到 5 个')
                expect(page.locator('#session-cleanup-list')).to_contain_text('hidden-child')
                page.locator('#session-cleanup-close').click()
                assert not stopped
                open_cleanup()
                previous_times = [row['updated'] for row in rows]
                for row in rows:
                    row['updated'] = datetime.now(timezone.utc).isoformat()
                page.locator('#session-cleanup-start').click()
                expect(page.locator('#session-cleanup-close')).to_have_text('完成', timeout=20000)
                expect(page.locator('#session-cleanup-status')).to_contain_text('跳过 5 个')
                expect(page.locator('#session-cleanup-list')).to_contain_text('复查后没有需要停止的会话')
                assert not stopped
                page.locator('#session-cleanup-close').click()
                for row, updated in zip(rows, previous_times):
                    row['updated'] = updated
                open_cleanup()
                rows[3]['updated'] = datetime.now(timezone.utc).isoformat()
                page.locator('#session-cleanup-start').click()
                remaining = page.locator('#session-cleanup-list [data-uid]')
                expect(remaining).to_have_count(4)
                expect(page.locator('#session-cleanup-list [data-state=stopping]')).to_have_count(4)
                assert len(pending_stops) == 4
                child = page.locator('#session-cleanup-list [data-uid]').filter(has_text='hidden-child').element_handle()
                first = next(route for route in pending_stops if route.request.post_data_json['uid'] == rows[0]['uid'])
                pending_stops.remove(first)
                finish_stop(first)
                expect(remaining).to_have_count(3)
                expect(page.locator('#session-cleanup-status')).to_contain_text('已停止 1/4')
                assert child.evaluate('node => node.isConnected'), 'remaining rows must stay in place'
                child.dispose()
                for route in pending_stops:
                    finish_stop(route)
                pending_stops.clear()
                expect(page.locator('#session-cleanup-close')).to_have_text('完成', timeout=20000)
                expect(page.locator('#session-cleanup-status')).to_contain_text('已停止 2/4，失败 1，未确认 1')
                expect(page.locator('#session-cleanup-status')).to_contain_text('跳过 1 个')
                expect(page.locator('#session-cleanup-list')).to_contain_text('synthetic stop failure')
                expect(remaining).to_have_count(2)
                expect(page.locator('#session-cleanup-list')).to_contain_text('stop-uncertain')
                assert set(stopped) == {rows[i]['uid'] for i in (0, 4, 6, 7)}, stopped
                for node, uid in zip(nodes, ('claude:stale-root', 'codex:hidden-child')):
                    writes = [body['uid'] for path, body in node.state()['writes'] if path == '/api/session/stop']
                    assert writes == [uid], writes
                page.locator('#session-cleanup-close').click()
                # Survivors are accurately reported; HTTP success must never
                # count them as stopped or make them vanish without explanation.
                stop_mode = 'still-running'
                open_cleanup()
                expect(page.locator('#session-cleanup-status')).to_contain_text('找到 2 个')
                page.locator('#session-cleanup-start').click()
                expect(page.locator('#session-cleanup-list [data-state=stopping]')).to_have_count(2)
                for route in pending_stops:
                    finish_stop(route)
                pending_stops.clear()
                expect(page.locator('#session-cleanup-close')).to_have_text('完成', timeout=20000)
                expect(page.locator('#session-cleanup-status')).to_contain_text('已停止 0/2，失败 0，未确认 2')
                expect(remaining).to_have_count(2)
                expect(page.locator('#session-cleanup-list [data-state=uncertain]')).to_have_count(2)
                expect(page.locator('#session-cleanup-list')).to_contain_text('复查仍在运行')
                page.locator('#session-cleanup-close').click()
                stop_mode = 'unverified'
                open_cleanup()
                page.locator('#session-cleanup-start').click()
                expect(page.locator('#session-cleanup-list [data-state=stopping]')).to_have_count(2)
                live_failure = True
                for route in pending_stops:
                    finish_stop(route)
                pending_stops.clear()
                expect(page.locator('#session-cleanup-close')).to_have_text('完成', timeout=20000)
                expect(page.locator('#session-cleanup-status')).to_contain_text('已停止 0/2，失败 0，未确认 2')
                expect(page.locator('#session-cleanup-status')).to_contain_text('刷新状态失败')
                expect(remaining).to_have_count(2)
                page.locator('#session-cleanup-close').click()
                live_failure = False
                # A no-op can also mean an actual exit. Fresh absence confirms it.
                stop_mode = 'already-exited'
                open_cleanup()
                page.locator('#session-cleanup-start').click()
                expect(page.locator('#session-cleanup-list [data-state=stopping]')).to_have_count(2)
                for route in pending_stops:
                    finish_stop(route)
                pending_stops.clear()
                expect(page.locator('#session-cleanup-close')).to_have_text('完成', timeout=20000)
                expect(page.locator('#session-cleanup-status')).to_contain_text('已停止 2/2，失败 0，未确认 0')
                expect(remaining).to_have_count(0)
                expect(page.locator('#session-cleanup-list')).to_contain_text('均已确认退出')
                page.locator('#session-cleanup-close').click()
                # Mobile folded-menu entry and the empty result are also usable.
                live.clear()
                page.set_viewport_size({'width': 390, 'height': 844})
                page.evaluate('() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))')
                if not page.locator('#session-cleanup').is_visible():
                    page.locator('#header-more-btn').click()
                page.locator('#session-cleanup').click()
                expect(page.locator('#session-cleanup-status')).to_have_text('没有超过 2 天未更新的活跃会话。')
                expect(page.locator('#session-cleanup-start')).to_be_disabled()
                page.locator('#session-cleanup-close').click()
                cleanup_setting(5, 2)
                header_action('#session-cleanup')
                expect(page.locator('#session-cleanup-status')).to_have_text('没有超过 5 天未更新的活跃会话。')
                assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                page.locator('#session-cleanup-close').click()
                assert not errors, errors
                context.close()
                browser.close()
            print('PASS cleanup: live counts, incremental rows, filters, lazy children, cancellation, recheck, '
                  'Hub routing, errors, false/contradictory stop replies, confirmed no-op exits and mobile')
    finally:
        if hub:
            hub.stop()
        for node in nodes:
            node.stop()


if __name__ == '__main__':
    main()
