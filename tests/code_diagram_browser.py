#!/usr/bin/env python3
"""Fenced diagrams retain terminal columns despite a CJK browser fixed font."""
import argparse
import os
import tempfile
from pathlib import Path

from playwright.sync_api import sync_playwright
from history_parity import BINARY, build_corpus, claude_row, isolated_server

DIAGRAM = ('  y\n'
           '  │    ┌─┐\n'
           '  │  ┌─┤ ├─┐\n'
           '  │┌─┤ │ │ ├─┐\n'
           '  └┴─┴─┴─┴─┴─┴── x\n'
           '   竖着切 x 轴')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-code-diagram-') as temporary:
        corpus = build_corpus(Path(temporary))
        corpus.put('claude-diagram', 'claude', [
            claude_row('claude-diagram', 'user', 'u', None, 'Show a diagram'),
            claude_row('claude-diagram', 'assistant', 'a', 'u',
                       'Diagram:\n```\n' + DIAGRAM + '\n```\n\n'
                       '```text\n' + DIAGRAM + '\n```')], [])
        with isolated_server(corpus, args.binary) as (base, _), sync_playwright() as p:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = p.chromium.launch(**launch)
            try:
                page = browser.new_page(viewport={'width': 1280, 'height': 900})
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                # A real CJK fixed font has half-width ASCII but full-width box
                # drawing. Do not depend on Chromium's Linux default matching
                # the affected user's browser/OS font settings.
                page.context.new_cdp_session(page).send('Page.setFontFamilies', {
                    'fontFamilies': {'fixed': 'Noto Sans Mono CJK SC'}})
                page.goto(base)
                page.locator(f'#side .item[data-uid="{corpus.uid("claude-diagram")}"]').click()
                page.wait_for_selector('.mb pre > code.code-block')
                page.evaluate('document.fonts.ready')
                for width, zoom in [(1280, 1), (1280, 1.1), (390, 1)]:
                    page.set_viewport_size({'width': width, 'height': 900})
                    page.evaluate('(zoom) => document.body.style.zoom = zoom', zoom)
                    codes = page.locator('.mb pre > code.code-block')
                    assert codes.count() == 2
                    for code in codes.all():
                        assert code.text_content() == DIAGRAM
                        measured = code.evaluate('''el => {
                          const style = getComputedStyle(el);
                          const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
                          const cells = []; let node, line = 0, column = 0;
                          while ((node = walker.nextNode())) {
                            for (let i = 0; i < node.length; i++) {
                              const ch = node.data[i];
                              if (ch === '\\n') { line++; column = 0; continue; }
                              const range = document.createRange();
                              range.setStart(node, i); range.setEnd(node, i + 1);
                              const rect = range.getBoundingClientRect();
                              if (line < 5) cells.push({ch, line, column, x: rect.x, width: rect.width});
                              column++;
                            }
                          }
                          return {cells, family: style.fontFamily};
                        }''')
                        cells = measured['cells']
                        unit, origin = cells[0]['width'], cells[0]['x']
                        for cell in cells:
                            assert abs(cell['width'] - unit) < 0.1, (width, zoom, cell, unit)
                            assert abs(cell['x'] - origin - cell['column'] * unit) < 0.5, (width, zoom, cell)
                assert not errors, errors
                print('PASS code diagram browser: exact text and box/space columns at desktop, zoom and narrow widths')
            finally:
                browser.close()


if __name__ == '__main__':
    main()
