import assert from 'node:assert/strict';
import {test} from 'node:test';
import {GridRenderer} from '../legacy-web/grid/render.js';

// A recording 2D context: enough surface for measure() and render().
function fakeCanvas(width, height) {
  const ops = [];
  const ctx = {
    font: '', fillStyle: '', globalAlpha: 1, textBaseline: '', textAlign: '',
    setTransform: () => {},
    measureText: text => ({width: 8 * [...text].length, actualBoundingBoxAscent: 10}),
    fillRect: (x, y, w, h) => ops.push(['fillRect', x, y, w, h]),
    fillText: (text, x, y) => ops.push(['fillText', text, x, y]),
    save: () => ops.push(['save']),
    restore: () => ops.push(['restore']),
    beginPath: () => ops.push(['beginPath']),
    rect: (x, y, w, h) => ops.push(['rect', x, y, w, h]),
    clip: () => ops.push(['clip']),
  };
  return {canvas: {width, height, style: {}, getContext: () => ctx}, ops};
}

function model(rowsText) {
  const rows = rowsText.map((text, i) => ({version: i + 1, text}));
  return {
    cols: 10, rows: rows.length, scrollback: [], cursor: {x: 0, y: 0, visible: false},
    rowAt: line => rows[line] || null,
    cellsOf: row => [...row.text].map((ch, col) => ({col, width: 1, text: ch, fg: -1, bg: -1, flags: 0})),
  };
}

test('every painted row is clipped to its own box, so tall glyphs cannot leak into neighbours', () => {
  globalThis.devicePixelRatio = 1;
  const {canvas, ops} = fakeCanvas(80, 51);
  const renderer = new GridRenderer(canvas, {fontSize: 14, lineHeight: 1.2});
  renderer.fit(80, 51);
  renderer.render(model(['▓▓❯ abc', 'second', 'third']), {});
  const ch = renderer.cellHeight;
  const cw = renderer.cellWidth;
  // Each fillText sits between a save/clip of exactly its row's box and the matching restore.
  let open = null;
  const clips = [];
  for (const op of ops) {
    if (op[0] === 'rect') open = op;
    else if (op[0] === 'clip') { assert.ok(open, 'clip without rect'); clips.push(open); }
    else if (op[0] === 'fillText') {
      const [, , , y] = op;
      const box = clips[clips.length - 1];
      assert.ok(box, 'fillText outside any clip');
      assert.ok(y > box[2] && y <= box[2] + box[4], `baseline ${y} outside clip box ${box}`);
    } else if (op[0] === 'restore') open = null;
  }
  assert.equal(clips.length, 3, 'one clip per painted row');
  assert.deepEqual(clips.map(c => [c[1], c[2], c[3], c[4]]), [0, 1, 2].map(i => [0, i * ch, 10 * cw, ch]));
});

test('cell height lands on whole device pixels for fractional dpr', () => {
  for (const dpr of [1, 1.25, 1.5, 2, 2.75]) {
    globalThis.devicePixelRatio = dpr;
    const {canvas} = fakeCanvas(80, 51);
    const renderer = new GridRenderer(canvas, {fontSize: 14, lineHeight: 1.2});
    const device = renderer.cellHeight * dpr;
    assert.ok(Math.abs(device - Math.round(device)) < 1e-9, `dpr ${dpr}: cell height ${renderer.cellHeight} css = ${device} device px`);
    const width = renderer.cellWidth * dpr;
    assert.ok(Math.abs(width - Math.round(width)) < 1e-9, `dpr ${dpr}: cell width not device-aligned`);
  }
  globalThis.devicePixelRatio = 1;
});
