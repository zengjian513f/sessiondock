// Grid domain model: viewport + scrollback + cursor + modes, reflow, selection.
// Pure: no DOM. Cells are materialized lazily; a wide cell occupies two columns
// with no spacer entry. The viewport is never reflowed (the server resends it).

import {segmentText} from './wire.js';

const FLAG_WIDE = 32;
/// 分页结果末尾与本地最旧几行重叠，用来对齐接缝；再多带一段余量，宿主行号
/// 估计偏小（重排、尺寸变化）时接缝仍落在结果里。
const HISTORY_SEAM_ROWS = 4;
const HISTORY_SEAM_SLACK = 64;

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

function sameRow(a, b) {
  return !!a && !!b && a.wrapped === b.wrapped
    && JSON.stringify(a.spans) === JSON.stringify(b.spans);
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
    this._cellCache = new Set();
    this.cursor = {x: 0, y: 0, visible: true};
    this.modes = {...DEFAULT_MODES};
    this.title = '';
    /// 服务端仍持有、尚未下发的更早历史行数，也就是本地最旧一行在宿主历史里
    /// 的绝对行号（0 = 最旧）的估计；分页时递减，接缝比对时校正。
    this.historyOlder = 0;
    /// 是否还能往前分页：reset 快照带来更早历史时打开，接缝对不上时关闭。
    this.historyPaging = false;
    /// reset 快照、重排或停止分页时递增，作废尚未落地的分页请求。
    this.historyEpoch = 0;
    /// 宿主历史总行数的估计 = historyBase + historyAppended（之后滚出的行）。
    this.historyBase = 0;
    this.historyAppended = 0;
    /// 本地按新宽度重排过回滚区，宿主行号要先用一次探测重新估计。
    this.historyResync = false;
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

  /// 下一页更早历史的请求范围（宿主绝对行号 [from, to)），没有可取的就
  /// 返回 null。范围末尾多带本地最旧的几行，用来在结果里找到接缝。
  historyRequest(maxRows = 500) {
    if (!this.historyPaging || this.modes.alt || !this.scrollback.length) return null;
    const base = {
      epoch: this.historyEpoch,
      head: this.scrollback[0],
      older: this.historyOlder,
      length: this.scrollback.length,
      appended: this.historyAppended,
      expected: this.historyBase + this.historyAppended,
    };
    if (this.historyResync) return {...base, probe: true, from: 0, to: 1};
    const edge = this.historyOlder;
    const room = this.scrollbackLimit - this.scrollback.length;
    const count = Math.min(maxRows | 0, room, edge);
    if (!(count > 0)) return null;
    const overlap = Math.min(HISTORY_SEAM_ROWS, this.scrollback.length);
    return {...base, from: edge - count, to: edge + overlap + HISTORY_SEAM_SLACK, edge, overlap};
  }

  /// 应用 `GET /api/term/grid/history` 的结果。status：applied（插入了
  /// added 行）、retry（估计已校正，按新范围再取）、stale（请求已作废）、
  /// stop（结果里找不到接缝，停止分页；绝不插入对不齐的行）。
  acceptHistory(req, result) {
    if (!req || req.epoch !== this.historyEpoch || !this.historyPaging) {
      return {status: 'stale', added: 0};
    }
    if (req.head !== this.scrollback[0] || req.older !== this.historyOlder) {
      return {status: 'retry', added: 0};
    }
    const total = typeof result?.total === 'number' ? result.total : null;
    if (req.probe) {
      if (total == null) return this._haltHistory();
      // 重排后假定本地回滚区就是宿主最新的那些历史行，接缝比对再校正。
      this.historyResync = false;
      this.historyBase = total - req.appended;
      this.historyOlder = Math.max(0, total - req.length);
      return {status: 'retry', added: 0};
    }
    // 宿主回滚区满了以后每进一行就从最旧一端丢一行，绝对行号随之前移；
    // 宿主总数比本地估计少多少，本地最旧一行的行号至少就前移了多少。
    if (total != null && total < req.expected) {
      const dropped = req.expected - total;
      this.historyBase -= dropped;
      this.historyOlder = Math.max(0, this.historyOlder - dropped);
      return {status: 'retry', added: 0};
    }
    const rows = Array.isArray(result?.rows) ? result.rows.map(raw => this._row(raw)) : [];
    if (result?.from !== req.from) return this._haltHistory();
    // 在结果里找本地最旧的 overlap 行；有多处时取离预期位置最近的一处。
    const expectedAt = req.edge - req.from;
    let at = -1;
    for (let p = 0; p + req.overlap <= rows.length; p++) {
      let same = true;
      for (let k = 0; k < req.overlap && same; k++) same = sameRow(rows[p + k], this.scrollback[k]);
      if (same && (at < 0 || Math.abs(p - expectedAt) < Math.abs(at - expectedAt))) at = p;
    }
    if (at < 0) return this._haltHistory();
    this.historyOlder = req.from + at;
    if (at === 0) return {status: req.from > 0 ? 'retry' : 'applied', added: 0};
    this.version++;
    this.scrollback.unshift(...rows.slice(0, at));
    this.historyOlder = req.from;
    // 请求发出后又滚进了新行时，超出上限的部分照常从最旧一端裁掉。
    const trimmed = Math.max(0, this.scrollback.length - this.scrollbackLimit);
    this._capScrollback();
    return {status: 'applied', added: at - trimmed};
  }

  stopHistory() {
    this.historyPaging = false;
    this.historyEpoch++;
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
    this._cellCache.delete(row);
    this._cellCache.add(row);
    if (this._cellCache.size > 512) {
      const oldest = this._cellCache.values().next().value;
      oldest.cells = null;
      this._cellCache.delete(oldest);
    }
    return row.cells;
  }

  // Searching/reflowing history must not turn every stored span into a
  // permanently cached array of cell objects.
  readCells(row) {
    if (!row) return [];
    return this._materialize(row.spans, this.cols);
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
      this.historyEpoch++;
      this.historyBase = total;
      this.historyAppended = 0;
      this.historyResync = false;
      this.historyPaging = this.historyOlder > 0;
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
      this.historyAppended += msg.scrolled.length;
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
    if (extra > 0) {
      this.scrollback.splice(0, extra);
      // 本地最旧一端前移了；它在宿主里仍是连续的，只是更早的行不再留着。
      this.historyOlder += extra;
    }
  }

  _haltHistory() {
    this.stopHistory();
    return {status: 'stop', added: 0};
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
    // 宿主按自己的宽度重排历史，本地重排后的行与原来的绝对行号不再对应。
    this.historyEpoch++;
    this.historyResync = this.historyPaging;
    const rebuilt = [];
    let group = [];
    const flush = () => {
      if (!group.length) return;
      if (group.length === 1 && !group[0].wrapped && newCols >= this.cols) {
        group[0].cells = null;
        rebuilt.push(group[0]);
        group = [];
        return;
      }
      const cells = [];
      for (const row of group) {
        for (const cell of this.readCells(row)) cells.push(cell);
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
