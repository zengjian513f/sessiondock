#!/usr/bin/env python3
# run_validation: skip
"""Operator-only real Agy tools and media, using a loopback synthetic model.

Invoke explicitly with --agy PATH. No account, credentials, paid model or
production sessions. The gateway emits only view_file calls for two temporary
fixtures. The real CLI creates native records that Chromium then opens.
"""
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import argparse
import base64
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import threading

from playwright.sync_api import expect, sync_playwright
from browser_runtime import js
from history_parity import BINARY, Corpus, isolated_server

MODEL = 'gemini-3.1-pro-low-thinking'
WIRE_MODEL = 'gemini-3.1-pro-preview'
FINAL = 'AGY_REAL_TOOLS_COMPLETE'
TEXT = 'PRIVATE_READ_ONLY_FIXTURE'
PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII='


@contextmanager
def gateway(root):
    records, errors, calls = [], [], []
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass
        def do_CONNECT(self):
            self.send_error(403, 'external network forbidden')
        def do_GET(self):
            self.send_error(403, 'catalog is explicitly configured')
        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))))
            records.append(request)
            (root / f'request-{len(records)}.json').write_text(json.dumps(request, indent=2))
            if request.get('model') != WIRE_MODEL or self.path != '/v1/chat/completions':
                errors.append('unexpected gateway destination/model')
                self.send_error(403)
                return
            main = any('<identity>' in str(m.get('content', '')) for m in request.get('messages', []))
            delta, finish = {'role': 'assistant', 'content': FINAL if main else 'Private tools fixture'}, 'stop'
            if main and len(calls) < 2:
                names = [t.get('function', {}).get('name') for t in request.get('tools', [])]
                if 'view_file' not in names:
                    errors.append('view_file missing from real model request')
                else:
                    path = root / 'work' / ('safe.txt' if not calls else 'pixel.png')
                    args = {'AbsolutePath': str(path), 'StartLine': 1, 'EndLine': 2,
                            'toolSummary': 'Temporary fixture inspection', 'toolAction': 'Reading temporary fixture'}
                    calls.append(args)
                    delta = {'role': 'assistant', 'tool_calls': [{'index': 0, 'id': f'fixture_{len(calls)}',
                        'type': 'function', 'function': {'name': 'view_file', 'arguments': json.dumps(args)}}]}
                    finish = 'tool_calls'
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            for part, reason in ((delta, None), ({}, finish)):
                chunk = {'id': 'local-fixture', 'object': 'chat.completion.chunk', 'model': WIRE_MODEL,
                         'choices': [{'index': 0, 'delta': part, 'finish_reason': reason}]}
                self.wfile.write(('data: ' + json.dumps(chunk) + '\n\n').encode())
            self.wfile.write(b'data: [DONE]\n\n')
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}', records, errors, calls
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--agy', type=Path, required=True)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    agy, binary = args.agy.resolve(strict=True), args.binary.resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix='sessiondock-agy-tools-real-')).resolve()
    print('ROOT', root, flush=True)
    for name in ('home', 'work', 'config', 'data', 'cache', 'state', 'tmp', 'claude', 'codex', 'grok', 'mirror'):
        (root / name).mkdir(mode=0o700)
    (root / 'work/safe.txt').write_text(TEXT + '\nSECOND_LINE\n')
    (root / 'work/pixel.png').write_bytes(base64.b64decode(PNG))
    settings = root / 'home/.antigravity/settings.json'
    settings.parent.mkdir(mode=0o700)
    settings.write_text(json.dumps({'models': {'list': [{'modelId': MODEL,
        'displayName': 'Synthetic loopback fixture', 'toolFormatterType': 'TOOL_FORMATTER_TYPE_NONE'}]}}))
    with gateway(root) as (url, requests, errors, calls):
        env = {'HOME': str(root / 'home'), 'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8',
            'TERM': 'dumb', 'AGY_CLI_DISABLE_AUTO_UPDATE': '1', 'AGY_LLM_GATEWAY_URL': url + '/v1',
            'AGY_LLM_GATEWAY_MODELS': MODEL, 'AGY_LLM_GATEWAY_API_KEY': 'synthetic-local-fixture',
            'AGY_LLM_GATEWAY_WIRE_PROTOCOL': 'openai', 'HTTP_PROXY': url, 'HTTPS_PROXY': url,
            'ALL_PROXY': url, 'NO_PROXY': 'localhost,127.0.0.1,::1'}
        for key, folder in (('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                ('XDG_CACHE_HOME', 'cache'), ('XDG_STATE_HOME', 'state'), ('TMPDIR', 'tmp')):
            env[key] = str(root / folder)
        argv = [str(agy), '--model', MODEL, '--print', 'Read only safe.txt and pixel.png in this temporary workspace.',
                '--output-format', 'stream-json', '--print-timeout', '20s']
        (root / 'argv.json').write_text(json.dumps(argv))
        process = subprocess.Popen(argv, cwd=root / 'work', env=env, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        try:
            out, err = process.communicate(timeout=30)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate()
            raise
        (root / 'stdout.jsonl').write_bytes(out)
        (root / 'stderr.log').write_bytes(err)
        assert process.returncode == 0, err.decode()
        assert not errors and len(calls) == 2 and requests, (errors, calls)
        assert all(r['model'] == WIRE_MODEL for r in requests)
    native = root / 'home/.gemini/antigravity-cli'
    files = list(native.glob('brain/*/.system_generated/logs/transcript_full.jsonl'))
    assert len(files) == 1, files
    transcript = files[0]
    sid = transcript.parents[2].name
    rows = [json.loads(line) for line in transcript.read_text().splitlines()]
    assert sum(len(r.get('tool_calls', [])) for r in rows) == 2, rows
    assert any(TEXT in r.get('content', '') for r in rows), rows
    images = [m for r in rows for m in r.get('media', [])]
    assert len(images) == 1 and images[0]['mime_type'] == 'image/png', images
    assert Path(images[0]['uri']).read_bytes() == (root / 'work/pixel.png').read_bytes()
    before = transcript.read_bytes()
    with isolated_server(Corpus(root), binary, extra_env={
            'SESSIONDOCK_AGY_HOME': str(native), 'SESSIONDOCK_AGY_ROOT': str(root / 'mirror')}) as (base, _), sync_playwright() as pw:
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        context = browser.new_context(service_workers='block')
        context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
        page = context.new_page()
        page.set_default_timeout(20000)
        page_errors = []
        page.on('pageerror', lambda e: page_errors.append(str(e)))
        page.goto(base, wait_until='networkidle')
        page.wait_for_function(js('sid => S.sessions.some(r => r.sid === sid)',
            'sid => runtime.core.state.catalog.sessions.some(r => r.sid === sid)'), arg=sid)
        uid = page.evaluate(js('sid => S.sessions.find(r => r.sid === sid).uid',
            'sid => runtime.core.state.catalog.sessions.find(r => r.sid === sid).uid'), sid)
        page.locator(f'#side .item[data-uid="{uid}"]').click()
        expect(page.locator('#msgs')).to_contain_text(FINAL)
        if not page.locator('#a-turns').is_visible():
            page.locator('#a-more').click()
        if page.locator('#a-turns').get_attribute('aria-pressed') != 'true':
            page.locator('#a-turns').click()
        for button in page.get_by_role('button', name='展开工具调用组', exact=True).all():
            button.click()
        for button in page.get_by_role('button', name='展开全文').all():
            button.click()
        expect(page.locator('#msgs')).to_contain_text('view_file')
        expect(page.locator('#msgs')).to_contain_text(TEXT)
        image = page.locator('#msgs img').first
        image.scroll_into_view_if_needed()
        expect(image).to_be_visible()
        page.wait_for_function('() => [...document.querySelectorAll("#msgs img")].some(i => i.complete && i.naturalWidth === 1)')
        assert not page_errors, page_errors
        context.close()
        browser.close()
    assert transcript.read_bytes() == before
    print('PASS real Agy view_file calls, native text result and native PNG rendered via Chromium; explicit synthetic wire model, no native transcript writes', flush=True)


if __name__ == '__main__':
    main()
