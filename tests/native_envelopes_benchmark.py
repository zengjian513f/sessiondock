#!/usr/bin/env python3
"""Linux fresh-process benchmark for synthetic stringified Codex tool images.

Defaults to one envelope layer, 3/32 MiB PNGs, three fresh servers per size.
For nested replay use --sizes-mib 3 --depth 8. This is not a budget-rejection
benchmark: HTTP 413 or any other failed response aborts, never becomes a timing
success. Shared replay work remains bounded at 512 MiB per operation.
Python fixture generation is outside timings and is not server RSS. GET timings
include complete HTTP body consumption and SHA256, not Python image decoding.
No CLI/model, native home, production service or external file roots are used.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import tempfile

from append_benchmark import DEFAULT_BINARY, fetch, server, server_memory
from native_envelopes import corpus_for, envelope
from native_spans import MIB, images, padded_png
from native_spans_benchmark import fetch_image, file_sha256, percentile

PHASES = ('first_window', 'cold_get', 'warm_get')
MEMORY_PHASES = ('startup', *PHASES)
SHARED_REPLAY_WORK_BYTES = 512 * MIB


def fixture(root, size, depth):
    payload = padded_png(size)
    digest = hashlib.sha256(payload).hexdigest()
    name = 'codex-envelope-benchmark'
    output = envelope(payload, depth)
    corpus = corpus_for(root, output, name)
    path = corpus.paths[name]
    with path.open('rb') as source:
        for _ in range(3):  # Session metadata, prompt and small tool-call record.
            prefix_record = source.readline(4097)
            assert len(prefix_record) <= 4096 and prefix_record.endswith(b'\n')
        tool_record_bytes = path.stat().st_size - source.tell()
    # No large body survives this function. Streaming later HTTP bytes into
    # SHA256 does not allocate another decoded image in the Python parent.
    return path, corpus.uid(name), digest, tool_record_bytes


def checked_token(history, native_size):
    assert history['meta']['supported'] is True and history['reset'] is True
    assert history['end'] == native_size
    assert any('NESTED TEXT' in row.get('text', '') for row in history['messages'])
    media = images(history)
    assert len(media) == 1 and media[0].get('lazy') is True
    assert re.fullmatch(r'/api/media/[0-9a-f]{32}', media[0]['src'])
    assert not {'data', 'encoded', 'plan', 'root', 'path'} & media[0].keys()
    public = json.dumps(history)
    assert 'data:image' not in public and 'base64' not in public
    return media[0]['src']


def sample(binary, size_mib, depth, index):
    size = size_mib * MIB
    with tempfile.TemporaryDirectory(prefix='sessiondock-envelope-bench-') as temporary:
        root = Path(temporary).resolve()
        path, uid, digest, tool_record_bytes = fixture(root, size, depth)
        native_size = path.stat().st_size
        native_digest = file_sha256(path)
        file_set = {entry for entry in root.rglob('*') if entry.is_file()}
        with server(root, binary) as (base, opener, process):
            memory = {'startup': server_memory(process)}
            route = '/api/messages/' + uid + '?window=1'
            history, initial = fetch(opener, base, route)
            memory['first_window'] = server_memory(process)
            token = checked_token(history, native_size)
            initial.update({'messages': len(history['messages']), 'images': 1, 'end': history['end']})
            cold = fetch_image(opener, base, token, size, digest)
            memory['cold_get'] = server_memory(process)
            warm = fetch_image(opener, base, token, size, digest)
            memory['warm_get'] = server_memory(process)
            # These correctness-only observations happen after all reported
            # phase, so neither a second window nor /proc IO changes its timing.
            current, _ = fetch(opener, base, route)
            assert checked_token(current, native_size) == token, 'unchanged source lost its token'
            assert current['end'] == history['end'] and current['anchor'] == history['anchor']
            assert process.poll() is None, 'exact benchmark child unexpectedly exited'
            assert path.stat().st_size == native_size and file_sha256(path) == native_digest
            assert {entry for entry in root.rglob('*') if entry.is_file()} == file_set
            result = {'kind': 'sample', 'source': 'codex', 'image_mib': size_mib,
                'envelope_depth': depth, 'sample': index, 'server_pid': process.pid,
                'native_bytes': native_size, 'tool_record_bytes': tool_record_bytes,
                'image_bytes': size, 'image_sha256': digest,
                'shared_replay_work_bytes': SHARED_REPLAY_WORK_BYTES,
                'first_window': initial, 'cold_get': cold, 'warm_get': warm,
                'memory_bytes': memory, 'token_stable': True, 'native_unchanged': True}
        assert file_sha256(path) == native_digest
        print(json.dumps(result), flush=True)
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=DEFAULT_BINARY)
    parser.add_argument('--samples', type=int, default=3)
    parser.add_argument('--depth', type=int, default=1, help='stringified envelope layers, 1..8')
    parser.add_argument('--sizes-mib', '--sizes', dest='sizes', type=int, nargs='+', default=[3, 32],
                        help='decoded PNG sizes in MiB (default: 3 32)')
    args = parser.parse_args()
    if not sys.platform.startswith('linux') or not Path('/proc/self/status').is_file():
        parser.error('requires Linux procfs; no inferred cross-platform memory measurements')
    if not 1 <= args.samples <= 20 or not 1 <= args.depth <= 8:
        parser.error('samples must be 1..20 and depth must be 1..8')
    if not 1 <= len(args.sizes) <= 16 or any(not 1 <= size <= 32 for size in args.sizes):
        parser.error('specify 1..16 sizes, each 1..32 MiB')
    if args.depth > 1 and 32 in args.sizes:
        parser.error('32 MiB nested-depth budget rejection is not a performance success; use --sizes-mib 3 for nested replay')
    binary = args.binary.resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error('--binary must name an existing executable server binary')
    binary_digest = file_sha256(binary)
    print(json.dumps({'kind': 'benchmark', 'binary_sha256': binary_digest,
        'binary_bytes': binary.stat().st_size, 'source': 'codex', 'samples': args.samples,
        'image_sizes_mib': args.sizes, 'envelope_depth': args.depth,
        'shared_replay_work_bytes': SHARED_REPLAY_WORK_BYTES, 'platform': sys.platform,
        'window_timing': 'loopback HTTP and JSON decode',
        'get_timing': 'loopback HTTP, complete streamed body consumption and SHA256 verification',
        'excluded': 'fixture generation, process startup, procfs sampling, post-phase token/native checks',
        'memory': 'exact live server child VmRSS/VmHWM bytes; excludes Python fixture process',
        'hwm': 'cumulative process high-water mark, not per-phase allocation or peak delta',
        'cache': 'fresh server per size/sample; OS page cache is not flushed',
        'assertions': 'correctness only; non-200 responses fail; no timing/RSS thresholds'}), flush=True)
    for size in args.sizes:
        rows = [sample(binary, size, args.depth, index + 1) for index in range(args.samples)]
        summary = {'kind': 'summary', 'source': 'codex', 'image_mib': size,
            'envelope_depth': args.depth, 'samples': len(rows),
            'measurements': {phase: {
                'p50_ms': percentile([row[phase]['ms'] for row in rows], .5),
                'p95_ms': percentile([row[phase]['ms'] for row in rows], .95),
            } for phase in PHASES},
            'memory_p50_bytes': {phase: {
                field: percentile([row['memory_bytes'][phase][field] for row in rows], .5)
                for field in ('VmRSS', 'VmHWM')
            } for phase in MEMORY_PHASES}}
        print(json.dumps(summary), flush=True)
    assert file_sha256(binary) == binary_digest, 'server binary changed during benchmark'


if __name__ == '__main__':
    main()
