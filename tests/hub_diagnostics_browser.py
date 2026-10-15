#!/usr/bin/env python3
"""Real Hub settings UI, independent registry assertions and bounded audit writer."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
from hub_fixtures import Hub, free_port
from node_auth_fixtures import node_env, TOKEN
from diagnostic_retention_browser import stamp, until


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('target/hub-diagnostics'))
    args = parser.parse_args(); args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-hub-diagnostics-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root = Path(tmp); nodes = []
        for name in ['a', 'b']:
            corpus = Corpus(root/name)
            corpus.put(name, 'codex', [codex_row('session_meta', {'id': name, 'cwd': '/synthetic'}),
                                      codex_message('user', 'Synthetic settings fixture')], [])
            (corpus.root/'ids').mkdir(); (corpus.root/'ids/node-id').write_text(name*32)
            node = SimpleNamespace(name=name, nid=name*32, token=TOKEN, port=free_port())
            stack.enter_context(isolated_server(corpus, args.binary, extra_env=node_env(corpus.root, node.port, '127.0.0.0/8')))
            nodes.append(node)
        hubroot = root/'hub'; hubroot.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, nodes)
        audit = hubroot/'audit'
        old = audit/'hub-2000-01-01.jsonl'
        old.write_text(json.dumps({'ts': stamp(-25), 'event': 'old'})+'\n')
        mixed = audit/'hub-2000-01-02.0001.jsonl'
        mixed.write_text(''.join(json.dumps({'ts': stamp(h), 'event': f'fixture.{h}'})+'\n' for h in [-25, -1]))
        evidence = audit/'saved-report.jsonl'; evidence.write_text('protected')
        hub.start(); stack.callback(hub.stop)
        browser = pw.chromium.launch(headless=True); stack.callback(browser.close)
        page = browser.new_page(viewport={'width':1280,'height':900})
        errors=[]; page.on('pageerror',lambda error:errors.append(str(error)))
        base=f'http://127.0.0.1:{hub.port}'
        page.goto(base)
        page.locator('#settings').click()
        page.get_by_role('tab',name='机器',exact=True).click()
        rows=page.locator('#machine-rows .machine-row')
        expect(rows).to_have_count(2)
        assert not old.exists()
        assert [json.loads(x)['event'] for x in mixed.read_text().splitlines()] == ['fixture.-1']
        name_input=rows.first.locator('input[type=text]')
        registry_before = (hubroot/'hub-nodes.json').read_bytes()
        page.route('**/api/nodes/*/display', lambda route: route.fulfill(status=503, json={'error':'synthetic unavailable'}), times=1)
        name_input.fill('Failed rename')
        name_input.press('Enter')
        expect(page.locator('#machine-note')).to_contain_text('保存失败')
        assert (hubroot/'hub-nodes.json').read_bytes() == registry_before
        name_input.fill('Synthetic renamed node')
        name_input.press('Enter')
        until(page, lambda: any(e['event']=='hub.node.display.changed' for e in hub.audit_events()))
        registry=json.loads((hubroot/'hub-nodes.json').read_text())
        assert 'Synthetic renamed node' in json.dumps(registry)
        event=next(e for e in hub.audit_events() if e['event']=='hub.node.display.changed')
        assert event['received_at'] and event['data']['duration_ms'] >= 0
        assert event['page_id'] and event['trace_id'], event
        # Tab traversal from the name input reaches the next enabled control;
        # arrow reorder is then exercised through the actual grip after a click.
        page.keyboard.press('Tab')
        assert page.evaluate('document.activeElement !== document.body')
        grip=page.locator('[data-machine-grip="'+nodes[1].nid+'"]')
        for _ in range(80):
            if grip.evaluate('e => e === document.activeElement'):
                break
            page.keyboard.press('Tab')
        expect(grip).to_be_focused()
        page.keyboard.press('ArrowUp')
        until(page, lambda: any(e['event']=='hub.node.order.changed' for e in hub.audit_events()))
        page.screenshot(path=str(args.output/'settings.png'))
        page.locator('#settings-dialog .modal-actions button').click()
        page.reload()
        page.locator('#settings').click();page.get_by_role('tab',name='机器',exact=True).click()
        expect(rows.first.locator('input[type=text]')).to_have_value('b')
        # A segment appearing during an idle server is pruned without new writes.
        old.write_text(json.dumps({'ts':stamp(-25),'event':'idle-old'})+'\n')
        until(page, lambda:not old.exists(),seconds=70)
        assert evidence.read_text()=='protected'
        page.close(); hub.stop()
        events=hub.audit_events()
        assert any(e['event']=='hub.node.order.changed' for e in events)
        # Restart with an unavailable diagnostic directory; business settings
        # must still work, with a startup warning rather than a service failure.
        unavailable = hubroot/'unavailable'; unavailable.write_text('not a directory')
        hub.env['SESSIONDOCK_AUDIT_DIR'] = str(unavailable)
        hub.start()
        page = browser.new_page(); page.goto(base)
        page.locator('#settings').click();page.get_by_role('tab',name='机器',exact=True).click()
        name_input=page.locator('#machine-rows .machine-row').first.locator('input[type=text]')
        name_input.fill('Sink failure survivor'); name_input.press('Enter')
        until(page, lambda:'Sink failure survivor' in (hubroot/'hub-nodes.json').read_text())
        page.close();hub.stop()
        assert 'hub.audit.unavailable' in hub.process.stderr.read()
        assert not errors, errors
        (args.output/'result.json').write_text(json.dumps({'passed':['settings rename persisted','page/trace/duration correlation','keyboard reorder persisted after reload','legacy ts and numbered mixed-age retention','idle cleanup','shutdown flush','protected evidence','failed save preserves registry and explicit retry succeeds','unavailable startup sink isolates business writes'], 'events':len(events)},indent=2))
        print('PASS Hub settings persistence, correlation, rolling retention, idle, shutdown flush',flush=True)


if __name__ == '__main__':
    from browser_evidence_fixtures import run_with_evidence
    run_with_evidence(main, 'target/hub-diagnostics')
