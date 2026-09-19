// Grid domain model: viewport + scrollback + cursor + modes, reflow, selection.
// Pure: no DOM. Cells are materialized lazily; a wide cell occupies two columns
// with no spacer entry. The viewport is never reflowed (the server resends it).

import {segmentText} from './wire.js';

const FLAG_WIDE = 32;

const DEFAULT_MODES = Object.freeze({
  alt: false,
  app_cursor: false,
  app_keypad: false,
  bracketed_paste: false,
  focus_events: false,
  mouse: 'none',
  mouse_encoding: 'default',
});

const DEFAULT_CURSOR = Object.freeze({x: 0, y: 0, visible: true});

function blankCell() {
  return {text: ' ', fg: -1, bg: -1, flags: 0, width: 1};
}

function isBlankCell(cell) {
  return cell && cell.text === ' ' && cell.fg === -1 && cell.bg === -1
    && (cell.flags | 0) === 0 && cell.width === 1 && !cell.link && cell.ul == null;
}

// 可选的第 5 个元素：{link: 超链接 URL, ul: 下划线颜色}；缺省为 null。
function extraOf(span) {
  const extra = span && span[4];
  if (!extra || typeof extra !== 'object') return null;
  const link = typeof extra.link === 'string' && extra.link ? extra.link : null;
  const ul = typeof extra.ul === 'number' ? extra.ul : null;
  return link || ul != null ? {link, ul} : null;
}

function sameExtra(cell, span) {
  const extra = span[4] || null;
  const link = extra && extra.link ? extra.link : null;
  const ul = extra && typeof extra.ul === 'number' ? extra.ul : null;
  return (cell.link || null) === link && (cell.ul == null ? null : cell.ul) === ul;
}

function trimTrailingBlanks(cells) {
  let end = cells.length;
  while (end > 0 && isBlankCell(cells[end - 1])) end--;
  if (end < cells.length) cells.length = end;
  return cells;
}

function spansFromCells(cells) {
  const spans = [];
  for (const cell of cells) {
    const last = spans[spans.length - 1];
    if (last && last[1] === cell.fg && last[2] === cell.bg && last[3] === cell.flags
        && sameExtra(cell, last)) {
      last[0] += cell.text;
    } else {
      const span = [cell.text, cell.fg, cell.bg, cell.flags];
      if (cell.link || cell.ul != null) {
        const extra = {};
        if (cell.link) extra.link = cell.link;
        if (cell.ul != null) extra.ul = cell.ul;
        span.push(extra);
      }
      spans.push(span);
    }
  }
  return spans;
}

function posLess(a, b) {
  return a.line !== b.line ? a.line < b.line : a.col < b.col;
}

export class GridModel {
  constructor({scrollbackLimit = 100000} = {}) {
    this.scrollbackLimit = scrollbackLimit;
    this.cols = 0;
    this.rows = 0;
    this.viewport = [];
    this.scrollback = [];
    this.cursor = {x: 0, y: 0, visible: true};
    this.modes = {...DEFAULT_MODES};
    this.title = '';
    /// 服务端仍持有、尚未下发的更早历史行数（分页时递减）。
    this.historyOlder = 0;
    this.seq = 0;
    this.lostMessages = 0;
    this.version = 0;
  }

  apply(msg) {
    if (!msg || typeof msg !== 'object') {
      return {changedRows: new Set(), scrolledCount: 0, full: false};
    }
    if (typeof msg.seq === 'number') {
      if (this.seq > 0 && msg.seq > this.seq + 1) this.lostMessages += msg.seq - this.seq - 1;
      this.seq = msg.seq;
    }
    if (msg.t === 'snapshot') return this._applySnapshot(msg);
    if (msg.t === 'diff') return this._applyDiff(msg);
    return {changedRows: new Set(), scrolledCount: 0, full: false};
  }

  lineCount() {
    return this.scrollback.length + this.rows;
  }

  /// 把更早的历史行插到回滚区最前面（按需分页）；返回实际插入的行数。
  prependHistory(rawRows) {
    if (!Array.isArray(rawRows) || !rawRows.length) return 0;
    this.version++;
    const rows = rawRows.map(raw => this._row(raw));
    this.scrollback.unshift(...rows);
    // 插入后若超限，从最旧一端裁掉，返回值只算真正留下的。
    const extra = this.scrollback.length - this.scrollbackLimit;
    if (extra > 0) {
      this.scrollback.splice(0, extra);
      return Math.max(0, rows.length - extra);
    }
    return rows.length;
  }

  rowAt(i) {
    if (i < 0 || i >= this.lineCount()) return undefined;
    const hist = this.scrollback.length;
    return i < hist ? this.scrollback[i] : this.viewport[i - hist];
  }

  cellsOf(row) {
    if (!row) return [];
    const cols = this.cols;
    if (row.cells) {
      let used = 0;
      for (const cell of row.cells) used += cell.width;
      if (used === cols) return row.cells;
    }
    row.cells = this._materialize(row.spans, cols);
    return row.cells;
  }

  textOf(row, from = 0, to = Infinity) {
    return this._sliceText(row, from, to).replace(/ +$/, '');
  }

  selectionText(a, b) {
    if (!a || !b) return '';
    let start = a;
    let end = b;
    if (posLess(end, start)) {
      start = b;
      end = a;
    }
    const n = this.lineCount();
    if (n <= 0) return '';
    if (end.line < 0 || start.line >= n) return '';
    const lastLine = n - 1;
    const fromLine = Math.max(0, start.line);
    let toLine = end.line;
    let endCol = end.col;
    if (toLine > lastLine) {
      toLine = lastLine;
      endCol = Infinity;
    }
    const pieces = [];
    let buf = '';
    for (let i = fromLine; i <= toLine; i++) {
      const row = this.rowAt(i);
      if (!row) break;
      const from = i === start.line ? start.col : 0;
      const to = i === toLine && end.line <= lastLine ? endCol : (i === toLine ? endCol : Infinity);
      buf += this._sliceText(row, from, to);
      if (!row.wrapped || i === toLine) {
        pieces.push(buf.replace(/ +$/, ''));
        buf = '';
      }
    }
    if (buf) pieces.push(buf.replace(/ +$/, ''));
    return pieces.join('\n');
  }

  reflow(newCols) {
    this.version++;
    if (this.cols > 0) this._rebuildScrollback(newCols);
    this.cols = newCols;
  }

  resize(cols, rows) {
    this.version++;
    if (this.cols > 0 && cols !== this.cols) this._rebuildScrollback(cols);
    this.cols = cols;
    this.rows = rows;
  }

  _applySnapshot(msg) {
    this.version++;
    const cols = msg.cols | 0;
    const rows = msg.rows | 0;
    if (msg.reset) {
      this.scrollback = Array.isArray(msg.history) ? msg.history.map(raw => this._row(raw)) : [];
      this._capScrollback();
      // 服务端还留着多少更早的历史：history_total - 快照随附的行数。
      const total = typeof msg.history_total === 'number' ? msg.history_total : this.scrollback.length;
      this.historyOlder = Math.max(0, total - this.scrollback.length);
    } else if (this.cols > 0 && cols !== this.cols) {
      this._rebuildScrollback(cols);
    }
    this.cols = cols;
    this.rows = rows;
    const grid = Array.isArray(msg.grid) ? msg.grid : [];
    this.viewport = [];
    for (let y = 0; y < rows; y++) {
      this.viewport.push(grid[y] != null ? this._row(grid[y]) : this._empty());
    }
    this.cursor = {...DEFAULT_CURSOR, ...(msg.cursor || {})};
    this.modes = {...DEFAULT_MODES, ...(msg.modes || {})};
    if (msg.title != null) this.title = String(msg.title);
    const changedRows = new Set();
    for (let y = 0; y < rows; y++) changedRows.add(y);
    return {changedRows, scrolledCount: 0, full: true};
  }

  _applyDiff(msg) {
    this.version++;
    const changedRows = new Set();
    let scrolledCount = 0;
    let full = false;
    if (Array.isArray(msg.scrolled) && msg.scrolled.length && !this.modes.alt) {
      for (const raw of msg.scrolled) this.scrollback.push(this._row(raw));
      this._capScrollback();
      scrolledCount = msg.scrolled.length;
      full = true;
      for (let y = 0; y < this.rows; y++) changedRows.add(y);
    }
    if (Array.isArray(msg.rows)) {
      for (const entry of msg.rows) {
        if (!Array.isArray(entry) || entry.length < 2) continue;
        const y = entry[0] | 0;
        if (y < 0) continue;
        const next = this._row(entry[1]);
        if (y < this.viewport.length) this.viewport[y] = next;
        else {
          while (this.viewport.length < y) this.viewport.push(this._empty());
          this.viewport.push(next);
        }
        changedRows.add(y);
      }
    }
    if (msg.cursor) this.cursor = {...this.cursor, ...msg.cursor};
    if (msg.modes) this.modes = {...this.modes, ...msg.modes};
    if (msg.title != null) this.title = String(msg.title);
    return {changedRows, scrolledCount, full};
  }

  _row(raw) {
    const spans = raw && Array.isArray(raw.s) ? raw.s
      : raw && Array.isArray(raw.spans) ? raw.spans
      : [];
    const wrapped = !!(raw && (raw.w ?? raw.wrapped));
    return {spans, wrapped, cells: null, version: this.version};
  }

  _empty() {
    return {spans: [], wrapped: false, cells: null, version: this.version};
  }

  _capScrollback() {
    const extra = this.scrollback.length - this.scrollbackLimit;
    if (extra > 0) this.scrollback.splice(0, extra);
  }

  _materialize(spans, cols) {
    const cells = [];
    if (!(cols > 0)) return cells;
    let used = 0;
    const list = Array.isArray(spans) ? spans : [];
    for (const span of list) {
      const text = span && span[0] != null ? String(span[0]) : '';
      const fg = span && span[1] != null ? span[1] : -1;
      const bg = span && span[2] != null ? span[2] : -1;
      const flags = span && span[3] != null ? span[3] | 0 : 0;
      const extra = extraOf(span);
      const width = flags & FLAG_WIDE ? 2 : 1;
      for (const g of segmentText(text)) {
        if (used + width > cols) {
          while (used < cols) {
            cells.push(blankCell());
            used++;
          }
          return cells;
        }
        const cell = {text: g, fg, bg, flags, width};
        if (extra) {
          if (extra.link) cell.link = extra.link;
          if (extra.ul != null) cell.ul = extra.ul;
        }
        cells.push(cell);
        used += width;
        if (used >= cols) return cells;
      }
    }
    while (used < cols) {
      cells.push(blankCell());
      used++;
    }
    return cells;
  }

  _sliceText(row, from, to) {
    if (!row) return '';
    const lo = from == null ? 0 : from;
    const hi = to == null ? Infinity : to;
    if (hi <= lo) return '';
    const cells = this.cellsOf(row);
    let col = 0;
    let out = '';
    for (const cell of cells) {
      const next = col + cell.width;
      if (col >= hi) break;
      if (next > lo) out += cell.text;
      col = next;
    }
    return out;
  }

  _rebuildScrollback(newCols) {
    const rebuilt = [];
    let group = [];
    const flush = () => {
      if (!group.length) return;
      const cells = [];
      for (const row of group) {
        for (const cell of this.cellsOf(row)) cells.push(cell);
      }
      trimTrailingBlanks(cells);
      for (const row of this._wrap(cells, newCols)) rebuilt.push(row);
      group = [];
    };
    for (const row of this.scrollback) {
      group.push(row);
      if (!row.wrapped) flush();
    }
    flush();
    this.scrollback = rebuilt;
  }

  _wrap(cells, cols) {
    if (!(cols > 0)) return [];
    if (!cells.length) return [this._empty()];
    const rows = [];
    let current = [];
    let used = 0;
    const emit = wrapped => {
      rows.push({
        spans: spansFromCells(current),
        wrapped,
        cells: null,
        version: this.version,
      });
      current = [];
      used = 0;
    };
    for (const cell of cells) {
      const width = cell.width === 2 ? 2 : 1;
      if (used + width > cols) {
        if (used > 0) emit(true);
        else if (width > cols) {
          // Does not fit even on an empty row; drop rather than loop.
          continue;
        }
      }
      current.push(cell);
      used += width;
      if (used >= cols) emit(true);
    }
    if (current.length) emit(false);
    else if (rows.length) rows[rows.length - 1].wrapped = false;
    else rows.push(this._empty());
    return rows;
  }
}
