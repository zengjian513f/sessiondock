import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GridModel} from '../legacy-web/grid/model.js';
import {LineDecoder, encodeResize, segmentText} from '../legacy-web/grid/wire.js';

const enc = new TextEncoder();
const u8 = s => enc.encode(s);
const ascii = (text, wrapped = false) => ({s: [[text, -1, -1, 0]], w: wrapped});
const columns = cells => cells.reduce((n, c) => n + c.width, 0);

function snapshot(model, extra) {
  return model.apply({
    t: 'snapshot',
    seq: 1,
    reset: true,
    cursor: {x: 0, y: 0, visible: true},
    modes: {},
    history: [],
    ...extra,
  });
}

test('LineDecoder splits and merges lines across pushes', () => {
  const d = new LineDecoder();
  assert.deepEqual(d.push(u8('{"t":"a"}\n{"t":"b"}')).map(m => m.t), ['a']);
  assert.deepEqual(d.push(u8('\n{"t":"c"}\n')).map(m => m.t), ['b', 'c']);
  assert.equal(d.dropped, 0);
});

test('LineDecoder joins a UTF-8 character split across pushes', () => {
  const d = new LineDecoder();
  const bytes = u8('{"n":"你"}\n');
  const split = bytes.indexOf(0xe4) + 1;
  assert.ok(split > 0 && split < bytes.length);
  assert.equal(d.push(bytes.subarray(0, split)).length, 0);
  const got = d.push(bytes.subarray(split));
  assert.equal(got.length, 1);
  assert.equal(got[0].n, '你');
});

test('LineDecoder skips a bad line and counts it', () => {
  const d = new LineDecoder();
  const got = d.push(u8('not-json\n{"ok":1}\n{bad\n{"ok":2}\n'));
  assert.equal(d.dropped, 2);
  assert.deepEqual(got.map(m => m.ok), [1, 2]);
});

test('encodeResize', () => {
  assert.equal(encodeResize(80, 24), '{"t":"resize","cols":80,"rows":24}');
});

test('segmentText on grapheme clusters', () => {
  assert.deepEqual(segmentText('a你😀b'), ['a', '你', '😀', 'b']);
});

test('snapshot reset true builds scrollback and pads viewport to cols', () => {
  const m = new GridModel();
  const result = snapshot(m, {
    cols: 8,
    rows: 2,
    history: [ascii('old')],
    grid: [ascii('hi'), {s: [], w: false}],
  });
  assert.equal(result.full, true);
  assert.equal(m.seq, 1);
  assert.equal(m.scrollback.length, 1);
  assert.equal(m.textOf(m.scrollback[0]), 'old');
  assert.equal(m.viewport.length, 2);
  assert.equal(m.rows, 2);
  assert.equal(m.lineCount(), 3);
  const cells = m.cellsOf(m.viewport[0]);
  assert.equal(columns(cells), 8);
  assert.equal(cells[0].text, 'h');
  assert.equal(cells[1].text, 'i');
  assert.equal(cells[2].text, ' ');
  assert.equal(cells[2].fg, -1);
  assert.equal(cells[2].bg, -1);
  assert.equal(cells[2].flags, 0);
  assert.equal(cells[2].width, 1);
  assert.equal(m.textOf(m.viewport[0]), 'hi');
  assert.equal(m.rowAt(0), m.scrollback[0]);
  assert.equal(m.rowAt(1), m.viewport[0]);
});

test('wide cell layout occupies two columns with no spacer entry', () => {
  const m = new GridModel();
  snapshot(m, {
    cols: 8,
    rows: 1,
    grid: [{s: [['你好', -1, -1, 32], ['x', -1, -1, 0]], w: false}],
  });
  const cells = m.cellsOf(m.viewport[0]);
  assert.equal(cells[0].text, '你');
  assert.equal(cells[0].width, 2);
  assert.equal(cells[1].text, '好');
  assert.equal(cells[1].width, 2);
  assert.equal(cells[2].text, 'x');
  assert.equal(cells[2].width, 1);
  assert.equal(columns(cells), 8);
  assert.equal(cells.length, 6);
});

test('diff replaces rows, appends scrolled, caps scrollback, patches cursor/modes', () => {
  const m = new GridModel({scrollbackLimit: 2});
  snapshot(m, {
    cols: 8,
    rows: 2,
    history: [ascii('h0')],
    grid: [ascii('a'), ascii('b')],
    modes: {alt: false, mouse: 'none'},
  });
  const result = m.apply({
    t: 'diff',
    seq: 2,
    scrolled: [ascii('s1'), ascii('s2')],
    rows: [[1, ascii('B')]],
    cursor: {x: 3, y: 1, visible: false},
    modes: {mouse: 'sgr'},
    title: 'term',
  });
  assert.equal(result.scrolledCount, 2);
  assert.equal(result.full, true);
  assert.ok(result.changedRows.has(1));
  assert.equal(m.scrollback.length, 2);
  assert.equal(m.textOf(m.scrollback[0]), 's1');
  assert.equal(m.textOf(m.scrollback[1]), 's2');
  assert.equal(m.textOf(m.viewport[1]), 'B');
  assert.equal(m.cursor.x, 3);
  assert.equal(m.cursor.y, 1);
  assert.equal(m.cursor.visible, false);
  assert.equal(m.modes.mouse, 'sgr');
  assert.equal(m.modes.alt, false);
  assert.equal(m.title, 'term');
});

test('seq gap increments lostMessages', () => {
  const m = new GridModel();
  snapshot(m, {cols: 4, rows: 1, grid: [{s: [], w: false}]});
  m.apply({t: 'diff', seq: 3});
  assert.equal(m.lostMessages, 1);
  assert.equal(m.seq, 3);
});

test('reflow narrows a 10-col wrapped pair and never splits a wide cell', () => {
  const m = new GridModel();
  snapshot(m, {
    cols: 10,
    rows: 1,
    history: [
      {s: [['ABCD', -1, -1, 0], ['你', -1, -1, 32], ['EFGH', -1, -1, 0]], w: true},
      ascii('IJ'),
    ],
    grid: [{s: [], w: false}],
  });
  m.reflow(5);
  assert.equal(m.cols, 5);
  assert.equal(m.scrollback.length, 3);
  assert.equal(m.scrollback[0].wrapped, true);
  assert.equal(m.scrollback[1].wrapped, true);
  assert.equal(m.scrollback[2].wrapped, false);
  assert.equal(m.textOf(m.scrollback[0]), 'ABCD');
  const r0 = m.cellsOf(m.scrollback[0]);
  assert.equal(columns(r0), 5);
  assert.equal(r0[4].text, ' ');
  const r1 = m.cellsOf(m.scrollback[1]);
  assert.equal(r1[0].text, '你');
  assert.equal(r1[0].width, 2);
  assert.equal(r1[1].text, 'E');
  assert.equal(r1[2].text, 'F');
  assert.equal(r1[3].text, 'G');
  assert.equal(columns(r1), 5);
  assert.equal(m.textOf(m.scrollback[2]), 'HIJ');
});

test('reflow widens back and un-wraps', () => {
  const m = new GridModel();
  snapshot(m, {
    cols: 10,
    rows: 1,
    history: [ascii('0123456789', true), ascii('ab')],
    grid: [{s: [], w: false}],
  });
  m.reflow(5);
  assert.equal(m.scrollback.length, 3);
  assert.equal(m.scrollback[0].wrapped, true);
  assert.equal(m.scrollback[1].wrapped, true);
  assert.equal(m.scrollback[2].wrapped, false);
  assert.equal(m.textOf(m.scrollback[0]), '01234');
  assert.equal(m.textOf(m.scrollback[1]), '56789');
  assert.equal(m.textOf(m.scrollback[2]), 'ab');
  m.reflow(20);
  assert.equal(m.scrollback.length, 1);
  assert.equal(m.scrollback[0].wrapped, false);
  assert.equal(m.textOf(m.scrollback[0]), '0123456789ab');
});

test('selectionText across a wrapped pair has no newline inside the logical line', () => {
  const m = new GridModel();
  snapshot(m, {
    cols: 10,
    rows: 1,
    history: [ascii('hello worl', true), ascii('d!'), ascii('next')],
    grid: [{s: [], w: false}],
  });
  const text = m.selectionText({line: 0, col: 0}, {line: 1, col: 2});
  assert.equal(text, 'hello world!');
  assert.equal(text.includes('\n'), false);
  assert.equal(m.selectionText({line: 0, col: 0}, {line: 2, col: 4}), 'hello world!\nnext');
});

test('textOf trims trailing spaces', () => {
  const m = new GridModel();
  snapshot(m, {
    cols: 8,
    rows: 1,
    grid: [ascii('ab  ')],
  });
  assert.equal(m.textOf(m.viewport[0]), 'ab');
  assert.equal(m.textOf(m.viewport[0], 0, 1), 'a');
});
