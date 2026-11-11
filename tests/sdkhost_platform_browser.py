#!/usr/bin/env python3
"""Chromium exercises the native host/client via a private loopback fixture UI.

Build sdkhost and sdkhost-client --example platform_fixture first. SDKHOST_PLATFORM_DRIVER
is a JSON argv array (optionally ssh + remote fixture binary); SDKHOST_PLATFORM_ROOT is
an absent private test directory. No production service or CLI is used.
"""
from __future__ import annotations
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[1]
HTML = '''<!doctype html><meta charset="utf-8"><title>SDK host platform fixture</title>
<h1>SDK host platform fixture</h1><label>Instance <input id="key" value="echo"></label>
<label>Transport ID <input id="id" value="input-1"></label>
<label>Input <textarea id="text"></textarea></label><label>Offset <input id="offset" value="0"></label>
<select id="stream"><option>stdout</option><option>stderr</option></select>
<div id="actions"></div><pre id="result"></pre><script>
for (const action of ['paths','launch','attach','status','write','receipt','read','time','close','stop','remove']) {
 const b=document.createElement('button');b.textContent=action;b.id=action;
 b.onclick=async()=>{const data={action};for(const k of ['key','id','text','offset','stream'])data[k]=document.getElementById(k).value;
 data.offset=Number(data.offset);document.getElementById('result').textContent='';
 document.getElementById('result').textContent=await(await fetch('/rpc',{method:'POST',body:JSON.stringify(data)})).text();};
 document.getElementById('actions').append(b);
}</script>'''


def main():
    driver = json.loads(os.environ.get('SDKHOST_PLATFORM_DRIVER', json.dumps([str(ROOT / 'target/debug/examples/platform_fixture')])))
    with tempfile.TemporaryDirectory(prefix='sdkhost-platform-') as tmp:
        root = os.environ.get('SDKHOST_PLATFORM_ROOT', str(Path(tmp) / ('long-state-' * 14)))
        def rpc(payload):
            payload['root'] = root
            p = subprocess.run(driver, input=json.dumps(payload), text=True, capture_output=True, timeout=25)
            assert p.returncode == 0, (p.returncode, p.stdout, p.stderr)
            return json.loads(p.stdout)
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_GET(self):
                self.send_response(200); self.end_headers(); self.wfile.write(HTML.encode())
            def do_POST(self):
                try: result = rpc(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
                except Exception as e: result = {'error': repr(e)}
                self.send_response(200); self.end_headers(); self.wfile.write(json.dumps(result).encode())
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            with sync_playwright() as pw:
                browser = pw.chromium.launch(headless=True)
                page = browser.new_page()
                url = f'http://127.0.0.1:{server.server_port}'
                page.goto(url)
                def act(action, **values):
                    for key, value in values.items():
                        if key == 'stream': page.locator('#stream').select_option(value)
                        else: page.locator('#' + key).fill(str(value))
                    page.locator('#' + action).click()
                    page.wait_for_function("document.querySelector('#result').textContent !== ''", timeout=30000)
                    result = json.loads(page.locator('#result').inner_text())
                    assert 'error' not in result, (action, result)
                    return result['value']
                def output(stream='stdout', key='echo'):
                    cursor = 0; data = b''
                    for _ in range(100):
                        part = act('read', key=key, stream=stream, offset=cursor)
                        data += bytes(part['bytes']); cursor = part['next_offset']
                        if not part['bytes']: break
                    return data.decode(), cursor
                def finished(key):
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        s = act('status', key=key)
                        if s['exit'] is not None and s['stdout_eof'] and s['stderr_eof']: return s
                        time.sleep(.05)
                    raise AssertionError(('host did not finish', s))
                try:
                    assert act('paths')['path_length'] > 260
                    first = act('launch')
                    assert act('launch')['pid'] == first['pid']
                    assert act('write', text='hello 中文\n', id='stable-1')['state'] == 'written'
                    assert act('write', text='must-not-appear\n', id='stable-1')['state'] == 'written'
                    page.reload()  # Every RPC is also a fresh process, with no retained Client.
                    assert act('attach')['pid'] == first['pid']
                    text, end = output()
                    assert all(f'{name}=true' in text for name in ['env:override','env:removed','env:unicode','arg:unicode']), text
                    assert text.count('echo:hello 中文') == 1 and 'must-not-appear' not in text, text
                    assert act('time', offset=end) > 0
                    assert 'err:hello 中文' in output('stderr')[0]
                    assert act('receipt', id='stable-1')['state'] == 'written'
                    assert act('close', id='close-1')['state'] == 'written'
                    s = finished('echo'); assert s['exit']['code'] == 0 and s['stdin_closed']
                    time.sleep(.15)
                    assert act('attach')['pid'] == first['pid']
                    assert act('receipt', id='stable-1')['state'] == 'written'
                    all_text, _ = output(); assert 'stdin:eof' in all_text
                    suffix = act('read', offset=end)
                    assert bytes(suffix['bytes']).decode() == all_text.encode()[end:].decode()
                    act('launch', key='tree')
                    act('write', key='tree', id='fork-1', text='fork\n')
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        s = act('status', key='tree')
                        if s['exit'] is not None: break
                        time.sleep(.05)
                    assert s['exit']['code'] == 0 and not s['stdout_eof'], s
                    assert 'worker:alive' in bytes(act('read', key='tree', offset=0, stream='stdout')['bytes']).decode()
                    act('stop', key='tree')
                    finished('tree')
                    act('launch', key='running')
                    act('stop', key='running')
                    finished('running')
                    for key in ['echo','tree','running']: act('remove', key=key)
                    print('sdkhost_platform_browser: PASS (stdio, stable receipt, restart attach, offsets, EOF, leader-first descendants, stop, long state path)')
                finally:
                    for key in ['echo','tree','running']:
                        try: rpc({'action':'stop','key':key})
                        except Exception: pass
                    browser.close()
        finally:
            server.shutdown(); server.server_close()

if __name__ == '__main__': main()
