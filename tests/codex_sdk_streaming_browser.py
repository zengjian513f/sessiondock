#!/usr/bin/env python3
"""Dense Codex SSE progress keeps the pane responsive without dropping text."""
import argparse
import asyncio
from pathlib import Path
import tempfile

from playwright.sync_api import sync_playwright, expect
from codex_sdk_browser import THREAD, build, server, open_sdk
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY
from sdk_ui_fixtures import start_sdk


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-streaming-') as tmp, sync_playwright() as p:
        root = Path(tmp)
        corpus = build(root)
        (root/'state').mkdir()
        with FakeAppServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake, server(corpus, args.binary, root/'app.sock', {'SESSIONDOCK_STATE_DIR':str(root/'state')}) as base:
            browser = p.chromium.launch(headless=True)
            for theme, width in [('light', 1440), ('dark', 390)]:
                context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
                page = context.new_page()
                page.goto(base, wait_until='networkidle')
                open_sdk(page, corpus, expect)
                start_sdk(page)
                page.locator('#sdk-input').click()
                page.locator('#sdk-input').fill('dense streaming ' + theme)
                page.locator('#sdk-send').click()
                expect(page.locator('.sdk-live')).to_contain_text('dense streaming ' + theme)
                page.locator('#sdk-input').fill('unsent draft ' + theme)

                async def notify(method, raw):
                    await fake.notify(method, {'threadId': THREAD, 'turnId': 'turn-1', **raw})

                def emit(method, raw):
                    asyncio.run_coroutine_threadsafe(notify(method, raw), fake.loop).result(5)

                # Even hidden raw reasoning previously rebuilt the pane per token.
                # Observe actual DOM work rather than relying on machine-speed timing.
                page.evaluate('''() => {
                    window.paneChanges = 0;
                    window.paneObserver = new MutationObserver(records => paneChanges += records.length);
                    paneObserver.observe(document.querySelector('.sdk-pane'), {childList:true, subtree:true});
                }''')

                async def raw_burst():
                    for _ in range(1500):
                        await notify('item/reasoning/textDelta', {'itemId':'raw-thought', 'delta':'x'})
                    await notify('item/started', {'item':{'id':'raw-end', 'type':'agentMessage', 'text':'RAW BURST END'}})
                asyncio.run_coroutine_threadsafe(raw_burst(), fake.loop).result(10)
                expect(page.locator('[data-sdk-item="raw-end"] .mb')).to_have_text('RAW BURST END', timeout=15000)
                changes = page.evaluate('paneObserver.disconnect(); paneChanges')
                assert changes < 500, f'hidden tokens repeatedly rebuilt the pane: {changes}'

                for index in range(3):
                    emit('item/started', {'item': {'id': f'kept-tool-{index}',
                        'type': 'commandExecution', 'command': f'completed-check-{index}',
                        'status': 'completed', 'aggregatedOutput': 'unchanged tool output'}})
                for item in [{'id':'dense-answer','type':'agentMessage','text':''},
                             {'id':'dense-plan','type':'plan','text':''},
                             {'id':'dense-thought','type':'reasoning','summary':['']},
                             {'id':'dense-tool','type':'commandExecution','command':'synthetic-dense-tool',
                              'status':'inProgress','aggregatedOutput':''}]:
                    emit('item/started', {'item': item})
                chunks = [f'{i:04d} 流式文本\n' for i in range(600)]
                # Writing to the fake peer's socket does not mean Chromium has
                # consumed the SSE packets yet. Wait for the final item before
                # inspecting the earlier group or counting subsequent work.
                expect(page.locator('[data-sdk-item="dense-tool"]')).to_be_attached()
                page.evaluate('''() => {
                    window.toolbarChanges = 0;
                    window.toolbarObserver = new MutationObserver(records => toolbarChanges += records.length);
                    toolbarObserver.observe(document.querySelector('#sdk-composer'), {childList:true, subtree:true});
                    window.keptGroup = document.querySelector('[data-sdk-item="kept-tool-0"]').closest('.sdk-tool-group');
                    if (!keptGroup) throw Error('missing tool group');
                    window.groupDetachments = 0;
                    window.groupObserver = new MutationObserver(records => {
                        for (const record of records) for (const node of record.removedNodes)
                            if (node === keptGroup) groupDetachments++;
                    });
                    groupObserver.observe(document.querySelector('.sdk-live'), {childList:true, subtree:true});
                }''')

                async def visible_burst():
                    for chunk in chunks:
                        for method, item in [('item/agentMessage/delta','dense-answer'),
                                             ('item/plan/delta','dense-plan'),
                                             ('item/reasoning/summaryTextDelta','dense-thought'),
                                             ('item/commandExecution/outputDelta','dense-tool')]:
                            await notify(method, {'itemId':item, 'delta':chunk, 'summaryIndex':0})
                asyncio.run_coroutine_threadsafe(visible_burst(), fake.loop).result(15)
                for item in ('dense-answer', 'dense-plan'):
                    expect(page.locator(f'[data-sdk-item="{item}"] .mb')).to_have_text(''.join(chunks), timeout=15000)
                toolbar_changes = page.evaluate('toolbarObserver.disconnect(); toolbarChanges')
                assert toolbar_changes < 20, f'text deltas rebuilt unrelated composer controls: {toolbar_changes}'
                assert page.evaluate('groupObserver.disconnect(); keptGroup.isConnected && groupDetachments===0'), 'text deltas detached an unchanged tool group'
                for item in ('dense-thought', 'dense-tool'):
                    card = page.locator(f'[data-sdk-item="{item}"]')
                    group = page.locator(f'.sdk-live .grp.folded:has([data-sdk-item="{item}"]) .fold-toggle')
                    if group.count():
                        group.first.click()
                    card.locator('.more').click()
                    expect(card).to_contain_text(chunks[-1].strip())
                expect(page.locator('#sdk-input')).to_have_value('unsent draft ' + theme)
                page.locator('#sdk-input').click()
                page.screenshot(path=str(args.screenshots/f'{theme}-{width}.png'))
                # Pending final text must be reconciled before the completion body.
                emit('item/agentMessage/delta', {'itemId':'dense-answer', 'delta':'pending tail'})
                emit('item/completed', {'item':{'id':'dense-answer','type':'agentMessage','text':'FINAL AUTHORITATIVE BODY'}})
                expect(page.locator('[data-sdk-item="dense-answer"] .mb')).to_have_text('FINAL AUTHORITATIVE BODY')
                page.locator('#sdk-stop').click()
                expect(page.locator('#sdk-stop')).to_be_hidden()
                expect(page.locator('#sdk-input')).to_have_value('unsent draft ' + theme)
                starts = len(fake.methods('turn/start'))
                page.locator('#a-chat').click()
                page.locator('#a-sdk').click()
                expect(page.locator('#sdk-input')).to_be_visible()
                assert len(fake.methods('turn/start')) == starts, 'view reentry replayed accepted input'
                context.close()
                print(f'PASS {theme}/{width}: 1500 hidden + 2400 visible SSE deltas, pane mutations={changes}, composer mutations={toolbar_changes}, complete text, draft and stop', flush=True)
            browser.close()


if __name__ == '__main__':
    main()
