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
for (const action of ['paths','launch','attach','status','write','receipt','read','time','close','stop','storage','remove']) {
 const b=document.createElement('button');b.textContent=action;b.id=action;
 b.onclick=async()=>{const data={action};for(const k of ['key','id','text','offset','stream'])data[k]=document.getElementById(k).value;
 data.offset=Number(data.offset);document.getElementById('result').textContent='';
 document.getElementById('result').textContent=await(await fetch('/rpc',{method:'POST',body:JSON.stringify(data)})).text();};
 document.getElementById('actions').append(b);
}</script>'''


def main():
    custom_driver = 'SDKHOST_PLATFORM_DRIVER' in os.environ
    driver = json.loads(os.environ.get('SDKHOST_PLATFORM_DRIVER', json.dumps([str(ROOT / 'target/debug/examples/platform_fixture')])))
    driver_env = os.environ.copy()
    if not custom_driver and 'SESSIONDOCK_SDKHOST_BIN' not in driver_env and 'SDKHOST' not in driver_env:
        driver_env['SDKHOST'] = str(ROOT / 'target/debug/sdkhost')
    with tempfile.TemporaryDirectory(prefix='sdkhost-platform-') as tmp:
        root = os.environ.get('SDKHOST_PLATFORM_ROOT', str(Path(tmp) / ('long-state-' * 14)))
        def rpc(payload):
            payload['root'] = root
            p = subprocess.run(driver, input=json.dumps(payload), text=True, capture_output=True, timeout=25, env=driver_env)
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
                    assert act('write', id='bulk-1', text='bulk\n')['state'] == 'written'
                    deadline = time.monotonic() + 15
                    while time.monotonic() < deadline:
                        current = act('status')
                        if current['stdout_len'] >= end + 9*1024*1024: break
                        time.sleep(.05)
                    large, large_end = output()
                    assert 'x'*(9*1024*1024)+'bulk:中文:end' in large
                    storage = act('storage')
                    assert storage['segments'] >= 2 and storage['stored_bytes'] < 2*1024*1024, storage
                    boundary = 8*1024*1024-32
                    part = act('read', offset=boundary)
                    assert bytes(part['bytes']) == large.encode()[boundary:part['next_offset']]
                    page.reload()
                    assert act('write', id='bulk-1', text='must-not-repeat\n')['state'] == 'written'
                    assert act('status')['stdout_len'] == large_end
                    assert act('close', id='close-1')['state'] == 'written'
                    s = finished('echo'); assert s['exit']['code'] == 0 and s['stdin_closed']
                    time.sleep(.15)
                    assert act('attach')['pid'] == first['pid']
                    assert act('receipt', id='stable-1')['state'] == 'written'
                    all_text, _ = output(); assert all_text == large + 'stdin:eof\n'
                    suffix = act('read', offset=large_end)
                    assert bytes(suffix['bytes']).decode() == all_text.encode()[large_end:].decode()
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
                    running = act('launch', key='running')
                    if Path('/proc/self/stat').exists():
                        child_stat = Path(f"/proc/{running['pid']}/stat").read_text()
                        host_pid = int(child_stat[child_stat.rfind(')') + 2:].split()[1])
                        assert host_pid > 1, host_pid
                        host_cmdline = Path(f'/proc/{host_pid}/cmdline').read_text().split('\0')
                        assert str(Path(root) / 'running') in host_cmdline, host_cmdline
                        def host_threads(pid):
                            for line in Path(f'/proc/{pid}/status').read_text().splitlines():
                                if line.startswith('Threads:'):
                                    return int(line.split()[1])
                            raise AssertionError(f'missing Threads in /proc/{pid}/status')
                        def host_sched_slices(pid):
                            total = 0
                            for tid in os.listdir(f'/proc/{pid}/task'):
                                try:
                                    total += int(Path(f'/proc/{pid}/task/{tid}/schedstat').read_text().split()[2])
                                except OSError:
                                    continue
                            return total
                        time.sleep(0.4)
                        threads = host_threads(host_pid)
                        assert threads <= 8, f'sdkhost threads {threads} exceeded 8'
                        slices0 = host_sched_slices(host_pid)
                        time.sleep(3.0)
                        slices1 = host_sched_slices(host_pid)
                        assert slices1 - slices0 <= 3, f'idle sdkhost scheduled {slices1 - slices0} times in 3s'
                        assert host_threads(host_pid) <= 8
                    act('stop', key='running')
                    finished('running')
                    for key in ['echo','tree','running']: act('remove', key=key)
                    print('sdkhost_platform_browser: PASS (stdio, stable receipt, restart attach, offsets, EOF, leader-first descendants, stop, long state path, active compression, boundary reads, offline byte identity)')
                finally:
                    for key in ['echo','tree','running']:
                        try: rpc({'action':'stop','key':key})
                        except Exception: pass
                    browser.close()
        finally:
            server.shutdown(); server.server_close()

if __name__ == '__main__': main()
