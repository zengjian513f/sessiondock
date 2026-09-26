#!/usr/bin/env python3
"""Fenced diagrams retain terminal columns despite a CJK browser fixed font."""
import argparse
import io
import os
import tempfile
from pathlib import Path

from playwright.sync_api import sync_playwright
from PIL import Image
from history_parity import BINARY, build_corpus, claude_row, isolated_server

DIAGRAM = ('  y\n'
           '  │    ┌─┐\n'
           '  │  ┌─┤ ├─┐\n'
           '  │┌─┤ │ │ ├─┐\n'
           '  └┴─┴─┴─┴─┴─┴── x\n'
           '   竖着切 x 轴')


ARROWS = ('自然数:  1   2   3   4   5  ...\n'
          '         ↕   ↕   ↕   ↕   ↕\n'
          '偶数:    2   4   6   8   10 ...')
FRACTIONS = ('分母→  1     2     3     4   ...\n'
             '分子\n'
             ' 1    1/1 → 1/2   1/3 → 1/4\n'
             '          ↙     ↗     ↙\n'
             ' 2    2/1   2/2   2/3   ...\n'
             '       ↓  ↗     ↙\n'
             ' 3    3/1   3/2   ...\n'
             '          ↙\n'
             ' 4    4/1   ...')
CASES = [('', DIAGRAM), ('text', ARROWS), ('', FRACTIONS),
         ('python', '# 中文：箭头↙↗\n数字 = "中文"'),
         ('text', ' é 1\n → 2'),
         ('text', '中文连续选择 Latin 123 ↙↗')]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-code-diagram-') as temporary:
        corpus = build_corpus(Path(temporary))
        corpus.put('claude-diagram', 'claude', [
            claude_row('claude-diagram', 'user', 'u', None, 'Show a diagram'),
            claude_row('claude-diagram', 'assistant', 'a', 'u',
                       '\n\n'.join('```' + lang + '\n' + text + '\n```' for lang, text in CASES))], [])
        with isolated_server(corpus, args.binary) as (base, _), sync_playwright() as p:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = p.chromium.launch(**launch)
            try:
                page = browser.new_page(viewport={'width': 1280, 'height': 900})
                page.context.grant_permissions(['clipboard-read', 'clipboard-write'])
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                # A real CJK fixed font has half-width ASCII but full-width box
                # drawing. Do not depend on Chromium's Linux default matching
                # the affected user's browser/OS font settings.
                page.context.new_cdp_session(page).send('Page.setFontFamilies', {
                    'fontFamilies': {'fixed': 'Noto Sans Mono CJK SC'}})
                page.goto(base)
                page.locator(f'#side .item[data-uid="{corpus.uid("claude-diagram")}"]').click()
                page.wait_for_selector('.mb pre > code.code-block[data-syntax-done]')
                page.evaluate('document.fonts.ready')
                page.add_style_tag(content='''
                  .code-block::selection, .code-block *::selection {
                    background: rgb(255, 0, 128); color: rgb(255, 255, 255);
                  }
                ''')
                for width, zoom in [(1280, 1), (1280, 1.1), (390, 1)]:
                    page.set_viewport_size({'width': width, 'height': 900})
                    page.evaluate('(zoom) => document.body.style.zoom = zoom', zoom)
                    if width < 720:
                        page.locator(f'#side .item[data-uid="{corpus.uid("claude-diagram")}"]').click()
                    codes = page.locator('.mb pre > code.code-block')
                    codes.first.wait_for(state='visible')
                    assert codes.count() == len(CASES)
                    assert page.locator('.code-cell').count() == 0
                    for code, (_, expected) in zip(codes.all(), CASES):
                        assert code.text_content() == expected
                        copied = code.evaluate('''el => {
                          const range = document.createRange(); range.selectNodeContents(el);
                          const selection = getSelection(); selection.removeAllRanges();
                          selection.addRange(range); const text = selection.toString();
                          selection.removeAllRanges(); return text;
                        }''')
                        assert copied == expected, (width, zoom, repr(copied), repr(expected))
                        measured = code.evaluate('''el => {
                          const style = getComputedStyle(el);
                          const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
                          const cells = []; let node, line = 0, column = 0;
                          while ((node = walker.nextNode())) {
                            for (const {segment: ch, index: i} of new Intl.Segmenter(undefined,
                              {granularity: 'grapheme'}).segment(node.data)) {
                              if (ch === '\\n') { line++; column = 0; continue; }
                              const range = document.createRange();
                              range.setStart(node, i); range.setEnd(node, i + ch.length);
                              const rect = range.getBoundingClientRect();
                              // Independent expectations for these fixtures: CJK/fullwidth
                              // punctuation uses two columns; arrows and accented Latin one.
                              const columns = /[\\u2e80-\\u9fff\\uff00-\\uffef]/u.test(ch) ? 2 : 1;
                              cells.push({ch, line, column, columns, x: rect.x, width: rect.width});
                              column += columns;
                            }
                          }
                          return {cells, family: style.fontFamily};
                        }''')
                        cells = measured['cells']
                        unit, origin = cells[0]['width'] / cells[0]['columns'], cells[0]['x']
                        assert unit > 0
                        for cell in cells:
                            assert abs(cell['width'] - unit * cell['columns']) < 0.1, (width, zoom, cell, unit)
                            assert abs(cell['x'] - origin - cell['column'] * unit) < 0.5, (width, zoom, cell)
                    # Real drag/copy, plus pixels: selection must paint continuously
                    # through the spaces between Chinese glyphs, not separate cells.
                    selection_code = codes.last
                    selection_code.scroll_into_view_if_needed()
                    boxes = selection_code.evaluate("""el => {
                      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
                      const boxes = []; let node;
                      while ((node = walker.nextNode())) {
                        for (let i = 0; i < node.length; i++) {
                          const r = document.createRange();
                          r.setStart(node, i); r.setEnd(node, i + 1);
                          boxes.push(r.getBoundingClientRect().toJSON());
                        }
                      }
                      return boxes;
                    }""")
                    first, last = boxes[0], boxes[-1]
                    assert first['width'] / zoom >= 14, first  # previously a 12px CJK glyph
                    y = (max(box['top'] for box in boxes) + min(box['bottom'] for box in boxes)) / 2
                    page.mouse.move(first['left'] + .2, y)
                    page.mouse.down()
                    page.mouse.move(last['right'] - .2, y, steps=30)
                    page.mouse.up()
                    assert page.evaluate('getSelection().toString()') == CASES[-1][1]
                    page.keyboard.press('Control+c')
                    assert page.evaluate('navigator.clipboard.readText()') == CASES[-1][1]
                    shot = Image.open(io.BytesIO(page.screenshot())).convert('RGB')
                    for x in range(round(first['left']) + 1, int(last['right']) - 1):
                        red, green, blue = shot.getpixel((x, int(y)))
                        assert red > 220 and blue > 100, (width, zoom, x, y, (red, green, blue))
                    page.evaluate('getSelection().removeAllRanges()')
                assert not errors, errors
                print('PASS code diagram browser: aligned diagrams, larger CJK, native drag selection without gaps, exact clipboard, syntax, zoom and mobile')
            finally:
                browser.close()


if __name__ == '__main__':
    main()
