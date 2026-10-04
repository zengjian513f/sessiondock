"""Intercept only the real renderer's batch size in a private browser fixture.

The fixture locates the real returned renderSession function and its cancellable loop by
syntax, including renamed/minified identifiers and ES split chunks. It changes
only the existing 250 literals (or legacy constant) to 10; no product batching options exist.
"""
from pathlib import Path
import json
import re
import subprocess

from frontend_paths import entry_asset, frontend_dir, frontend_html


def install_small_render_batches(page, context):
    directory = frontend_dir()
    repository = Path(__file__).resolve().parents[1]
    matches = []
    inspector = Path(__file__).with_suffix('.cjs')
    if not frontend_html().modules:
        asset = entry_asset(directory)
        source = asset.read_text()
        literals = list(re.finditer(r'const RENDER_BATCH = (250);', source))
        assert len(literals) == 1, 'expected one legacy renderer batch constant'
        literal = literals[0]
        positions = [len(source[:position].encode('utf-16-le')) // 2
                     for position in literal.span(1)]
        matches.append((asset, source, {'name': 'renderSession', 'literals': [positions]}))
    for candidate in directory.rglob('*.js'):
        source = candidate.read_text()
        if 'renderSession' not in source or '250' not in source:
            continue
        result = subprocess.run(['node', str(inspector), str(repository), str(candidate)],
                                check=True, capture_output=True, text=True, timeout=20)
        for site in json.loads(result.stdout):
            matches.append((candidate, source, site))
    assert len(matches) == 1, f'expected one actual renderer batch loop, found {len(matches)}'
    asset, source, site = matches[0]
    renderer_name = site['name']
    # Babel positions count UTF-16 code units, including astral text in
    # the bundle; Python string positions count Unicode code points.
    encoded = source.encode('utf-16-le')
    for start, end in sorted(site['literals'], reverse=True):
        start *= 2
        end *= 2
        assert encoded[start:end].decode('utf-16-le') == '250'
        encoded = encoded[:start] + '10'.encode('utf-16-le') + encoded[end:]
    patched = encoded.decode('utf-16-le')
    # The stack predicate keeps the real Promise/setTimeout yield. Compiled
    # function names come from the actual asset, rather than a source-name guess.
    context.add_init_script('window.__raceRendererName = ' + json.dumps(renderer_name) + ';')
    pattern = '**/' + asset.relative_to(directory).as_posix() + '*'

    def intercept(route):
        response = route.fetch()
        assert response.text() == source, 'served renderer differs from inspected artifact'
        route.fulfill(response=response, body=patched)

    page.route(pattern, intercept)
