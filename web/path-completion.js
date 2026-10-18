'use strict';

// Shared by both composers. The node resolves cwd; the browser only identifies
// the word at the caret and renders edits. No shell evaluation is involved.
const PathCompletion = {
  states: new WeakMap(),
  nextId: 0,

  token(input) {
    const text = input.value, cursor = input.selectionStart;
    if (cursor !== input.selectionEnd) return null;
    let start = 0, quote = '', decoded = '', prefix = '', end = text.length;
    const windows = /(?:^|\s)[A-Za-z]:\\/.test(text.slice(0, cursor));
    for (let i = 0; i < text.length; i++) {
      const ch = text[i];
      if (i === cursor) prefix = decoded;
      if (!quote && /\s/.test(ch)) {
        if (i >= cursor) { end = i; break; }
        start = i + 1; decoded = ''; continue;
      }
      if (!windows && ch === '\\' && quote !== "'" && quote !== '`'
          && i + 1 < text.length && /[\s\\"'`$]/.test(text[i + 1])) {
        decoded += text[++i]; continue;
      }
      if (ch === quote) { quote = ''; continue; }
      if (!quote && ['"', "'", '`'].includes(ch)) { quote = ch; continue; }
      decoded += ch;
    }
    if (cursor === text.length) prefix = decoded;
    // Leading ! belongs to command mode, not the filename.
    if (/^\s*!/.test(text) && text.slice(0, start).trim() === '' && prefix.startsWith('!')) {
      start++; prefix = prefix.slice(1);
    }
    if (!prefix || /^(?:[a-z][a-z0-9+.-]*:\/\/|www\.)/i.test(prefix)) return null;
    const raw = text.slice(start, end);
    return {start, end, prefix, windows, quote: ['"', "'", '`'].includes(raw[0]) ? raw[0] : ''};
  },

  close(input) {
    const state = this.states.get(input);
    state?.abort.abort(); clearTimeout(state?.timeout); clearTimeout(state?.refresh); state?.box.remove();
    this.states.delete(input);
    input.removeAttribute('aria-controls');
    input.removeAttribute('aria-activedescendant');
    input.setAttribute('aria-expanded', 'false');
  },

  bind(input) {
    if (input.dataset.pathCompletion) return;
    input.dataset.pathCompletion = 'true';
    for (const event of ['blur', 'click']) input.addEventListener(event, () => this.close(input));
    const refresh = event => {
      const state = this.states.get(input);
      if (state) this.open(input, state.getUid, false, event.isComposing || event.type === 'compositionstart');
    };
    for (const event of ['input', 'compositionstart', 'compositionend']) input.addEventListener(event, refresh);
  },

  valid(input, state) {
    return this.states.get(input) === state && input.isConnected
      && state.uid === state.getUid() && input.value === state.text
      && input.selectionStart === state.cursor && input.selectionEnd === state.cursor;
  },

  insert(input, state, entry) {
    if (!this.valid(input, state)) { this.close(input); return; }
    let value = entry.path;
    let quote = state.token.quote;
    if (quote && value.includes(quote)) quote = '"';
    if (!quote && /[\s"'`$;&|()<>{}*?\[\]\\]/.test(value)) quote = '"';
    if (quote === '"') value = value.replace(state.token.windows ? /["$`]/g : /[\\"$`]/g, '\\$&');
    if (quote) value = quote + value + quote;
    input.setRangeText(value, state.token.start, state.token.end, 'end');
    this.close(input); input.focus();
    input.dispatchEvent(new Event('input', {bubbles: true}));
  },

  select(input, state, index) {
    state.active = (index + state.entries.length) % state.entries.length;
    state.buttons.forEach((button, i) => {
      button.classList.toggle('selected', i === state.active);
      button.setAttribute('aria-selected', String(i === state.active));
    });
    const button = state.buttons[state.active];
    input.setAttribute('aria-activedescendant', button.id);
    button.scrollIntoView({block: 'nearest'});
  },

  key(input, event, getUid) {
    this.bind(input);
    // In a composer Tab belongs to completion, including empty/unsupported
    // input and reverse traversal; never hand it to browser focus navigation.
    const tab = event.key === 'Tab';
    if (tab) event.preventDefault();
    if (event.isComposing || event.ctrlKey || event.metaKey || event.altKey
        || ['Shift', 'Control', 'Meta', 'Alt'].includes(event.key)) return tab;
    let state = this.states.get(input);
    if (state && !this.valid(input, state)) { this.close(input); state = null; }
    if (state) {
      if (event.key === 'Escape') { event.preventDefault(); this.close(input); return true; }
      if (['Tab', 'ArrowUp', 'ArrowDown', 'Enter'].includes(event.key)) {
        event.preventDefault();
        if (state.entries.length) {
          if (event.key === 'Enter' || (tab && state.entries.length === 1))
            this.insert(input, state, state.entries[state.active]);
          else this.select(input, state, state.active + (event.shiftKey || event.key === 'ArrowUp' ? -1 : 1));
        } else if (event.key === 'Enter') this.close(input);
        else if (tab && !state.loading) this.open(input, getUid);
        return true;
      }
      // Typing/backspace/delete/paste refresh via input. Cursor navigation has
      // no input event, so dismiss its now unrelated suggestions.
      if (['ArrowLeft', 'ArrowRight', 'Home', 'End', 'PageUp', 'PageDown'].includes(event.key)) this.close(input);
    }
    if (!tab) return false;
    if (!event.shiftKey && !input.disabled) this.open(input, getUid);
    return true;
  },

  open(input, getUid, autoFill = true, composing = false) {
    const uid = getUid();
    const capabilities = HUB_MODE ? Nodes.capabilities[nodeOf(uid)] : SessionDockCapabilities.config;
    const token = this.token(input);
    if (!uid || capabilities?.conversation_path_completion !== true || !token) {
      this.close(input); return;
    }
    const previous = this.states.get(input);
    previous?.abort.abort(); clearTimeout(previous?.timeout); clearTimeout(previous?.refresh);
    // Keep the same popup in place while its query changes.
    const box = previous?.box || el('div', 'input-history');
    if (!previous) {
      box.id = `path-completion-${++this.nextId}`;
      box.setAttribute('aria-label', '路径补全');
      input.parentElement.append(box);
    }
    const head = el('div', 'input-history-head', '路径 · Tab / ↑↓ 选择 · Enter 填入 · Esc 关闭');
    const status = el('div', 'input-history-empty', composing ? '输入完成后更新…' : '正在读取路径…');
    status.setAttribute('role', 'status');
    box.replaceChildren(head, status);
    const state = {uid, getUid, token, text: input.value, cursor: input.selectionStart,
      box, status, autoFill, loading: true, entries: [], buttons: [], active: 0, abort: new AbortController()};
    this.states.set(input, state);
    input.setAttribute('aria-controls', box.id);
    input.removeAttribute('aria-activedescendant');
    input.setAttribute('aria-expanded', 'true');
    if (composing) return;
    if (autoFill) void this.load(input, state);
    else state.refresh = setTimeout(() => { if (this.valid(input, state)) void this.load(input, state); }, 100);
  },

  async load(input, state) {
    state.timeout = setTimeout(() => state.abort.abort(), 8000);
    try {
      const params = new URLSearchParams({uid: state.uid, path: state.token.prefix});
      const response = await fetch(appUrl(`api/session/conversation/complete-path?${params}`),
        {signal: state.abort.signal, cache: 'no-store'});
      const data = await response.json();
      if (!this.valid(input, state)) return;
      if (!response.ok) throw new Error(data.error || '路径补全暂不可用');
      state.entries = data.entries || [];
      if (!state.entries.length) { state.status.textContent = '没有匹配的路径'; return; }
      if (state.autoFill && state.entries.length === 1) { this.insert(input, state, state.entries[0]); return; }
      state.status.remove();
      const list = el('div', 'input-history-list'); list.setAttribute('role', 'listbox');
      list.setAttribute('aria-label', '路径候选');
      state.entries.forEach((entry, index) => {
        const button = el('button', 'input-history-item'); button.type = 'button'; button.tabIndex = -1;
        button.id = `${state.box.id}-${index}`; button.setAttribute('role', 'option');
        button.append(el('span', 'input-history-text', entry.path), el('small', '', entry.directory ? '目录' : '文件'));
        button.onmousedown = event => event.preventDefault();
        button.onclick = () => this.insert(input, state, entry);
        state.buttons.push(button); list.append(button);
      });
      state.box.append(list); this.select(input, state, 0);
    } catch (error) {
      if (this.valid(input, state)) state.status.textContent = error.name === 'AbortError'
        ? '路径读取超时，请按 Esc 后重试' : `路径补全失败：${error.message}`;
    } finally { state.loading = false; clearTimeout(state.timeout); }
  },
};
