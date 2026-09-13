#!/usr/bin/env python3
"""Actual HTTP/optional Chromium checks for synthetic native image spans.

No external file roots, model CLI, production data or network access are used.
The PNG padding is a valid ancillary chunk with a checked CRC, not fake pixels.
"""
from __future__ import annotations
import argparse
import base64
import json
import os
from pathlib import Path
import struct
import tempfile
import zlib
from urllib.error import HTTPError

from history_parity import BINARY, Corpus, claude_row, codex_row, encoded, isolated_server
from media_browser import PNG, image, uid

MIB = 1024 * 1024

def padded_png(size):
    small = base64.b64decode(PNG)
    payload = b'p' * (size - len(small) - 12)
    kind = b'npAD'
    chunk = struct.pack('>I', len(payload)) + kind + payload + struct.pack('>I', zlib.crc32(kind + payload))
    result = small[:-12] + chunk + small[-12:]
    assert len(result) == size
    return result

def get(opener, base, route):
    with opener.open(base + route, timeout=90) as response:
        return json.load(response)

def images(history):
    return [item for message in history['messages'] for item in message.get('media', [])]

def check_bytes(opener, base, token, expected):
    with opener.open(base + token, timeout=90) as response:
        assert response.status == 200 and response.headers['Content-Type'] == 'image/png'
        assert response.read() == expected

def status(opener, base, route, expected):
    try:
        opener.open(base + route, timeout=90)
    except HTTPError as error:
        assert error.code == expected, (error.code, expected)
        body = json.loads(error.read(8193))
        assert body.get('error') and 'messages' not in body
        return
    raise AssertionError(f'expected explicit {expected}')

def build(root, payload):
    corpus = Corpus(root)
    for source in ('claude', 'codex', 'grok'):
        (root / source).mkdir()
        name = source + '-native-span'
        block = image(source, base64.b64encode(payload).decode())
        if source == 'claude':
            rows = [claude_row(name, 'user', 'u0', None, 'BEFORE IMAGE'),
                    claude_row(name, 'assistant', 'a0', 'u0', 'READY'),
                    claude_row(name, 'user', 'u1', 'a0', [{'type':'text','text':'LARGE NATIVE'}, block]),
                    claude_row(name, 'assistant', 'call', 'u1', [{'type':'tool_use','id':'tool','name':'synthetic_image','input':{}}]),
                    claude_row(name, 'user', 'result', 'call', [{'type':'tool_result','tool_use_id':'tool',
                        'content':{'content':[block], 'isError':False}}])]
            corpus.put(name, source, rows, [])
        elif source == 'codex':
            rows = [codex_row('session_meta', {'id':name,'cwd':'/synthetic/native-span'}),
                    codex_row('response_item', {'type':'message','role':'user','content':[{'type':'input_text','text':'LARGE NATIVE'}, block]}),
                    codex_row('response_item', {'type':'function_call','name':'synthetic_image','call_id':'tool','arguments':'{}'}),
                    codex_row('response_item', {'type':'function_call_output','call_id':'tool','output':{'content':[block],'isError':False}})]
            corpus.put(name, source, rows, [])
        else:
            path = root / source / 'project-span' / name
            path.mkdir(parents=True)
            rows = [{'type':'user','prompt_index':1,'content':[{'type':'text','text':'LARGE NATIVE'}, block]},
                    {'type':'assistant','content':'','tool_calls':[{'id':'tool','name':'synthetic_image','arguments':'{}'}]},
                    {'type':'tool_result','tool_call_id':'tool','content':{'content':[block],'isError':False}}]
            (path / 'summary.json').write_text(json.dumps({'info':{'id':name,'cwd':'/synthetic/native-span'}}))
            (path / 'chat_history.jsonl').write_bytes(b''.join(encoded(row) for row in rows))
            corpus.paths[name] = path
    return corpus

def browser(corpus, base):
    from playwright.sync_api import expect, sync_playwright
    with sync_playwright() as playwright:
        options = {'headless':True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = playwright.chromium.launch(**options)
        try:
            context = browser.new_context(viewport={'width':1280,'height':900}, service_workers='block')
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.goto(base, wait_until='networkidle')
            for source in ('claude','codex','grok'):
                target = uid(corpus, source + '-native-span')
                page.locator(f'#side .item[data-uid="{target}"]').click()
                expect(page.locator('#msgs')).to_contain_text('LARGE NATIVE')
                picture = page.locator('#msgs img').first
                picture.scroll_into_view_if_needed()
                page.wait_for_function("() => [...document.querySelectorAll('#msgs img')].some(i => i.complete && i.naturalWidth===2 && i.naturalHeight===3)", timeout=90000)
                expect(page.locator('#a-term')).to_be_visible()
                expect(page.locator('#a-term')).to_be_enabled()
            page.set_viewport_size({'width':390,'height':844})
            page.locator(f'#side .item[data-uid="{uid(corpus,"claude-native-span")}"]').click()
            expect(page.locator('#a-term')).to_be_visible()
            assert not errors, errors
            context.close()
        finally:
            browser.close()

def spellings(binary):
    payload = padded_png(3 * MIB)
    data = base64.b64encode(payload).decode()
    alias = image('claude', data, 'image/apng')
    alias['image_url'] = 'data:image/png;base64,' + data
    conflict = image('claude', data)
    conflict['image_url'] = 'data:image/jpeg;base64,' + data
    cases = [
        ('escaped', [image('claude', data)], True),
        ('aliases', [alias], True),
        ('conflict', [conflict], True),
        ('tutorial', [{'type':'text', 'text':'TUTORIAL', 'image_url':'data:image/png;base64,' + data}], 'tutorial'),
        # Batch 34 (WP-B): a giant string outside a reviewed media position is
        # plain text materialized from the checked range, never an image
        # source — the tool call renders, with no media registered.
        ('arguments', [{'type':'tool_use', 'id':'t', 'name':'example', 'input':{'image':image('claude', data)}}], None),
    ]
    for label, content, valid in cases:
        with tempfile.TemporaryDirectory(prefix='sessiondock-native-spelling-') as temporary:
            root = Path(temporary)
            corpus = Corpus(root)
            for source in ('claude','codex','grok'): (root / source).mkdir()
            name = 'claude-' + label
            corpus.put(name, 'claude', [claude_row(name, 'assistant' if label == 'arguments' else 'user', 'u', None, content)], [])
            path = corpus.paths[name]
            if label == 'escaped':
                # Expand actual base64 characters into JSON escapes, not a fake
                # marker. Scanner offsets must count these physical bytes.
                raw = path.read_bytes()
                path.write_bytes(raw.replace(data.encode(), data.replace('c', '\\u0063').encode()))
            before = path.read_bytes()
            with isolated_server(corpus, binary) as (base, opener):
                route = '/api/messages/' + uid(corpus,name) + '?window=1'
                if valid:
                    media = images(get(opener,base,route))
                    assert len(media) == 1, label
                    check_bytes(opener,base,media[0]['src'],payload)
                elif valid is None:
                    body = get(opener,base,route)
                    assert images(body) == [], label
                    assert [m['role'] for m in body['messages']] == ['tool'], label
                    assert 'base64' in body['messages'][0]['text'], label
                elif valid == 'tutorial':
                    body = get(opener,base,route)
                    assert images(body) == [], label
                    assert [m['text'] for m in body['messages']] == ['TUTORIAL'], label
                else:
                    status(opener,base,route,501)
            assert path.read_bytes() == before
    print('PASS native span spellings: physical JSON escapes, first-match MIME/data URL aliases; tutorial extensions and tool arguments stay plain text without media')

def run(binary, use_browser):
    payload = padded_png(3 * MIB)
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-spans-') as temporary:
        corpus = build(Path(temporary), payload)
        before = {path:path.read_bytes() for path in corpus.root.rglob('*') if path.is_file()}
        with isolated_server(corpus, binary) as (base, opener):
            searched = get(opener, base, '/api/search?q=LARGE%20NATIVE')
            assert len(searched['results']) == 3 and searched['incomplete'] is False
            assert 'data:image' not in json.dumps(searched) and 'base64' not in json.dumps(searched)
            tokens = {}
            for source in ('claude','codex','grok'):
                name = source + '-native-span'
                history = get(opener, base, '/api/messages/' + uid(corpus, name) + '?window=1')
                assert history['meta']['supported'] is True
                assert 'data:image' not in json.dumps(history) and 'base64' not in json.dumps(history)
                media = images(history)
                assert len(media) == 2 and all(item.get('lazy') is True for item in media), (source, history)
                tokens[source] = [item['src'] for item in media]
                check_bytes(opener, base, media[0]['src'], payload)
                check_bytes(opener, base, media[0]['src'], payload)  # warm authorization
            if use_browser:
                browser(corpus, base)
            assert all(path.read_bytes() == content for path,content in before.items())
            # Ordinary append keeps the exact old source usable. Native branch
            # rewind then rejects both tokens (the second is never materialized
            # in API-only mode; Chromium may load it in browser mode).
            path = corpus.paths['claude-native-span']
            with path.open('ab') as stream:
                stream.write(encoded(claude_row('claude-native-span','assistant','after','result','APPENDED')))
            check_bytes(opener, base, tokens['claude'][0], payload)
            with path.open('ab') as stream:
                stream.write(encoded({'type':'last-prompt','leafUuid':'a0'}))
            status(opener, base, tokens['claude'][0], 409)
            status(opener, base, tokens['claude'][1], 409)
        print('PASS native spans: three providers structured messages/MCP tools, exact cold/warm GET, append stability, branch revocation, no external file roots' + ('; Chromium desktop/mobile decode' if use_browser else ''))

    # Separate inventories avoid making fixture multiplicity a scan-budget test.
    for size in (32 * MIB, 32 * MIB + 1):
        with tempfile.TemporaryDirectory(prefix='sessiondock-native-boundary-') as temporary:
            root = Path(temporary)
            corpus = Corpus(root)
            for source in ('claude','codex','grok'): (root / source).mkdir()
            payload = padded_png(size)
            name = 'claude-boundary'
            corpus.put(name, 'claude', [claude_row(name,'user','u',None,[image('claude',base64.b64encode(payload).decode())])], [])
            with isolated_server(corpus, binary) as (base, opener):
                history = get(opener, base, '/api/messages/' + uid(corpus,name) + '?window=1')
                media = images(history)
                assert len(media) == 1
                if size == 32 * MIB: check_bytes(opener,base,media[0]['src'],payload)
                else: status(opener,base,media[0]['src'],413)
        print(f'PASS native image boundary {size} bytes')
    spellings(binary)

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--browser', action='store_true')
    args = parser.parse_args()
    run(args.binary, args.browser)
