// Canvas 2D grid renderer. The page injects a model; this module does not import it.
// Pixel math is in CSS pixels unless noted; the backing store is CSS × devicePixelRatio.

const FLAG_BOLD = 1;
const FLAG_DIM = 2;
const FLAG_ITALIC = 4;
const FLAG_UNDERLINE = 8;
const FLAG_INVERSE = 16;
const FLAG_STRIKE = 64;
const FLAG_HIDDEN = 128;

const TRUECOLOR_BASE = 0x1000000;

// xterm / VS Code default 16-color palette (indexes 0–15).
const XTERM_16 = Object.freeze([
  '#000000', '#cd3131', '#0dbc79', '#e5e510',
  '#2472c8', '#bc3fbc', '#11a8cd', '#e5e5e5',
  '#666666', '#f14c4c', '#23d18b', '#f5f543',
  '#3b8eea', '#d670d6', '#29b8db', '#ffffff',
]);

// 256-color cube (16–231) and gray ramp (232–255), pre-rendered as CSS rgb().
const INDEXED_CSS = buildIndexedCss();

function buildIndexedCss() {
  const out = new Array(256);
  const level = i => (i === 0 ? 0 : 55 + i * 40);
  for (let idx = 16; idx <= 231; idx++) {
    const n = idx - 16;
    const r = Math.floor(n / 36);
    const g = Math.floor(n / 6) % 6;
    const b = n % 6;
    out[idx] = `rgb(${level(r)},${level(g)},${level(b)})`;
  }
  for (let idx = 232; idx <= 255; idx++) {
    const v = 8 + 10 * (idx - 232);
    out[idx] = `rgb(${v},${v},${v})`;
  }
  return out;
}

export const THEMES = Object.freeze({
  dark: Object.freeze({
    background: '#000000',
    foreground: '#cccccc',
    cursor: '#ffffff',
    selection: '#264f78',
    palette: XTERM_16,
  }),
  light: Object.freeze({
    background: '#f4f6f8',
    foreground: '#252a32',
    cursor: '#252a32',
    selection: '#c8d6ea',
    palette: XTERM_16,
  }),
});

function currentDpr() {
  const n = Number(globalThis.devicePixelRatio);
  return n > 0 ? n : 1;
}

// Offscreen measure surface so metrics work when the visible canvas is detached.
function makeSurface(width, height) {
  try {
    if (typeof OffscreenCanvas === 'function') {
      return new OffscreenCanvas(width, height);
    }
  } catch (_) { /* ignore */ }
  try {
    if (typeof document !== 'undefined' && document.createElement) {
      const c = document.createElement('canvas');
      c.width = width;
      c.height = height;
      return c;
    }
  } catch (_) { /* ignore */ }
  return null;
}

function safeContext(surface) {
  if (!surface || typeof surface.getContext !== 'function') return null;
  try {
    return surface.getContext('2d');
  } catch (_) {
    return null;
  }
}

function indexedCss(idx, palette) {
  if (idx >= 0 && idx <= 15) return palette[idx] || palette[0];
  if (idx >= 16 && idx <= 255) return INDEXED_CSS[idx];
  return null;
}

// Resolve a cell fg/bg: -1 default, 0–255 indexed, ≥ 0x1000000 truecolor.
// Bold + fg 0–7 uses the bright pair 8–15 (xterm drawBoldTextInBrightColors).
function resolveColor(value, theme, isFg, bold) {
  if (value == null || value === -1) {
    return isFg ? theme.foreground : theme.background;
  }
  if (value >= TRUECOLOR_BASE) {
    const rgb = value - TRUECOLOR_BASE;
    const r = (rgb >> 16) & 255;
    const g = (rgb >> 8) & 255;
    const b = rgb & 255;
    return `rgb(${r},${g},${b})`;
  }
  if (value >= 0 && value <= 255) {
    let idx = value | 0;
    if (isFg && bold && idx >= 0 && idx <= 7) idx += 8;
    return indexedCss(idx, theme.palette) || (isFg ? theme.foreground : theme.background);
  }
  return isFg ? theme.foreground : theme.background;
}

// Stream selection, start ≤ end, columns half-open [c0, c1) on a line.
// Intermediate lines are fully selected (0 … cols).
function selectionOnLine(sel, line, cols) {
  if (!sel || !sel.start || !sel.end) return null;
  const s = sel.start;
  const e = sel.end;
  if (line < s.line || line > e.line) return null;
  let c0 = 0;
  let c1 = cols;
  if (s.line === e.line) {
    c0 = s.col;
    c1 = e.col;
  } else if (line === s.line) {
    c0 = s.col;
    c1 = cols;
  } else if (line === e.line) {
    c0 = 0;
    c1 = e.col;
  }
  c0 = Math.max(0, Math.min(cols, c0 | 0));
  c1 = Math.max(0, Math.min(cols, c1 | 0));
  if (c1 <= c0) return null;
  return { c0, c1 };
}

function cellCovers(col, width, x) {
  return x >= col && x < col + width;
}

export class GridRenderer {
  constructor(canvas, {
    theme = 'dark',
    fontFamily = 'UbuntuSansMono, Consolas, monospace',
    fontSize = 14,
    lineHeight = 1.2,
  } = {}) {
    this.canvas = canvas;
    this.fontFamily = fontFamily;
    this.fontSize = fontSize;
    this.lineHeight = lineHeight;
    this.themeName = THEMES[theme] ? theme : 'dark';
    this.theme = THEMES[this.themeName];
    this.dpr = 1;
    this.cellWidth = 8;
    this.cellHeight = 17;
    this.baseline = 13;
    this.lastPaintedLines = 0;
    this._viewCols = 2;
    this._viewRows = 1;
    this._slots = [];
    this._allDirty = true;
    this._ctx = null;
    this._measureSurface = null;
    this.measure();
  }

  setTheme(name) {
    this.themeName = THEMES[name] ? name : 'dark';
    this.theme = THEMES[this.themeName];
    this.measure();
    this.invalidate();
  }

  setFont(family, size) {
    if (family) this.fontFamily = family;
    if (size > 0) this.fontSize = size;
    this.measure();
    this.invalidate();
  }

  invalidate() {
    this._allDirty = true;
  }

  _fontCss(weight = 400, italic = false) {
    const style = italic ? 'italic' : 'normal';
    return `${style} ${weight} ${this.fontSize}px ${this.fontFamily}`;
  }

  measure() {
    this.dpr = currentDpr();
    const dpr = this.dpr;
    // Fallback metrics if no 2D context exists (Node, or getContext → null).
    const fallbackWidth = Math.max(1, Math.round(this.fontSize * 0.6 * dpr) / dpr);
    const cellHeight = Math.max(1, Math.round(this.fontSize * this.lineHeight));
    this.cellHeight = cellHeight;

    if (!this._measureSurface) this._measureSurface = makeSurface(32, 32);
    const ctx = safeContext(this._measureSurface);
    if (!ctx) {
      this.cellWidth = fallbackWidth;
      this.baseline = Math.round((cellHeight * 0.8) * dpr) / dpr;
      return;
    }

    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.font = this._fontCss();
    const wAdvance = ctx.measureText('W').width;
    // CJK should be ~2× a single cell; only used if "W" measured as empty.
    const cjkAdvance = ctx.measureText('中').width;
    let cssWidth = wAdvance;
    if (!(cssWidth > 0) && cjkAdvance > 0) cssWidth = cjkAdvance / 2;
    if (!(cssWidth > 0)) cssWidth = this.fontSize * 0.6;
    // Snap cell width so col * cellWidth * dpr lands on an integer device pixel.
    this.cellWidth = Math.max(1, Math.round(cssWidth * dpr) / dpr);

    const m = ctx.measureText('M');
    const ascent = (m && m.actualBoundingBoxAscent > 0)
      ? m.actualBoundingBoxAscent
      : this.fontSize * 0.8;
    // Center the em-box in the cell, then place the alphabetic baseline
    // `ascent` below that top. Underline sits at baseline+2.
    const pad = (cellHeight - this.fontSize) / 2;
    this.baseline = Math.round((pad + ascent) * dpr) / dpr;
  }

  _ensureCtx() {
    const ctx = safeContext(this.canvas);
    this._ctx = ctx;
    return ctx;
  }

  _syncBacking(cssW, cssH) {
    const dpr = this.dpr;
    const canvas = this.canvas;
    if (!canvas) return;
    if (canvas.style) {
      canvas.style.width = `${cssW}px`;
      canvas.style.height = `${cssH}px`;
    }
    // Backing store is CSS size × dpr; assignment resets the 2D context.
    const bw = Math.max(0, Math.round(cssW * dpr));
    const bh = Math.max(0, Math.round(cssH * dpr));
    if (canvas.width !== bw) canvas.width = bw;
    if (canvas.height !== bh) canvas.height = bh;
    const ctx = this._ensureCtx();
    if (ctx) ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }

  fit(containerWidthCss, containerHeightCss) {
    this.measure();
    const cw = this.cellWidth > 0 ? this.cellWidth : 1;
    const ch = this.cellHeight > 0 ? this.cellHeight : 1;
    const boxW = Math.max(0, Number(containerWidthCss) || 0);
    const boxH = Math.max(0, Number(containerHeightCss) || 0);
    const cols = Math.max(2, Math.floor(boxW / cw));
    const rows = Math.max(1, Math.floor(boxH / ch));
    this._viewCols = cols;
    this._viewRows = rows;
    this._syncBacking(boxW, boxH);
    this.invalidate();
    return { cols, rows };
  }

  cellAt(cssX, cssY, viewportTop) {
    const cw = this.cellWidth > 0 ? this.cellWidth : 1;
    const ch = this.cellHeight > 0 ? this.cellHeight : 1;
    const cols = Math.max(1, this._viewCols | 0);
    const rows = Math.max(1, this._viewRows | 0);
    const col = Math.max(0, Math.min(cols - 1, Math.floor(Number(cssX) / cw)));
    const row = Math.max(0, Math.min(rows - 1, Math.floor(Number(cssY) / ch)));
    return { line: (viewportTop | 0) + row, col };
  }

  render(model, {
    viewportTop = 0,
    selection = null,
    focused = false,
    cursorBlinkOn = true,
  } = {}) {
    this.lastPaintedLines = 0;
    if (!model) return;
    const ctx = this._ensureCtx();
    if (!ctx) return;

    const dpr = this.dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.textBaseline = 'alphabetic';
    ctx.textAlign = 'left';

    const cols = Math.max(0, model.cols | 0);
    const rows = Math.max(0, model.rows | 0);
    const top = viewportTop | 0;
    const cw = this.cellWidth;
    const ch = this.cellHeight;
    const cssW = this.canvas && this.canvas.width ? this.canvas.width / dpr : cols * cw;
    const cssH = this.canvas && this.canvas.height ? this.canvas.height / dpr : rows * ch;

    const cursorAbs = (model.scrollback && model.scrollback.length | 0) +
      ((model.cursor && model.cursor.y) | 0);
    const cursorVisible = !!(model.cursor && model.cursor.visible);
    const showCursor = cursorVisible && cursorBlinkOn !== false &&
      cursorAbs >= top && cursorAbs < top + rows;
    const cursorX = showCursor ? (model.cursor.x | 0) : -1;
    const cursorMode = !showCursor || cursorX < 0 || cursorX >= cols
      ? 0
      : (focused ? 1 : 2);

    if (this._allDirty) {
      ctx.fillStyle = this.theme.background;
      ctx.fillRect(0, 0, cssW, cssH);
    } else if (this._slots.length > rows) {
      ctx.fillStyle = this.theme.background;
      ctx.fillRect(0, rows * ch, cssW, Math.max(0, cssH - rows * ch));
    }
    this._slots.length = rows;

    let painted = 0;
    for (let i = 0; i < rows; i++) {
      const absLine = top + i;
      const row = typeof model.rowAt === 'function' ? model.rowAt(absLine) : null;
      const version = row && typeof row.version === 'number' ? row.version : -1;
      const sel = selectionOnLine(selection, absLine, cols);
      const sel0 = sel ? sel.c0 : -1;
      const sel1 = sel ? sel.c1 : -1;
      const cursorHere = cursorMode !== 0 && cursorAbs === absLine ? cursorMode : 0;
      const cursorCol = cursorHere ? cursorX : -1;
      const prev = this._slots[i];
      const dirty = this._allDirty ||
        !prev ||
        prev.line !== absLine ||
        prev.version !== version ||
        prev.sel0 !== sel0 ||
        prev.sel1 !== sel1 ||
        prev.cursorCol !== cursorCol ||
        prev.cursorMode !== cursorHere;
      if (!dirty) continue;

      const cells = row && typeof model.cellsOf === 'function'
        ? (model.cellsOf(row) || [])
        : [];
      this._paintSlot(ctx, i, cols, cells, sel, cursorCol, cursorHere);
      this._slots[i] = {
        line: absLine,
        version,
        sel0,
        sel1,
        cursorCol,
        cursorMode: cursorHere,
      };
      painted++;
    }

    this._allDirty = false;
    this.lastPaintedLines = painted;
  }

  _paintSlot(ctx, slot, cols, cells, sel, cursorCol, cursorMode) {
    const y = slot * this.cellHeight;
    const cw = this.cellWidth;
    const ch = this.cellHeight;
    const theme = this.theme;
    const rowW = cols * cw;

    ctx.fillStyle = theme.background;
    ctx.fillRect(0, y, rowW, ch);

    // Walk cells once: background runs (post-inverse), then a second pass for glyphs.
    const prepared = [];
    let col = 0;
    const list = Array.isArray(cells) ? cells : [];
    for (let i = 0; i < list.length && col < cols; i++) {
      const cell = list[i] || {};
      const width = cell.width === 2 ? 2 : 1;
      const span = Math.min(width, cols - col);
      const flags = cell.flags | 0;
      const bold = !!(flags & FLAG_BOLD);
      let fg = resolveColor(cell.fg, theme, true, bold);
      let bg = resolveColor(cell.bg, theme, false, false);
      if (flags & FLAG_INVERSE) {
        const swapped = fg;
        fg = bg;
        bg = swapped;
      }
      prepared.push({
        col,
        width: span,
        text: cell.text == null ? '' : String(cell.text),
        fg,
        bg,
        flags,
        bold,
        link: cell.link || null,
        ul: cell.ul == null ? null : cell.ul,
      });
      col += span;
    }
    while (col < cols) {
      prepared.push({
        col,
        width: 1,
        text: ' ',
        fg: theme.foreground,
        bg: theme.background,
        flags: 0,
        bold: false,
      });
      col++;
    }

    // Background runs: consecutive cells sharing a fill (skip default, already cleared).
    let runBg = null;
    let runX = 0;
    let runW = 0;
    const flushBg = () => {
      if (runW > 0 && runBg && runBg !== theme.background) {
        ctx.fillStyle = runBg;
        ctx.fillRect(runX, y, runW, ch);
      }
      runW = 0;
      runBg = null;
    };
    for (const cell of prepared) {
      const bg = cell.bg;
      if (bg === runBg) {
        runW += cell.width * cw;
      } else {
        flushBg();
        runBg = bg;
        runX = cell.col * cw;
        runW = cell.width * cw;
      }
    }
    flushBg();

    if (sel) {
      ctx.fillStyle = theme.selection;
      ctx.fillRect(sel.c0 * cw, y, (sel.c1 - sel.c0) * cw, ch);
    }

    if (cursorMode === 1 && cursorCol >= 0) {
      ctx.fillStyle = theme.cursor;
      ctx.fillRect(cursorCol * cw, y, cw, ch);
    }

    let lastFont = '';
    for (const cell of prepared) {
      const flags = cell.flags;
      const hidden = !!(flags & FLAG_HIDDEN);
      const dim = !!(flags & FLAG_DIM);
      const italic = !!(flags & FLAG_ITALIC);
      const x = cell.col * cw;
      const onCursor = cursorMode === 1 && cellCovers(cell.col, cell.width, cursorCol);
      let fg = cell.fg;
      if (onCursor) fg = theme.background;

      const font = this._fontCss(cell.bold ? 600 : 400, italic);
      if (font !== lastFont) {
        ctx.font = font;
        lastFont = font;
      }

      if (dim) ctx.globalAlpha = 0.5;
      if (!hidden && cell.text && cell.text !== ' ') {
        ctx.fillStyle = fg;
        // Glyph origin is the cell's left edge (wide cells are not centered).
        ctx.fillText(cell.text, x, y + this.baseline);
      }
      if (!hidden && ((flags & FLAG_UNDERLINE) || cell.link)) {
        // 下划线颜色（SGR 58）优先；超链接没有下划线属性时也画一条，便于识别。
        ctx.fillStyle = cell.ul != null ? resolveColor(cell.ul, theme, true, false) : fg;
        const uy = y + this.baseline + 2;
        if (cell.link && !(flags & FLAG_UNDERLINE)) {
          for (let dx = 0; dx < cell.width * cw; dx += 3) ctx.fillRect(x + dx, uy, 1, 1);
        } else {
          ctx.fillRect(x, uy, cell.width * cw, 1);
        }
      }
      if (!hidden && (flags & FLAG_STRIKE)) {
        ctx.fillStyle = fg;
        ctx.fillRect(x, y + ch / 2, cell.width * cw, 1);
      }
      if (dim) ctx.globalAlpha = 1;
    }

    if (cursorMode === 2 && cursorCol >= 0) {
      // 1px outline inside the cell; fillRect stays on device pixels after dpr scale.
      const x = cursorCol * cw;
      ctx.fillStyle = theme.cursor;
      ctx.fillRect(x, y, cw, 1);
      ctx.fillRect(x, y + ch - 1, cw, 1);
      ctx.fillRect(x, y, 1, ch);
      ctx.fillRect(x + cw - 1, y, 1, ch);
    }
  }
}
