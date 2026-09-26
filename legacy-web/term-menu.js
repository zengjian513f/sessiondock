/* Local terminal actions shared by the grid and xterm console renderers. */
function installTermMenu(view) {
  const {host, term} = view;
  const menu = document.createElement('div');
  menu.className = 'ctx-menu term-context-menu';
  menu.setAttribute('role', 'menu');
  menu.hidden = true;
  const search = document.createElement('form');
  search.className = 'term-find';
  search.hidden = true;
  search.innerHTML = '<input aria-label="查找终端输出" placeholder="查找已加载输出" type="search">'
    + '<span role="status"></span><button type="button" aria-label="上一个">↑</button>'
    + '<button type="submit" aria-label="下一个">↓</button>'
    + '<button type="button" aria-label="关闭查找">×</button>';
  host.append(menu, search);
  const query = search.querySelector('input');
  const status = search.querySelector('[role="status"]');
  let matches = [], current = -1;
  const active = () => T.views.get(view.name) === view && T.name === view.name && host.isConnected;
  const writable = () => active() && !view.replay && !view.ended && !view.revoked
    && !view.retired && view.ws?.readyState === 1;
  const clearSelection = () => {
    view.selectionLocked = false;
    view.selectionSnapshot = null;
    term.clearSelection();
  };
  const closeMenu = () => { menu.hidden = true; };
  const closeSearch = () => { search.hidden = true; clearSelection(); term.focus(); };
  const find = (direction, rebuild = false) => {
    if (rebuild) {
      matches = []; current = -1;
      const needle = query.value;
      if (needle) {
        // Map UTF-16 offsets back to terminal cells, including wide characters
        // and combining sequences. Join soft-wrapped rows before searching.
        let text = '', positions = [];
        const scan = () => {
          for (let at = text.indexOf(needle); at !== -1; at = text.indexOf(needle, at + Math.max(1, needle.length))) {
            const start = positions[at], end = positions[at + needle.length - 1];
            if (start && end) matches.push({start, end});
          }
          text = ''; positions = [];
        };
        const buffer = term.buffer.active;
        for (let row = 0; row < buffer.length; row++) {
          const line = buffer.getLine(row);
          if (!line) continue;
          if (!view.grid && !line.isWrapped) scan();
          const cells = view.grid ? term.model.cellsOf(term.model.rowAt(row)) : null;
          let col = 0;
          const add = (value, width) => {
            text += value;
            for (let i = 0; i < value.length; i++) positions.push({row, col, width});
            col += width;
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
    }
    clearSelection();
    if (!matches.length) { status.textContent = query.value ? '无匹配' : ''; return; }
    current = (current + direction + matches.length) % matches.length;
    const {start, end} = matches[current];
    term.select(start.col, start.row, (end.row - start.row) * term.cols + end.col + end.width - start.col);
    term.scrollToLine(start.row);
    status.textContent = `${current + 1}/${matches.length}`;
  };
  query.addEventListener('input', () => find(1, true));
  search.addEventListener('submit', event => { event.preventDefault(); find(1); });
  search.querySelector('[aria-label="上一个"]').onclick = () => find(-1);
  search.querySelector('[aria-label="关闭查找"]').onclick = closeSearch;
  search.addEventListener('keydown', event => {
    if (event.key === 'Escape') { event.preventDefault(); closeSearch(); }
    event.stopPropagation();
  });
  // UI gestures must never become terminal selection or remote mouse input.
  for (const panel of [menu, search]) {
    for (const type of ['mousedown', 'mouseup', 'click', 'contextmenu']) {
      panel.addEventListener(type, event => event.stopPropagation());
    }
  }
  for (const [label, action] of [
    ['粘贴', async () => {
      if (!writable()) return;
      try {
        const text = await navigator.clipboard.readText();
        if (!writable()) return;
        clearSelection();
        term.focus();
        const data = new DataTransfer();
        data.setData('text/plain', text);
        term.textarea.dispatchEvent(new ClipboardEvent('paste', {clipboardData: data, bubbles: true, cancelable: true}));
      } catch { showConsoleToast('无法读取剪贴板，请聚焦终端后按 Ctrl+V（macOS 使用 ⌘V）。'); }
    }],
    ['复制全部', () => {
      clearSelection(); term.selectAll(); copyTermSelection(term); clearSelection(); term.focus();
    }],
    ['查找', () => { search.hidden = false; query.focus(); query.select(); find(1, true); }],
  ]) {
    const button = document.createElement('button');
    button.type = 'button'; button.textContent = label; button.setAttribute('role', 'menuitem');
    button.onclick = () => { closeMenu(); if (active()) action(); };
    menu.appendChild(button);
  }
  host.addEventListener('contextmenu', event => {
    if (search.contains(event.target)) return;
    event.preventDefault(); event.stopPropagation();
    if (!active()) return;
    menu.firstElementChild.disabled = !writable();
    menu.hidden = false;
    const box = host.getBoundingClientRect();
    menu.style.left = `${Math.max(0, Math.min(event.clientX - box.left, box.width - menu.offsetWidth))}px`;
    menu.style.top = `${Math.max(0, Math.min(event.clientY - box.top, box.height - menu.offsetHeight))}px`;
    menu.querySelector('button:not(:disabled)')?.focus();
  });
  menu.addEventListener('keydown', event => {
    const buttons = [...menu.querySelectorAll('button:not(:disabled)')];
    if (event.key === 'Escape') { closeMenu(); term.focus(); }
    else if (['ArrowDown', 'ArrowUp'].includes(event.key)) {
      const step = event.key === 'ArrowDown' ? 1 : -1;
      buttons[(buttons.indexOf(document.activeElement) + step + buttons.length) % buttons.length]?.focus();
    } else return;
    event.preventDefault(); event.stopPropagation();
  });
  host.addEventListener('mousedown', event => {
    if (menu.contains(event.target) || search.contains(event.target)) {
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
  host.addEventListener('focusout', event => {
    if (!host.contains(event.relatedTarget)) closeMenu();
  });
}
