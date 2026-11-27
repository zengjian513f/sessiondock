#!/usr/bin/env python3
"""Native automatic compaction needs no manual RPC and preserves accepted input."""
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
from contextlib import ExitStack
from types import SimpleNamespace
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from playwright.sync_api import expect, sync_playwright
from codex_sdk_queue_browser import QueueServer
from codex_sdk_browser import THREAD, build, server
from codex_sdk_actions_browser import environment
from history_fixtures import BINARY, codex_row
from codex_ledger_fixtures import queue_rows, legacy_queue
from sdk_ui_fixtures import start_sdk


class CompactionServer(QueueServer):
    async def _handle(self, writer, message):
        if message.get('method') == 'turn/steer' and getattr(self, 'defer_steer', False):
            await self.send(writer, {'id': message['id'], 'result': {'turnId': self.turn}})
        elif message.get('method') == 'thread/turns/list' and getattr(self, 'offline_consumed', None):
            cursor = message['params'].get('cursor')
            if cursor and cursor in ('expired', getattr(self, 'expire_cursor', None)):
                self.expire_cursor = None
                await self.send(writer, {'id':message['id'],'error':{'code':-32602,'message':'expired cursor'}})
                return
            page = int((cursor or 'receipt-page-0').rsplit('-',1)[-1])
            if page < 9:
                assert message['params']['itemsView'] == 'summary', 'unrelated historical tool bodies were requested'
                data = [{'id':f'unrelated-{page}','status':'completed','items':[]}]
                after = f'receipt-page-{page+1}'
            else:
                data, after = [self.offline_consumed], None
            await self.send(writer, {'id':message['id'],'result':{'data':data,'nextCursor':after}})
        elif message.get('method') == 'turn/start' and self.compact_next:
            self.compact_next = False
            self.turn = 'compacting-' + str(len(self.methods('turn/start')))
            self.current_text = message['params']['input'][0]['text']
            await self.notify('thread/status/changed', {'status': {'type': 'active'}})
            await self.notify('turn/started', {'turn': {'id': self.turn, 'status': 'inProgress', 'items': []}})
            if getattr(self, 'terminal_before_reply', None):
                turn, self.turn = self.turn, None
                await self.notify('turn/completed', {'turn': {'id': turn, 'status': self.terminal_before_reply, 'items': []}})
                await self.notify('thread/status/changed', {'status': {'type': 'idle'}})
                await self.send(writer, {'id': message['id'], 'result': {'turn': {'id': turn}}})
                return
            await self.send(writer, {'id': message['id'], 'result': {'turn': {'id': self.turn}}})
            await self.notify('item/started', {'turnId': self.turn, 'item': {'id': 'compact', 'type': 'contextCompaction'}})
        else:
            await super()._handle(writer, message)

    def consume(self):
        async def finish():
            await self.notify('item/completed', {'turnId': self.turn, 'item': {'id': 'compact', 'type': 'contextCompaction'}})
            await self._user_item(self.turn, self.current_text)
        asyncio.run_coroutine_threadsafe(finish(), self.loop).result(timeout=5)


def open_page(browser, base, width, theme):
    context = browser.new_context(viewport={'width': width, 'height': 844},
                                  color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.goto(base, wait_until='networkidle')
    page.locator('#side .item').filter(has_text='earlier synthetic question').first.click()
    expect(page.locator('.sdk-history')).to_be_visible()
    start_sdk(page)
    expect(page.locator('#sdk-input')).to_be_visible()
    expect(page.locator('#sdk-input')).to_be_enabled()
    page.locator('#sdk-input').click()
    page.wait_for_function('CodexSdkView.connected')
    return context, page


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-compact-') as tmp, sync_playwright() as p:
        root = Path(tmp); corpus = build(root)
        port = free_port()
        env = {**environment(root, args.binary), **node_env(root, port, '127.0.0.0/8')}
        browser = p.chromium.launch(headless=True)
        try:
            with CompactionServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake:
                with server(corpus, args.binary, root/'app.sock', env) as base, ExitStack() as cleanup:
                    hub_root = root/'hub'; hub_root.mkdir()
                    hub = Hub(args.binary.with_name('sessiondock-hub').resolve(), hub_root,
                              [SimpleNamespace(name='CompactionNode', token=TOKEN, port=port)])
                    hub.start(); cleanup.callback(hub.stop)
                    for mode, url, width, theme in [('node', base, 1280, 'light'), ('node', base, 390, 'dark'),
                            ('hub', f'http://127.0.0.1:{hub.port}', 1280, 'dark'),
                            ('hub', f'http://127.0.0.1:{hub.port}', 390, 'light')]:
                        context, page = open_page(browser, url, width, theme)
                        text = f'压缩之前输入的消息，接收确认后发送已完成 · {mode}-{width}'
                        fake.compact_next = True
                        page.locator('#sdk-input').fill(text); page.locator('#sdk-send').click()
                        row = page.locator('.sdk-queued').filter(has_text=text)
                        expect(row).to_have_count(0)
                        status = page.locator('.sdk-pane .sdk-status')
                        expect(status).to_have_text('压缩中')
                        expect(status).to_have_attribute('data-state', 'maintenance')
                        accepted = page.locator('.sdk-accepted').filter(has_text=text)
                        expect(accepted).to_be_visible()
                        expect(accepted.locator('button')).to_have_count(0)
                        expect(page.locator('[data-sdk-item="compact"]')).to_have_count(0)
                        starts = len(fake.methods('turn/start'))
                        page.reload(wait_until='networkidle')
                        expect(row).to_have_count(0)
                        expect(status).to_have_text('压缩中')
                        expect(accepted).to_be_visible()
                        assert len(fake.methods('turn/start')) == starts, 'reconnect must not resubmit during automatic compaction'
                        assert not fake.methods('thread/compact/start'), 'native automatic compaction needs no manual request'
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'compact-{mode}-{width}.png'))
                        page.locator('#sdk-stop').click()
                        expect(row).to_have_count(0)
                        starts = len(fake.methods('turn/start'))
                        page.reload(wait_until='networkidle')
                        expect(row).to_have_count(0)
                        ended = page.locator('.sdk-queue details').filter(has_text=text)
                        expect(ended.locator('summary')).to_have_text('回合已中断，未见输入记录')
                        expect(accepted).to_be_hidden()
                        assert len(fake.methods('turn/start')) == starts
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'interrupted-{mode}-{width}.png'))
                        ended.locator('summary').click()
                        expect(accepted).to_be_visible()
                        expect(accepted).not_to_contain_text('等待原生记录')
                        page.locator('#sdk-input').fill('保留这份草稿')
                        page.route('**/api/codex-sdk/queue', lambda route: route.fulfill(status=503,
                            json={'error': 'synthetic dismiss failure'}), times=1)
                        ended.get_by_role('button', name='移除提示').click()
                        expect(page.locator('.sdk-input-feedback')).to_contain_text('移除提示失败')
                        expect(accepted).to_be_visible()
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'receipt-{mode}-{width}.png'))
                        receipt_id = accepted.get_attribute('data-receipt-id')
                        ended.get_by_role('button', name='移除提示').click()
                        expect(ended).to_have_count(0)
                        page.reload(wait_until='networkidle')
                        expect(accepted).to_have_count(0)
                        expect(page.locator('#sdk-input')).to_have_value('保留这份草稿')
                        receipt = next(row for row in queue_rows(root)[THREAD]
                                       if row['id'] == receipt_id)
                        assert receipt['state'] == 'sent' and receipt['text'] == text
                        assert receipt['turn_status'] == 'interrupted' and receipt['receipt_dismissed']
                        assert len(fake.methods('turn/start')) == starts
                        expect(page.locator('#sdk-send')).to_have_text('发送')
                        fake.compact_next = True
                        page.locator('#sdk-input').fill('压缩成功后才确认消息'); page.locator('#sdk-send').click()
                        expect(page.locator('.sdk-queued')).to_have_count(0)
                        expect(status).to_have_text('压缩中')
                        consumed = page.locator('.sdk-accepted').filter(has_text='压缩成功后才确认消息')
                        expect(consumed).to_be_visible()
                        fake.consume()
                        expect(consumed).to_have_count(0)
                        expect(page.locator('.sdk-queued')).to_have_count(0)
                        expect(page.locator('.sdk-live')).to_contain_text('压缩成功后才确认消息')
                        expect(status).to_have_text('处理中')
                        expect(status).to_have_attribute('data-state', 'working')
                        assert not fake.methods('thread/compact/start')
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'continued-{mode}-{width}.png'))
                        # The reported case: an async answer was accepted by
                        # steer, then interrupted before a native userMessage.
                        fake.defer_steer = True
                        asyncio.run_coroutine_threadsafe(fake.notify('item/completed', {
                            'turnId': fake.turn, 'item': {'id': f'async-{mode}-{width}',
                            'type': 'agentMessage', 'delivery': 'async',
                            'questions': [{'title': '是否继续这次修改？请确认这些设置是否需要同时应用到所有现有会话，并保留已经完成的工作、尚未发送的草稿与后台任务。原生处理仍在继续，可以在核对后补充说明。', 'options': ['继续', '稍后']}],
                            'text': '是否继续这次修改？'}}), fake.loop).result(timeout=5)
                        with corpus.paths['sdk'].open('a') as native:
                            native.write(json.dumps(codex_row('response_item', {'type': 'function_call',
                                'name': 'request_user_input_async', 'call_id': f'async-{mode}-{width}',
                                'arguments': json.dumps({'questions': [{'title': '是否继续这次修改？',
                                    'options': ['继续', '稍后']}]}, ensure_ascii=False)})) + '\n')
                        question = page.locator('[data-sdk-async-question]')
                        question.get_by_label('继续', exact=True).check()
                        question.get_by_role('button', name='提交回答').click()
                        answer = page.locator('.sdk-accepted').filter(has_text='是否继续这次修改？')
                        expect(answer).to_be_visible()
                        steers = len(fake.methods('turn/steer'))
                        page.locator('#sdk-stop').click()
                        expect(answer).to_be_hidden()
                        notice = page.locator('.sdk-queue details').filter(has_text='是否继续这次修改？')
                        expect(notice.locator('summary')).to_have_text('回合已中断，未见输入记录')
                        notice.locator('summary').click()
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'async-receipt-{mode}-{width}.png'))
                        answer_id = answer.get_attribute('data-receipt-id')
                        next_text = f'继续处理下一条消息 · {mode}-{width}'
                        fake.compact_next = True
                        page.locator('#sdk-input').fill(next_text)
                        page.locator('#sdk-send').click()
                        next_receipt = page.locator('.sdk-accepted').filter(has_text=next_text)
                        expect(next_receipt).to_be_visible()
                        expect(notice).to_have_count(0)
                        expect(answer).to_have_count(0)
                        receipt = next(row for row in queue_rows(root)[THREAD] if row['id'] == answer_id)
                        assert receipt['state'] == 'sent' and '是否继续这次修改？' in receipt['text']
                        assert receipt['turn_status'] == 'interrupted' and receipt['receipt_dismissed']
                        starts = len(fake.methods('turn/start'))
                        if args.screenshots: page.screenshot(path=str(args.screenshots/f'next-turn-{mode}-{width}.png'))
                        page.reload(wait_until='networkidle')
                        expect(answer).to_have_count(0)
                        expect(question).to_have_count(0)
                        expect(next_receipt).to_be_visible()
                        assert len(fake.methods('turn/start')) == starts
                        assert len(fake.methods('turn/steer')) == steers
                        fake.consume()
                        expect(next_receipt).to_have_count(0)
                        fake.complete()
                        fake.defer_steer = False
                        context.close()
                        print(f'PASS {mode} {width} {theme}: automatic compaction, interrupt, next turn retires the old notice, reload without replay', flush=True)
                    for status in ('completed', 'failed'):
                        context, page = open_page(browser, base, 1280, 'light')
                        fake.compact_next = True; fake.terminal_before_reply = status
                        text = '回合先于接收回执结束 ' + status
                        page.locator('#sdk-input').fill(text); page.locator('#sdk-send').click()
                        ended = page.locator('.sdk-queue details').filter(has_text=text)
                        expect(ended.locator('summary')).to_have_text('回合已结束，未见输入记录')
                        expect(page.locator('.sdk-queued')).to_have_count(0)
                        ended.locator('summary').click()
                        ended.get_by_role('button', name='移除提示').click()
                        expect(ended).to_have_count(0)
                        context.close()
                    fake.terminal_before_reply = None
                    fake.compact_next = True
                    context, page = open_page(browser, base, 1280, 'dark')
                    page.locator('#sdk-input').fill('restart while compacting')
                    page.locator('#sdk-send').click()
                    expect(page.locator('.sdk-queued')).to_have_count(0)
                    expect(page.locator('.sdk-pane .sdk-status')).to_have_text('压缩中')
                    page.wait_for_timeout(200)
                    starts_before_restart = len(fake.methods('turn/start'))
                    context.close()
                entries = queue_rows(root)[THREAD]
                assert entries[-1]['state'] == 'sent'
                with server(corpus, args.binary, root/'app.sock', env) as base:
                    context, page = open_page(browser, base, 1280, 'dark')
                    expect(page.locator('.sdk-queued')).to_have_count(0)
                    expect(page.locator('.sdk-accepted').filter(has_text='restart while compacting')).to_be_visible()
                    page.locator('#sdk-stop').click()
                    expect(page.locator('.sdk-queued')).to_have_count(0)
                    assert len(fake.methods('turn/start')) == starts_before_restart
                    context.close()
                    print('PASS service restart preserves completed delivery without replay', flush=True)
                # Legacy receipts have no terminal status. Recover the exact
                # older turn after restart. A persisted hint for a visible
                # receipt cannot establish that it is still the latest turn.
                ledger = legacy_queue(root)
                legacy = json.loads(ledger.read_text())
                legacy[THREAD] = legacy[THREAD][-1:]
                legacy[THREAD][-1].pop('turn_status', None)
                legacy[THREAD][-1]['recovery_cursor'] = 'expired'
                ledger.write_text(json.dumps(legacy))
                fake.turn = None
                fake.offline_consumed = {'id': entries[-1]['turn_id'], 'status': 'interrupted', 'items': []}
                with server(corpus, args.binary, root/'app.sock', env) as base:
                    context, page = open_page(browser, base, 1280, 'dark')
                    recovered = page.locator('.sdk-queue details').filter(has_text='restart while compacting')
                    expect(recovered).to_have_count(0)
                    receipt = next(row for row in queue_rows(root)[THREAD] if row['id'] == entries[-1]['id'])
                    assert receipt['state'] == 'sent' and receipt['text'] == 'restart while compacting'
                    assert receipt['turn_status'] == 'interrupted' and receipt['receipt_dismissed']
                    assert len(fake.methods('turn/start')) == starts_before_restart
                    assert not page.locator('.sdk-queued').count()
                    context.close()
                    print('PASS legacy terminal receipt retired after recovery finds a newer native turn', flush=True)
                before_hinted = len(fake.methods('thread/turns/list'))
                fake.expire_cursor = 'receipt-page-9'
                fake.offline_consumed = {'id': entries[-1]['turn_id'], 'status': 'completed', 'items': [{
                    'id': 'consumed-while-offline', 'type': 'userMessage',
                    'content': [{'type': 'text', 'text': 'restart while compacting'}]}]}
                with server(corpus, args.binary, root/'app.sock', env) as base:
                    context, page = open_page(browser, base, 1280, 'dark')
                    expect(page.locator('.sdk-accepted').filter(has_text='restart while compacting')).to_have_count(0)
                    assert len(fake.methods('turn/start')) == starts_before_restart
                    context.close()
                    hinted = fake.methods('thread/turns/list')[before_hinted:]
                    assert [call['params']['cursor'] for call in hinted[:2]] == ['receipt-page-9', None], hinted
                    receipt = next(row for row in queue_rows(root)[THREAD] if row['id'] == entries[-1]['id'])
                    assert receipt['native_item_id'] == 'consumed-while-offline'
                    print('PASS native late consumption reconciles retired receipt after exact-turn cursor expires', flush=True)
        finally: browser.close()


if __name__ == '__main__': main()
