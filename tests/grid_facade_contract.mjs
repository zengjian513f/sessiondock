import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GridTerm, proposeGridDimensions} from '../legacy-web/grid/facade.js';

if (typeof globalThis.requestAnimationFrame !== 'function') {
  globalThis.requestAnimationFrame = fn => setTimeout(fn, 0);
  globalThis.cancelAnimationFrame = id => clearTimeout(id);
}

const ascii = (text, wrapped = false) => ({s: [[text, -1, -1, 0]], w: wrapped});
const line = obj => JSON.stringify(obj) + '\n';

function snapshot(extra) {
  return line({
    t: 'snapshot',
    seq: 1,
    reset: true,
    cursor: {x: 0, y: 0, visible: true},
    modes: {},
    history: [],
    cols: 8,
    rows: 2,
    grid: [ascii('hello'), ascii('world')],
    ...extra,
  });
}

test('constructor does not touch document and defaults to 80×24', () => {
  assert.equal(typeof document, 'undefined');
  const term = new GridTerm({});
  assert.equal(term.cols, 80);
  assert.equal(term.rows, 24);
  assert.ok(term.model);
  assert.ok(term.renderer);
  assert.equal(term.unicode.activeVersion, '11');
  term.unicode.activeVersion = '6';
  assert.equal(term.unicode.activeVersion, '6');
  const osc = term.parser.registerOscHandler(52, () => {});
  assert.equal(typeof osc.dispose, 'function');
  osc.dispose();
  term.loadAddon();
  term.clearTextureAtlas();
});

test('write snapshot and a diff split across two writes', () => {
  const term = new GridTerm({});
  term.write(snapshot({
    history: [ascii('old')],
    grid: [ascii('hi'), ascii('there')],
  }));
  assert.equal(term.cols, 8);
  assert.equal(term.rows, 2);
  assert.equal(term.buffer.active.length, 3);
  assert.equal(term.buffer.active.baseY, 1);
  assert.equal(term.buffer.active.getLine(0).translateToString(true), 'old');
  assert.equal(term.buffer.active.getLine(1).translateToString(true), 'hi');
  assert.equal(term.buffer.active.getLine(2).translateToString(true), 'there');
  assert.equal(term.buffer.normal, term.buffer.active);
  assert.equal(term.buffer.alternate, term.buffer.active);

  const diff = JSON.stringify({
    t: 'diff',
    seq: 2,
    rows: [[0, ascii('HI')]],
  });
  const mid = Math.floor(diff.length / 2);
  term.write(diff.slice(0, mid));
  assert.equal(term.buffer.active.getLine(1).translateToString(true), 'hi');
  term.write(diff.slice(mid) + '\n');
  assert.equal(term.buffer.active.getLine(1).translateToString(true), 'HI');
});

test('write callback runs after the scheduled render', async () => {
  const term = new GridTerm({});
  let painted = false;
  const orig = term.renderer.render.bind(term.renderer);
  term.renderer.render = (...args) => {
    painted = true;
    return orig(...args);
  };
  let called = false;
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('write callback timed out')), 1000);
    term.write(snapshot(), () => {
      called = true;
      clearTimeout(timer);
      resolve();
    });
    assert.equal(called, false);
  });
  assert.equal(called, true);
  assert.equal(painted, true);
});

test('resize updates cols/rows and fires onResize', () => {
  const term = new GridTerm({});
  let got = null;
  term.onResize(size => { got = size; });
  term.resize(40, 12);
  assert.equal(term.cols, 40);
  assert.equal(term.rows, 12);
  assert.deepEqual(got, {cols: 40, rows: 12});
});

test('select / getSelection / getSelectionPosition / hasSelection / clearSelection', () => {
  const term = new GridTerm({});
  term.write(snapshot({rows: 1, grid: [ascii('hello')]}));
  assert.equal(term.hasSelection(), false);
  assert.equal(term.getSelection(), '');
  assert.equal(term.getSelectionPosition(), undefined);
  term.select(0, 0, 5);
  assert.equal(term.hasSelection(), true);
  assert.equal(term.getSelection(), 'hello');
  assert.deepEqual(term.getSelectionPosition(), {
    start: {x: 0, y: 0},
    end: {x: 5, y: 0},
  });
  term.clearSelection();
  assert.equal(term.hasSelection(), false);
  assert.equal(term.getSelection(), '');
});

test('modes.mouseTrackingMode mapping', () => {
  const term = new GridTerm({});
  assert.equal(term.modes.mouseTrackingMode, 'none');
  const cases = [
    ['none', 'none'],
    ['press_release', 'vt200'],
    ['button_motion', 'drag'],
    ['any_motion', 'any'],
  ];
  for (const [mouse, expected] of cases) {
    term.write(line({t: 'diff', modes: {mouse}}));
    assert.equal(term.modes.mouseTrackingMode, expected, mouse);
  }
});

test('reset empties the buffer', () => {
  const term = new GridTerm({});
  term.write(snapshot({
    history: [ascii('old')],
    grid: [ascii('a'), ascii('b')],
  }));
  term.select(0, 1, 1);
  assert.equal(term.buffer.active.baseY, 1);
  assert.equal(term.hasSelection(), true);
  term.reset();
  assert.equal(term.cols, 8);
  assert.equal(term.rows, 2);
  assert.equal(term.buffer.active.baseY, 0);
  assert.equal(term.buffer.active.length, 2);
  assert.equal(term.buffer.active.getLine(0).translateToString(true), '');
  assert.equal(term.hasSelection(), false);
});

test('onTitleChange fires for a diff with title', () => {
  const term = new GridTerm({});
  const titles = [];
  term.onTitleChange(title => titles.push(title));
  term.write(line({t: 'diff', title: 'hello'}));
  assert.deepEqual(titles, ['hello']);
});

test('dropped counts a bad line', () => {
  const term = new GridTerm({});
  term.write('not-json\n{"t":"diff"}\n{bad\n');
  assert.equal(term.dropped, 2);
});

test('proposeGridDimensions is a floor division of cell metrics', () => {
  const term = new GridTerm({});
  const {cellWidth, cellHeight} = term.renderer;
  assert.ok(cellWidth > 0);
  assert.ok(cellHeight > 0);
  const width = cellWidth * 40 + cellWidth / 2;
  const height = cellHeight * 12 + cellHeight / 3;
  const expected = {
    cols: Math.max(2, Math.floor(width / cellWidth)),
    rows: Math.max(1, Math.floor(height / cellHeight)),
  };
  assert.deepEqual(proposeGridDimensions(term, width, height), expected);
  assert.deepEqual(term.proposeDimensions(width, height), expected);
});

test('renderer methods do not throw without a canvas', () => {
  const term = new GridTerm({});
  term.renderer.measure();
  term.renderer.invalidate();
  term.renderer.setFont(term.renderer.fontFamily, term.renderer.fontSize);
  term.renderer.setTheme('dark');
  term.renderer.fit(800, 400);
  term.renderer.render(term.model, {viewportTop: 0});
  term.refresh(0, 10);
  term.resize(20, 10);
  term.write(snapshot());
  term.reset();
});
