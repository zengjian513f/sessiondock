/* Local terminal actions shared by the grid and xterm console renderers. */
export function createTermMenuController(view, elements, state, publish, context) {
  const {host, term} = view;
  const {menu, search, menuButton, query} = elements;
  const {T, copyTermSelection, showConsoleToast, gestures: SessionDockGestures} = context;
  let matches = [], current = -1;
  const active = () => T.views.get(view.name) === view && T.name === view.name && host.isConnected;
  const writable = () => active() && !view.replay && !view.ended && !view.revoked
    && !view.retired && view.ws?.readyState === 1;
  const clearSelection = () => {
    view.selectionLocked = false;
    view.selectionSnapshot = null;
    term.clearSelection();
  };
  const closeMenu = () => { state.menuHidden = true; publish(); };
  let searchEpoch = 0, searching = false;
  let parsedVersion = 0, matchesRevision = null;
  if (!view.grid) {
    // These listeners belong to the terminal's event emitters and are released
    // with that terminal. Parsing can trim scrollback without changing length.
    term.onWriteParsed?.(() => { parsedVersion++; });
    term.onResize?.(() => { parsedVersion++; });
  }
  const revision = () => ({model: view.grid ? term.model : null,
    version: view.grid ? term.model.version : parsedVersion,
    buffer: term.buffer.active, cols: term.cols, rows: term.rows});
  const unchanged = before => {
    if (!before) return false;
    const now = revision();
    return Object.keys(now).every(key => now[key] === before[key]);
  };
  const closeSearch = () => { ++searchEpoch; searching = false; state.searchHidden = true; publish(); clearSelection(); term.focus(); };
  const find = async (direction, rebuild = false, retries = 0) => {
    if (!rebuild && !searching && !unchanged(matchesRevision)) rebuild = true;
    if (rebuild) {
      const epoch = ++searchEpoch;
      const before = revision();
      const valid = () => epoch === searchEpoch && active() && unchanged(before);
      const interrupted = async () => {
        if (epoch !== searchEpoch) return;
        searching = false;
        matches = []; matchesRevision = null; current = -1;
        if (!active()) return;
        clearSelection();
        state.status = '输出已变化，请重试查找'; publish();
        // A short burst may settle quickly. Do not restart indefinitely while
        // output keeps arriving: the next search/navigation gesture retries.
        if (retries < 1) {
          await new Promise(resolve => setTimeout(resolve, 50));
          if (epoch === searchEpoch && active()) return find(direction, true, retries + 1);
        }
      };
      searching = true;
      matchesRevision = null;
      matches = []; current = -1;
      const found = [];
      const needle = query.value;
      if (needle) {
        // Map UTF-16 offsets back to terminal cells, including wide characters
        // and combining sequences. Join soft-wrapped rows before searching.
        let text = '', positions = [];
        const positionAt = offset => {
          let lo = 0, hi = positions.length;
          while (lo + 1 < hi) {
            const mid = (lo + hi) >>> 1;
            if (positions[mid].offset <= offset) lo = mid; else hi = mid;
          }
          const part = positions[lo];
          if (!part) return null;
          let at = part.offset, col = 0;
          for (const cell of part.cells) {
            const value = cell.text || ' ';
            const width = cell.width === 2 ? 2 : 1;
            if (offset < at + value.length) return {row: part.row, col, width};
            at += value.length; col += width;
          }
          return null;
        };
        const scan = () => {
          for (let at = text.indexOf(needle); at !== -1; at = text.indexOf(needle, at + Math.max(1, needle.length))) {
            const start = positionAt(at), end = positionAt(at + needle.length - 1);
            if (start && end) found.push({start, end});
          }
          text = ''; positions = [];
        };
        const buffer = term.buffer.active;
        const rowCount = buffer.length;
        let deadline = performance.now() + 4;
        for (let row = 0; row < rowCount; row++) {
          if (performance.now() >= deadline) {
            await new Promise(resolve => setTimeout(resolve, 0));
            if (!valid()) return interrupted();
            deadline = performance.now() + 4;
          }
          const line = buffer.getLine(row);
          if (!line) continue;
          if (!view.grid && !line.isWrapped) scan();
          const raw = view.grid ? term.model.rowAt(row) : null;
          const cells = raw ? term.model.readCells(raw) : null;
          const rowCells = cells || [];
          positions.push({row, offset: text.length, cells: rowCells});
          const add = (value, width) => {
            if (!cells) rowCells.push({text: value, width});
            text += value;
          };
          if (cells) {
            for (const cell of cells) add(cell.text || ' ', cell.width === 2 ? 2 : 1);
            if (!line.isWrapped) scan();
          } else {
            for (let x = 0; x < term.cols; x++) {
              const cell = line.getCell(x), width = cell?.getWidth();
              if (!width) continue;
              add(cell.getChars() || ' ', width);
            }
          }
        }
        scan();
      }
      if (!valid()) return interrupted();
      matches = found;
      matchesRevision = before;
      searching = false;
    }
    if (searching) return;
    clearSelection();
    if (!matches.length) { state.status = query.value ? '无匹配' : ''; publish(); return; }
    current = (current + direction + matches.length) % matches.length;
    const {start, end} = matches[current];
    term.select(start.col, start.row, (end.row - start.row) * term.cols + end.col + end.width - start.col);
    term.scrollToLine(start.row);
    state.status = `${current + 1}/${matches.length}`; publish();
  };
  const paste = async () => {
    closeMenu();
    if (!writable()) return;
    try {
      const text = await navigator.clipboard.readText();
      if (!writable()) return;
      clearSelection(); term.focus();
      const data = new DataTransfer(); data.setData('text/plain', text);
      term.textarea.dispatchEvent(new ClipboardEvent('paste', {clipboardData:data, bubbles:true, cancelable:true}));
    } catch { showConsoleToast('无法读取剪贴板，请聚焦终端后按 Ctrl+V（macOS 使用 ⌘V）。'); }
  };
  const showSearch = () => {
    closeMenu(); if (!active()) return;
    state.searchHidden = false; publish(); query.focus(); query.select(); find(1, true);
  };
  const searchKeydown = event => {
    if (event.key === 'Escape') { event.preventDefault(); closeSearch(); }
    event.stopPropagation();
  };
  const openMenu = (x, y) => {
    if (!active()) return;
    state.pasteDisabled = !writable();
    state.menuHidden = false; publish();
    const box = host.getBoundingClientRect();
    const scaleX = box.width / host.offsetWidth || 1;
    const scaleY = box.height / host.offsetHeight || 1;
    state.left = `${Math.max(0, Math.min((x - box.left) / scaleX, host.clientWidth - menu.offsetWidth))}px`;
    state.top = `${Math.max(0, Math.min((y - box.top) / scaleY, host.clientHeight - menu.offsetHeight))}px`;
    publish(); menu.querySelector('button:not(:disabled)')?.focus();
  };
  host.addEventListener('sessiondock-pinch-start', closeMenu);
  host.addEventListener('contextmenu', event => {
    if (search.contains(event.target)) return;
    event.preventDefault(); event.stopPropagation();
    // Mobile long press belongs to text selection, never to the context menu.
    if (SessionDockGestures.pinching || SessionDockGestures.isTouchEvent(event)) return;
    openMenu(event.clientX, event.clientY);
  });
  const toggleMenu = () => {
    if (!state.menuHidden) { closeMenu(); return; }
    const box = menuButton.getBoundingClientRect();
    openMenu(box.left, box.bottom);
  };
  const menuKeydown = event => {
    const buttons = [...menu.querySelectorAll('button:not(:disabled)')];
    if (event.key === 'Escape') { closeMenu(); term.focus(); }
    else if (['ArrowDown', 'ArrowUp'].includes(event.key)) {
      const step = event.key === 'ArrowDown' ? 1 : -1;
      buttons[(buttons.indexOf(document.activeElement) + step + buttons.length) % buttons.length]?.focus();
    } else return;
    event.preventDefault(); event.stopPropagation();
  };
  host.addEventListener('mousedown', event => {
    if (menu.contains(event.target) || search.contains(event.target) || menuButton.contains(event.target)) {
      event.stopImmediatePropagation(); return;
    }
    if (!menu.contains(event.target)) closeMenu();
    if (event.button === 2 && !search.contains(event.target)) {
      event.preventDefault(); event.stopImmediatePropagation();
    }
  }, true);
  host.addEventListener('mouseup', event => {
    if (event.button === 2 && !search.contains(event.target)) {
      event.preventDefault(); event.stopImmediatePropagation();
    }
  }, true);
  installTermTouchSelection(view, () => { closeMenu(); clearSelection(); }, context);
  host.addEventListener('focusout', event => {
    if (!host.contains(event.relatedTarget)) closeMenu();
  });
  return {paste, showSearch, toggleMenu, searchKeydown, menuKeydown, closeSearch,
    input: () => find(1, true), previous: () => find(-1), next: () => find(1)};
}

// Canvas text has no native browser selection. Long press starts a local
// selection for either renderer; a swipe before the hold remains scrolling.
function installTermTouchSelection(view, begin, context) {
  const {T, copyTermSelection, gestures: SessionDockGestures} = context;
  const {host, term} = view;
  let gesture = null, suppressClickUntil = 0;
  const screen = () => host.querySelector(view.grid ? '.grid-canvas' : '.xterm-screen');
  const cellAt = touch => {
    const rect = screen().getBoundingClientRect();
    const col = Math.max(0, Math.min(term.cols - 1, Math.floor((touch.clientX - rect.left) * term.cols / rect.width)));
    const y = Math.max(0, Math.min(term.rows - 1, Math.floor((touch.clientY - rect.top) * term.rows / rect.height)));
    const row = Math.min(term.buffer.active.length - 1, term.buffer.active.viewportY + y);
    // Snap wide-character spacer cells back to the character's first column.
    if (view.grid) {
      let x = 0;
      for (const cell of term.model.cellsOf(term.model.rowAt(row))) {
        const width = cell.width === 2 ? 2 : 1;
        if (col < x + width) return {row, col: x, width};
        x += width;
      }
    } else {
      const line = term.buffer.active.getLine(row);
      const x = line?.getCell(col)?.getWidth() === 0 ? Math.max(0, col - 1) : col;
      return {row, col: x, width: line?.getCell(x)?.getWidth() || 1};
    }
    return {row, col, width: 1};
  };
  const select = touch => {
    const anchor = gesture.anchor, cell = cellAt(touch);
    const before = cell.row < anchor.row || (cell.row === anchor.row && cell.col < anchor.col);
    const start = before ? cell : anchor, end = before ? anchor : cell;
    term.select(start.col, start.row, (end.row - start.row) * term.cols + end.col + end.width - start.col);
  };
  const finish = (event, copy) => {
    if (!gesture) return;
    clearTimeout(gesture.timer);
    if (gesture.selecting) {
      if (event?.cancelable) event.preventDefault();
      event?.stopImmediatePropagation();
      suppressClickUntil = Date.now() + 1000;
      view.selectionLocked = false;
      view.selectionSnapshot = null;
      if (copy) copyTermSelection(term);
      term.clearSelection();
    }
    gesture = null;
  };
  host.addEventListener('sessiondock-pinch-start', () => finish(null, false));
  host.addEventListener('touchstart', event => {
    if (SessionDockGestures.pinching || event.touches.length !== 1) { finish(event, false); return; }
    if (!screen()?.contains(event.target)) return;
    const touch = event.touches[0];
    gesture = {id: touch.identifier, x: touch.clientX, y: touch.clientY,
      anchor: cellAt(touch), selecting: false};
    const pending = gesture;
    const startSelection = () => {
      if (gesture !== pending || !host.isConnected || T.name !== view.name || SessionDockGestures.pinching) return;
      begin();
      gesture.selecting = true;
      view.selectionLocked = true;
      select(touch);
    };
    if (T.shiftSelect) {
      startSelection();
      event.preventDefault();
      event.stopImmediatePropagation();
    } else pending.timer = setTimeout(startSelection, 450);
  }, {passive: false, capture: true});
  host.addEventListener('touchmove', event => {
    if (!gesture) return;
    if (SessionDockGestures.pinching || event.touches.length !== 1) { finish(event, false); return; }
    const touch = [...event.touches].find(item => item.identifier === gesture.id);
    if (!touch) { finish(event, false); return; }
    if (gesture.selecting) {
      event.preventDefault(); event.stopImmediatePropagation(); select(touch);
    } else if (Math.hypot(touch.clientX - gesture.x, touch.clientY - gesture.y) > 8) {
      clearTimeout(gesture.timer); gesture = null;
    }
  }, {passive: false, capture: true});
  host.addEventListener('touchend', event => finish(event, !SessionDockGestures.pinching && event.touches.length === 0), {passive: false, capture: true});
  host.addEventListener('touchcancel', event => finish(event, false), {passive: false, capture: true});
  host.addEventListener('click', event => {
    if (Date.now() < suppressClickUntil && SessionDockGestures.isTouchEvent(event) && screen()?.contains(event.target)) {
      event.preventDefault(); event.stopImmediatePropagation();
    }
  }, true);
}
