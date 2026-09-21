// xterm.js-compatible facade over the grid model/renderer/input modules.
// The constructor does not touch `document`; `open` creates the canvas and
// helper textarea. Incoming `write` data is newline-delimited JSON text.

import {GridModel} from './model.js';
import {GridRenderer, THEMES, isLightBackground} from './render.js';
import {InputEncoder, KeyCapture} from './input.js';

const DEFAULT_COLS = 80;
const DEFAULT_ROWS = 24;
const BLINK_MS = 530;
const WHEEL_LINES = 3;

const ANSI_NAMES = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white'];
const BRIGHT_NAMES = [
  'brightBlack', 'brightRed', 'brightGreen', 'brightYellow',
  'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite',
];

function addListener(list, fn) {
  if (typeof fn !== 'function') return {dispose() {}};
  list.push(fn);
  return {
    dispose() {
      const i = list.indexOf(fn);
      if (i >= 0) list.splice(i, 1);
    },
  };
}

function emit(list, value) {
  if (!list.length) return;
  for (const fn of list.slice()) fn(value);
}

function mapTheme(theme) {
  const dark = THEMES.dark;
  const palette = Array.from(dark.palette);
  if (typeof theme === 'string' && THEMES[theme]) {
    const named = THEMES[theme];
    return {
      background: named.background,
      foreground: named.foreground,
      cursor: named.cursor,
      selection: named.selection,
      palette: Array.from(named.palette),
    };
  }
  if (!theme || typeof theme !== 'object') {
    return {
      background: dark.background,
      foreground: dark.foreground,
      cursor: dark.cursor,
      selection: dark.selection,
      palette,
    };
  }
  for (let i = 0; i < 8; i++) {
    if (typeof theme[ANSI_NAMES[i]] === 'string') palette[i] = theme[ANSI_NAMES[i]];
    if (typeof theme[BRIGHT_NAMES[i]] === 'string') palette[i + 8] = theme[BRIGHT_NAMES[i]];
  }
  return {
    background: theme.background || dark.background,
    foreground: theme.foreground || dark.foreground,
    cursor: theme.cursor || dark.cursor,
    selection: theme.selectionBackground || theme.selection || dark.selection,
    palette,
    light: isLightBackground(theme.background || dark.background),
  };
}

function decodeChunk(decoder, data) {
  if (data instanceof Uint8Array) return decoder.decode(data, {stream: true});
  if (data instanceof ArrayBuffer) return decoder.decode(new Uint8Array(data), {stream: true});
  if (ArrayBuffer.isView(data)) {
    return decoder.decode(
      new Uint8Array(data.buffer, data.byteOffset, data.byteLength),
      {stream: true},
    );
  }
  if (data == null) return '';
  return String(data);
}

function isWordChar(text) {
  if (!text) return false;
  const ch = text[0];
  if (ch === '_') return true;
  if (ch >= '0' && ch <= '9') return true;
  if ((ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z')) return true;
  return ch.charCodeAt(0) > 127;
}

function mouseTrackingMode(mouse) {
  if (mouse === 'none') return 'none';
  if (mouse === 'press_release') return 'vt200';
  if (mouse === 'button_motion') return 'drag';
  return 'any';
}

export function proposeGridDimensions(term, containerWidthCss, containerHeightCss) {
  const renderer = term.renderer;
  renderer.measure();
  const cw = renderer.cellWidth > 0 ? renderer.cellWidth : 1;
  const ch = renderer.cellHeight > 0 ? renderer.cellHeight : 1;
  const boxW = Math.max(0, Number(containerWidthCss) || 0);
  const boxH = Math.max(0, Number(containerHeightCss) || 0);
  return {
    cols: Math.max(2, Math.floor(boxW / cw)),
    rows: Math.max(1, Math.floor(boxH / ch)),
  };
}

export class GridTerm {
  constructor({
    fontFamily,
    fontSize,
    theme,
    scrollback = 100000,
    cursorBlink = true,
    onDiagnostic = null,
  } = {}) {
    this._cols = DEFAULT_COLS;
    this._rows = DEFAULT_ROWS;
    this._scrollback = scrollback;
    this._cursorBlink = cursorBlink !== false;
    this._xtermTheme = theme;
    this._onDiagnostic = onDiagnostic;
    this._firstSnapshot = false;
    this._firstPaint = false;
    this._parseErrorReported = false;
    this.dropped = 0;
    this._pending = '';
    this._decoder = new TextDecoder('utf-8');
    this._viewportTop = 0;
    this._following = true;
    this._selection = null;
    this._selecting = false;
    this._selectAnchor = null;
    this._mouseHeld = null;
    this._lastMouseCell = null;
    this._focused = false;
    this._blinkOn = true;
    this._blinkTimer = 0;
    this._renderPending = false;
    this._rafId = 0;
    this._renderCallbacks = [];
    this._dataListeners = [];
    this._selectionListeners = [];
    this._titleListeners = [];
    this._clipboardListeners = [];
    this._resizeListeners = [];
    this._scrollListeners = [];
    this._customKeyHandler = null;
    this._customWheelHandler = null;
    this._disposers = [];
    this._disposed = false;
    this._host = null;
    this._canvas = null;
    this._textarea = null;
    this._keyCapture = null;

    this.model = new GridModel({scrollbackLimit: scrollback});
    this.renderer = new GridRenderer(null, {
      theme: 'dark',
      ...(fontFamily ? {fontFamily} : {}),
      ...(fontSize > 0 ? {fontSize} : {}),
    });
    this._applyTheme(theme);
    this._encoder = new InputEncoder(() => this.model.modes);

    const term = this;
    this.unicode = {activeVersion: '11'};
    this.parser = {
      registerOscHandler() {
        return {dispose() {}};
      },
    };
    this.options = {
      get fontFamily() { return term.renderer.fontFamily; },
      set fontFamily(value) {
        term.renderer.setFont(value, term.renderer.fontSize);
        term._scheduleRender();
      },
      get fontSize() { return term.renderer.fontSize; },
      set fontSize(value) {
        term.renderer.setFont(term.renderer.fontFamily, value);
        term._scheduleRender();
      },
      get theme() { return term._xtermTheme; },
      set theme(value) {
        term._xtermTheme = value;
        term._applyTheme(value);
        term._scheduleRender();
      },
      get cursorBlink() { return term._cursorBlink; },
      set cursorBlink(value) {
        term._cursorBlink = !!value;
        if (term._cursorBlink && term._focused) term._startBlink();
        else {
          term._stopBlink();
          term._blinkOn = true;
          term._scheduleRender();
        }
      },
      get scrollback() { return term._scrollback; },
      set scrollback(value) {
        const n = Math.max(0, value | 0);
        term._scrollback = n;
        term.model.scrollbackLimit = n;
        if (term.model.scrollback.length > n) {
          term.model.scrollback.splice(0, term.model.scrollback.length - n);
          term._stickFollow();
          term._scheduleRender();
        }
      },
    };

    const buffer = {
      get length() { return term.model.lineCount(); },
      get viewportY() { return term._viewportTop; },
      get baseY() { return term.model.scrollback.length; },
      get cursorX() { return term.model.cursor.x | 0; },
      get cursorY() { return term.model.cursor.y | 0; },
      getLine(i) {
        const row = term.model.rowAt(i);
        if (!row) return undefined;
        return {
          translateToString(trimRight) {
            if (trimRight === false) {
              const cells = term.model.cellsOf(row);
              let out = '';
              let used = 0;
              for (const cell of cells) {
                out += cell.text == null ? '' : cell.text;
                used += cell.width === 2 ? 2 : 1;
              }
              while (used < term.model.cols) {
                out += ' ';
                used++;
              }
              return out;
            }
            return term.model.textOf(row);
          },
          get length() { return term.model.cols; },
          get isWrapped() { return !!row.wrapped; },
        };
      },
    };
    this.buffer = {active: buffer, normal: buffer, alternate: buffer};
  }

  get cols() {
    return this.model.cols > 0 ? this.model.cols : this._cols;
  }

  get rows() {
    return this.model.rows > 0 ? this.model.rows : this._rows;
  }

  get textarea() {
    return this._textarea;
  }

  get element() {
    return this._host;
  }

  get modes() {
    const modes = this.model.modes;
    return {
      mouseTrackingMode: mouseTrackingMode(modes.mouse),
      bracketedPasteMode: !!modes.bracketed_paste,
      applicationCursorKeysMode: !!modes.app_cursor,
    };
  }

  open(hostElement) {
    if (this._disposed || !hostElement) return;
    this._host = hostElement;
    const doc = hostElement.ownerDocument;
    const canvas = doc.createElement('canvas');
    canvas.className = 'grid-canvas';
    canvas.style.display = 'block';
    const textarea = doc.createElement('textarea');
    textarea.className = 'xterm-helper-textarea';
    textarea.setAttribute('aria-label', '终端输入');
    textarea.setAttribute('autocapitalize', 'off');
    textarea.setAttribute('autocomplete', 'off');
    textarea.setAttribute('autocorrect', 'off');
    textarea.setAttribute('spellcheck', 'false');
    textarea.spellcheck = false;
    textarea.tabIndex = 0;
    textarea.style.cssText = 'position:absolute;opacity:0;width:1px;height:1px;left:0;top:0;'
      + 'padding:0;border:0;resize:none;overflow:hidden;z-index:0;white-space:pre';
    hostElement.appendChild(canvas);
    hostElement.appendChild(textarea);
    this._canvas = canvas;
    this._textarea = textarea;
    this.renderer.canvas = canvas;

    this._keyCapture = new KeyCapture(textarea, {
      encoder: this._encoder,
      onBytes: bytes => {
        this._clearSelection();
        this._emitData(bytes);
      },
      onPaste: text => {
        this._clearSelection();
        this._emitData(this._encoder.paste(text));
      },
    });

    this._listen(textarea, 'keydown', event => {
      if (this._customKeyHandler && this._customKeyHandler(event) === false) {
        event.stopImmediatePropagation();
      }
    }, true);

    this._listen(textarea, 'focus', () => {
      this._focused = true;
      this._resetBlink();
      this._emitData(this._encoder.focus(true));
      this._scheduleRender();
    });
    this._listen(textarea, 'blur', () => {
      this._focused = false;
      this._stopBlink();
      this._blinkOn = true;
      this._emitData(this._encoder.focus(false));
      this._scheduleRender();
    });

    this._listen(hostElement, 'click', () => textarea.focus());
    this._listen(hostElement, 'wheel', event => {
      if (this._customWheelHandler && this._customWheelHandler(event) === false) return;
      event.preventDefault();
      const modes = this.model.modes;
      if (modes.alt && modes.mouse === 'none') {
        this._emitData(this._encoder.wheelAsArrows(event.deltaY, WHEEL_LINES));
        return;
      }
      if (modes.mouse !== 'none') {
        const cell = this._cellFromEvent(event);
        this._emitData(this._mouseSeq('wheel', 0, cell, event, event.deltaY));
        return;
      }
      this._following = false;
      this._viewportTop += event.deltaY < 0 ? -WHEEL_LINES : WHEEL_LINES;
      this._stickFollow();
      this._emit(this._scrollListeners, this._viewportTop);
      this._scheduleRender();
    }, {passive: false});

    this._listen(canvas, 'mousedown', event => {
      textarea.focus();
      const cell = this._cellFromEvent(event);
      if (event.button === 0 && (event.ctrlKey || event.metaKey)) {
        const link = this._linkAt(cell);
        if (link) {
          event.preventDefault();
          const win = doc.defaultView;
          if (win && typeof win.open === 'function') win.open(link, '_blank', 'noopener');
          return;
        }
      }
      if (this.model.modes.mouse !== 'none' && !event.shiftKey) {
        event.preventDefault();
        this._emitData(this._mouseSeq('down', event.button, cell, event));
        this._mouseHeld = {button: event.button};
        this._lastMouseCell = cell;
        return;
      }
      if (event.button !== 0) return;
      event.preventDefault();
      this._selectAnchor = {line: cell.line, col: cell.col};
      if (event.detail >= 2) {
        this._selecting = false;
        this._selectWord(cell.line, cell.col);
        this._scheduleRender();
        return;
      }
      this._selecting = true;
      this._setSelection({start: this._selectAnchor, end: this._selectAnchor});
    });

    const win = doc.defaultView || globalThis;
    this._listen(win, 'mousemove', event => {
      const cell = this._cellFromEvent(event);
      // A local selection owns the whole drag, even if Shift is released
      // before mouseup. In any-motion mode, Shift also suppresses hover reports.
      if (!this._selecting && !event.shiftKey
          && (this._mouseHeld || this.model.modes.mouse === 'any_motion')) {
        const button = this._mouseHeld ? this._mouseHeld.button : 0;
        const last = this._lastMouseCell;
        if (!last || last.col !== cell.col || last.line !== cell.line) {
          this._emitData(this._mouseSeq('move', button, cell, event));
          this._lastMouseCell = cell;
        }
      }
      if (!this._selecting || !this._selectAnchor) return;
      const a = this._selectAnchor;
      const b = {line: cell.line, col: cell.col};
      const backward = b.line < a.line || (b.line === a.line && b.col < a.col);
      this._setSelection(backward
        ? {start: b, end: {line: a.line, col: a.col + 1}}
        : {start: a, end: {line: b.line, col: b.col + 1}});
      this._scheduleRender();
    });
    this._listen(win, 'mouseup', event => {
      if (this._mouseHeld) {
        this._emitData(this._mouseSeq('up', this._mouseHeld.button, this._cellFromEvent(event), event));
        this._mouseHeld = null;
        this._lastMouseCell = null;
      }
      if (this._selecting) {
        this._selecting = false;
        const sel = this._selection;
        if (sel && sel.start.line === sel.end.line && sel.start.col === sel.end.col) {
          this._clearSelection();
        }
      }
    });
  }

  write(data, callback) {
    if (this._disposed) {
      if (typeof callback === 'function') callback();
      return;
    }
    this._pending += decodeChunk(this._decoder, data);
    let nl;
    while ((nl = this._pending.indexOf('\n')) >= 0) {
      const line = this._pending.slice(0, nl);
      this._pending = this._pending.slice(nl + 1);
      let msg;
      try {
        msg = JSON.parse(line);
      } catch {
        this.dropped++;
        if (!this._parseErrorReported) {
          this._parseErrorReported = true;
          this._diagnostic('grid_parse_error', {line_chars: line.length, dropped: this.dropped});
        }
        continue;
      }
      this.model.apply(msg);
      if (msg?.t === 'snapshot' && !this._firstSnapshot) {
        this._firstSnapshot = true;
        this._diagnostic('snapshot_applied', {
          seq: msg.seq, cols: this.model.cols, rows: this.model.rows,
          history_rows: this.model.scrollback.length,
        });
      }
      if (this.model.cols > 0) this._cols = this.model.cols;
      if (this.model.rows > 0) this._rows = this.model.rows;
      if (msg && (msg.t === 'diff' || msg.t === 'snapshot') && msg.title != null) {
        emit(this._titleListeners, String(msg.title));
      }
      // OSC 52：宿主模型解码后的剪贴板文本，交给页面的剪贴板策略。
      if (msg && msg.t === 'clipboard' && typeof msg.text === 'string') {
        emit(this._clipboardListeners, msg.text);
      }
    }
    this._resetBlink();
    this._stickFollow();
    this._scheduleRender(callback);
  }

  resize(cols, rows) {
    cols = cols | 0;
    rows = rows | 0;
    this._cols = cols;
    this._rows = rows;
    this.model.resize(cols, rows);
    this.renderer.measure();
    const cw = this.renderer.cellWidth > 0 ? this.renderer.cellWidth : 1;
    const ch = this.renderer.cellHeight > 0 ? this.renderer.cellHeight : 1;
    this.renderer.fit(cols * cw, rows * ch);
    this._stickFollow();
    this._paint();
    emit(this._resizeListeners, {cols: this.cols, rows: this.rows});
  }

  reset() {
    this._firstSnapshot = false;
    this._firstPaint = false;
    this._parseErrorReported = false;
    const cols = this.cols;
    const rows = this.rows;
    this.model = new GridModel({scrollbackLimit: this._scrollback});
    this.model.apply({
      t: 'snapshot',
      reset: true,
      cols,
      rows,
      grid: [],
      history: [],
      cursor: {x: 0, y: 0, visible: true},
    });
    this._pending = '';
    this._viewportTop = 0;
    this._following = true;
    this._clearSelection();
    this._paint();
  }

  refresh() {
    this.renderer.invalidate();
    this._paint();
  }

  clearTextureAtlas() {}

  loadAddon() {}

  onData(listener) {
    return addListener(this._dataListeners, listener);
  }

  onSelectionChange(listener) {
    return addListener(this._selectionListeners, listener);
  }

  onClipboard(listener) {
    return addListener(this._clipboardListeners, listener);
  }

  onTitleChange(listener) {
    return addListener(this._titleListeners, listener);
  }

  onResize(listener) {
    return addListener(this._resizeListeners, listener);
  }

  onScroll(listener) {
    return addListener(this._scrollListeners, listener);
  }

  attachCustomKeyEventHandler(fn) {
    this._customKeyHandler = fn;
  }

  attachCustomWheelEventHandler(fn) {
    this._customWheelHandler = fn;
  }

  hasSelection() {
    return !!this._orderedSelection();
  }

  getSelection() {
    const range = this._orderedSelection();
    if (!range) return '';
    return this.model.selectionText(range.start, range.end);
  }

  getSelectionPosition() {
    const range = this._orderedSelection();
    if (!range) return undefined;
    return {
      start: {x: range.start.col, y: range.start.line},
      end: {x: range.end.col, y: range.end.line},
    };
  }

  select(column, row, length) {
    column = column | 0;
    row = row | 0;
    length = length | 0;
    if (length <= 0) {
      this._clearSelection();
      return;
    }
    const cols = Math.max(1, this.cols);
    const start = {line: row, col: column};
    let line = row;
    let col = column + length;
    while (col > cols) {
      col -= cols;
      line += 1;
    }
    this._setSelection({start, end: {line, col}});
    this._scheduleRender();
  }

  clearSelection() {
    this._clearSelection();
  }

  selectAll() {
    const n = this.model.lineCount();
    if (n <= 0) {
      this._clearSelection();
      return;
    }
    this._setSelection({
      start: {line: 0, col: 0},
      end: {line: n - 1, col: this.cols},
    });
    this._scheduleRender();
  }

  selectLines(start, end) {
    let a = start | 0;
    let b = end | 0;
    if (a > b) {
      const tmp = a;
      a = b;
      b = tmp;
    }
    const last = Math.max(0, this.model.lineCount() - 1);
    a = Math.max(0, Math.min(last, a));
    b = Math.max(0, Math.min(last, b));
    this._setSelection({start: {line: a, col: 0}, end: {line: b, col: this.cols}});
    this._scheduleRender();
  }

  scrollToBottom() {
    this._following = true;
    const next = this._maxTop();
    const changed = next !== this._viewportTop;
    this._viewportTop = next;
    if (changed) emit(this._scrollListeners, this._viewportTop);
    this._scheduleRender();
  }

  scrollToTop() {
    this._following = false;
    this._setViewportTop(0);
    this._scheduleRender();
  }

  scrollLines(n) {
    this._following = false;
    this._setViewportTop(this._viewportTop + (n | 0));
    this._following = this._viewportTop === this._maxTop();
    this._scheduleRender();
  }

  scrollToLine(n) {
    this._following = false;
    this._setViewportTop(n | 0);
    this._following = this._viewportTop === this._maxTop();
    this._scheduleRender();
  }

  proposeDimensions(widthCss, heightCss) {
    return proposeGridDimensions(this, widthCss, heightCss);
  }

  focus() {
    if (this._keyCapture) this._keyCapture.focus();
    else if (this._textarea && typeof this._textarea.focus === 'function') this._textarea.focus();
  }

  blur() {
    if (this._textarea && typeof this._textarea.blur === 'function') this._textarea.blur();
  }

  dispose() {
    if (this._disposed) return;
    this._disposed = true;
    this._stopBlink();
    if (this._rafId) {
      if (typeof cancelAnimationFrame === 'function') cancelAnimationFrame(this._rafId);
      clearTimeout(this._rafId);
      this._rafId = 0;
    }
    this._renderPending = false;
    if (this._keyCapture) this._keyCapture.dispose();
    this._keyCapture = null;
    for (const drop of this._disposers.splice(0)) {
      try { drop(); } catch { /* already removed */ }
    }
    if (this._canvas && this._canvas.parentNode) this._canvas.parentNode.removeChild(this._canvas);
    if (this._textarea && this._textarea.parentNode) this._textarea.parentNode.removeChild(this._textarea);
    this._canvas = null;
    this._textarea = null;
    this._host = null;
    this.renderer.canvas = null;
  }

  _applyTheme(theme) {
    const mapped = mapTheme(theme);
    if (typeof theme === 'string' && THEMES[theme]) {
      this.renderer.setTheme(theme);
      return;
    }
    this.renderer.setTheme('dark');
    this.renderer.theme = mapped;
    this.renderer.invalidate();
  }

  _maxTop() {
    return Math.max(0, this.model.lineCount() - this.rows);
  }

  _stickFollow() {
    const max = this._maxTop();
    if (this._following) this._viewportTop = max;
    else this._viewportTop = Math.max(0, Math.min(this._viewportTop, max));
    this._following = this._viewportTop === max;
  }

  _setViewportTop(top) {
    const next = Math.max(0, Math.min(top | 0, this._maxTop()));
    if (next === this._viewportTop) {
      this._following = this._viewportTop === this._maxTop();
      return;
    }
    this._viewportTop = next;
    this._following = this._viewportTop === this._maxTop();
    emit(this._scrollListeners, this._viewportTop);
  }

  _diagnostic(event, data) {
    try { this._onDiagnostic?.(event, data); } catch { /* diagnostics cannot interrupt output */ }
  }

  _paint() {
    if (this._disposed) return;
    this._stickFollow();
    this.renderer.render(this.model, {
      viewportTop: this._viewportTop,
      selection: this._orderedSelection(),
      focused: this._focused,
      cursorBlinkOn: this._cursorBlink ? this._blinkOn : true,
    });
    if (this._firstSnapshot && !this._firstPaint && this.renderer.lastPaintedLines > 0
        && this._canvas?.getClientRects().length) {
      const rect = this._canvas.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        this._firstPaint = true;
        this._diagnostic('first_paint', {seq: this.model.seq, width: rect.width, height: rect.height});
      }
    }
  }

  _scheduleRender(callback) {
    if (typeof callback === 'function') this._renderCallbacks.push(callback);
    if (this._renderPending) return;
    this._renderPending = true;
    const run = () => {
      this._renderPending = false;
      this._rafId = 0;
      this._paint();
      const cbs = this._renderCallbacks.splice(0);
      for (const cb of cbs) cb();
    };
    if (typeof requestAnimationFrame === 'function') {
      this._rafId = requestAnimationFrame(run);
    } else {
      this._rafId = setTimeout(run, 0);
    }
  }

  _startBlink() {
    this._stopBlink();
    if (!this._cursorBlink) return;
    this._blinkOn = true;
    this._blinkTimer = setInterval(() => {
      if (!this._focused) return;
      this._blinkOn = !this._blinkOn;
      this._scheduleRender();
    }, BLINK_MS);
  }

  _stopBlink() {
    if (this._blinkTimer) {
      clearInterval(this._blinkTimer);
      this._blinkTimer = 0;
    }
  }

  _resetBlink() {
    this._blinkOn = true;
    if (this._focused) this._startBlink();
  }

  _emitData(str) {
    if (str == null || str === '') return;
    emit(this._dataListeners, str);
  }

  _orderedSelection() {
    if (!this._selection) return null;
    const a = this._selection.start;
    const b = this._selection.end;
    if (!a || !b) return null;
    if (a.line === b.line && a.col === b.col) return null;
    if (a.line < b.line || (a.line === b.line && a.col <= b.col)) return this._selection;
    return {start: b, end: a};
  }

  _setSelection(next) {
    this._selection = next;
    emit(this._selectionListeners);
  }

  _clearSelection() {
    if (!this._selection) return;
    this._selection = null;
    emit(this._selectionListeners);
    this._scheduleRender();
  }

  _cellFromEvent(event) {
    const canvas = this._canvas;
    if (!canvas || typeof canvas.getBoundingClientRect !== 'function') {
      return {line: this._viewportTop, col: 0};
    }
    const rect = canvas.getBoundingClientRect();
    return this.renderer.cellAt(
      event.clientX - rect.left,
      event.clientY - rect.top,
      this._viewportTop,
    );
  }

  _mouseSeq(kind, button, cell, event, wheelDelta) {
    const col = cell.col;
    const row = cell.line - this._viewportTop;
    return this._encoder.mouse(kind, button, col, row, {
      shift: !!event.shiftKey,
      alt: !!event.altKey,
      ctrl: !!event.ctrlKey,
    }, wheelDelta);
  }

  _linkAt(cell) {
    const row = this.model.rowAt(cell.line);
    if (!row) return null;
    let used = 0;
    for (const item of this.model.cellsOf(row)) {
      if (cell.col >= used && cell.col < used + item.width) return item.link || null;
      used += item.width;
    }
    return null;
  }

  _selectWord(line, col) {
    const row = this.model.rowAt(line);
    if (!row) {
      this._setSelection({start: {line, col}, end: {line, col: col + 1}});
      return;
    }
    const cells = this.model.cellsOf(row);
    const texts = [];
    let used = 0;
    for (const cell of cells) {
      texts[used] = cell.text;
      for (let i = 1; i < cell.width; i++) texts[used + i] = '';
      used += cell.width;
    }
    if (!isWordChar(texts[col])) {
      this._setSelection({start: {line, col}, end: {line, col: col + 1}});
      return;
    }
    let start = col;
    let end = col + 1;
    while (start > 0 && isWordChar(texts[start - 1])) start--;
    while (end < texts.length && isWordChar(texts[end])) end++;
    this._setSelection({start: {line, col: start}, end: {line, col: end}});
  }

  _listen(target, type, fn, opts) {
    if (!target || typeof target.addEventListener !== 'function') return;
    target.addEventListener(type, fn, opts);
    this._disposers.push(() => target.removeEventListener(type, fn, opts));
  }
}
