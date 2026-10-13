// Grid domain model: viewport + scrollback + cursor + modes, reflow, selection.
// Pure: no DOM. Cells are materialized lazily; a wide cell occupies two columns
// with no spacer entry. The viewport is never reflowed (the server resends it).
// A width change can reflow the scrollback lazily (`lazyReflow`): each row keeps
// the width it was wrapped at (`wcols`) until its logical line is rewrapped, the
// owner reflows the visible window at once and the rest in cancellable slices.

import {segmentText} from './wire.js';

const FLAG_WIDE = 32;
/// 分页结果末尾与本地最旧几行重叠，用来对齐接缝；再多带一段余量，宿主行号
/// 估计偏小（重排、尺寸变化）时接缝仍落在结果里。
const HISTORY_SEAM_ROWS = 4;
const HISTORY_SEAM_SLACK = 64;
/// 尺寸变化后与宿主历史末尾对齐：取宿主最新的这么多行，用其中最旧的几行在本地
/// 找到位置，再用宿主的行替换本地末尾（宿主重排时在历史与屏幕之间搬动的行）。
const TAIL_SYNC_ROWS = 400;
const TAIL_ANCHOR_ROWS = 8;

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

function isDefaultSpan(span) {
  const fg = span[1] ?? -1;
  const bg = span[2] ?? -1;
  return fg === -1 && bg === -1 && (span[3] | 0) === 0 && !extraOf(span);
}

/// 行内容的规范形式：相邻同属性 span 合并，末尾默认属性的空格去掉（宿主的
/// 行就是这样编码的；本地重排出的行可能带着这些空格）。缓存在行上：行的
/// spans 从不原地修改。
function rowKey(row) {
  if (row.key !== undefined) return row.key;
  const merged = [];
  for (const span of Array.isArray(row.spans) ? row.spans : []) {
    if (!span) continue;
    const text = span[0] != null ? String(span[0]) : '';
    const attrs = JSON.stringify([span[1] ?? -1, span[2] ?? -1, span[3] | 0, extraOf(span)]);
    const last = merged[merged.length - 1];
    if (last && last.attrs === attrs) last.text += text;
    else merged.push({text, attrs, plain: isDefaultSpan(span)});
  }
  while (merged.length) {
    const last = merged[merged.length - 1];
    if (!last.plain) break;
    last.text = last.text.replace(/ +$/, '');
    if (last.text) break;
    merged.pop();
  }
  row.key = (row.wrapped ? 'w' : 'n') + JSON.stringify(merged.map(m => [m.text, m.attrs]));
  return row.key;
}

function sameRow(a, b) {
  return !!a && !!b && rowKey(a) === rowKey(b);
}

/// 一定放得下：按 UTF-16 码元数（≥ 字素数）乘宽度估计，不必拆字素。
function spansFit(spans, cols) {
  let used = 0;
  for (const span of Array.isArray(spans) ? spans : []) {
    const text = span && span[0] != null ? String(span[0]) : '';
    used += text.length * ((span && span[3] | 0) & FLAG_WIDE ? 2 : 1);
    if (used > cols) return false;
  }
  return true;
}

function now() {
  return globalThis.performance?.now ? globalThis.performance.now() : Date.now();
}

function posLess(a, b) {
  return a.line !== b.line ? a.line < b.line : a.col < b.col;
}

export class GridModel {
  constructor({scrollbackLimit = 100000, lazyReflow = false} = {}) {
    this.scrollbackLimit = scrollbackLimit;
    /// true：宽度变化只登记，由调用方 reflowRange/reflowStep 分批重排；false：
    /// 立即整段重排（契约测试与没有调度器的调用方）。
    this.lazyReflow = lazyReflow;
    /// 还有回滚行没按当前宽度重排。_reflowCursor 以下（更新的一端）都已重排。
    this.reflowPending = false;
    this._reflowCursor = 0;
    /// 回滚区 [start, end) 被替换成 count 行时调用 (start, end, count, map)；
    /// map 把区间内的 {line, col} 换算到新位置。视口与选区靠它不漂移。
    this.onLinesReplaced = null;
    /// 宿主最近一个快照里的尺寸（判断宿主是否重排过）。
    this.hostCols = 0;
    this.hostRows = 0;
    /// 尺寸变化后待做的末尾对齐 {total}；tailEpoch 作废过期的结果。
    this.tailSync = null;
    this.tailEpoch = 0;
    /// 备用屏幕期间的快照不带主屏的 history_total，之后的高度补齐不可信。
    this._historyBaseStale = false;
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
    // 重排进行中，最旧几行的形状还会变，接缝比对要等重排完。
    if (this.reflowPending) return null;
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
    if (this.reflowPending) this._reflowCursor += at;
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

  /// 尺寸变化后宿主历史末尾的请求范围 [from, to)（宿主绝对行号，截止于快照
  /// 时的 history_total，之后滚出的行不在其中）；没有待做的对齐时为 null。
  historyTailRequest(maxRows = TAIL_SYNC_ROWS) {
    if (!this.tailSync || this.modes.alt) return null;
    const total = this.tailSync.total;
    const count = Math.min(maxRows | 0, total);
    if (!(count > TAIL_ANCHOR_ROWS)) {
      this.tailSync = null;
      return null;
    }
    return {tail: true, epoch: this.tailEpoch, from: total - count, to: total};
  }

  /// 用宿主历史末尾替换本地末尾。返回 applied（changed 表示本地有改动）、
  /// stale、stop（结果不完整或找不到锚点：本地保持原样）。
  acceptHistoryTail(req, result) {
    if (!req || req.epoch !== this.tailEpoch || !this.tailSync) return {status: 'stale'};
    this.tailSync = null;
    const raw = Array.isArray(result?.rows) ? result.rows : [];
    if (result?.from !== req.from || raw.length !== req.to - req.from || this.modes.alt) {
      return {status: 'stop'};
    }
    const rows = raw.map(r => this._row(r));
    // 快照之后滚出的行接在宿主这段之后，不参与比对。
    const limit = this.scrollback.length - Math.min(this.historyAppended, this.scrollback.length);
    const expected = limit - rows.length;
    const lo = Math.max(0, expected - rows.length);
    const hi = Math.min(limit - TAIL_ANCHOR_ROWS, expected + rows.length);
    if (hi < lo) return {status: 'stop'};
    this.reflowRange(lo, limit);
    let at = -1;
    for (let p = lo; p <= hi; p++) {
      let same = true;
      for (let k = 0; k < TAIL_ANCHOR_ROWS && same; k++) same = sameRow(this.scrollback[p + k], rows[k]);
      if (same && (at < 0 || Math.abs(p - expected) < Math.abs(at - expected))) at = p;
    }
    if (at < 0) return {status: 'stop'};
    let changed = limit - at !== rows.length;
    for (let k = 0; !changed && k < rows.length; k++) changed = !sameRow(this.scrollback[at + k], rows[k]);
    if (!changed) return {status: 'applied', changed: false};
    // 本地多出的行（宿主已拉回屏幕）或缺的行（宿主推进了历史）都在末尾；
    // 区间内的位置保持绝对行号，之后的（屏幕、快照后滚出的行）随增减移动。
    this._replaceRows(at, limit, rows, pos => pos);
    return {status: 'applied', changed: true};
  }

  rowAt(i) {
    if (i < 0 || i >= this.lineCount()) return undefined;
    const hist = this.scrollback.length;
    return i < hist ? this.scrollback[i] : this.viewport[i - hist];
  }

  /// 行当前的折行宽度：尚未重排的回滚行是它原来的宽度。
  rowCols(row) {
    return row && row.wcols > 0 ? row.wcols : this.cols;
  }

  cellsOf(row) {
    if (!row) return [];
    const cols = this.rowCols(row);
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
    return this._materialize(row.spans, this.rowCols(row));
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
    this._changeCols(newCols);
  }

  resize(cols, rows) {
    this.version++;
    this._changeCols(cols);
    this.rows = rows;
  }

  /// 把 [lo, hi) 所在的逻辑行按当前宽度重排（视口附近立即做）。
  reflowRange(lo, hi) {
    if (!this.reflowPending) return;
    const sb = this.scrollback;
    lo = Math.max(0, lo | 0);
    let end = Math.min(sb.length, hi | 0);
    if (lo >= end) return;
    while (end < sb.length && sb[end - 1].wrapped) end++;
    while (end > lo) {
      const start = this._groupStart(end);
      this._reflowGroup(start, end);
      end = start;
    }
  }

  /// 从最新一端往前重排一批，到 deadline（performance.now() 毫秒）为止；
  /// 不给 deadline 就做完。返回是否还有没重排的行。
  reflowStep(deadline = null) {
    if (!this.reflowPending) return false;
    const sb = this.scrollback;
    this._reflowCursor = Math.min(this._reflowCursor, sb.length);
    while (this._reflowCursor > 0) {
      let end = this._reflowCursor;
      while (end < sb.length && sb[end - 1].wrapped) end++;
      const start = this._groupStart(end);
      // 组在游标以下的部分已是新宽度；_reflowGroup 会按增减移动游标。
      this._reflowGroup(start, end);
      this._reflowCursor = start;
      if (deadline != null && now() >= deadline) return true;
    }
    for (const row of sb) {
      if (this.rowCols(row) !== this.cols) {
        this._reflowCursor = sb.length;
        return true;
      }
    }
    this.reflowPending = false;
    return false;
  }

  finishReflow() {
    while (this.reflowStep(null)) { /* until done */ }
  }

  _applySnapshot(msg) {
    this.version++;
    const cols = msg.cols | 0;
    const rows = msg.rows | 0;
    if (msg.reset) {
      this.scrollback = Array.isArray(msg.history) ? msg.history.map(raw => this._row(raw)) : [];
      this.reflowPending = false;
      this._reflowCursor = 0;
      this.tailSync = null;
      this.tailEpoch++;
      this._historyBaseStale = !!msg.modes?.alt;
      this._capScrollback();
      // 服务端还留着多少更早的历史：history_total - 快照随附的行数。
      const total = typeof msg.history_total === 'number' ? msg.history_total : this.scrollback.length;
      this.historyOlder = Math.max(0, total - this.scrollback.length);
      this.historyEpoch++;
      this.historyBase = total;
      this.historyAppended = 0;
      this.historyResync = false;
      this.historyPaging = this.historyOlder > 0;
    } else {
      this._hostResized(msg, cols);
    }
    this.cols = cols;
    this.rows = rows;
    this.hostCols = cols;
    this.hostRows = rows;
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
      // 宿主回滚区满了以后每滚进一行就从最旧一端挤掉一行，history_total 不再
      // 增长；按它校正估计，本地最旧一行的宿主行号随之前移（同 acceptHistory）。
      const total = msg.history_total;
      const dropped = typeof total === 'number' ? this.historyBase + this.historyAppended - total : 0;
      if (dropped > 0) {
        this.historyBase -= dropped;
        this.historyOlder = Math.max(0, this.historyOlder - dropped);
      }
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
      if (this.reflowPending) this._reflowCursor = Math.max(0, this._reflowCursor - extra);
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

  /// 宽度变化：尚未标记宽度的回滚行记下原宽度，然后登记重排（lazyReflow 时
  /// 由调用方分批完成，否则立即做完）。
  _changeCols(newCols) {
    const old = this.cols;
    if (!(old > 0) || newCols === old) {
      this.cols = newCols;
      return;
    }
    for (const row of this.scrollback) if (!(row.wcols > 0)) row.wcols = old;
    this.cols = newCols;
    // 宿主按自己的宽度重排历史，本地重排后的行与原来的绝对行号不再对应。
    this.historyEpoch++;
    this.historyResync = this.historyPaging;
    this.tailEpoch++;
    this.reflowPending = this.scrollback.length > 0;
    this._reflowCursor = this.scrollback.length;
    if (!this.lazyReflow) this.finishReflow();
  }

  /// reset:false 快照（宿主 resize）。宿主重排时会在历史和屏幕之间搬行：变矮
  /// 把光标以上放不下的行推进历史，变高从历史拉回行，宽度变化整体重排后屏幕
  /// 取最后几行。这些行不在 `scrolled` 里：高度变化按 history_total 的增减在
  /// 本地补上，并登记一次与宿主历史末尾的对齐（活连接上由页面取 grid/history）。
  _hostResized(msg, cols) {
    const widthChanged = this.hostCols > 0 && cols !== this.hostCols;
    this._changeCols(cols);
    const total = typeof msg.history_total === 'number' ? msg.history_total : null;
    if (total == null) return;
    if (this.modes.alt || msg.modes?.alt) {
      // 备用屏幕上的 history_total 不是主屏的；主屏在此期间也可能被重排过。
      this._historyBaseStale = true;
      return;
    }
    const expected = this.historyBase + this.historyAppended;
    // 宿主给出 history_moved 时以它为准：宿主历史满了以后行数不再变化，
    // 按 history_total 的增减算不出变矮推进历史的行（同 _applyDiff 的 dropped）。
    const moved = typeof msg.history_moved === 'number' ? msg.history_moved : null;
    const delta = moved ?? total - expected;
    // 宽度变了或基数不可信时本地算不出搬了哪些行，只能和宿主历史末尾对齐。
    const local = !widthChanged && !this._historyBaseStale;
    this._historyBaseStale = false;
    const dropped = moved == null ? 0 : expected + moved - total;
    if (local && dropped > 0) this.historyOlder = Math.max(0, this.historyOlder - dropped);
    if (local && delta > 0 && delta <= this.viewport.length) {
      // 变矮：原屏幕最上面 delta 行进了宿主历史；绝对行号不变。
      for (let y = 0; y < delta; y++) this.scrollback.push(this.viewport[y]);
      this._capScrollback();
    } else if (local && delta < 0 && -delta <= this.scrollback.length) {
      // 变高：宿主历史最后 -delta 行回到屏幕顶部，新快照里已有它们。
      this.scrollback.length += delta;
      if (this.reflowPending) this._reflowCursor = Math.min(this._reflowCursor, this.scrollback.length);
    }
    this.historyBase = total;
    this.historyAppended = 0;
    if (!local || delta !== 0) {
      this.tailEpoch++;
      this.tailSync = {total};
    }
  }

  _groupStart(end) {
    const sb = this.scrollback;
    let start = end - 1;
    while (start > 0 && sb[start - 1].wrapped) start--;
    return Math.max(0, start);
  }

  /// 把回滚区 [start, end)（一个逻辑行；最后一行可能接着折进屏幕）按当前宽度
  /// 重排。已经是当前宽度时不动。
  _reflowGroup(start, end) {
    const sb = this.scrollback;
    const target = this.cols;
    let done = true;
    for (let k = start; k < end && done; k++) done = this.rowCols(sb[k]) === target;
    if (done) return;
    const last = sb[end - 1];
    if (end - start === 1 && !last.wrapped
        && (this.rowCols(last) <= target || spansFit(last.spans, target))) {
      last.wcols = target;
      last.cells = null;
      this._cellCache.delete(last);
      last.version = ++this.version;
      return;
    }
    const cells = [];
    const oldStarts = [];
    let offset = 0;
    for (let k = start; k < end; k++) {
      oldStarts.push(offset);
      for (const cell of this.readCells(sb[k])) {
        cells.push(cell);
        offset += cell.width === 2 ? 2 : 1;
      }
    }
    trimTrailingBlanks(cells);
    this.version++;
    const {rows, starts} = this._wrapWithStarts(cells, target);
    // 逻辑行接着折进屏幕时保留续行标记。
    if (last.wrapped && rows.length) rows[rows.length - 1].wrapped = true;
    const map = pos => {
      const k = Math.min(end - start - 1, Math.max(0, pos.line - start));
      const g = oldStarts[k] + Math.max(0, pos.col);
      let r = 0;
      while (r + 1 < starts.length && starts[r + 1] <= g) r++;
      return {line: start + r, col: g - starts[r]};
    };
    this._replaceRows(start, end, rows, map);
  }

  _replaceRows(start, end, rows, map) {
    for (const row of rows) row.wcols = this.cols;
    this.scrollback.splice(start, end - start, ...rows);
    const delta = rows.length - (end - start);
    if (this.reflowPending && end <= this._reflowCursor) this._reflowCursor += delta;
    this.version++;
    if (typeof this.onLinesReplaced === 'function') this.onLinesReplaced(start, end, rows.length, map);
  }

  _wrap(cells, cols) {
    return this._wrapWithStarts(cells, cols).rows;
  }

  /// 折行结果和每行在逻辑行里的起始列。
  _wrapWithStarts(cells, cols) {
    const starts = [];
    if (!(cols > 0)) return {rows: [], starts};
    if (!cells.length) return {rows: [this._empty()], starts: [0]};
    const rows = [];
    let current = [];
    let used = 0;
    let offset = 0;
    const emit = wrapped => {
      rows.push({
        spans: spansFromCells(current),
        wrapped,
        cells: null,
        version: this.version,
      });
      starts.push(offset);
      offset += used;
      current = [];
      used = 0;
    };
    for (const cell of cells) {
      const width = cell.width === 2 ? 2 : 1;
      if (used + width > cols) {
        if (used > 0) emit(true);
        else if (width > cols) {
          // Does not fit even on an empty row; drop rather than loop.
          offset += width;
          continue;
        }
      }
      current.push(cell);
      used += width;
      if (used >= cols) emit(true);
    }
    if (current.length) emit(false);
    else if (rows.length) rows[rows.length - 1].wrapped = false;
    else {
      rows.push(this._empty());
      starts.push(0);
    }
    return {rows, starts};
  }
}
