"""Intercept only the real renderer's batch size in a private browser fixture.

The fixture rewrites the served entry's single `RENDER_BATCH` constant from 250
to 10; no product batching options exist.
"""
import json
import re

from frontend_paths import entry_asset, frontend_dir


def install_small_render_batches(page, context):
    directory = frontend_dir()
    asset = entry_asset(directory)
    source = asset.read_text()
    literals = list(re.finditer(r'const RENDER_BATCH = (250);', source))
    assert len(literals) == 1, 'expected one renderer batch constant'
    start, end = literals[0].span(1)
    patched = source[:start] + '10' + source[end:]
    # The stack predicate keeps the real Promise/setTimeout yield.
    context.add_init_script('window.__raceRendererName = ' + json.dumps('renderSession') + ';')
    pattern = '**/' + asset.relative_to(directory).as_posix() + '*'

    def intercept(route):
        response = route.fetch()
        assert response.text() == source, 'served renderer differs from inspected artifact'
        route.fulfill(response=response, body=patched)

    page.route(pattern, intercept)
