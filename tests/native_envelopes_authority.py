#!/usr/bin/env python3
"""Actual HTTP authority checks for inherited and agent-owned tool envelopes.

Uses only synthetic 3 MiB PNGs and private native roots. Non-image outer JSON
tail rewrites must revoke old capabilities even when image bytes are identical.
No model CLI, native home, external file roots or production service is used.
"""
from __future__ import annotations

import argparse
import base64
import json
from pathlib import Path
import re
import tempfile
from urllib.parse import urlencode

from history_parity import BINARY, codex_message, codex_row, cursor_query, encoded, isolated_server
from native_envelopes import corpus_for, envelope
from native_spans import MIB, check_bytes, get, images, padded_png, status
from native_spans_authority import OwnedFiles, alternate_png


def meta(name, **extra):
    return codex_row('session_meta', {'id': name, 'session_id': name,
        'cwd': '/synthetic/envelope-authority', **extra})


def build(root, payload):
    parent = 'codex-envelope-parent'
    corpus = corpus_for(root, envelope(payload, count=2), parent)
    cutoff = corpus.paths[parent].stat().st_size
    with corpus.paths[parent].open('ab') as stream:
        stream.write(encoded(codex_message('assistant', 'PARENT OUTSIDE FIXED CUT')))
    leaves = ('codex-envelope-leaf-a', 'codex-envelope-leaf-b')
    for name in leaves:
        corpus.put(name, 'codex', [meta(name, forked_from_id=parent,
            history_mode='paginated', history_base={'thread_id': parent, 'end_byte_offset': cutoff}),
            codex_message('assistant', name + ' LEAF ONLY')], [])
    agents = []
    for suffix, data in [('a', payload), ('b', alternate_png(payload))]:
        owner = 'codex-envelope-owner-' + suffix
        agent = 'codex-envelope-agent-' + suffix
        corpus.put(owner, 'codex', [meta(owner), codex_message('user', owner + ' MAIN ONLY')], [])
        output = envelope(data, count=2).replace('NESTED TEXT', agent + ' NESTED TEXT')
        corpus.put(agent, 'codex', [meta(agent, session_id=owner, thread_source='subagent',
            parent_thread_id=owner, forked_from_id=owner,
            source={'subagent': {'thread_spawn': {'parent_thread_id': owner,
                                                 'agent_path': '/root/' + agent}}}),
            codex_message('user', agent + ' EXACT AGENT'),
            codex_row('response_item', {'type': 'function_call', 'name': 'synthetic_image',
                                       'call_id': 'call', 'arguments': '{}'}),
            codex_row('response_item', {'type': 'function_call_output', 'call_id': 'call',
                                       'output': output})], [], parent=owner)
        agents.append((owner, agent, data))
    return corpus, parent, leaves, cutoff, agents


def rewrite_outer_tail(owned, path, payload):
    before = owned.expected[path]
    encoded_image = base64.b64encode(payload)
    assert before.count(encoded_image) == 2
    assert before.count(b'tail-old') == 1
    marker = before.index(b'tail-old')
    assert marker > 4096
    # Both image strings precede the changed non-image field. No image byte,
    # inner image metadata, record length or native mtime is changed by this test.
    assert before.rfind(encoded_image) + len(encoded_image) <= marker
    changed = before[:marker] + b'tail-new' + before[marker + len(b'tail-old'):]
    assert len(changed) == len(before) and changed[:4096] == before[:4096]
    assert changed[:marker] == before[:marker] and changed.count(encoded_image) == 2
    assert changed[marker + 8:] == before[marker + 8:]
    owned.write(path, changed, restore_mtime=True)


def run(binary):
    payload = padded_png(3 * MIB)
    with tempfile.TemporaryDirectory(prefix='sessiondock-envelope-authority-') as temporary:
        corpus, parent, leaves, cutoff, agents = build(Path(temporary), payload)
        owned = OwnedFiles(corpus.root)
        with isolated_server(corpus, binary) as (base, opener):
            def read(name, agent='', previous=None):
                query = (cursor_query(previous, agent=agent, window=1) if previous is not None
                         else urlencode({'agent': agent, 'window': 1}))
                try:
                    history = get(opener, base, '/api/messages/' + corpus.uid(name) + '?' + query)
                    public = json.dumps(history)
                    assert 'data:image' not in public and 'base64' not in public
                    assert 'tail-old' not in public and 'tail-new' not in public
                    for item in images(history):
                        assert item.get('lazy') is True and re.fullmatch(r'/api/media/[0-9a-f]{32}', item['src'])
                        assert not {'plan', 'path', 'root', 'data', 'encoded'} & item.keys()
                    return history
                finally:
                    owned.unchanged()

            def blob(token, data):
                try:
                    check_bytes(opener, base, token, data)
                finally:
                    owned.unchanged()

            def reject(route, expected):
                try:
                    status(opener, base, route, expected)
                finally:
                    owned.unchanged()

            old = {name: read(name) for name in (parent, *leaves)}
            tokens = {name: [item['src'] for item in images(history)] for name, history in old.items()}
            assert all(len(items) == 2 for items in tokens.values())
            assert len(set(sum(tokens.values(), []))) == 6, 'inherited image capability crossed view scopes'
            for name in (parent, *leaves):
                assert 'NESTED TEXT' in json.dumps(old[name]['messages'])
                assert [item['src'] for item in images(read(name))] == tokens[name]
                blob(tokens[name][0], payload)
                blob(tokens[name][0], payload)
            for name in leaves:
                assert old[name]['end'] == len(owned.expected[corpus.paths[name]]) < cutoff
                assert 'PARENT OUTSIDE FIXED CUT' not in json.dumps(old[name]['messages'])

            owned.append(corpus.paths[parent], encoded(codex_row('response_item', {'type': 'message', 'role': 'user', 'content': 42})))
            reject('/api/messages/' + corpus.uid(parent), 501)
            for name in leaves:
                idle = read(name, previous=old[name])
                assert idle['reset'] is False and idle['messages'] == []
                assert idle['end'] == old[name]['end']
                blob(tokens[name][0], payload)
            blob(tokens[leaves[0]][1], payload)  # Cold GET remains valid after a bad cut-external tail.
            # leaf B's second token deliberately stays cold until revocation.
            rewrite_outer_tail(owned, corpus.paths[parent], payload)
            for name in leaves:
                for token in tokens[name]:
                    reject(token, 409)
                fresh = read(name)
                assert fresh['end'] == old[name]['end']
                new_tokens = [item['src'] for item in images(fresh)]
                assert len(new_tokens) == 2 and not set(new_tokens) & set(tokens[name])
                for token in new_tokens:
                    blob(token, payload)  # Exactly the original image bytes, not a replacement PNG.
            print('PASS envelope fixed-cut authority: distinct scopes, cut-external unreadable append, unchanged-image outer-tail rewrite revokes cold/warm tokens, leaf-only end', flush=True)

            inventory = get(opener, base, '/api/sessions?force=1')['sessions']
            owned.unchanged()
            public = {row['sid']: row for row in inventory}
            agent_tokens = {}
            for owner, agent, data in agents:
                assert agent not in public
                assert {item['id'] for item in public[owner]['agent_items']} == {agent}
                assert images(read(owner)) == []
                history = read(owner, agent)
                assert agent + ' EXACT AGENT' in json.dumps(history['messages'])
                selected = [item['src'] for item in images(history)]
                assert len(selected) == 2
                agent_tokens[agent] = selected
                blob(selected[0], data)
                blob(selected[0], data)
                reject('/api/messages/' + corpus.uid(agent), 404)
                other_owner = next(item[0] for item in agents if item[0] != owner)
                for wrong_owner in (other_owner, leaves[0]):
                    reject('/api/messages/' + corpus.uid(wrong_owner) + '?' + urlencode({'agent': agent}), 404)
                for forged in (str(corpus.paths[agent]), '../' + agent, agent + '.jsonl'):
                    reject('/api/messages/' + corpus.uid(owner) + '?' + urlencode({'agent': forged}), 404)
            assert len(set(sum(agent_tokens.values(), []))) == 4
            assert not set(sum(agent_tokens.values(), [])) & set(sum(tokens.values(), []))

            owner, agent, data = agents[0]
            rewrite_outer_tail(owned, corpus.paths[agent], data)
            for token in agent_tokens[agent]:
                reject(token, 409)  # Warm first and never-materialized second image.
            fresh = [item['src'] for item in images(read(owner, agent))]
            assert len(fresh) == 2 and not set(fresh) & set(agent_tokens[agent])
            for token in fresh:
                blob(token, data)
            other_owner, other_agent, other_data = agents[1]
            for token in agent_tokens[other_agent]:
                blob(token, other_data)
            assert images(read(other_owner)) == []
            print('PASS envelope Codex agent authority: exact inventory owner/selector, hidden physical UID and path rejection, cold/warm outer-plan revocation stays isolated', flush=True)
        owned.unchanged()
    print('PASS envelope authority: real HTTP, exact PNG bytes and unchanged native fixture inventory; no external roots or model calls', flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    run(parser.parse_args().binary)
