'use strict';
(() => {
  const $ = id => document.getElementById(id);
  const base = new URL('.', location.href);
  const node = new URLSearchParams(location.search).get('node');
  const embedded = new URLSearchParams(location.search).get('embedded') === '1' || window.self !== window.top;
  // Hub pages reach a node through /api/nodes/{nid}/api/{*path}.
  const prefix = node ? ['api', 'nodes', encodeURIComponent(node), 'api', ''].join('/') : 'api/';
  const fitKey = SessionDockCapabilities.namespace + 'records-fit';
  const GAP_NOTE = '（录制有缺口，已从下一个快照继续）';
  const state = {id: '', status: '', live: false, ended: false, gaps: 0, bytes: 0};

  let records = [];
  let term = null, fitAddon = null, socket = null;
  let generation = 0, reconnectTimer = 0, reconnectDelay = 1000, leaving = false;
  let fitOn = false, recordedCols = 0, recordedRows = 0;
  let loading = false;

  globalThis.__records = {term: () => term, socket: () => socket, state};

  $('machine').textContent = node || location.hostname;
  // 嵌在主页面对话框里时外层已有标题栏，省掉自己的。
  if (embedded) document.body.classList.add('embedded');

  if (SessionDockCapabilities.config.terminal_records === false
      || SessionDockCapabilities.config.terminal === false) {
    setStatus('此机器未启用终端录制');
    return;
  }

  try { fitOn = localStorage.getItem(fitKey) === 'true'; } catch {}
  $('fit').setAttribute('aria-pressed', String(fitOn));
  $('xterm').classList.toggle('fit', fitOn);
  $('empty').hidden = false;

  function apiURL(path, query) {
    const url = new URL(prefix + path, base);
    if (query) for (const [key, value] of Object.entries(query)) url.searchParams.set(key, value);
    return url;
  }
  function attachURL(id) {
    const url = apiURL('term/records/attach', {id});
    url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    return url.href;
  }
  function setStatus(message, error = false) {
    state.status = message;
    $('status').textContent = message;
    $('status').className = error ? 'error' : '';
  }
  function appendStatus(note) {
    if (!state.status.includes(note)) setStatus(state.status + note);
  }
  function sizeText(bytes) {
    const n = Number(bytes) || 0;
    if (n >= 1024 * 1024) return (n / (1024 * 1024)).toFixed(1) + ' MiB';
    return (n / 1024).toFixed(1) + ' KiB';
  }
  function element(tag, text, className) {
    const node = document.createElement(tag);
    if (text !== undefined && text !== null) node.textContent = text;
    if (className) node.className = className;
    return node;
  }
  function termTheme() {
    const dark = document.documentElement.dataset.theme === 'dark';
    return dark
      ? {background:'#000000', foreground:'#9da5b0', cursor:'#9da5b0', selectionBackground:'#264f78'}
      : {background:'#f4f6f8', foreground:'#252a32', cursor:'#252a32', selectionBackground:'#c8d6ea'};
  }
  function atBottom() {
    if (!term) return true;
    const buf = term.buffer.active;
    return buf.viewportY + term.rows >= buf.length;
  }
  function visibleRecords() {
    const liveOnly = $('live-only').checked;
    return records.filter(row => !liveOnly || row.live);
  }
  function syncURL(id) {
    const url = new URL(location.href);
    if (id) url.searchParams.set('id', id);
    else url.searchParams.delete('id');
    history.replaceState(history.state, '', url);
  }
  function highlight() {
    const list = $('records');
    let active = null;
    for (const item of list.children) {
      const on = item.dataset.id === state.id;
      item.setAttribute('aria-selected', String(on));
      if (on) active = item;
    }
    if (active) list.setAttribute('aria-activedescendant', active.id);
    else list.removeAttribute('aria-activedescendant');
  }
  function renderList() {
    const list = $('records');
    list.replaceChildren();
    for (const row of visibleRecords()) {
      const item = element('li');
      item.role = 'option';
      item.dataset.id = row.id;
      item.id = 'rec-' + row.id;
      item.setAttribute('aria-selected', 'false');
      const heading = element('div', undefined, 'heading');
      heading.append(element('span', row.name || row.id, 'name'));
      heading.append(element('span', row.live ? '进行中' : '已结束', 'badge ' + (row.live ? 'live' : 'ended')));
      item.append(heading);
      const created = row.created_ms ? new Date(row.created_ms).toLocaleString() : '';
      const size = sizeText(row.bytes);
      const dim = `${row.cols || 0}×${row.rows || 0}`;
      item.append(element('div', [created, size, dim].filter(Boolean).join(' · '), 'meta'));
      if (row.cwd) item.append(element('div', row.cwd, 'cwd'));
      // 同一模型的网格回放：新页面打开，不影响这里的 xterm 回放。
      const gridLink = element('a', '网格回放', 'grid-link');
      const gridUrl = new URL('grid.html', base);
      gridUrl.searchParams.set('record', row.id);
      if (node) gridUrl.searchParams.set('node', node);
      if (embedded) gridUrl.searchParams.set('embedded', '1');
      gridLink.href = gridUrl.href;
      // 嵌在应用内对话框时在本框架内导航（新标签在 PWA 里看不到）；网格页有"返回"。
      gridLink.target = embedded ? '_self' : '_blank';
      gridLink.rel = 'noopener';
      gridLink.addEventListener('click', event => event.stopPropagation());
      item.append(gridLink);
      list.append(item);
    }
    highlight();
  }
  async function loadList() {
    if (loading) return;
    loading = true;
    try {
      const response = await fetch(apiURL('term/records'), {cache: 'no-store'});
      if (!(response.headers.get('Content-Type') || '').includes('application/json'))
        throw new Error('无法读取响应，请确认登录状态后刷新');
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || `请求失败（${response.status}）`);
      records = Array.isArray(result.records) ? result.records : [];
      renderList();
      if (!state.id && !$('status').textContent) setStatus(records.length ? '' : '没有录制');
    } catch (error) {
      if (!state.id) setStatus(error.message, true);
    } finally {
      loading = false;
    }
  }

  function closeSocket() {
    if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = 0; }
    generation += 1;
    if (socket) {
      const ws = socket;
      socket = null;
      try { ws.close(); } catch {}
    }
  }
  // reset() keeps the old scrollback; dispose and recreate when switching recordings.
  function destroyTerm() {
    if (term) {
      try { term.dispose(); } catch {}
      term = null;
      fitAddon = null;
    }
    $('xterm').replaceChildren();
    $('xterm').classList.toggle('fit', fitOn);
  }
  function createTerm() {
    destroyTerm();
    term = new Terminal({
      allowProposedApi: true,
      disableStdin: true,
      cursorBlink: false,
      scrollback: 100000,
      fontFamily: 'UbuntuSansMono, Consolas, monospace',
      fontSize: 14,
      theme: termTheme(),
    });
    fitAddon = new FitAddon.FitAddon();
    term.loadAddon(fitAddon);
    try {
      if (globalThis.Unicode11Addon?.Unicode11Addon) {
        term.loadAddon(new Unicode11Addon.Unicode11Addon());
        term.unicode.activeVersion = '11';
      }
    } catch {}
    term.open($('xterm'));
    if (fitOn) fitAddon.fit();
  }
  function applyRecordedSize() {
    if (!term || !recordedCols || !recordedRows) return;
    if (fitOn) fitAddon?.fit();
    else term.resize(recordedCols, recordedRows);
  }
  function handleFrame(msg) {
    if (!msg || !term) return;
    if (msg.t === 'record') {
      recordedCols = msg.cols || recordedCols;
      recordedRows = msg.rows || recordedRows;
      term.reset();
      applyRecordedSize();
      state.bytes = 0;
      state.live = !!msg.live;
      state.ended = !state.live;
      reconnectDelay = 1000;
      $('size').textContent = recordedCols && recordedRows ? `${recordedCols}×${recordedRows}` : '';
      setStatus(state.live ? '进行中 · 实时跟随' : '已结束');
      return;
    }
    if (msg.t === 'resize') {
      recordedCols = msg.cols || recordedCols;
      recordedRows = msg.rows || recordedRows;
      $('size').textContent = recordedCols && recordedRows ? `${recordedCols}×${recordedRows}` : '';
      if (!fitOn) term.resize(recordedCols, recordedRows);
      return;
    }
    if (msg.t === 'gap') {
      state.gaps += 1;
      term.reset();
      appendStatus(GAP_NOTE);
      return;
    }
    if (msg.t === 'exit') {
      const exit = msg.exit || {};
      state.live = false;
      state.ended = true;
      let text = `已结束 · 退出码 ${exit.code}`;
      if (exit.output_complete === false) text += ' · 输出不完整';
      if (state.gaps) text += GAP_NOTE;
      setStatus(text);
      return;
    }
    if (msg.t === 'end') {
      state.live = false;
      state.ended = true;
      appendStatus(' · 回放完成');
    }
  }
  function writeBytes(data) {
    if (!term) return;
    const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
    if (!bytes.byteLength) return;
    const follow = !state.live || atBottom();
    state.bytes += bytes.byteLength;
    term.write(bytes, () => { if (follow && term) term.scrollToBottom(); });
  }
  function scheduleReconnect(id, event) {
    const delay = reconnectDelay;
    reconnectDelay = Math.min(reconnectDelay * 2, 30000);
    const reason = event.reason || String(event.code);
    setStatus(`连接断开：${reason}，${delay / 1000}s 后重试`);
    reconnectTimer = setTimeout(() => {
      reconnectTimer = 0;
      if (leaving || document.visibilityState !== 'visible') return;
      if (state.id !== id || !state.live) return;
      connect(id);
    }, delay);
  }
  function connect(id) {
    closeSocket();
    const gen = generation;
    const ws = new WebSocket(attachURL(id));
    ws.binaryType = 'arraybuffer';
    socket = ws;
    ws.addEventListener('message', event => {
      if (gen !== generation) return;
      if (typeof event.data === 'string') {
        try { handleFrame(JSON.parse(event.data)); } catch {}
        return;
      }
      writeBytes(event.data);
    });
    ws.addEventListener('close', event => {
      if (gen !== generation) return;
      if (socket === ws) socket = null;
      if (leaving || event.code === 1000 || !state.live) return;
      if (document.visibilityState !== 'visible') return;
      scheduleReconnect(id, event);
    });
  }
  function openRecord(id) {
    if (!id) return;
    const switching = state.id !== id;
    state.id = id;
    if (switching) {
      state.status = '';
      state.live = false;
      state.ended = false;
      state.gaps = 0;
      state.bytes = 0;
      recordedCols = 0;
      recordedRows = 0;
      reconnectDelay = 1000;
      $('size').textContent = '';
      setStatus('正在连接…');
      createTerm();
    }
    $('empty').hidden = true;
    syncURL(id);
    highlight();
    if (!switching && socket && socket.readyState <= WebSocket.OPEN) return;
    connect(id);
  }

  $('records').addEventListener('click', event => {
    const item = event.target.closest('[role=option]');
    if (item) openRecord(item.dataset.id);
  });
  $('records').addEventListener('keydown', event => {
    const items = [...$('records').children];
    if (!items.length) return;
    const current = items.findIndex(item => item.dataset.id === state.id);
    const go = index => {
      event.preventDefault();
      const next = items[Math.max(0, Math.min(items.length - 1, index))];
      if (next) openRecord(next.dataset.id);
    };
    if (event.key === 'ArrowDown') go(current < 0 ? 0 : current + 1);
    else if (event.key === 'ArrowUp') go(current < 0 ? 0 : current - 1);
    else if (event.key === 'Home') go(0);
    else if (event.key === 'End') go(items.length - 1);
    else if (event.key === 'Enter' || event.key === ' ') go(current < 0 ? 0 : current);
  });
  $('refresh').addEventListener('click', () => loadList());
  $('live-only').addEventListener('change', () => renderList());
  $('fit').addEventListener('click', () => {
    fitOn = !fitOn;
    $('fit').setAttribute('aria-pressed', String(fitOn));
    $('xterm').classList.toggle('fit', fitOn);
    try { localStorage.setItem(fitKey, String(fitOn)); } catch {}
    applyRecordedSize();
  });
  $('copy').addEventListener('click', () => {
    if (!term) return;
    const text = term.getSelection();
    if (!text) return;
    navigator.clipboard.writeText(text);
  });
  $('bottom').addEventListener('click', () => { term?.scrollToBottom(); });
  addEventListener('resize', () => { if (fitOn) fitAddon?.fit(); });
  addEventListener('pagehide', () => {
    leaving = true;
    closeSocket();
  });
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') loadList();
  });
  setInterval(() => {
    if (document.visibilityState === 'visible') loadList();
  }, 5000);

  const wanted = new URLSearchParams(location.search).get('id');
  loadList().then(() => { if (wanted) openRecord(wanted); });
})();
