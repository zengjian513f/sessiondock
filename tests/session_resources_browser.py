#!/usr/bin/env python3
"""Exercise session resource drawer with a private node and injected fleet samples."""
from browser_runtime import js
import argparse
from pathlib import Path
import os
import json
import tempfile
from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, codex_row, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-resources-') as temporary, sync_playwright() as pw:
        corpus = Corpus(Path(temporary))
        corpus.put('resources', 'codex', [
            codex_row('session_meta', {'id': 'resources', 'cwd': '/synthetic/resources', 'timestamp': '2026-09-11T10:00:00Z'}),
            codex_row('response_item', {'type': 'message', 'role': 'user', 'content': '资源统计测试'})], [])
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
                page.evaluate(js('''() => {
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
                }''', """() => {
                  window.resourceCalls = [];
                  window.probeState = {state:"unsupported", remaining_seconds:0};
                  runtime.resources.setLoader(async (uid, scope) => {
                    resourceCalls.push({uid, scope});
                    const metrics = {cpu_cores: {value: scope === 'inclusive' ? 4.5 : 1.25, status:'ok'}, gpu_count:{value:1,status:'partial',reason:'NVIDIA compute-app residency only'},
                      memory_pss_bytes:{value:1073741824,status:'ok'}, nfs_read_bytes_per_second:{value:null,status:'unsupported',reason:'NFS 探针不可用'}};
                    return {sampled_at:1790812800, totals:metrics, nodes:[
                      {node_id:'a',node_name:'compute-a',status:'ok',metrics,diagnostic:{...probeState}} ,
                      {node_id:'b',node_name:'compute-b',status:'offline',reason:'机器离线',metrics:{},diagnostic:{state:'unsupported',remaining_seconds:0}}]};
                  });
                }"""))
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
                page.get_by_role('button', name='关闭资源面板').click()
                page.evaluate(js("SessionDockResources.setLoader(async () => {throw new Error('测试采集端离线')})", "runtime.resources.setLoader(async () => {throw new Error('测试采集端离线')})"))
                page.locator('#side .item-resources').first.click()
                page.get_by_text('测试采集端离线').wait_for()
                print('PASS resource drawer: selection, inclusive totals, Chinese tooltips, compact layout, refresh, offline/unknown, mobile, close, unsupported probe and error', flush=True)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
