/** Grid terminal input: bytes xterm.js would send, plus a hidden-textarea capture helper. */

const MODIFIER_ONLY = new Set([
  'Shift', 'Control', 'Alt', 'Meta', 'CapsLock', 'NumLock', 'ScrollLock', 'Dead', 'Process',
]);

const ARROW_FINAL = {ArrowUp: 'A', ArrowDown: 'B', ArrowRight: 'C', ArrowLeft: 'D'};
const HOME_END_FINAL = {Home: 'H', End: 'F'};
const TILDE_NUM = {Insert: 2, Delete: 3, PageUp: 5, PageDown: 6};
const FKEY_SS3 = {F1: 'P', F2: 'Q', F3: 'R', F4: 'S'};
const FKEY_TILDE = {F5: 15, F6: 17, F7: 18, F8: 19, F9: 20, F10: 21, F11: 23, F12: 24};

function modifierParam(event) {
  return 1
    + (event.shiftKey ? 1 : 0)
    + (event.altKey ? 2 : 0)
    + (event.ctrlKey ? 4 : 0)
    + (event.metaKey ? 8 : 0);
}

function isPrintableKey(key) {
  if (key.length === 1) return true;
  // Named keys are identifiers (ArrowUp, F1, Unidentified). Emoji / other
  // graphemes are not.
  return key.length > 1 && !/^[A-Za-z]/.test(key);
}

function ctrlChar(key) {
  if (key.length !== 1) return null;
  const upper = key.toUpperCase();
  if (upper >= 'A' && upper <= 'Z') return String.fromCharCode(upper.charCodeAt(0) & 31);
  switch (key) {
    case '@': case '2': case ' ': return '\x00';
    case '[': case '3': return '\x1b';
    case '\\': case '4': return '\x1c';
    case ']': case '5': return '\x1d';
    case '^': case '6': return '\x1e';
    case '_': case '7': case '-': return '\x1f';
    case '?': case '8': return '\x7f';
    default: return null;
  }
}

function csiTilde(number, m) {
  return m > 1 ? `\x1b[${number};${m}~` : `\x1b[${number}~`;
}

function cursorSeq(final, appCursor, m) {
  if (m > 1) return `\x1b[1;${m}${final}`;
  return appCursor ? `\x1bO${final}` : `\x1b[${final}`;
}

function utf8Coord(oneBased) {
  const value = 32 + oneBased;
  return oneBased > 95 ? String.fromCodePoint(value) : String.fromCharCode(value);
}

export class InputEncoder {
  constructor(getModes) {
    this._getModes = getModes;
  }

  key(event) {
    const key = event.key ?? '';
    if (event.isComposing) return null;
    if (MODIFIER_ONLY.has(key)) return null;

    const m = modifierParam(event);
    const appCursor = !!this._getModes().app_cursor;

    if (key === 'Enter') return '\r';
    if (key === 'Backspace') {
      if (event.ctrlKey) return '\x08';
      if (event.altKey) return '\x1b\x7f';
      return '\x7f';
    }
    if (key === 'Tab') return event.shiftKey ? '\x1b[Z' : '\t';
    if (key === 'Escape') return '\x1b';

    const arrow = ARROW_FINAL[key];
    if (arrow) return cursorSeq(arrow, appCursor, m);

    const homeEnd = HOME_END_FINAL[key];
    if (homeEnd) return cursorSeq(homeEnd, appCursor, m);

    const tilde = TILDE_NUM[key];
    if (tilde !== undefined) return csiTilde(tilde, m);

    const fss3 = FKEY_SS3[key];
    if (fss3) return m > 1 ? `\x1b[1;${m}${fss3}` : `\x1bO${fss3}`;
    const ftilde = FKEY_TILDE[key];
    if (ftilde) return csiTilde(ftilde, m);

    if (event.ctrlKey && !event.altKey) return ctrlChar(key);

    if (event.altKey && !event.ctrlKey && isPrintableKey(key)) return '\x1b' + key;

    if (event.metaKey) return null;
    if (isPrintableKey(key)) return key;
    return null;
  }

  paste(text) {
    const normalized = String(text ?? '').replaceAll('\r\n', '\r').replaceAll('\n', '\r');
    return this._getModes().bracketed_paste
      ? `\x1b[200~${normalized}\x1b[201~`
      : normalized;
  }

  focus(inside) {
    if (!this._getModes().focus_events) return null;
    return inside ? '\x1b[I' : '\x1b[O';
  }

  mouse(kind, button, col, row, mods = {}, wheelDelta) {
    const modes = this._getModes();
    const mouse = modes.mouse;
    if (mouse === 'none') return null;
    if (kind === 'up' && mouse === 'press') return null;
    if (kind === 'move') {
      const held = button === 0 || button === 1 || button === 2;
      if (!(mouse === 'any_motion' || (mouse === 'button_motion' && held))) return null;
    }

    const extra = (mods.shift ? 4 : 0) + (mods.alt ? 8 : 0) + (mods.ctrl ? 16 : 0);
    let cb;
    let reportKind = kind;
    if (kind === 'wheel') {
      cb = 64 + (wheelDelta < 0 ? 0 : 1) + extra;
      reportKind = 'down';
    } else {
      cb = button + extra + (kind === 'move' ? 32 : 0);
    }

    const x = col + 1;
    const y = row + 1;
    if (modes.mouse_encoding === 'sgr') {
      return `\x1b[<${cb};${x};${y}${reportKind === 'up' ? 'm' : 'M'}`;
    }

    // X10 / default: release is always button 3; modifiers still apply.
    const encoded = reportKind === 'up' ? 3 + extra : cb;
    const btn = String.fromCharCode(32 + encoded);
    if (modes.mouse_encoding === 'utf8') {
      return `\x1b[M${btn}${utf8Coord(x)}${utf8Coord(y)}`;
    }
    return `\x1b[M${btn}${String.fromCharCode(32 + Math.min(x, 223))}${String.fromCharCode(32 + Math.min(y, 223))}`;
  }

  wheelAsArrows(deltaY, lines = 3) {
    const modes = this._getModes();
    if (!modes.alt || modes.mouse !== 'none') return null;
    const final = deltaY < 0 ? 'A' : 'B';
    const seq = modes.app_cursor ? `\x1bO${final}` : `\x1b[${final}`;
    return seq.repeat(lines);
  }
}

export class KeyCapture {
  constructor(textarea, {onBytes, onPaste, encoder}) {
    this._textarea = textarea;
    this._onBytes = onBytes;
    this._onPaste = onPaste;
    this._encoder = encoder;
    this._composing = false;
    this._skipInput = false;
    this._disposed = false;

    this._onKeyDown = event => {
      const bytes = this._encoder.key(event);
      if (bytes != null) {
        event.preventDefault();
        this._onBytes(bytes);
      }
    };
    this._onCompositionStart = () => {
      this._composing = true;
    };
    this._onCompositionEnd = event => {
      this._composing = false;
      this._skipInput = true;
      const text = event.data || textarea.value;
      textarea.value = '';
      if (text) this._onBytes(text);
    };
    this._onInput = event => {
      if (this._skipInput) {
        this._skipInput = false;
        return;
      }
      if (this._composing || event.isComposing) return;
      const text = event.data != null && event.data !== '' ? event.data : textarea.value;
      textarea.value = '';
      if (text) this._onBytes(text);
    };
    this._onPasteEvent = event => {
      event.preventDefault();
      const data = event.clipboardData;
      this._onPaste(data ? data.getData('text/plain') : '');
    };

    textarea.addEventListener('keydown', this._onKeyDown);
    textarea.addEventListener('compositionstart', this._onCompositionStart);
    textarea.addEventListener('compositionend', this._onCompositionEnd);
    textarea.addEventListener('input', this._onInput);
    textarea.addEventListener('paste', this._onPasteEvent);
  }

  focus() {
    this._textarea.focus();
  }

  dispose() {
    if (this._disposed) return;
    this._disposed = true;
    const el = this._textarea;
    el.removeEventListener('keydown', this._onKeyDown);
    el.removeEventListener('compositionstart', this._onCompositionStart);
    el.removeEventListener('compositionend', this._onCompositionEnd);
    el.removeEventListener('input', this._onInput);
    el.removeEventListener('paste', this._onPasteEvent);
  }
}
