#!/usr/bin/env python3
"""Keyboard-to-PTY-to-grid timing with an isolated, sequence-numbered echo child.

Default: exercise the real node and Hub UI locally. --serve keeps the private
loopback fixture available for a browser on another machine through SSH port
forwarding; --base/--uid runs only that browser. Timings are observations, not
machine-dependent latency assertions. No production sessions or paid CLIs.
"""
import argparse
from contextlib import contextmanager
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace


ECHO = r"""
import os, tty
tty.setraw(0)
os.write(1, b'RTT_READY\r\n')
n = 0
while True:
    key = os.read(0, 1)
    if not key or key == b'\x04':
        break
    if not b'a' <= key <= b'z':
        continue
    n += 1
    os.write(1, ('RTT_%06d:' % n).encode() + key + b'\r\n')
"""

PROBE = r"""() => {
  window.__rtt = {samples: [], phase: 'warmup', wire: '', stats: {}};
  const Native = window.WebSocket;
  window.WebSocket = class extends Native {
    constructor(...args) {
      super(...args);
      if (!new URL(args[0], location.href).pathname.endsWith('/api/term/attach')) return;
      const decoder = new TextDecoder();
      this.addEventListener('message', event => {
        if (!(event.data instanceof ArrayBuffer)) return;
        const now = performance.now();
        const stats = __rtt.stats[__rtt.phase] ||= {frames:0, bytes:0, paints:0, draw_ms:0};
        stats.frames++;
        stats.bytes += event.data.byteLength;
        __rtt.wire = (__rtt.wire + decoder.decode(event.data, {stream:true})).slice(-65536);
        for (const s of __rtt.samples) {
          if (s.received == null && __rtt.wire.includes(s.marker)) s.received = now;
        }
      });
    }
    send(data) {
      if (typeof data !== 'string') {
        const key = new TextDecoder().decode(data);
        const s = __rtt.samples.find(s => s.sent == null && s.key === key);
        if (s) s.sent = performance.now();
      }
      return super.send(data);
    }
  };
  document.addEventListener('keydown', event => {
    if (!event.target.matches('#termpane .xterm-helper-textarea') || !/^[a-z]$/.test(event.key)) return;
    const n = (__rtt.offset || 0) + __rtt.samples.length + 1;
    __rtt.samples.push({n, key:event.key, phase:__rtt.phase, start:performance.now(),
      marker:`RTT_${String(n).padStart(6,'0')}:${event.key}`});
  }, true);
}"""

INSTALL_PAINT = r"""() => {
  const term = currentTermViewObject().term;
  const initial = term.buffer.active;
  const text = Array.from({length:initial.length}, (_,i) =>
    initial.getLine(i)?.translateToString(true) || '').join('\n');
  __rtt.offset = Math.max(0, ...Array.from(text.matchAll(/RTT_(\d+):/g), m => Number(m[1])));
  const paint = term._paint;
  term._paint = function(...args) {
    const start = performance.now();
    const result = paint.apply(this, args);
    const now = performance.now();
    const stats = __rtt.stats[__rtt.phase] ||= {frames:0, bytes:0, paints:0, draw_ms:0};
    stats.paints++;
    stats.draw_ms += now-start;
    const buffer = this.buffer.active;
    const text = Array.from({length: Math.min(buffer.length, this.rows)}, (_, i) =>
      buffer.getLine(buffer.length - 1 - i)?.translateToString(true) || '').join('\n');
    for (const s of __rtt.samples) {
      if (s.painted == null && text.includes(s.marker)) s.painted = now;
    }
    return result;
  };
}"""


def summary(values):
    ordered = sorted(values)
    return {"n": len(values), "p50_ms": round(statistics.median(values), 2),
            "p95_ms": round(ordered[math.ceil(len(ordered)*.95)-1], 2),
            "max_ms": round(max(values), 2)}


def browser_run(base, uid, args):
    from playwright.sync_api import sync_playwright, expect
    with sync_playwright() as pw:
        launch = {"headless": True}
        executable = os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE")
        if executable:
            launch["executable_path"] = executable
        browser = pw.chromium.launch(**launch)
        try:
            context = browser.new_context(viewport={"width":1280, "height":900}, service_workers="block")
            context.add_init_script('(' + PROBE + ')()')
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(base, wait_until="networkidle", timeout=30000)
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            if not page.locator('#termpane').is_visible():
                page.locator('#a-term').click()
            expect(page.locator('#termpane .grid-canvas')).to_be_visible()
            page.wait_for_function("T.ws?.readyState === 1 && currentTermViewObject()?.term?._firstPaint")
            page.evaluate(INSTALL_PAINT)
            keyboard = page.locator('#termpane .xterm-helper-textarea')
            keyboard.focus()
            print('READY browser console', flush=True)
            for phase, count, delay in [('warmup', 5, 80), ('single', args.samples, 80), ('burst', 40, 10)]:
                page.evaluate('phase => __rtt.phase = phase', phase)
                for i in range(count):
                    keyboard.press(chr(97 + i % 26))
                    if phase != 'burst' or i % 8 == 7:
                        page.wait_for_function('__rtt.samples.every(s => s.painted != null)', timeout=10000)
                    page.wait_for_timeout(delay)
                page.wait_for_function('__rtt.samples.every(s => s.painted != null)', timeout=10000)
                print(f'PASS {phase}: {count} keys returned and painted', flush=True)
            samples = page.evaluate('__rtt.samples')
            assert len(samples) == 5 + args.samples + 40, len(samples)
            text = page.evaluate(r"""() => { const b=currentTermViewObject().term.buffer.active;
                return Array.from({length:b.length}, (_,i)=>b.getLine(i)?.translateToString(true)||'').join('\n'); }""")
            assert all(text.count(s['marker']) == 1 for s in samples), 'missing or duplicated echo'
            assert not errors, errors
            assert all(s.get('sent') is not None and s.get('received') is not None for s in samples), samples
            result = {"browser": browser.version, "samples": samples, "phases": {},
                      "render_boundary": "Canvas draw completion, not compositor presentation",
                      "stats": page.evaluate('__rtt.stats'),
                      "build": context.request.get(base.rstrip('/')+'/api/meta').json().get('build')}
            for phase in ('single', 'burst'):
                rows = [s for s in samples if s['phase'] == phase]
                result['phases'][phase] = {
                    'keydown_to_send': summary([s['sent']-s['start'] for s in rows]),
                    'send_to_receive': summary([s['received']-s['sent'] for s in rows]),
                    'receive_to_paint': summary([s['painted']-s['received'] for s in rows]),
                    'keydown_to_paint': summary([s['painted']-s['start'] for s in rows]),
                }
            print(json.dumps(result['phases']), flush=True)
            context.close()
            return result
        finally:
            browser.close()


@contextmanager
def fixture(args):
    from history_fixtures import REPO, Corpus, codex_row, codex_message, isolated_server
    from hub_fixtures import Hub, free_port, scoped
    from terminal_browser import stop
    with tempfile.TemporaryDirectory(prefix='sessiondock-rtt-') as temporary:
        root = Path(temporary)
        for name in ('host','work','claude','codex','grok','hub'):
            (root/name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-rtt-session'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id':sid,'cwd':str(root/'work')}),
                                codex_message('user','Synthetic terminal round trip')], [])
        uid = corpus.uid(sid)
        native = corpus.paths[sid].read_bytes()
        node = SimpleNamespace(nid='b'*32, name='RTTFixture', token='f'*64, port=free_port())
        token, identity = root/'node-token', root/'node-id'
        for path, value in ((token,node.token),(identity,node.nid)):
            path.touch(mode=0o600)
            path.write_text(value)
        env = {k:v for k,v in os.environ.items() if k in ('PATH','LANG','LC_ALL','LC_CTYPE')}
        env['TERM'] = 'xterm-256color'
        metadata = {'source':'codex','sid':sid,'uid':uid,'instance_id':'synthetic-rtt-instance'}
        host_binary = args.ptyhost or str(Path(args.binary).with_name('ptyhost'))
        with open(root/'host.log','w+b') as log:
            host = subprocess.Popen([host_binary,'--dir',str(root/'host'),'run','--name','synthetic-rtt',
                '--cwd',str(root/'work'),'--cols','100','--rows','32','--meta',json.dumps(metadata),
                '--',sys.executable,'-u','-c',ECHO], env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic()+10
                while not (root/'host/synthetic-rtt.json').exists():
                    assert host.poll() is None, 'private host exited'
                    if time.monotonic() > deadline:
                        raise AssertionError('private host startup timeout')
                    time.sleep(.03)
                extra = {'SESSIONDOCK_NODE_BIND':f'127.0.0.1:{node.port}',
                         'SESSIONDOCK_NODE_TOKEN_FILE':str(token),'SESSIONDOCK_NODE_ID_FILE':str(identity),
                         'SESSIONDOCK_NODE_PEERS':'127.0.0.0/8'}
                with isolated_server(corpus,Path(args.binary),host_dir=root/'host',extra_env=extra) as (base,_):
                    hub = Hub(Path(args.hub_binary or str(Path(args.binary).with_name('sessiondock-hub'))),root/'hub',[node])
                    hub.start()
                    try:
                        yield {'node':{'base':base,'uid':uid},
                               'hub':{'base':f'http://127.0.0.1:{hub.port}','uid':scoped(node.nid,uid)}}
                    finally:
                        hub.stop()
                assert corpus.paths[sid].read_bytes() == native
            finally:
                # Only this fixture's host/child; no discovery or production roots.
                if host.poll() is None:
                    subprocess.run([host_binary,'--dir',str(root/'host'),'kill','synthetic-rtt','--force'],
                                   env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)
                stop(host)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default=str(Path(__file__).resolve().parents[1]/'target/release/sessiondock'))
    parser.add_argument('--ptyhost')
    parser.add_argument('--hub-binary')
    parser.add_argument('--base')
    parser.add_argument('--uid')
    parser.add_argument('--samples', type=int, default=40)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--serve', type=Path, help='write loopback fixture endpoints here; wait for <file>.stop')
    args = parser.parse_args()
    if args.samples < 1:
        parser.error('--samples must be positive')
    if args.base:
        assert args.uid, '--uid is required with --base'
        result = browser_run(args.base,args.uid,args)
    else:
        with fixture(args) as endpoints:
            if args.serve:
                args.serve.write_text(json.dumps(endpoints))
                print('READY '+json.dumps(endpoints),flush=True)
                deadline = time.monotonic()+900
                while not args.serve.with_suffix('.stop').exists():
                    if time.monotonic()>deadline:
                        raise TimeoutError('private fixture lifetime expired')
                    time.sleep(.2)
                return
            result = {}
            # Each browser starts with sequence one, so local paths get separate fixtures.
            result['node'] = browser_run(**endpoints['node'],args=args)
        with fixture(args) as endpoints:
            result['hub'] = browser_run(**endpoints['hub'],args=args)
    if args.output:
        args.output.write_text(json.dumps(result,indent=2))


if __name__ == '__main__':
    main()
