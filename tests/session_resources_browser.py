#!/usr/bin/env python3
"""Exercise session resource drawer with a private node and injected fleet samples."""

import argparse
from pathlib import Path
import os
import json
import tempfile
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, isolated_server


def assert_drawer_layout(page, case):
    geometry = page.locator('.session-resources').evaluate('''dialog => {
      const box = dialog.getBoundingClientRect(), issues = [];
      const inside = (rect, parent) => rect.left >= parent.left - 1 && rect.right <= parent.right + 1;
      for (const element of dialog.querySelectorAll('h2, h3, button, .sr-probe, .sr-summary-head, .sr-section-heading, .sr-metric')) {
        if (!inside(element.getBoundingClientRect(), box)) issues.push(element.textContent);
      }
      for (const cell of dialog.querySelectorAll('.sr-metric')) {
        const rect = cell.getBoundingClientRect(), label = cell.querySelector('dt'), value = cell.querySelector('dd');
        const a = label.getBoundingClientRect(), b = value.getBoundingClientRect();
        if (!inside(a, rect) || !inside(b, rect) || (b.top < a.bottom - 1 && b.left < a.right - 1)
            || label.scrollWidth > label.clientWidth + 1 || value.scrollWidth > value.clientWidth + 1) {
          issues.push(cell.textContent);
        }
      }
      const title = dialog.querySelector('.sr-top > div').getBoundingClientRect();
      const actions = dialog.querySelector('.sr-actions').getBoundingClientRect();
      if (title.right > actions.left + 1) issues.push('title overlaps actions');
      return {left:box.left, right:box.right, top:box.top, bottom:box.bottom,
              width:innerWidth, height:innerHeight, overflow:dialog.scrollWidth-dialog.clientWidth, issues};
    }''')
    assert geometry['left'] >= -1 and abs(geometry['right'] - geometry['width']) <= 1, (case, geometry)
    assert abs(geometry['top']) <= 1 and abs(geometry['bottom'] - geometry['height']) <= 1, (case, geometry)
    assert geometry['overflow'] <= 1 and not geometry['issues'], (case, geometry)


def check_responsive_drawer(page, screenshots):
    # Exercise the app's real zoom setting, including the former clipped 600px/105% case.
    viewports = [(600, 850), (320, 568), (360, 800), (390, 844), (412, 915),
                 (520, 800), (521, 800), (640, 900), (768, 1024), (1024, 768),
                 (844, 390), (1280, 960), (1920, 1080)]
    for width, height in viewports:
        page.set_viewport_size({'width': width, 'height': height})
        for scale in (105, 50, 80, 100, 125, 150):
            case = f'{width}x{height}-{scale}%'
            page.evaluate('applyInterfaceScale', scale)
            page.evaluate("document.documentElement.dataset.theme = 'light'")
            page.locator('.sr-top').scroll_into_view_if_needed()
            assert_drawer_layout(page, case)
            page.get_by_role('button', name='仅当前会话', exact=True).click()
            page.wait_for_function("resourceCalls.at(-1).scope === 'direct'")
            page.get_by_role('button', name='包含子会话', exact=True).click()
            page.wait_for_function("resourceCalls.at(-1).scope === 'inclusive'")
            calls = page.evaluate('resourceCalls.length')
            page.get_by_role('button', name='刷新资源').click()
            page.wait_for_function('(count) => resourceCalls.length > count', arg=calls)
            # The final metric must remain reachable in a short landscape viewport.
            last = page.locator('.sr-node .sr-metric').last
            last.focus()
            # Background refresh can replace the metric between focus and
            # measurement. Resolve the current node and retry its visibility.
            expect(last).to_be_in_viewport(ratio=1)
            page.locator('.sr-top').scroll_into_view_if_needed()
            page.evaluate("document.documentElement.dataset.theme = 'dark'")
            assert_drawer_layout(page, case + '-dark')
            if screenshots and scale in (100, 150):
                page.screenshot(path=str(screenshots / f'resources-{width}x{height}-{scale}-dark.png'))
        print(f'PASS resource layout {width}x{height}: 50/80/100/105/125/150%, light/dark, scope/refresh/scroll', flush=True)
    page.evaluate('applyInterfaceScale', 100)
    page.set_viewport_size({'width': 390, 'height': 844})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-resources-') as temporary, sync_playwright() as pw:
        corpus = Corpus(Path(temporary))
        corpus.put('resources', 'codex', [
            codex_row('session_meta', {'id': 'resources', 'cwd': '/synthetic/resources', 'timestamp': '2026-09-11T10:00:00Z'}),
            codex_row('response_item', {'type': 'message', 'role': 'user', 'content': '资源统计测试：长会话标题也不能挤出刷新和关闭按钮 / synthetic-session-' * 5})], [])
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**launch)
        try:
            with isolated_server(corpus, args.binary) as (base, _):
                page = browser.new_page(viewport={'width': 1280, 'height': 960})
                page.goto(base, wait_until='networkidle')
                page.locator('#side .item').first.click()
                assert page.locator('#detail [data-session-resources]').count() == 0
                page.get_by_role('button', name='列表资源', exact=True).click()
                page.evaluate('''() => {
                  window.resourceCalls = [];
                  window.probeState = {state:"unsupported", remaining_seconds:0};
                  SessionDockResources.setLoader(async (uid, scope) => {
                    resourceCalls.push({uid, scope});
                    const metrics = {cpu_cores: {value: scope === 'inclusive' ? 4.5 : 1.25, status:'ok'}, gpu_count:{value:1,status:'partial',reason:'NVIDIA compute-app residency only'},
                      memory_pss_bytes:{value:1073741824,status:'ok'}, nfs_read_bytes_per_second:{value:null,status:'unsupported',reason:'NFS 探针不可用'}};
                    return {sampled_at:1790812800, totals:metrics, nodes:[
                      {node_id:'a',node_name:'compute-a',status:'ok',metrics,diagnostic:{...probeState}} ,
                      {node_id:'b',node_name:'compute-b',status:'offline',reason:'机器离线',metrics:{},diagnostic:{state:'unsupported',remaining_seconds:0}}]};
                  });
                }''')
                page.locator('#side .item-resources').first.click()
                page.get_by_role('dialog').wait_for()
                page.wait_for_function("document.querySelector('.sr-totals').textContent.includes('4.5')")
                assert page.locator('.sr-metric[title*="NFS 探针不可用"]').count() > 0
                assert 'NFS 探针不可用' not in page.locator('.sr-content').inner_text()
                assert page.locator('.sr-footnote, .sr-scope-help').count() == 0
                assert 'NVIDIA' not in ' '.join(page.locator('.sr-metric').evaluate_all('(cells) => cells.map(cell => cell.title)'))
                gpu = page.locator('.sr-totals .sr-metric').nth(1)
                gpu.hover()
                assert '不代表独占' in gpu.get_attribute('title')
                assert page.locator('.sr-totals').bounding_box()['height'] < 180
                assert page.locator('.sr-toolbar').evaluate("e => {const a=e.querySelector('.sr-scopes').getBoundingClientRect(), b=e.querySelector('.sr-probe').getBoundingClientRect();return Math.abs(a.y+a.height/2-b.y-b.height/2)<2}")
                offline = page.locator('.sr-node').filter(has_text='compute-b')
                assert '机器离线' in offline.inner_text() and '—' in offline.inner_text()
                assert page.evaluate('resourceCalls.at(-1).scope') == 'inclusive'
                page.get_by_role('button', name='仅当前会话', exact=True).click()
                page.wait_for_function("document.querySelector('.sr-totals').textContent.includes('1.25')")
                assert page.evaluate('resourceCalls.at(-1).scope') == 'direct'
                assert page.get_by_role('button', name='仅当前会话', exact=True).get_attribute('aria-pressed') == 'true'
                page.get_by_role('button', name='包含子会话', exact=True).click()
                page.wait_for_function("document.querySelector('.sr-totals').textContent.includes('4.5')")
                assert page.evaluate('resourceCalls.at(-1).scope') == 'inclusive'
                assert page.locator('.sr-probe').inner_text() == '未探测'
                if args.screenshots:
                    args.screenshots.mkdir(parents=True, exist_ok=True)
                    page.screenshot(path=str(args.screenshots / 'resources-desktop.png'))
                page.keyboard.press('Escape')
                assert not page.get_by_role('dialog').is_visible()
                page.set_viewport_size({'width':390,'height':844})
                if not page.locator('#side').is_visible():
                    page.locator('.mobile-back:visible').click()
                page.locator('#side .item-resources').first.focus()
                page.keyboard.press('Enter')
                page.wait_for_function("document.querySelector('.sr-totals').textContent.includes('4.5')")
                assert page.evaluate("document.querySelector('.session-resources').scrollWidth <= innerWidth")
                page.locator('.sr-node h3').first.scroll_into_view_if_needed()
                assert page.locator('.sr-node h3').first.is_visible()
                page.locator('.sr-top').scroll_into_view_if_needed()
                page.evaluate("document.documentElement.dataset.theme = 'dark'")
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / 'resources-mobile-dark.png'))
                # Realistic upper-end values and unavailable data must remain readable together.
                page.evaluate('''() => SessionDockResources.setLoader(async (uid, scope) => {
                  resourceCalls.push({uid, scope});
                  const metrics = Object.fromEntries([
                    ['cpu_cores', 256.75], ['gpu_count', 16], ['gpu_memory_bytes', 192 * 1024 ** 3],
                    ['memory_pss_bytes', 1.9 * 1024 ** 4], ['process_count', 123456],
                    ['memory_bandwidth_bytes_per_second', 999.9 * 1024 ** 3],
                    ['disk_read_operations_per_second', 1234567.8],
                    ['proc_storage_write_bytes_per_second', 999.9 * 1024 ** 2],
                  ].map(([key, value]) => [key, {value, status:'ok'}]));
                  return {sampled_at:1790812800, totals:metrics, nodes:[
                    {node_id:'a', node_name:'compute-with-a-long-hostname-测试执行机器', status:'ok', metrics},
                    {node_id:'b', node_name:'compute-offline', status:'offline', metrics:{}}]};
                })''')
                page.get_by_role('button', name='刷新资源').click()
                page.get_by_text('compute-with-a-long-hostname-测试执行机器', exact=True).wait_for()
                check_responsive_drawer(page, args.screenshots)
                page.get_by_role('button', name='关闭资源面板').click()
                page.evaluate("SessionDockResources.setLoader(async () => {throw new Error('测试采集端离线')})")
                page.locator('#side .item-resources').first.click()
                page.get_by_text('测试采集端离线').wait_for()
                print('PASS resource drawer: selection, inclusive totals, Chinese tooltips, compact layout, refresh, offline/unknown, mobile, close, unsupported probe and error', flush=True)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
