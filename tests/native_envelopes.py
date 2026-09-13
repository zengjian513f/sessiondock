#!/usr/bin/env python3
"""Isolated real HTTP/Chromium acceptance for stringified native tool images.

All sources are synthetic; no CLI, native home or external file root is used.
"""
from __future__ import annotations
import argparse
import base64
import json
import os
from pathlib import Path
import tempfile

from history_parity import BINARY, Corpus, codex_message, codex_row, encoded, isolated_server
from media_browser import image
from native_spans import MIB, check_bytes, get, images, padded_png, status


def stringify(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':'))


def envelope(payload, depth=1, *, prefix=False, escaped=False, count=1):
    block = image('codex', base64.b64encode(payload).decode())
    value = {'content':[{'type':'text','text':'NESTED TEXT'}, *[block for _ in range(count)]], 'isError':False}
    for level in range(depth):
        value = stringify({'wall_time_seconds':.25, 'exit_code':0, 'output':value,
                           'audit_marker':'tail-old', 'unicode':'尾部🧪', 'level':level})
        if escaped and level == 0:
            value = value.replace('cHBw', '\\u0063HBw')
    if prefix:
        value = 'protocol header\nOutput:\n' + value + '\n  '
    return value


def corpus_for(root, output, name='codex-nested', *, argument=False, message=False):
    corpus = Corpus(root)
    for source in ('claude','codex','grok'): (root / source).mkdir()
    rows = [codex_row('session_meta', {'id':name, 'cwd':'/synthetic/nested-tools'}),
            codex_message('user', 'BEFORE NESTED')]
    if message:
        rows.append(codex_row('response_item', {'type':'message','role':'user',
                    'content':[{'type':'input_text','text':output}]}))
    else:
        rows.append(codex_row('response_item', {'type':'function_call', 'name':'synthetic_image',
                    'call_id':'call', 'arguments':output if argument else '{"z":"first","a":false}'}))
        if not argument:
            rows.append(codex_row('response_item', {'type':'function_call_output', 'call_id':'call', 'output':output}))
    corpus.put(name,'codex',rows,[])
    return corpus


def browser(corpus, base):
    from playwright.sync_api import expect, sync_playwright
    with sync_playwright() as playwright:
        options = {'headless':True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        chrome = playwright.chromium.launch(**options)
        try:
            context = chrome.new_context(viewport={'width':1280,'height':900},service_workers='block')
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base+'/') else route.abort())
            page = context.new_page()
            errors = []
            page.on('pageerror',lambda error: errors.append(str(error)))
            page.goto(base,wait_until='networkidle')
            page.locator(f'#side .item[data-uid="{corpus.uid("codex-nested")}"]').click()
            expect(page.locator('#msgs')).to_contain_text('NESTED TEXT')
            page.locator('#msgs img').first.scroll_into_view_if_needed()
            page.wait_for_function("() => [...document.querySelectorAll('#msgs img')].some(i=>i.complete&&i.naturalWidth===2&&i.naturalHeight===3)", timeout=90000)
            expect(page.locator('#a-term')).to_be_visible()
            expect(page.locator('#a-term')).to_be_enabled()
            page.set_viewport_size({'width':390,'height':844})
            # Responsive navigation opens the mobile list; explicitly enter
            # the same chat before checking its console, as a user would.
            page.locator(f'#side .item[data-uid="{corpus.uid("codex-nested")}"]').click()
            expect(page.locator('#msgs')).to_contain_text('NESTED TEXT')
            expect(page.locator('#a-term')).to_be_visible()
            assert not errors, errors
            context.close()
        finally:
            chrome.close()


def valid_case(binary, payload, output, label, use_browser=False, expected_error=False):
    with tempfile.TemporaryDirectory(prefix='sessiondock-nested-tool-') as temporary:
        corpus = corpus_for(Path(temporary),output)
        path = corpus.paths['codex-nested']
        original = path.read_bytes()
        with isolated_server(corpus,binary) as (base,opener):
            route = '/api/messages/'+corpus.uid('codex-nested')+'?window=1'
            history = get(opener,base,route)
            media = images(history)
            assert len(media) == 1 and media[0].get('lazy') is True, label
            results = [row for row in history['messages'] if 'NESTED TEXT' in row.get('text','')]
            assert len(results) == 1 and results[0].get('error') is expected_error, label
            assert results[0].get('call_id') == 'call' and results[0].get('exit_code') == 0
            assert 'data:image' not in stringify(history) and 'base64' not in stringify(history)
            check_bytes(opener,base,media[0]['src'],payload)
            check_bytes(opener,base,media[0]['src'],payload)
            # Existing search intentionally excludes command/tool output roles.
            # Search ordinary user text in the same large-image session.
            searched = get(opener,base,'/api/search?q=BEFORE%20NESTED')
            assert len(searched['results']) == 1 and searched['incomplete'] is False
            tool_only = get(opener,base,'/api/search?q=NESTED%20TEXT')
            assert not tool_only['results'] and tool_only['incomplete'] is False
            if use_browser: browser(corpus,base)
        assert path.read_bytes() == original
    print('PASS nested tool '+label,flush=True)


def run(binary, use_browser, smoke):
    payload = padded_png(3*MIB)
    valid_case(binary,payload,envelope(payload,prefix=True),'single prefixed source cold/warm/search',use_browser)
    if smoke: return
    for depth in (2,4,8):
        valid_case(binary,payload,envelope(payload,depth),f'{depth} stringified layers')
    valid_case(binary,payload,{'type':'text','text':envelope(payload,2,prefix=True)},'text block candidate')
    valid_case(binary,payload,[{'type':'text','text':envelope(payload,prefix=True)}],'single array text candidate')
    for is_error in (False,True):
        valid_case(binary,payload,{'content':[{'type':'text','text':envelope(payload,prefix=True)}],
                   'isError':is_error},f'MCP single text envelope isError={is_error}',expected_error=is_error)
    valid_case(binary,payload,envelope(payload,2,escaped=True),'escaped inner base64 and outer JSON')
    valid_case(binary,payload,'\u2003'+envelope(payload)+'\u00a0','Unicode candidate trim')
    valid_case(binary,payload,'protocol line\n\u2003'+envelope(payload)+'\u2002\n','Unicode object-line candidate')
    mixed = stringify({'wall_time_seconds':.5,'exit_code':0,'output':{
        'wall_time_seconds':.25,'exit_code':0,'output':json.loads(envelope(payload))['output']}})
    valid_case(binary,payload,mixed,'mixed structured/stringified envelope')

    # Identical image bytes do not authorize a stale outer-source plan. The
    # unrendered outer tail changes; the physical file keeps size and mtime.
    with tempfile.TemporaryDirectory(prefix='sessiondock-nested-authority-') as temporary:
        corpus = corpus_for(Path(temporary),envelope(payload,count=2))
        path = corpus.paths['codex-nested']
        with isolated_server(corpus,binary) as (base,opener):
            route = '/api/messages/'+corpus.uid('codex-nested')+'?window=1'
            old = images(get(opener,base,route))
            assert len(old) == 2
            check_bytes(opener,base,old[0]['src'],payload)
            with path.open('ab') as stream: stream.write(encoded(codex_message('assistant','APPENDED')))
            check_bytes(opener,base,old[0]['src'],payload)
            before = path.read_bytes()
            stamp = path.stat()
            changed = before.replace(b'tail-old',b'tail-new')
            assert changed != before and len(changed) == len(before)
            path.write_bytes(changed)
            os.utime(path,ns=(stamp.st_atime_ns,stamp.st_mtime_ns))
            status(opener,base,old[0]['src'],409)
            status(opener,base,old[1]['src'],409)
            new = images(get(opener,base,route))
            assert len(new) == 2 and new[0]['src'] != old[0]['src']
            check_bytes(opener,base,new[0]['src'],payload)
            assert path.read_bytes() == changed
    print('PASS nested authority: append stability and cold/warm outer-tail rewrite revocation',flush=True)

    value = envelope(payload)
    bad = [
        ('duplicate inner keys',value.replace('"exit_code":0','"exit_code":0,"exit_code":0',1),{},501),
        ('ninth envelope',envelope(payload,9),{},501),
    ]
    for label, output, options, expected in bad:
        with tempfile.TemporaryDirectory(prefix='sessiondock-nested-reject-') as temporary:
            corpus = corpus_for(Path(temporary),output,**options)
            with isolated_server(corpus,binary) as (base,opener):
                status(opener,base,'/api/messages/'+corpus.uid('codex-nested')+'?window=1',expected)
        print('PASS nested rejection: '+label,flush=True)

    # Batch 34: a giant string that is not a reviewed tool envelope is ordinary
    # text, read back verbatim from the stamped record (64 MiB record budget);
    # it never becomes an image, a marker or an empty string.
    residual = stringify({'wall_time_seconds':0,'exit_code':0,
        'output':[{'type':'text','text':'x'*(MIB+MIB//4)},
                  {'type':'text','text':'y'*(MIB+MIB//4)}]})
    text = [
        ('unknown giant object', stringify({'tutorial':json.loads(value)}), {}),
        ('ordinary message',value,{'message':True}),
        ('tool arguments',value,{'argument':True}),
        ('ordinary residual body over the old 2 MiB budget', residual, {'envelope':True}),
    ]
    for label, output, options in text:
        envelope_text = options.pop('envelope', False)
        with tempfile.TemporaryDirectory(prefix='sessiondock-nested-text-') as temporary:
            corpus = corpus_for(Path(temporary),output,**options)
            with isolated_server(corpus,binary) as (base,opener):
                history = get(opener,base,'/api/messages/'+corpus.uid('codex-nested'))
                assert images(history) == [], label
                bodies = json.dumps(history['messages'], ensure_ascii=False)
                assert 'BEFORE NESTED' in bodies, label
                if options.get('message'):
                    assert any(message.get('text') == output for message in history['messages']), label
                elif options.get('argument'):
                    assert any('synthetic_image' in json.dumps(message, ensure_ascii=False) for message in history['messages']), label
                elif envelope_text:
                    # A recognised envelope is unwrapped like the small parser
                    # does; both giant text blocks survive intact.
                    texts = [message.get('text') or '' for message in history['messages'] if message.get('role') == 'tool_result']
                    assert any('x'*(MIB+MIB//4) in text and 'y'*(MIB+MIB//4) in text for text in texts), label
                else:
                    assert any(message.get('role') == 'tool_result' and message.get('text') == output for message in history['messages']), label
        print('PASS giant ordinary text: '+label,flush=True)

    # Batch 36 (WP-G): a multi-part `exec` result (`[header, chunk, chunk, …]`,
    # one stringified envelope per streamed chunk) shows every chunk's output
    # in part order; exit_code is the last chunk's, duration_s the sum. A giant
    # chunk is decoded in place as a span candidate under the shared budget
    # (its nested image keeps a decode plan), an image part next to the chunks
    # registers directly, a giant ordinary part is read back but — like the
    # header, an empty trailing part and the script's own prints — is not a
    # chunk and never enters the text.
    def chunk(cid, output, wall, **extra):
        return {'type':'input_text','text':stringify({'chunk_id':cid,'wall_time_seconds':wall,'original_token_count':1,'output':output,**extra})}
    direct = padded_png(3*MIB+7)
    multi = [{'type':'input_text','text':'Script completed\nWall time 1.5 seconds\nOutput:\n'},
             chunk('a','first chunk\n',0.25,exit_code=0),
             {'type':'input_text','text':'--- 2 ---'},
             {'type':'input_text','text':envelope(payload)},
             {'type':'input_text','text':'z'*(2*MIB+1)},
             chunk('c','third chunk\r\n',0.5,session_id=4242),
             chunk('d','',1.0,exit_code=3),
             image('codex',base64.b64encode(direct).decode()),
             {'type':'input_text','text':''}]
    with tempfile.TemporaryDirectory(prefix='sessiondock-nested-chunks-') as temporary:
        corpus = corpus_for(Path(temporary),multi)
        path = corpus.paths['codex-nested']
        original = path.read_bytes()
        with isolated_server(corpus,binary) as (base,opener):
            route = '/api/messages/'+corpus.uid('codex-nested')+'?window=1'
            history = get(opener,base,route)
            results = [row for row in history['messages'] if row.get('role') == 'tool_result']
            assert len(results) == 1, results
            result = results[0]
            assert result['text'] == 'first chunk\nNESTED TEXTthird chunk\r\n', result['text'][:200]
            assert result['exit_code'] == 3 and result['error'] is True and result['call_id'] == 'call', result
            assert abs(result['duration_s'] - 2.0) < 1e-9, result['duration_s']
            media = images(history)
            assert len(media) == 2 and all(item.get('lazy') is True for item in media), media
            public = stringify(history)
            assert 'data:image' not in public and 'base64' not in public and 'zzzz' not in public
            check_bytes(opener,base,media[0]['src'],payload)   # inside the giant chunk (decode plan)
            check_bytes(opener,base,media[1]['src'],direct)    # image part after the chunks
            check_bytes(opener,base,media[0]['src'],payload)
            warm = [row for row in get(opener,base,route)['messages'] if row.get('role') == 'tool_result']
            assert warm[0]['text'] == result['text'] and warm[0]['duration_s'] == result['duration_s']
        assert path.read_bytes() == original
    print('PASS multi-chunk output: every chunk shown, giant chunk decoded in place, both images served',flush=True)

    for size in (32*MIB,32*MIB+1):
        payload = padded_png(size)
        with tempfile.TemporaryDirectory(prefix='sessiondock-nested-boundary-') as temporary:
            corpus = corpus_for(Path(temporary),envelope(payload,prefix=True))
            with isolated_server(corpus,binary) as (base,opener):
                media = images(get(opener,base,'/api/messages/'+corpus.uid('codex-nested')+'?window=1'))
                assert len(media) == 1
                if size == 32*MIB: check_bytes(opener,base,media[0]['src'],payload)
                else: status(opener,base,media[0]['src'],413)
        print(f'PASS nested image boundary {size} bytes',flush=True)

    # A legal depth is not a fresh per-layer work allowance. Replaying this
    # two-layer maximum image exceeds the shared 512 MiB scan/decode budget.
    with tempfile.TemporaryDirectory(prefix='sessiondock-nested-work-limit-') as temporary:
        corpus = corpus_for(Path(temporary),envelope(padded_png(32*MIB),2))
        with isolated_server(corpus,binary) as (base,opener):
            status(opener,base,'/api/messages/'+corpus.uid('codex-nested')+'?window=1',413)
    print('PASS nested shared-work limit: depth does not replenish replay budget',flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--browser',action='store_true')
    parser.add_argument('--smoke',action='store_true',help='only the first complete-source scenario; not full acceptance')
    args = parser.parse_args()
    run(args.binary,args.browser,args.smoke)
