#!/usr/bin/env python3
"""Claim/header/body/first-grid-paint receipts; isolated shell and delayed HTTP only."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import tempfile
import threading
import time
import uuid

from playwright.sync_api import expect, sync_playwright
from audit_browser import audit_lines, wait_for_events
from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
from terminal_input_browser import host, open_console, xterm_contains


class DelayedResponse(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def cors(self):
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Headers', '*')
        self.send_header('Access-Control-Allow-Methods', 'POST, OPTIONS')

    def do_OPTIONS(self):
        self.send_response(204)
        self.cors()
        self.end_headers()

    def do_POST(self):
        self.rfile.read(int(self.headers.get('Content-Length', 0)))
        if self.path == '/headers':
            time.sleep(.8)
        payload = b'{"ok":true,"token":"DIAGNOSTIC_SECRET_SENTINEL"}'
        self.send_response(200)
        self.cors()
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.flush()
        if self.path == '/body':
            time.sleep(.8)
        try:
            self.wfile.write(payload)
        except (BrokenPipeError, ConnectionResetError):
            pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    slow = ThreadingHTTPServer(('127.0.0.1', 0), DelayedResponse)
    thread = threading.Thread(target=slow.serve_forever, daemon=True)
    thread.start()
    slow_base = f'http://127.0.0.1:{slow.server_port}'
    try:
        with tempfile.TemporaryDirectory(prefix='sessiondock-terminal-diagnostics-') as temporary:
            root = Path(temporary)
            for name in ['host', 'work', 'claude', 'codex', 'grok', 'audit']:
                (root/name).mkdir(mode=0o700)
            corpus = Corpus(root)
            sid = 'synthetic-native-sid'
            corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root/'work')}),
                                    codex_message('user', 'Synthetic diagnostic console')], [])
            uid = corpus.uid(sid)
            native = corpus.paths[sid].read_bytes()
            with host(root, 'synthetic-'+uuid.uuid4().hex, uid), \
                    isolated_server(corpus, args.binary, host_dir=root/'host', audit_dir=root/'audit') as (base, _), \
                    sync_playwright() as pw:
                options = {'headless': True}
                if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                    options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                browser = pw.chromium.launch(**options)
                try:
                    context = browser.new_context(viewport={'width': 608, 'height': 761}, service_workers='block')
                    context.route(lambda url: not url.startswith((base+'/', slow_base+'/')), lambda route: route.abort())
                    context.add_init_script("localStorage.setItem('sessiondock.consoleRenderer', JSON.stringify('grid'))")
                    page = context.new_page()
                    errors = []
                    page.on('pageerror', lambda e: errors.append(str(e)))
                    page.goto(base, wait_until='networkidle')
                    open_console(page, uid)
                    assert page.evaluate('currentTermViewObject().grid')
                    keyboard = page.locator('#termpane .xterm-helper-textarea')
                    keyboard.fill('ping')
                    keyboard.press('Enter')
                    xterm_contains(page, 'RS_PING_OK')
                    expected = {'terminal.claim.received', 'terminal.claim.response_ready',
                                'browser.http.response.headers', 'browser.terminal.connecting',
                                'browser.terminal.first_output', 'browser.terminal.snapshot_applied',
                                'browser.terminal.first_paint'}
                    wait_for_events(page, root/'audit', expected)
                    rows = audit_lines(root/'audit')
                    ready = next(r for r in rows if r['event'] == 'terminal.claim.response_ready')
                    assert ready['data']['status'] == 200 and ready['data']['elapsed_ms'] >= 0
                    assert ready['trace_id'] and ready['page_id']
                    assert any(r['event'] == 'browser.http.response.headers' and r['trace_id'] == ready['trace_id']
                               and r['page_id'] == ready['page_id'] for r in rows)
                    frames = [r for r in rows if r['event'] in {
                        'browser.terminal.connecting', 'browser.terminal.first_output',
                        'browser.terminal.snapshot_applied', 'browser.terminal.first_paint'}]
                    assert len(frames) == 4 and len({r['connection_id'] for r in frames}) == 1, frames
                    assert frames[-1]['event'] == 'browser.terminal.first_paint'
                    assert frames[-1]['data']['visible'] and frames[-1]['data']['width'] > 0
                    # The node's own receipts for the typed bytes and the echo,
                    # joined to the page's rows by page and connection id.
                    connection = frames[0]['connection_id']
                    wait_for_events(page, root/'audit', {'terminal.input.written', 'terminal.output.sent'})
                    rows = audit_lines(root/'audit')
                    written = [r for r in rows if r['event'] == 'terminal.input.written']
                    sent = [r for r in rows if r['event'] == 'terminal.output.sent']
                    for row in written + sent:
                        assert row['client'] == 'server' and row['page_id'] == ready['page_id'], row
                        assert row['data']['connection'] == connection, row
                        assert row['data']['frames'] >= 1 and row['data']['max_write_ms'] >= 0, row
                    assert sum(r['data']['bytes'] for r in written) >= len('ping\r'), written
                    assert sum(r['data']['bytes'] for r in sent) > 0, sent
                    # A split snapshot must not claim success before the last byte;
                    # a hidden canvas must not claim it was visibly painted.
                    page.evaluate("""() => {
                      window.gridReceipts=[];
                      const mount=document.createElement('div'); mount.id='diagnostic-grid';
                      mount.style='position:fixed;top:180px;left:10px;width:300px;height:100px;z-index:99999';
                      document.body.append(mount);
                      window.diagnosticGrid=new GridTerm({onDiagnostic:(event,data)=>gridReceipts.push({event,data})});
                      diagnosticGrid.open(mount); mount.hidden=true;
                      const wire=JSON.stringify({t:'snapshot',seq:1,reset:true,cols:20,rows:2,
                        grid:[{s:[['DIAGNOSTIC_GRID',-1,-1,0]],w:false},{s:[],w:false}],history:[],
                        cursor:{x:0,y:1,visible:true}})+'\\n';
                      const b=document.createElement('button'); b.id='diagnostic-frame'; b.textContent='Frame';
                      b.style='position:fixed;top:140px;left:10px;z-index:99999'; document.body.append(b);
                      let step=0;
                      b.onclick=()=>{
                        if(step===0)diagnosticGrid.write(wire.slice(0,-1));
                        if(step===1)diagnosticGrid.write(wire.slice(-1));
                        if(step===2){mount.hidden=false;diagnosticGrid.refresh();}
                        step++;
                      };
                    }""")
                    frame = page.locator('#diagnostic-frame')
                    frame.click()
                    page.wait_for_timeout(80)
                    assert page.evaluate('gridReceipts.length') == 0
                    frame.click()
                    page.wait_for_timeout(80)
                    assert page.evaluate('gridReceipts.map(r=>r.event)') == ['snapshot_applied']
                    frame.click()
                    page.wait_for_function("gridReceipts.some(r=>r.event==='first_paint')")
                    assert page.evaluate('gridReceipts.map(r=>r.event)') == ['snapshot_applied', 'first_paint']
                    page.evaluate("diagnosticGrid.dispose(); document.querySelector('#diagnostic-frame').remove(); document.querySelector('#diagnostic-grid').remove()")
                    # A synthetic action button uses the production post() and real delayed HTTP.
                    page.evaluate('''base => {
                      const b = document.createElement('button'); b.id='diagnostic-request';
                      b.style='position:fixed;top:100px;left:10px;z-index:99999';
                      b.textContent='Probe'; document.body.append(b);
                      b.onclick=async()=>{
                        try {await post(base+'/'+b.dataset.phase,{request_id:'diag-'+b.dataset.phase}, {timeoutMs:400});
                          b.dataset.result='ok';}
                        catch(e){b.dataset.result=e.name;}
                      };
                    }''', slow_base)
                    button = page.locator('#diagnostic-request')
                    for phase in ['headers', 'body', 'ok']:
                        button.evaluate('(b,phase)=>{b.dataset.phase=phase;delete b.dataset.result}', phase)
                        button.click()
                        expect(button).to_have_attribute('data-result', 'ok' if phase == 'ok' else 'TimeoutError')
                    wait_for_events(page, root/'audit', {'browser.http.request.failed'})
                    page.wait_for_function('!browserAuditQueue.length && !browserAuditSending')
                    rows = audit_lines(root/'audit')
                    failures = {r['trace_id']: r['data'] for r in rows if r['event'] == 'browser.http.request.failed'}
                    assert failures['diag-headers']['phase'] == 'headers' and failures['diag-headers']['headers_ms'] is None
                    assert failures['diag-body']['phase'] == 'body' and failures['diag-body']['status'] == 200
                    assert failures['diag-body']['headers_ms'] >= 0
                    ok = next(r['data'] for r in rows if r['trace_id'] == 'diag-ok' and r['event'] == 'browser.http.response.received')
                    assert ok['headers_ms'] >= 0 and ok['body_ms'] >= 0
                    assert 'DIAGNOSTIC_SECRET_SENTINEL' not in json.dumps(rows)
                    assert 'RS_PING_OK' not in json.dumps(rows), 'terminal bytes reached the audit'
                    # Leaving the page ends the attach; the node records why.
                    page.close()
                    deadline = time.monotonic() + 10
                    while not any(r['event'] == 'terminal.attach.closed' and r['data']['connection'] == connection
                                  for r in audit_lines(root/'audit')):
                        assert time.monotonic() < deadline, 'no terminal.attach.closed receipt'
                        time.sleep(.1)
                    assert not errors, errors
                    assert corpus.paths[sid].read_bytes() == native
                finally:
                    browser.close()
    finally:
        slow.shutdown()
        slow.server_close()
        thread.join()
    print('PASS terminal diagnostics: node/browser trace correlation, header vs body deadlines, mobile grid first output/snapshot/paint, keyboard input, metadata only')


if __name__ == '__main__':
    main()
