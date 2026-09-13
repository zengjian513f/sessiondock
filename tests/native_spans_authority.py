#!/usr/bin/env python3
"""Real HTTP native-span authority checks using only private synthetic roots.

Exercises inherited fixed cuts and inventory-owned Claude agents with actual
3 MiB PNGs. Never starts a model CLI, reads native homes or configures file roots.
"""
from __future__ import annotations

import argparse
import base64
import json
from pathlib import Path
import re
import struct
import tempfile
from urllib.parse import urlencode
import zlib

from history_parity import (
    BINARY, Corpus, claude_row, codex_message, codex_row, cursor_query,
    encoded, isolated_server,
)
from media_browser import image
from native_spans import MIB, check_bytes, get, images, padded_png, status
from native_streaming import OwnedFiles


def alternate_png(original):
    """Change ancillary bytes past 4 KiB and recalculate their CRC; keep size."""
    result = bytearray(original)
    position = 8
    while position < len(result):
        size = struct.unpack_from('>I', result, position)[0]
        if result[position + 4:position + 8] == b'npAD':
            result[position + 8 + size // 2] ^= 1
            checksum = zlib.crc32(result[position + 4:position + 8 + size])
            struct.pack_into('>I', result, position + 8 + size, checksum)
            changed = bytes(result)
            assert changed != original and len(changed) == len(original)
            assert changed[:4096] == original[:4096]
            return changed
        position += size + 12
    raise AssertionError('padded_png ancillary fixture chunk missing')


def build(root, first, second):
    corpus = Corpus(root)
    for source in ('claude', 'codex', 'grok'):
        (root / source).mkdir()
    payload = base64.b64encode(first).decode()

    def meta(name, **extra):
        return codex_row('session_meta', {
            'id': name, 'session_id': name, 'cwd': '/synthetic/span-authority', **extra,
        })

    parent = 'codex-span-parent'
    prefix = [meta(parent)]
    for ordinal in range(2):
        prefix.append(codex_row('response_item', {
            'type': 'message', 'role': 'user', 'content': [
                {'type': 'input_text', 'text': f'INHERITED IMAGE {ordinal}'},
                image('codex', payload),
            ],
        }, ordinal + 1))
    cutoff = sum(len(encoded(row)) for row in prefix)
    corpus.put(parent, 'codex', prefix + [codex_message('assistant', 'OUTSIDE FIXED CUT')], [])
    leaves = ('codex-span-leaf-a', 'codex-span-leaf-b')
    for name in leaves:
        corpus.put(name, 'codex', [meta(name, forked_from_id=parent,
            history_mode='paginated', history_base={'thread_id': parent, 'end_byte_offset': cutoff}),
            codex_message('assistant', name + ' LEAF ONLY')], [])

    agents = []
    for suffix, data in [('a', first), ('b', second)]:
        owner = 'claude-span-owner-' + suffix
        agent = 'claude-span-worker-' + suffix
        owner_path = corpus.put(owner, 'claude', [
            claude_row(owner, 'user', 'main-u', None, owner + ' MAIN ONLY'),
            claude_row(owner, 'assistant', 'main-a', 'main-u', 'MAIN DONE'),
        ], [])
        path = owner_path.with_suffix('') / 'subagents' / f'agent-{agent}.jsonl'
        path.parent.mkdir(parents=True)
        path.write_bytes(b''.join(encoded(row) for row in [
            claude_row(owner, 'user', 'agent-u', None, [
                {'type': 'text', 'text': agent + ' EXACT AGENT'},
                image('claude', base64.b64encode(data).decode()),
            ], isSidechain=True, agentId=agent),
            claude_row(owner, 'assistant', 'agent-a', 'agent-u', 'AGENT DONE',
                       isSidechain=True, agentId=agent),
        ]))
        corpus.paths[agent] = path
        corpus.owners[agent] = owner
        agents.append((owner, agent, data))
    return corpus, parent, leaves, cutoff, agents


def run(binary):
    original = padded_png(3 * MIB)
    replacement = alternate_png(original)
    old_encoded = base64.b64encode(original)
    new_encoded = base64.b64encode(replacement)
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-authority-') as temporary:
        corpus, parent, leaves, cutoff, agents = build(Path(temporary), original, replacement)
        owned = OwnedFiles(corpus.root)
        with isolated_server(corpus, binary) as (base, opener):
            def read(name, agent='', previous=None):
                query = (cursor_query(previous, agent=agent, window=1) if previous is not None
                         else urlencode({'agent': agent, 'window': 1}))
                try:
                    history = get(opener, base, '/api/messages/' + corpus.uid(name) + '?' + query)
                    public = json.dumps(history)
                    assert 'data:image' not in public and 'base64' not in public
                    for item in images(history):
                        # Existing meta.path is public legacy inventory metadata;
                        # the new image capability must not contain source paths.
                        assert str(corpus.root) not in json.dumps(item)
                        assert not {'path', 'root', 'data', 'encoded'} & item.keys()
                        assert item.get('lazy') is True and re.fullmatch(r'/api/media/[0-9a-f]{32}', item['src'])
                    return history
                finally:
                    owned.unchanged()

            def blob(token, expected):
                try:
                    check_bytes(opener, base, token, expected)
                finally:
                    owned.unchanged()

            def reject(route, expected):
                try:
                    status(opener, base, route, expected)
                finally:
                    owned.unchanged()

            histories = {name: read(name) for name in (parent, *leaves)}
            tokens = {name: [item['src'] for item in images(history)]
                      for name, history in histories.items()}
            assert all(len(items) == 2 for items in tokens.values())
            assert len(set(sum(tokens.values(), []))) == 6, 'inherited images reused another view scope token'
            for name in (parent, *leaves):
                assert [item['src'] for item in images(read(name))] == tokens[name]
                blob(tokens[name][0], original)
                blob(tokens[name][0], original)  # Warm cache still checks current authority.
            for name in leaves:
                assert histories[name]['end'] == len(owned.expected[corpus.paths[name]]) < cutoff
                assert 'OUTSIDE FIXED CUT' not in json.dumps(histories[name]['messages'])

            # The parent full view is now invalid, but both inherited fixed-cut
            # views and their already-issued tokens remain valid authorities.
            path = corpus.paths[parent]
            owned.append(path, encoded(codex_row('response_item', {'type': 'message', 'role': 'user', 'content': 42})))
            reject('/api/messages/' + corpus.uid(parent), 501)
            for name in leaves:
                delta = read(name, previous=histories[name])
                assert delta['reset'] is False and delta['messages'] == []
                assert delta['end'] == histories[name]['end']
                blob(tokens[name][0], original)
            blob(tokens[leaves[0]][1], original)  # First GET after the external bad tail.
            # leaves[1][1] deliberately remains cold until revoked below.

            before = owned.expected[path]
            assert before.count(old_encoded) == 2
            changed = before.replace(old_encoded, new_encoded)
            assert len(changed) == len(before) and changed[:4096] == before[:4096]
            owned.write(path, changed, restore_mtime=True)
            for name in leaves:
                for token in tokens[name]:
                    reject(token, 409)  # Includes the never-materialized leaf B image.
                fresh = read(name, previous=histories[name])
                assert fresh['reset'] is True and fresh['end'] == histories[name]['end']
                new_tokens = [item['src'] for item in images(fresh)]
                assert len(new_tokens) == 2 and not set(new_tokens) & set(tokens[name])
                for token in new_tokens:
                    blob(token, replacement)
            print('PASS native span fixed cuts: distinct parent/leaf scopes, external unreadable tail, cold/warm revocation after same-size restored-mtime prefix rewrite, leaf-only cursor')

            listed = get(opener, base, '/api/sessions?force=1')['sessions']
            public = {row['sid']: row for row in listed}
            agent_tokens = {}
            for owner, agent, data in agents:
                assert agent not in public, 'physical agent became a top-level session'
                assert {item['id'] for item in public[owner]['agent_items']} == {agent}
                assert images(read(owner)) == [], 'main view borrowed subagent media'
                history = read(owner, agent)
                pictures = images(history)
                assert len(pictures) == 1 and agent + ' EXACT AGENT' in json.dumps(history['messages'])
                agent_tokens[agent] = pictures[0]['src']
                blob(pictures[0]['src'], data)
                blob(pictures[0]['src'], data)
                reject('/api/messages/' + corpus.uid(agent), 404)
                other_owner = next(item[0] for item in agents if item[0] != owner)
                reject('/api/messages/' + corpus.uid(other_owner) + '?' + urlencode({'agent': agent}), 404)
                for forged in (str(corpus.paths[agent]), '../' + agent, agent + '.jsonl'):
                    reject('/api/messages/' + corpus.uid(owner) + '?' + urlencode({'agent': forged}), 404)
            assert len(set(agent_tokens.values())) == 2

            owner, agent, _ = agents[0]
            path = corpus.paths[agent]
            before = owned.expected[path]
            assert before.count(old_encoded) == 1
            changed = before.replace(old_encoded, new_encoded)
            assert changed[:4096] == before[:4096] and len(changed) == len(before)
            owned.write(path, changed, restore_mtime=True)
            reject(agent_tokens[agent], 409)
            fresh = images(read(owner, agent))
            assert len(fresh) == 1 and fresh[0]['src'] != agent_tokens[agent]
            blob(fresh[0]['src'], replacement)
            other_owner, other_agent, other_data = agents[1]
            blob(agent_tokens[other_agent], other_data)
            assert images(read(other_owner)) == []
            print('PASS native span Claude agents: inventory-owned exact selection, hidden physical UID, cross-owner/path rejection, isolated warm token revocation and replacement bytes')
        owned.unchanged()
    print('PASS native authority: all HTTP checks preserve native fixture bytes and file inventory; no external roots or model calls')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    run(parser.parse_args().binary)
