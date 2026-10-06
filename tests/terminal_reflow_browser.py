#!/usr/bin/env python3
"""Main console width/height changes over ~9000 rows of fully paged history.

A synthetic shell prints 9000 numbered rows (every 90th row is a 300-column
line that soft-wraps) before the page opens its console. The page scrolls to
the top until every host row is loaded, then a real mouse drag starts a
selection across wrapped lines in the middle of the history and, while the
button is still down, the window width changes several times.

Each resize is measured in Chromium: the synchronous `term.resize` call and the
longest main-thread task (PerformanceObserver `longtask`) until the scrollback
reflow has finished. The suite asserts a long-task budget, then that rows
1..9000 are contiguous with every wrapped line intact, the selection still
covers the same text (and is what release copies), and the logical line at the
top of the viewport stays on top.

At the bottom of the console it then prints wrapped lines that fill the screen
and changes the width and the height: the rows the host moves between its
history and its screen during the resize must appear exactly once at the seam
(no duplicates after widening or growing, nothing missing after narrowing or
shrinking).
"""

import os
from pathlib import Path
import re
import tempfile
import time
import uuid

from playwright.sync_api import sync_playwright

from history_fixtures import BINARY, Corpus, codex_message, codex_row, isolated_server
import terminal_input_browser as fixture

ROWS = 9000
LONG_EVERY = 90
PAYLOAD = 'abcdefghij' * 30
TAIL = 40
SHELL = f"""stty -echo
awk 'BEGIN {{ s = ""; for (k = 0; k < 30; k++) s = s "abcdefghij";
  for (i = 1; i <= {ROWS}; i++) if (i % {LONG_EVERY} == 0) printf "REFLOW_ROW_%05d_%s\\n", i, s;
  else printf "REFLOW_ROW_%05d\\n", i }}'
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  case "$command" in
    ping) printf 'REFLOW_PING_OK\\n' ;;
    tail) awk 'BEGIN {{ s = ""; for (k = 0; k < 30; k++) s = s "abcdefghij";
      for (i = 1; i <= {TAIL}; i++) printf "REFLOW_TAIL_%03d_%s\\n", i, substr(s, 1, 150 + i) }}' ;;
    short) seq -f 'REFLOW_SHORT_%03g' 1 120 ;;
  esac
done
"""

TERM = '[...T.views.values()][0].term'
# Logical lines: `isWrapped` on a grid row means it continues on the next row.
LOGICAL = """() => { const b = %s.buffer.active, out = []; let buf = '';
  for (let i = 0; i < b.length; i++) {
    const l = b.getLine(i); if (!l) continue;
    if (l.isWrapped) buf += l.translateToString(false);
    else { buf += l.translateToString(true); out.push(buf.replace(/ +$/, '')); buf = ''; }
  }
  if (buf) out.push(buf.replace(/ +$/, ''));
  return out; }""" % TERM
# The logical line that contains the top visible row.
TOP_LOGICAL = """() => { const b = %s.buffer.active; let i = b.viewportY;
  while (i > 0 && b.getLine(i - 1)?.isWrapped) i--;
  let buf = '';
  for (; i < b.length; i++) { const l = b.getLine(i);
    if (l.isWrapped) buf += l.translateToString(false); else { buf += l.translateToString(true); break; } }
  return buf.replace(/ +$/, ''); }""" % TERM
REFLOW_IDLE = f'!{TERM}.model.reflowPending'
INSTRUMENT = """() => {
  const t = %s;
  window.reflowLongTasks = [];
  window.reflowResizeCalls = [];
  new PerformanceObserver(list => {
    for (const e of list.getEntries()) reflowLongTasks.push({start: e.startTime, duration: e.duration});
  }).observe({type: 'longtask', buffered: true});
  const resize = t.resize;
  t.resize = function (cols, rows) {
    const start = performance.now();
    const out = resize.call(this, cols, rows);
    const call = {start, duration: performance.now() - start, cols, rows, done: null};
    reflowResizeCalls.push(call);
    // Wall time until the whole scrollback is at the new width.
    const poll = () => { if (!t.model.reflowPending) call.done = performance.now() - start;
      else setTimeout(poll, 1); };
    poll();
    return out;
  };
}""" % TERM
ROW_RE = re.compile(r'^REFLOW_ROW_(\d{5})(?:_(.*))?$')
TAIL_RE = re.compile(r'^REFLOW_TAIL_(\d{3})_(.*)$')
SHORT_RE = re.compile(r'^REFLOW_SHORT_(\d{3})$')

# Long-task budget per resize with slack for a loaded CI machine. The old
# whole-history reflow took several hundred milliseconds here.
LONG_TASK_BUDGET_MS = float(os.environ.get('REFLOW_LONG_TASK_BUDGET_MS', '120'))
SYNC_RESIZE_BUDGET_MS = float(os.environ.get('REFLOW_SYNC_BUDGET_MS', '60'))
BASELINE = os.environ.get('REFLOW_BASELINE') == '1'


def launch_browser(pw):
    options = {'headless': True}
    if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
        options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
    return pw.chromium.launch(**options)


def problems(lines, first=1, last=ROWS, tail=0, short=0):
    """Order/completeness problems of the numbered rows in `lines`."""
    found, tails, shorts, stray = [], [], [], []
    for line in lines:
        if m := ROW_RE.match(line):
            n = int(m.group(1))
            want = PAYLOAD if n % LONG_EVERY == 0 else None
            if m.group(2) != want:
                stray.append(('payload', n, (m.group(2) or '')[:20], len(m.group(2) or '')))
            found.append(n)
        elif m := TAIL_RE.match(line):
            n = int(m.group(1))
            if m.group(2) != PAYLOAD[:150 + n]:
                stray.append(('tail payload', n, len(m.group(2))))
            tails.append(n)
        elif m := SHORT_RE.match(line):
            shorts.append(int(m.group(1)))
        elif line and line not in ('RS_SHELL_READY', 'REFLOW_PING_OK'):
            stray.append(('stray', line[:40]))
    out = []
    if found != list(range(first, last + 1)):
        seams = [(found[i], n) for i, n in enumerate(found[1:]) if n != found[i] + 1]
        out.append(('rows', found[:2], found[-2:], len(found), seams[:6]))
    if tail and tails != list(range(1, tail + 1)):
        seams = [(tails[i], n) for i, n in enumerate(tails[1:]) if n != tails[i] + 1]
        out.append(('tail', tails[:2], tails[-2:], len(tails), seams[:6]))
    if short and shorts != list(range(1, short + 1)):
        seams = [(shorts[i], n) for i, n in enumerate(shorts[1:]) if n != shorts[i] + 1]
        out.append(('short', shorts[:2], shorts[-2:], len(shorts), seams[:6]))
    return out + stray[:6]


def page_to_top(page):
    bar = page.get_by_role('scrollbar', name='终端历史')
    for _ in range(40):
        if page.evaluate(f'{TERM}.model.historyOlder') == 0:
            break
        bar.focus()
        bar.press('Home')
        page.wait_for_timeout(120)
        page.wait_for_function('!currentTermViewObject()?.historyLoading', timeout=10000)
    page.wait_for_function('!currentTermViewObject()?.historyLoading', timeout=10000)
    assert page.evaluate(f'{TERM}.model.historyOlder') == 0, 'history not fully paged'


def cell_point(page, line, col):
    """Client coordinates of the centre of model cell (line, col)."""
    return page.evaluate("""([line, col]) => { const t = %s, r = t.renderer;
      const c = document.querySelector('#termpane .grid-canvas').getBoundingClientRect();
      const sx = c.width / r.canvas.offsetWidth || 1, sy = c.height / r.canvas.offsetHeight || 1;
      return {x: c.left + (col + 0.5) * r.cellWidth * sx,
              y: c.top + (line - t.buffer.active.viewportY + 0.5) * r.cellHeight * sy}; }""" % TERM,
                         [line, col])


def resize_measured(page, width, height):
    before = page.evaluate(f'({{cols: {TERM}.cols, rows: {TERM}.rows, '
                           f'calls: reflowResizeCalls.length, now: performance.now()}})')
    page.set_viewport_size({'width': width, 'height': height})
    page.wait_for_function(f'n => reflowResizeCalls.length > n', arg=before['calls'], timeout=10000)
    page.wait_for_function(REFLOW_IDLE, timeout=20000)
    page.wait_for_timeout(250)
    page.wait_for_function(REFLOW_IDLE, timeout=20000)
    stats = page.evaluate("""since => {
      const calls = reflowResizeCalls.filter(c => c.start >= since);
      const tasks = reflowLongTasks.filter(t => t.start + t.duration >= since);
      return {calls, longest: Math.max(0, ...tasks.map(t => t.duration)),
              total: tasks.reduce((a, t) => a + t.duration, 0), count: tasks.length,
              cols: %s.cols, rows: %s.rows}; }""" % (TERM, TERM), before['now'])
    stats['sync'] = max((c['duration'] for c in stats['calls']), default=0)
    stats['done'] = max((c['done'] or 0 for c in stats['calls']), default=0)
    stats['from'] = (before['cols'], before['rows'])
    return stats


def run(pw, root, corpus, uid):
    with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid), \
         isolated_server(corpus, BINARY, host_dir=root / 'host') as (base, _):
        browser = launch_browser(pw)
        context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block',
                                      permissions=['clipboard-read', 'clipboard-write'])
        context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
        errors = []
        page = context.new_page()
        page.on('pageerror', lambda e: errors.append(str(e)))
        failures = []
        try:
            page.goto(base, wait_until='networkidle')
            fixture.open_console(page, uid)
            fixture.xterm_contains(page, f'REFLOW_ROW_{ROWS:05d}')
            page_to_top(page)
            lines = page.evaluate(LOGICAL)
            assert not problems(lines), problems(lines)
            loaded = page.evaluate(f'{TERM}.model.scrollback.length')
            print(f'PASS paged all {ROWS} rows into the console ({loaded} scrollback rows)', flush=True)

            # 1. Selection in the middle of the history, mouse still down.
            page.evaluate(INSTRUMENT)
            target = page.evaluate("""() => { const b = %s.buffer.active;
              for (let i = Math.floor(b.baseY / 2); i < b.length; i++)
                if (/^REFLOW_ROW_\\d{5}_abc/.test(b.getLine(i).translateToString(true))) return i;
              return -1; }""" % TERM)
            assert target > 0, target
            page.evaluate(f'{TERM}.scrollToLine({target - 3})')
            page.wait_for_timeout(100)
            top_before = page.evaluate(TOP_LOGICAL)
            page.evaluate("navigator.clipboard.writeText('reflow-sentinel')")
            start, end = cell_point(page, target, 4), cell_point(page, target + 4, 9)
            page.mouse.move(start['x'], start['y'])
            page.mouse.down()
            page.mouse.move(end['x'], end['y'], steps=6)
            selection = page.evaluate(f'{TERM}.getSelection()')
            assert selection.startswith('OW_ROW_') and '\n' in selection, selection[:80]

            # 2. Width changes with the selection held, measured.
            stats = []
            for width in (900, 1180, 820, 1040, 1280):
                s = resize_measured(page, width, 900)
                stats.append(s)
                print(f"  resize {s['from'][0]}x{s['from'][1]} -> {s['cols']}x{s['rows']}: "
                      f"sync {s['sync']:.1f} ms, whole reflow {s['done']:.0f} ms, longest task {s['longest']:.1f} ms, "
                      f"long tasks {s['count']} ({s['total']:.0f} ms)", flush=True)
                lines = page.evaluate(LOGICAL)
                if p := problems(lines):
                    failures.append(('rows after resize', width, p))
                if (got := page.evaluate(f'{TERM}.getSelection()')) != selection:
                    failures.append(('selection after resize', width, got[:60], selection[:60]))
                if (top := page.evaluate(TOP_LOGICAL)) != top_before:
                    failures.append(('viewport anchor', width, top[:24], top_before[:24]))
            page.mouse.up()
            if not failures:
                page.wait_for_function("t => navigator.clipboard.readText().then(c => c === t)", arg=selection,
                                       timeout=5000)
            worst = max(s['longest'] for s in stats)
            sync = max(s['sync'] for s in stats)
            print(f'MEASURE longest main-thread task {worst:.1f} ms, longest sync resize {sync:.1f} ms '
                  f'over {len(stats)} width changes with {loaded} scrollback rows', flush=True)
            if not BASELINE:
                assert worst <= LONG_TASK_BUDGET_MS, (worst, LONG_TASK_BUDGET_MS, stats)
                assert sync <= SYNC_RESIZE_BUDGET_MS, (sync, SYNC_RESIZE_BUDGET_MS, stats)
                assert not failures, failures
                print('PASS width changes stay within the long-task budget; rows contiguous, selection '
                      'text and viewport anchor kept; release copies the same text', flush=True)

            # 3. The seam between host history and the screen across resizes.
            bar = page.get_by_role('scrollbar', name='终端历史')
            bar.focus()
            bar.press('End')
            keyboard = page.locator('#termpane .xterm-helper-textarea')
            keyboard.press_sequentially('tail')
            keyboard.press('Enter')
            fixture.xterm_contains(page, f'REFLOW_TAIL_{TAIL:03d}_')
            page.wait_for_timeout(300)
            seam = []
            tail_requests = []

            def on_response(response):
                if '/api/term/grid/history' in response.url:
                    tail_requests.append(response.status)
            page.on('response', on_response)
            blocked = []

            def block(route):
                blocked.append(route.request.url)
                route.abort()
            # Height changes twice: once aligned with the host's history tail,
            # once with that request failing, so the local history_total
            # correction alone must keep the seam.
            steps = [('wider', 1600, 900, False), ('narrower', 840, 900, False), ('wider', 1280, 900, False),
                     ('shorter', 1280, 600, False), ('taller', 1280, 900, False),
                     ('shorter', 1280, 640, True), ('taller', 1280, 900, True)]
            short = 0
            for label, width, height, offline in steps:
                if label == 'shorter':
                    keyboard.press_sequentially('short')
                    keyboard.press('Enter')
                    short += 120
                    page.wait_for_function(f'n => ({LOGICAL})().filter(l => l === n).length === {short // 120}',
                                           arg='REFLOW_SHORT_120', timeout=10000)
                    page.wait_for_timeout(300)
                if offline:
                    page.route('**/api/term/grid/history?*', block)
                page.set_viewport_size({'width': width, 'height': height})
                page.wait_for_timeout(400)
                page.wait_for_function(f'{REFLOW_IDLE} && !{TERM}.model.tailSync && '
                                       '!currentTermViewObject()?.historyTailLoading', timeout=20000)
                page.wait_for_timeout(300)
                if offline:
                    page.unroute('**/api/term/grid/history?*', block)
                lines = page.evaluate(LOGICAL)
                # Every `short` printed 001..120 again; check the latest run.
                if short > 120:
                    last = max(i for i, line in enumerate(lines) if line == 'REFLOW_SHORT_001')
                    head = [line for line in lines[:last] if not SHORT_RE.match(line)]
                    lines = head + lines[last:]
                p = problems(lines, tail=TAIL, short=120 if short else 0)
                print(f"  {label}{' (history request failing)' if offline else ''} "
                      f"({page.evaluate(f'{TERM}.cols')}x{page.evaluate(f'{TERM}.rows')}): "
                      f"{'ok' if not p else p}", flush=True)
                if p:
                    seam.append((label, offline, p))
            if not BASELINE:
                assert tail_requests and all(status == 200 for status in tail_requests), tail_requests
                assert len(blocked) >= 2, blocked
            keyboard.press_sequentially('ping')
            keyboard.press('Enter')
            fixture.xterm_contains(page, 'REFLOW_PING_OK')
            if not BASELINE:
                assert not seam, seam
                print('PASS rows moved between host history and screen by width and height changes appear '
                      'exactly once', flush=True)
            assert not errors, errors
            if BASELINE:
                print('BASELINE failures', failures, 'seam', seam, flush=True)
        finally:
            context.close()
            browser.close()


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-reflow-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        for name in ['host', 'work', 'claude', 'codex', 'grok']:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
                                  codex_message('user', 'Synthetic reflow')], [])
        fixture.SHELL = SHELL
        run(pw, root, corpus, corpus.uid(sid))
        print('PASS terminal_reflow_browser', flush=True)


if __name__ == '__main__':
    main()
