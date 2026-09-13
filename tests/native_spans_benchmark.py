#!/usr/bin/env python3
"""Linux-only synthetic native-span HTTP timing and exact-child RSS evidence.

Each size/sample uses one fresh server and a valid ancillary-padded Claude PNG.
Fixture generation is Python work, not server RSS. HTTP timings exclude fixture
generation, process startup and /proc sampling. GET timings include complete
stream consumption and SHA256 verification, without decoding the image in Python.
Fresh processes do not imply a cold OS page cache. No performance thresholds,
native homes, external file roots, model calls or CLI sessions are involved.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sys
import tempfile
import time

from append_benchmark import DEFAULT_BINARY, fetch, server, server_memory
from history_parity import Corpus, claude_row
from media_browser import image
from native_spans import MIB, images, padded_png

PHASES = ('first_window', 'cold_get', 'warm_get')
MEMORY_PHASES = ('startup', *PHASES)


def file_sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def fixture(root, size):
    corpus = Corpus(root)
    for source in ('claude', 'codex', 'grok'):
        (root / source).mkdir()
    payload = padded_png(size)
    expected_digest = hashlib.sha256(payload).hexdigest()
    sid = 'claude-native-span-benchmark'
    path = corpus.put(sid, 'claude', [claude_row(sid, 'user', 'image-u', None,
        [image('claude', base64.b64encode(payload).decode())])], [])
    # Only small metadata escapes this function. Large Python fixture buffers
    # are released before the server is spawned and never counted as its RSS.
    return path, corpus.uid(sid), expected_digest


def fetch_image(opener, base, route, expected_size, expected_digest):
    assert re.fullmatch(r'/api/media/[0-9a-f]{32}', route)
    started = time.perf_counter()
    digest = hashlib.sha256()
    count = 0
    with opener.open(base + route, timeout=90) as response:
        assert response.status == 200
        assert response.headers['Content-Type'] == 'image/png'
        assert int(response.headers['Content-Length']) == expected_size
        assert response.headers['Cache-Control'] == 'private, no-store'
        assert response.headers['X-Content-Type-Options'] == 'nosniff'
        while chunk := response.read(128 * 1024):
            count += len(chunk)
            assert count <= expected_size, 'media response exceeded fixture size'
            digest.update(chunk)
    assert count == expected_size and digest.hexdigest() == expected_digest
    return {'ms': round((time.perf_counter() - started) * 1000, 3),
            'http_status': 200, 'http_bytes': count, 'sha256_verified': True}


def sample(binary, size_mib, index):
    size = size_mib * MIB
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-span-bench-') as temporary:
        root = Path(temporary).resolve()
        path, uid, digest = fixture(root, size)
        native_digest = file_sha256(path)
        native_size = path.stat().st_size
        file_set = {entry for entry in root.rglob('*') if entry.is_file()}
        with server(root, binary) as (base, opener, process):
            memory = {'startup': server_memory(process)}
            history, initial = fetch(opener, base, '/api/messages/' + uid + '?window=1')
            memory['first_window'] = server_memory(process)
            assert history['meta']['supported'] is True and history['reset'] is True
            assert history['end'] == native_size
            assert len(history['messages']) == 1
            media = images(history)
            assert len(media) == 1 and media[0].get('lazy') is True
            assert 'data:image' not in json.dumps(history) and 'base64' not in json.dumps(history)
            initial.update({'messages': len(history['messages']), 'images': len(media),
                            'end': history['end']})
            cold = fetch_image(opener, base, media[0]['src'], size, digest)
            memory['cold_get'] = server_memory(process)
            warm = fetch_image(opener, base, media[0]['src'], size, digest)
            memory['warm_get'] = server_memory(process)
            assert process.poll() is None, 'exact benchmark child unexpectedly exited'
            assert file_sha256(path) == native_digest and path.stat().st_size == native_size
            assert {entry for entry in root.rglob('*') if entry.is_file()} == file_set
            result = {'kind': 'sample', 'source': 'claude', 'image_mib': size_mib,
                      'sample': index, 'server_pid': process.pid, 'native_bytes': native_size,
                      'image_bytes': size, 'image_sha256': digest,
                      'first_window': initial, 'cold_get': cold, 'warm_get': warm,
                      'memory_bytes': memory, 'native_unchanged': True}
        assert file_sha256(path) == native_digest
        print(json.dumps(result), flush=True)
        return result


def percentile(values, fraction):
    return sorted(values)[math.ceil(len(values) * fraction) - 1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=DEFAULT_BINARY)
    parser.add_argument('--samples', type=int, default=3)
    parser.add_argument('--sizes-mib', '--sizes', dest='sizes', type=int, nargs='+', default=[3, 32],
                        help='decoded PNG sizes in MiB (default: 3 32)')
    args = parser.parse_args()
    if not sys.platform.startswith('linux') or not Path('/proc/self/status').is_file():
        parser.error('requires Linux procfs; no inferred cross-platform memory measurements')
    if not 1 <= args.samples <= 20 or not 1 <= len(args.sizes) <= 16 or any(not 1 <= size <= 32 for size in args.sizes):
        parser.error('samples must be 1..20; specify 1..16 sizes, each 1..32 MiB')
    binary = args.binary.resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error('--binary must name an existing executable server binary')
    binary_digest = file_sha256(binary)
    print(json.dumps({'kind': 'benchmark', 'binary_sha256': binary_digest,
        'binary_bytes': binary.stat().st_size, 'samples': args.samples, 'image_sizes_mib': args.sizes,
        'platform': sys.platform, 'window_timing': 'loopback HTTP and JSON decode',
        'get_timing': 'loopback HTTP, full streamed body consumption and SHA256 verification',
        'excluded': 'fixture construction, server startup, procfs memory sampling',
        'memory': 'exact live server child VmRSS/VmHWM bytes; excludes Python fixture process',
        'hwm': 'cumulative process high-water mark, not a per-phase allocation or peak delta',
        'cache': 'fresh server per size/sample; OS page cache is not flushed',
        'assertions': 'correctness only; no timing or RSS pass/fail thresholds'}), flush=True)
    for size in args.sizes:
        rows = [sample(binary, size, index + 1) for index in range(args.samples)]
        summary = {'kind': 'summary', 'source': 'claude', 'image_mib': size, 'samples': len(rows),
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
